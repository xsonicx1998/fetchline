// Guard test (Round 75, item 1): every Tauri `invoke(...)` argument
// object in the frontend must use camelCase keys.
//
// Tauri v2 `#[tauri::command]` defaults to camelCase argument keys, so
// a snake_case key fails the call at runtime with "missing required
// key <camelKey>" — that is exactly how `open_add_url_window` broke
// in Round 74 (`{ link_id: linkId }` instead of `{ linkId }`), which
// silently killed every browser/video capture. This test scans the
// source (not a running app) and fails if any invoke argument key
// contains "_", so the mistake can never come back.
//
// Run with: node --test src/lib/apiKeys.test.mjs (from app/).
// No browser or build is needed: api.ts is read as text.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SRC_DIR = path.join(__dirname, "..");

function listTsFiles(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      // No test-only or generated trees under src.
      if (entry.name === "node_modules") continue;
      out.push(...listTsFiles(full));
    } else if (/\.(ts|tsx)$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

// Strip string literals (command names like 'get_pending_link' hold
// underscores legitimately — they are values, not keys) and comments,
// so the scan only ever sees code structure.
function stripStringsAndComments(src) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const ch = src[i];
    if (ch === "'" || ch === '"' || ch === "`") {
      const quote = ch;
      i++;
      while (i < src.length) {
        if (src[i] === "\\") {
          i += 2;
          continue;
        }
        if (src[i] === quote) {
          i++;
          break;
        }
        i++;
      }
      out += " ";
    } else if (ch === "/" && src[i + 1] === "/") {
      while (i < src.length && src[i] !== "\n") i++;
      out += "\n";
    } else if (ch === "/" && src[i + 1] === "*") {
      i += 2;
      while (i < src.length && !(src[i] === "*" && src[i + 1] === "/")) i++;
      i += 2;
      out += " ";
    } else {
      out += ch;
      i++;
    }
  }
  return out;
}

// Top-level keys of the object literal starting at `open` (the index
// of its `{`). Nested objects are ignored: only top-level keys are
// command arguments (nested keys belong to serde structs, whose
// field names are snake_case by design).
function topLevelKeys(src, open) {
  const keys = [];
  let depth = 0;
  let segmentStart = open + 1;
  const flush = (end) => {
    const segment = src.slice(segmentStart, end).trim();
    if (!segment) return;
    // `key: value` or shorthand `key`.
    const m = /^([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::|,|$)/.exec(segment);
    if (m) keys.push(m[1]);
  };
  for (let i = open; i < src.length; i++) {
    const ch = src[i];
    if (ch === "{" || ch === "[" || ch === "(") {
      depth++;
    } else if (ch === "}" || ch === "]" || ch === ")") {
      depth--;
      if (depth === 0 && ch === "}") {
        flush(i);
        return keys;
      }
    } else if (ch === "," && depth === 1) {
      flush(i);
      segmentStart = i + 1;
    }
  }
  return keys;
}

function invokeArgKeys(src) {
  const bad = [];
  const re = /\binvoke\s*(?:<[^>]*>)?\s*\(/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    // The first argument is the command name (a string, stripped
    // above); the argument object, if any, is the first `{` after it.
    const open = src.indexOf("{", m.index + m[0].length);
    const closeParen = src.indexOf(")", m.index + m[0].length);
    if (open === -1 || (closeParen !== -1 && closeParen < open)) continue;
    for (const key of topLevelKeys(src, open)) {
      if (key.includes("_")) bad.push(key);
    }
  }
  return bad;
}

test("no invoke() argument key contains an underscore (Tauri v2 is camelCase)", () => {
  const files = listTsFiles(SRC_DIR);
  assert.ok(files.length > 0, "expected TypeScript sources under src/");
  const offenders = [];
  for (const file of files) {
    const src = stripStringsAndComments(fs.readFileSync(file, "utf8"));
    for (const key of invokeArgKeys(src)) {
      offenders.push(`${path.relative(SRC_DIR, file)}: ${key}`);
    }
  }
  assert.deepEqual(offenders, [], `snake_case invoke keys found:\n${offenders.join("\n")}`);
});

test("the two known linkId calls send camelCase keys", () => {
  const api = fs.readFileSync(path.join(__dirname, "api.ts"), "utf8");
  assert.match(api, /get_pending_link', \{ linkId \}/);
  assert.match(api, /open_add_url_window', \{ linkId \}/);
  assert.doesNotMatch(api, /link_id:/);
});
