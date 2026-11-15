# Components — IDM rebuild (Tauri desktop)

Source: `replica/recon.md` (components table), `replica/architecture.md`
(Tauri 2 + React + Tailwind). Tokens: `tokens.json` / `tokens.css`.
Fonts: Inter (open, OFL) + system mono. Icons: Lucide (ISC) — no IDM
icons, logo or artwork anywhere. Every label and empty state is written
fresh. Accent is violet (`#6d28d9` light / `#a78bfa` dark), deliberately
not IDM's blue/green.

Layout: header 48px, toolbar 44px, sidebar 232px, status bar 28px,
table row 40px, spacing base 4px, radius md 8px. Window is resizable;
the download table is the flexible centre, sidebar and status bar are fixed.

## Button

```
variants  primary, secondary, ghost, danger, icon-only
sizes     sm 28px, md 32px, lg 40px
states    default, hover (accent-hover / surface-2), active (pressed, scale 0.98),
          focus-visible (2px ring, accent, offset 2px), disabled (opacity .45, no pointer),
          loading (spinner replaces icon, label stays for screen readers)
tokens    primary: bg accent, text on-accent, radius md, font base-strong
          secondary: bg surface-2, text text, border border
          ghost: transparent, text text-muted, hover bg surface-2
          danger: bg danger, text white (light) / bg danger-soft, text danger (dark, soft variant)
a11y      real <button>, type set, aria-busy while loading, disabled via attribute
used on   S01 toolbar, S02 Start/Later, S03 Pause/Resume, S05 Start Now, S09 OK/Cancel
```

## DownloadTable (the main list)

```
variants  full (S01), compact (queue files, S05)
columns   status icon, Name, Size, Status, Speed, Time left, Category, Added
          — sortable by Name, Size, Added; resizable columns in v2
states    empty (fresh copy: "No downloads yet — paste a link or drop a file here"),
          filled, row selected (accent-soft bg), row downloading (progress inline),
          row paused (muted), row complete (success check), row error (danger text + retry)
tokens    row height 40px, font base, border-bottom border, header font xs/600 uppercase,
          text-muted, sticky header
a11y      real <table> semantics, arrow-key row navigation, Enter opens S03,
          aria-sort on headers, status announced via aria-live polite
used on   S01, S05 Files in Queue
```

## ProgressBar + SegmentBar

```
variants  inline (table row, 4px), dialog (S03, 8px), segment bar (per-connection blocks)
states    0-100%, indeterminate (unknown size), stalled (no bytes for 10s → warning),
          complete (success), error (danger)
tokens    track surface-2, fill accent, complete fill success, error fill danger,
          radius pill, height 4/8px
a11y      role=progressbar, aria-valuenow/min/max, text % beside it, never colour-only
used on   S01 rows, S03 progress dialog
```

## SpeedGraph (S03)

```
variants  single-line area chart, 60s window
states    downloading (accent line + soft fill), paused (flat, muted), stalled (dashed)
tokens    line accent 2px, fill accent-soft at 20% opacity, grid border, font xs mono for values
a11y      aria-label with current speed in text; the graph is decorative, the numbers are real text
used on   S03
```

## StatusBadge

```
variants  queued (surface-2, muted), downloading (accent-soft, accent),
          paused (surface-2, muted), complete (success-soft, success),
          error (danger-soft, danger), sync (info-soft, info)
sizes     sm, pill
states    default only; text is always the status word, never an icon alone
tokens    radius pill, font xs/600, padding 2px 8px
a11y      text, not colour-only
used on   S01, S05
```

## Dialog / Modal

```
variants  sm 440px (confirm, S02 short), md 560px (S02, S07, S10), lg 720px (S05, S09)
states    default, loading (skeleton fields), error (inline danger text under the field),
          closing (fade + scale .98, 120ms)
tokens    bg surface-raised, radius lg, shadow pop, border border, padding 24px,
          header font lg/600, footer right-aligned buttons, 16px gap
a11y      role=dialog, aria-modal, focus trapped, Esc closes, focus returns to trigger,
          labelled by the title
used on   S02, S05, S06, S07, S09, S10, S11
```

