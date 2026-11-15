//! Minimal UTC time helpers for the scheduler.
//!
//! No date library is pulled in for this: queue times are stored as
//! UTC ISO-8601 strings (`2026-11-14T20:30:00Z`), which sort
//! lexicographically, and the scheduler only needs to compare them
//! against "now". Conversion uses Howard Hinnant's days-from-civil
//! algorithm, so it is exact for every date the app will ever see.

/// Current time, as Unix seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` (a trailing `Z` is required; offsets
/// are not accepted) to Unix seconds. Fractional seconds are accepted
/// and ignored, since JavaScript `Date.toISOString()` produces them
/// (`...SS.000Z`) and the scheduler UI passes its values straight
/// through. Returns `None` for anything malformed.
pub fn parse_iso_to_unix(value: &str) -> Option<i64> {
    let without_z = value.strip_suffix('Z')?;
    // Split off an optional ".sss" fraction (digits only).
    let main = match without_z.split_once('.') {
        Some((m, frac)) if !frac.is_empty() && frac.bytes().all(|c| c.is_ascii_digit()) => m,
        Some(_) => return None,
        None => without_z,
    };
    let b = main.as_bytes();
    if b.len() != 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        std::str::from_utf8(&b[from..to]).ok()?.parse::<i64>().ok()
    };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Reject days that do not exist (e.g. 30 February): without this,
    // days_from_civil would silently roll them into the next month.
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in_month = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    if d < 1 || d > days_in_month {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn unix_to_iso(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Days since 1970-01-01 for a civil date (Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // Mar=0 … Feb=11
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date for days since 1970-01-01 (Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; // Mar=0 … Feb=11
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_known_dates() {
        assert_eq!(parse_iso_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_to_unix("2026-11-14T20:30:00Z"), Some(1_794_688_200));
        assert_eq!(unix_to_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_to_iso(1_794_688_200), "2026-11-14T20:30:00Z");
        // Leap day survives the round trip.
        assert_eq!(
            unix_to_iso(parse_iso_to_unix("2028-02-29T12:00:00Z").unwrap()),
            "2028-02-29T12:00:00Z"
        );
    }

    #[test]
    fn accepts_fractional_seconds_like_js_to_iso_string() {
        // JavaScript Date.toISOString() output.
        assert_eq!(parse_iso_to_unix("2026-11-14T20:30:00.000Z"), Some(1_794_688_200));
        assert_eq!(parse_iso_to_unix("2026-11-14T20:30:00.5Z"), Some(1_794_688_200));
    }

    #[test]
    fn rejects_impossible_dates() {
        // Days that do not exist must not silently roll into the next
        // month: a scheduler window on "30 February" would otherwise
        // run on 2 March with no warning (BUG-005).
        assert_eq!(parse_iso_to_unix("2026-02-30T00:00:00Z"), None);
        assert_eq!(parse_iso_to_unix("2026-04-31T00:00:00Z"), None);
        assert_eq!(parse_iso_to_unix("2025-02-29T00:00:00Z"), None);
        assert!(parse_iso_to_unix("2024-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(parse_iso_to_unix("2026-11-14 20:30:00"), None);
        assert_eq!(parse_iso_to_unix("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso_to_unix(""), None);
        assert_eq!(parse_iso_to_unix("2026-11-14T20:30:00+02:00"), None);
        assert_eq!(parse_iso_to_unix("2026-11-14T20:30:00.Z"), None);
        assert_eq!(parse_iso_to_unix("2026-11-14T20:30:00.abcZ"), None);
    }

    #[test]
    fn iso_strings_sort_like_time() {
        let a = parse_iso_to_unix("2026-11-14T20:00:00Z").unwrap();
        let b = parse_iso_to_unix("2026-11-14T21:00:00Z").unwrap();
        assert!(a < b);
    }
}