## Tabs (Options S09, Scheduler S05)

```
variants  horizontal, underline style
states    active (accent text + 2px accent underline), inactive (text-muted),
          hover (text), focus-visible ring, disabled
tokens    font base-strong, padding 8px 12px, border-bottom border
a11y      role=tablist/tab/tabpanel, arrow keys move between tabs
used on   S05, S09
```

## TextInput / Textarea

```
variants  text, url, number, password, multiline, with browse button
states    default, hover (border-input), focus (2px accent ring + border accent),
          disabled, invalid (border danger + danger message below), read-only
tokens    height 32px, bg bg, border border-input, radius md, font base, padding 0 12px
a11y      real <label> linked by id, aria-invalid + aria-describedby for errors
used on   S02 URL/filename, S06 project, S09 proxy/logins, S10 URL list
```

## Dropdown / Select

```
variants  native-styled select, custom listbox (quality list in S08)
states    default, open (pop shadow, max-height 240px, scroll), hover option,
          selected (accent-soft bg + check icon), disabled
tokens    same as TextInput; option height 32px, font base
a11y      real <select> where possible; listbox uses aria-expanded/activedescendant
used on   S02 category, S07 filter, S08 quality/format
```

## CheckboxList

```
variants  links (S07), files in queue (S05)
states    checked (accent box + check), unchecked, indeterminate (partial select),
          disabled, hover row (surface-2)
tokens    box 16px, radius sm, row height 32px, font base
a11y      real <input type=checkbox>, label wraps the row, "Select all" is tri-state
used on   S05, S07
```

## CategoriesTree (sidebar, S04)

```
variants  tree with folders + queues section
states    collapsed/expanded (chevron rotates 90deg), selected (accent-soft bg, accent text),
          drop target (accent border, while dragging), badge count (xs, muted)
tokens    width 232px, row height 32px, font base, indent 16px per level
a11y      role=tree/treeitem, arrow keys expand/collapse, drag-and-drop has a menu alternative
used on   S01, S04, S05
```

## Toolbar (S01)

```
variants  main toolbar (44px), dialog footer toolbar
states    button enabled/disabled by selection (e.g. Resume disabled with no paused row),
          overflow menu (>> when narrow)
tokens    bg surface, border-bottom border, icon 18px Lucide, label font sm
a11y      toolbar role, buttons grouped, tooltips on hover + focus (not on touch alone)
used on   S01
```

## Toast / StatusBar

```
variants  toast (top-right, auto-dismiss 4s), status bar (bottom, 28px, persistent)
states    info, success, error (danger), with action button (e.g. "Open folder", "Retry")
tokens    toast bg surface-raised, shadow pop, radius md, padding 12px 16px; status bar
          bg surface, border-top border, font xs, text-muted
a11y      role=status / role=alert for errors, aria-live polite/assertive, dismiss button
used on   S01, S03, S14
```

## Toggle / Radio / Slider

```
variants  switch (dark theme, browser integration), radio group (one-time vs periodic, S05),
          slider (max connections, speed limit)
states    on (accent), off (surface-2), focus ring, disabled
tokens    switch 36x20px, knob 16px; radio 16px circle; slider track surface-2, fill accent
a11y      real inputs, labels, keyboard operable, value announced
used on   S05, S09, S12
```

## EmptyState

```
variants  table empty, category empty, queue empty, search no-results
states    default only — one open-illustration style (own artwork, Lucide-based),
          one headline, one sentence, one primary action
tokens    icon 40px text-faint, headline font lg, body font base text-muted, gap 12px
a11y      text, not image-only; action is a real Button
used on   S01, S04, S05
```

## Contrast report

Run: `python3 contrast.py replica/design/tokens.json`
Result: **24 pairs, 0 failing AA** (light + dark, text on bg/surface,
on-accent on accent, status colours on bg, input borders as UI).
Worst passing pair: light `border-input` on `bg` at 3.07:1 (UI, needs 3:1).
