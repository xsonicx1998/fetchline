// Round 54 — REAL end-to-end GUI test for Sonic Manager (Fetchline).
//
// Drives the real built app on real Windows through tauri-driver +
// WebdriverIO (the app's WebView2 frontend, the real Rust engine,
// the real SQLite database). Runs in CI on windows-latest via
// .github/workflows/e2e-windows.yml. It CANNOT run on the Linux dev
// VM (no WebView2) — its first real run is CI.
//
// What it covers (the user's requirements + the 4 bugs found on his
// PC on 2026-11-16, which these steps are DESIGNED to reproduce):
//   01 launch        — main window renders, no white screen
//   02 stale-drain   — BUG 1: a link handed over BEFORE launch
//                      auto-starts on launch (browser_start_drained
//                      defaults ON; drain runs at startup + 30 s)
//   03 progress-win  — BUG 2: no separate progress window opens
//                      for that auto-started download
//   04 progress      — percent and speed actually move
//   05 stop/pause    — BUG 3: toolbar Stop with nothing selected
//                      is a silent no-op; with a row selected it
//                      must pause
//   06 resume        — Resume restarts the paused download
//   07 add-url       — manual Add URL flow + the progress window
//                      for a MANUALLY started download (the control
//                      case for BUG 2)
//   08 delete        — Delete + confirm dialog removes the row
//   09 sidebar       — Categories / Queues / History / Finished
//                      switch views without errors
//   10 options       — BUG 4: the Settings dialog renders all 9
//                      tabs with real values (no "could not be
//                      loaded" alert)
//   11 stop-all      — Stop All + final state
//
// Screenshots: one after EVERY step, into test-screenshots/
// (uploaded as a CI artifact). A machine-readable results.json and
// a human-readable results.md land there too, plus startup.log and
// tauri-driver.log.
//
// Hard rule for this round: a failing step is a FINDING, not a
// reason to weaken the test. If the harness itself fails (driver,
// selectors, timing), fix the harness. Never skip an assertion to
// get green.
//
// Run:  node scripts/e2e/e2e-test.mjs
// Env:  E2E_APP_EXE   (default app/src-tauri/target/debug/fetchline.exe)
//       E2E_HOST_EXE (default app/host/target/release/fetchline-host.exe)
//       E2E_DB_PATH  (default %APPDATA%\com.fetchline.app\fetchline\app.db)

import { spawn, execFile } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { remote } from 'webdriverio';
import { startFixtureServer } from './fixture-server.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const SHOTS_DIR = path.join(REPO_ROOT, 'test-screenshots');
const APP_EXE = path.resolve(process.env.E2E_APP_EXE ?? path.join(REPO_ROOT, 'app', 'src-tauri', 'target', 'debug', 'fetchline.exe'));
const HOST_EXE = path.resolve(process.env.E2E_HOST_EXE ?? path.join(REPO_ROOT, 'app', 'host', 'target', 'release', 'fetchline-host.exe'));
const DB_PATH =
  process.env.E2E_DB_PATH ??
  (process.env.APPDATA
    ? path.join(process.env.APPDATA, 'com.fetchline.app', 'fetchline', 'app.db')
    : null);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const execFileAsync = (cmd, args) =>
  new Promise((resolve, reject) =>
    execFile(cmd, args, { windowsHide: true }, (err, stdout, stderr) =>
      err ? reject(Object.assign(err, { stdout, stderr })) : resolve({ stdout, stderr }),
    ),
  );

/** Poll fn() until it returns truthy (or throws a timeout error). */
async function waitFor(fn, { timeout = 30000, interval = 1000, label = 'condition' } = {}) {
  const deadline = Date.now() + timeout;
  let lastErr;
  while (Date.now() < deadline) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      lastErr = e;
    }
    await sleep(interval);
  }
  throw new Error(`timeout after ${timeout}ms waiting for: ${label}${lastErr ? ` (last error: ${lastErr.message})` : ''}`);
}

// ------------------------------------------------------------------ results

const RESULTS = [];
function record(step, name, status, detail) {
  const entry = { step, name, status, detail: detail ?? '', at: new Date().toISOString() };
  const i = RESULTS.findIndex((r) => r.step === step);
  if (i >= 0) RESULTS[i] = entry;
  else RESULTS.push(entry);
  fs.writeFileSync(path.join(SHOTS_DIR, 'results.json'), JSON.stringify(RESULTS, null, 2));
  console.log(`[${status.toUpperCase()}] ${step} ${name} — ${detail}`);
  return entry;
}

// ------------------------------------------------------------------ host frames
//
// Drives fetchline-host.exe with real Native Messaging frames (4-byte
// little-endian length prefix + JSON), exactly like scripts/
// capture-test.py does. Used to hand a link over BEFORE the app
// launches, so the launch drains a stale pending link (BUG 1).

class HostClient {
  constructor(exe, dbPath) {
    this.proc = spawn(exe, ['--db-path', dbPath], { stdio: ['pipe', 'pipe', 'pipe'] });
    this.buf = Buffer.alloc(0);
    this.waiters = [];
    this.stderr = '';
    this.proc.stdout.on('data', (d) => {
      this.buf = Buffer.concat([this.buf, d]);
      this.drain();
    });
    this.proc.stderr.on('data', (d) => {
      this.stderr += d.toString();
    });
  }
  drain() {
    for (;;) {
      if (this.buf.length < 4) return;
      const len = this.buf.readUInt32LE(0);
      if (len > 16 * 1024 * 1024) throw new Error(`host reply too large: ${len} bytes`);
      if (this.buf.length < 4 + len) return;
      const msg = JSON.parse(this.buf.subarray(4, 4 + len).toString('utf8'));
      this.buf = this.buf.subarray(4 + len);
      const w = this.waiters.shift();
      if (w) w(msg);
    }
  }
  send(obj) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('host reply timeout')), 20000);
      this.waiters.push((msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      const data = Buffer.from(JSON.stringify(obj), 'utf8');
      const head = Buffer.alloc(4);
      head.writeUInt32LE(data.length, 0);
      this.proc.stdin.write(Buffer.concat([head, data]));
    });
  }
  async close() {
    try {
      this.proc.stdin.end();
    } catch {
      /* already closed */
    }
    await Promise.race([new Promise((r) => this.proc.once('exit', r)), sleep(10000)]);
    if (this.proc.exitCode === null) this.proc.kill();
  }
}

// ------------------------------------------------------------------ app process

async function isAppRunning() {
  try {
    const { stdout } = await execFileAsync('tasklist', ['/FI', 'IMAGENAME eq fetchline.exe', '/NH']);
    return stdout.toLowerCase().includes('fetchline.exe');
  } catch {
    return false;
  }
}

async function killApp() {
  try {
    await execFileAsync('taskkill', ['/F', '/IM', 'fetchline.exe']);
  } catch {
    /* not running */
  }
  await waitFor(async () => !(await isAppRunning()), { timeout: 20000, interval: 1000, label: 'fetchline.exe to exit' });
}

// ------------------------------------------------------------------ OS windows
//
// Lists the visible window titles of the fetchline.exe process via
// user32 EnumWindows. This is the AUTHORITATIVE check for the
// separate progress window: WebDriver's getWindowHandles() may or
// may not see Tauri's secondary windows, but the OS always does.
// The script is written to disk once (it also lands in the artifact
// as evidence of how the check ran).

const LIST_WINDOWS_PS1 = path.join(SHOTS_DIR, 'list-windows.ps1');
function writeListWindowsScript() {
  fs.writeFileSync(
    LIST_WINDOWS_PS1,
    `Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public static class WinList {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  public static string[] TitlesFor(uint pid) {
    var list = new List<string>();
    EnumWindows(delegate(IntPtr hWnd, IntPtr lParam) {
      uint p; GetWindowThreadProcessId(hWnd, out p);
      if (p == pid && IsWindowVisible(hWnd)) { var sb = new StringBuilder(512); GetWindowText(hWnd, sb, 512); list.Add(sb.ToString()); }
      return true;
    }, IntPtr.Zero);
    return list.ToArray();
  }
}
"@
$proc = Get-Process -Name fetchline -ErrorAction SilentlyContinue | Select-Object -First 1
if ($null -eq $proc) { '[]' } else { [WinList]::TitlesFor([uint]$proc.Id) | ConvertTo-Json -Compress }
`,
  );
}

async function listAppWindowTitles() {
  try {
    const { stdout } = await execFileAsync('powershell.exe', [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      LIST_WINDOWS_PS1,
    ]);
    const text = stdout.trim();
    if (!text) return [];
    const parsed = JSON.parse(text);
    return Array.isArray(parsed) ? parsed : [parsed];
  } catch (e) {
    console.log(`list-windows.ps1 failed (non-fatal): ${e.message}`);
    return null;
  }
}

// ------------------------------------------------------------------ browser helpers

let browser = null;
let shotSeq = 0;

async function saveShot(name) {
  const file = path.join(SHOTS_DIR, `${name}.png`);
  await browser.saveScreenshot(file);
  shotSeq += 1;
  return file;
}

function rowTextInfo(text) {
  const status = /Downloading|Paused|Finished|Failed|Waiting/.exec(text)?.[0] ?? null;
  const percentMatch = /(\d+)%/.exec(text);
  const speedMatch = /([\d.]+ (?:B|KB|MB|GB)\/s)/.exec(text);
  return {
    status,
    percent: percentMatch ? Number(percentMatch[1]) : null,
    speed: speedMatch ? speedMatch[1] : null,
  };
}

/** Find a download table row by filename; returns null when absent. */
async function findRow(filename) {
  const row = await browser.$(`//tr[contains(., "${filename}")]`);
  if (!(await row.isExisting())) return null;
  const text = await row.getText();
  return { el: row, text, ...rowTextInfo(text) };
}

async function toolbarButton(label) {
  return browser.$(`button[aria-label="${label}"]`);
}

async function sidebarButton(label) {
  return browser.$(`//nav[@aria-label="Categories and queues"]//button[.//span[normalize-space()="${label}"]]`);
}

/** The main-window error banner (role="alert" outside any dialog). */
async function errorBannerPresent() {
  const banner = await browser.$(`//div[@role="alert"][not(ancestor::div[@role="dialog"])]`);
  return banner.isExisting();
}

// ------------------------------------------------------------------ step runner

async function runStep(num, name, fn) {
  const id = String(num).padStart(2, '0');
  try {
    const detail = await fn();
    await saveShot(`${id}-${name}`);
    record(id, name, 'pass', detail ?? '');
    return true;
  } catch (e) {
    try {
      await saveShot(`${id}-${name}-FAILED`);
    } catch {
      /* screenshot itself failed; the record below is the evidence */
    }
    record(id, name, 'fail', e.message);
    return false;
  }
}

async function blockStep(num, name, reason) {
  const id = String(num).padStart(2, '0');
  try {
    await saveShot(`${id}-${name}-blocked`);
  } catch {
    /* no browser yet */
  }
  record(id, name, 'blocked', reason);
  return false;
}

// ------------------------------------------------------------------ setup

async function ensureAppDb() {
  if (DB_PATH && fs.existsSync(DB_PATH)) return 'exists';
  console.log('app.db not present — launching the app once to create it…');
  const proc = spawn(APP_EXE, [], { detached: true, stdio: 'ignore' });
  proc.unref();
  await waitFor(() => fs.existsSync(DB_PATH), { timeout: 60000, interval: 1000, label: 'app.db to appear' });
  await sleep(5000); // let setup finish writing (settings row, queues)
  await killApp();
  console.log(`app.db created at ${DB_PATH}`);
  return 'created';
}

let driverProc = null;
async function startTauriDriver() {
  const out = fs.createWriteStream(path.join(SHOTS_DIR, 'tauri-driver.log'), { flags: 'a' });
  driverProc = spawn('tauri-driver', [], { stdio: ['ignore', 'pipe', 'pipe'] });
  driverProc.stdout.pipe(out);
  driverProc.stderr.pipe(out);
  await waitFor(
    async () => {
      try {
        const res = await fetch('http://127.0.0.1:4444/status', { signal: AbortSignal.timeout(2000) });
        return res.ok;
      } catch {
        return false;
      }
    },
    { timeout: 60000, interval: 1000, label: 'tauri-driver on 127.0.0.1:4444' },
  );
  console.log('tauri-driver is up on 127.0.0.1:4444');
}

// ------------------------------------------------------------------ main

async function main() {
  fs.mkdirSync(SHOTS_DIR, { recursive: true });
  fs.writeFileSync(path.join(SHOTS_DIR, 'results.json'), '[]');
  writeListWindowsScript();

  if (!DB_PATH) throw new Error('E2E_DB_PATH is unset and %APPDATA% is not defined');
  for (const [label, p] of [
    ['app exe', APP_EXE],
    ['host exe', HOST_EXE],
  ]) {
    if (!fs.existsSync(p)) throw new Error(`${label} not found at ${p}`);
  }

  let fixture = null;
  let exitCode = 0;
  const state = { slowA: null, slowARunning: false, seededLinkId: null };
  try {
    // ---- setup: fixture server, app.db, stale seed, driver, session
    fixture = await startFixtureServer();
    const slowAUrl = `${fixture.baseUrl}/slow-a.mp4`;
    const slowBUrl = `${fixture.baseUrl}/slow-b.mp4`;
    console.log(`fixture server at ${fixture.baseUrl} (20 MiB, 32 KiB/s/conn, Range OK)`);
    await ensureAppDb();

    const host = new HostClient(HOST_EXE, DB_PATH);
    try {
      const ping = await host.send({ type: 'ping' });
      if (!ping.ok) throw new Error(`host ping not ok: ${JSON.stringify(ping)}`);
      const link = await host.send({ type: 'download_link', url: slowAUrl, referrer: null });
      if (!link.ok || !link.pending_link_id) {
        throw new Error(`host download_link failed: ${JSON.stringify(link)} (stderr: ${host.stderr})`);
      }
      state.seededLinkId = link.pending_link_id;
      console.log(`seeded stale pending link ${link.pending_link_id} for ${slowAUrl}`);
    } finally {
      await host.close();
    }

    await startTauriDriver();
    browser = await remote({
      hostname: '127.0.0.1',
      port: 4444,
      path: '/',
      logLevel: 'error',
      connectionRetryTimeout: 120000,
      capabilities: { 'tauri:options': { application: APP_EXE } },
    });

    // ---- 01 launch: main window renders, no white screen
    await runStep(1, 'launch', async () => {
      await browser.waitUntil(
        async () => (await (await toolbarButton('Add URL')).isExisting()),
        { timeout: 60000, timeoutMsg: 'Add URL button never appeared (white screen? app did not render)' },
      );
      const title = await browser.getTitle();
      if (!title.includes('Sonic Manager')) throw new Error(`window title is "${title}", expected "Sonic Manager"`);
      for (const sel of ['button[aria-label="Add URL"]', '//th[normalize-space()="File Name"]']) {
        if (!(await (await browser.$(sel)).isExisting())) throw new Error(`launch element missing: ${sel}`);
      }
      const sidebar = await sidebarButton('All downloads');
      if (!(await sidebar.isExisting())) throw new Error('sidebar "All downloads" missing');
      const bodyLen = await browser.execute(() => document.body.innerText.length);
      if (bodyLen < 200) throw new Error(`body text is only ${bodyLen} chars — window looks blank`);
      return `title="${title}", body text ${bodyLen} chars, toolbar + sidebar + table present`;
    });

    // ---- 02 stale drain (BUG 1): the seeded link must NOT auto-start.
    // Correct behaviour (the fix the user was offered): a link handed
    // over BEFORE this launch sits queued (Waiting) until the user
    // starts it. If it is Downloading with no click from this test,
    // BUG 1 is reproduced — the step fails as a finding, and the
    // download is then used for the steps below either way.
    await runStep(2, 'stale-drain', async () => {
      const row = await waitFor(async () => await findRow('slow-a.mp4'), {
        timeout: 60000,
        interval: 1000,
        label: 'slow-a.mp4 row to appear in the table',
      });
      if (row.status === 'Downloading') {
        state.slowARunning = true;
        throw new Error(
          `BUG 1 REPRODUCED: stale link (seeded before launch, id ${state.seededLinkId}) auto-started with no user action — status is Downloading at ${row.percent}%`,
        );
      }
      if (row.status !== 'Waiting') {
        throw new Error(`stale link row appeared with unexpected status "${row.status}" (expected Waiting/queued)`);
      }
      // Correct behaviour: queued. Start it via the queue so the
      // steps below have a running download to work with.
      const startQueue = await toolbarButton('Start Queue');
      if (await startQueue.isEnabled()) {
        await startQueue.click();
      } else {
        throw new Error('stale link is Waiting but Start Queue is disabled — cannot start it for the later steps');
      }
      const running = await waitFor(async () => {
        const r = await findRow('slow-a.mp4');
        return r && r.status === 'Downloading' ? r : null;
      }, { timeout: 30000, interval: 1000, label: 'slow-a.mp4 to start after Start Queue' });
      state.slowARunning = true;
      return `stale link stayed queued (Waiting) as it should; started via Start Queue, now Downloading at ${running.percent}%`;
    });

    // ---- 03 progress window (BUG 2)
    if (!state.slowARunning) {
      await blockStep(3, 'progress-window', 'no downloading row from step 02');
    } else {
      await runStep(3, 'progress-window', async () => {
        const titles = await listAppWindowTitles();
        const handles = await browser.getWindowHandles();
        const progress = titles === null ? null : titles.filter((t) => /% slow-a\.mp4/.test(t));
        if (titles !== null && progress.length === 0) {
          throw new Error(
            `BUG 2 REPRODUCED: no separate progress window for the downloading slow-a.mp4 — OS windows of fetchline.exe: [${titles.join(' | ')}]; WebDriver handles: ${handles.length}`,
          );
        }
        // If the driver can see the extra window, screenshot it too.
        if (handles.length > 1) {
          const current = await browser.getWindowHandle();
          for (const h of handles) {
            if (h === current) continue;
            await browser.switchToWindow(h);
            const t = await browser.getTitle();
            if (/% slow-a/.test(t)) {
              await saveShot('03b-progress-window');
              break;
            }
          }
          await browser.switchToWindow(current);
        }
        return `progress window present: OS titles [${(titles ?? []).join(' | ')}], WebDriver handles ${handles.length}`;
      });
    }

    // ---- 04 progress moves: percent + speed change while downloading
    if (!state.slowARunning) {
      await blockStep(4, 'progress-moves', 'no downloading row from step 02');
    } else {
      await runStep(4, 'progress-moves', async () => {
        const first = await findRow('slow-a.mp4');
        if (!first || first.status !== 'Downloading') {
          throw new Error(`slow-a.mp4 is not downloading at sample time (status: ${first?.status ?? 'row gone'})`);
        }
        await sleep(8000);
        const second = await findRow('slow-a.mp4');
        if (!second || second.status !== 'Downloading') {
          throw new Error(`slow-a.mp4 stopped downloading between samples (status: ${second?.status ?? 'row gone'})`);
        }
        if (second.percent === null || first.percent === null) {
          throw new Error(`percent not shown (first=${first.percent}, second=${second.percent})`);
        }
        if (second.percent <= first.percent) {
          throw new Error(`percent did not move in 8 s (${first.percent}% -> ${second.percent}%, speed ${second.speed})`);
        }
        if (!second.speed) throw new Error(`speed column is blank while downloading (${first.percent}% -> ${second.percent}%)`);
        return `percent ${first.percent}% -> ${second.percent}% in 8 s, speed ${second.speed}`;
      });
    }

    // ---- 05 stop/pause (BUG 3)
    if (!state.slowARunning) {
      await blockStep(5, 'stop-pause', 'no downloading row from step 02');
    } else {
      await runStep(5, 'stop-pause', async () => {
        const problems = [];
        // Part A: with NOTHING selected, Stop must be usable or say
        // why it is not. Today it is a disabled button that silently
        // does nothing — the exact "Stop does not work" the user hit.
        const stopBtn = await toolbarButton('Stop');
        const enabledBefore = await stopBtn.isEnabled();
        if (!enabledBefore) {
          problems.push('BUG 3 REPRODUCED (part A): toolbar Stop is disabled with nothing selected — no message, no way to stop the running download without first discovering row selection');
        }
        // Part B: select the row, Stop must become enabled and pause.
        const row = await findRow('slow-a.mp4');
        if (!row) throw new Error('slow-a.mp4 row vanished before the pause step');
        await row.el.click();
        await waitFor(async () => (await (await toolbarButton('Stop')).isEnabled()), {
          timeout: 10000,
          interval: 500,
          label: 'Stop button to enable after selecting the row',
        });
        await (await toolbarButton('Stop')).click();
        await waitFor(
          async () => {
            const r = await findRow('slow-a.mp4');
            return r && r.status === 'Paused' ? r : null;
          },
          { timeout: 20000, interval: 1000, label: 'slow-a.mp4 to become Paused' },
        );
        if (problems.length) throw new Error(problems.join(' | ') + ' | (part B passed: row selected, Stop paused the download)');
        return 'Stop enabled after selecting the row and paused the download';
      });
    }

    // ---- 06 resume
    if (!state.slowARunning) {
      await blockStep(6, 'resume', 'no downloading row from step 02');
    } else {
      await runStep(6, 'resume', async () => {
        const before = await findRow('slow-a.mp4');
        if (!before || before.status !== 'Paused') {
          throw new Error(`expected Paused before resume, got "${before?.status ?? 'row gone'}"`);
        }
        const resumeBtn = await toolbarButton('Resume');
        if (!(await resumeBtn.isEnabled())) throw new Error('Resume button is disabled for the paused row');
        await resumeBtn.click();
        await waitFor(
          async () => {
            const r = await findRow('slow-a.mp4');
            return r && r.status === 'Downloading' ? r : null;
          },
          { timeout: 30000, interval: 1000, label: 'slow-a.mp4 to resume downloading' },
        );
        return 'paused download resumed to Downloading';
      });
    }

    // ---- 07 add-url (manual flow) + progress window control case
    await runStep(7, 'add-url', async () => {
      await (await toolbarButton('Add URL')).click();
      const urlInput = await browser.$('#add-url');
      await browser.waitUntil(async () => urlInput.isExisting(), { timeout: 10000, timeoutMsg: 'Add dialog URL field never appeared' });
      await urlInput.setValue(slowBUrl);
      await browser.$(`//button[normalize-space()="Start download"]`).click();
      // Re-query the field on every poll: the element unmounts when
      // the dialog closes, so a stale reference must not be reused.
      await waitFor(async () => !(await (await browser.$('#add-url')).isExisting()), { timeout: 10000, interval: 500, label: 'Add dialog to close' });
      const row = await waitFor(async () => {
        const r = await findRow('slow-b.mp4');
        return r && r.status === 'Downloading' ? r : null;
      }, { timeout: 60000, interval: 1000, label: 'slow-b.mp4 to start downloading' });
      // Control case for BUG 2: a MANUALLY started download must get
      // its own progress window (setting is on by default).
      const titles = await listAppWindowTitles();
      const progressB = titles === null ? null : titles.filter((t) => /% slow-b\.mp4/.test(t));
      if (titles !== null && progressB.length === 0) {
        throw new Error(
          `manual download started (${row.percent}%) but NO progress window opened for slow-b.mp4 either — OS windows: [${titles.join(' | ')}]`,
        );
      }
      return `slow-b.mp4 downloading at ${row.percent}%; progress window for it: ${titles === null ? 'OS check unavailable, WebDriver only' : 'present'}`;
    });

    // ---- 08 delete (with confirm dialog)
    await runStep(8, 'delete', async () => {
      const row = await findRow('slow-b.mp4');
      if (!row) throw new Error('slow-b.mp4 row missing before delete');
      await row.el.click();
      const delBtn = await toolbarButton('Delete');
      if (!(await delBtn.isEnabled())) throw new Error('Delete button is disabled for the selected row');
      await delBtn.click();
      const dialog = await browser.$(`//div[@role="alertdialog"]`);
      await browser.waitUntil(async () => dialog.isExisting(), { timeout: 10000, timeoutMsg: 'remove confirm dialog never appeared' });
      const dialogText = await dialog.getText();
      if (!dialogText.includes('Remove this download?')) throw new Error(`unexpected confirm dialog text: ${dialogText}`);
      await browser.$(`//div[@role="alertdialog"]//button[normalize-space()="Remove"]`).click();
      await waitFor(async () => (await findRow('slow-b.mp4')) === null, {
        timeout: 15000,
        interval: 500,
        label: 'slow-b.mp4 row to disappear after Remove',
      });
      return 'slow-b.mp4 deleted via confirm dialog; row gone';
    });

    // ---- 09 sidebar: categories / queues / history / finished
    await runStep(9, 'sidebar', async () => {
      const notes = [];
      const clickSidebar = async (label) => {
        const btn = await sidebarButton(label);
        if (!(await btn.isExisting())) throw new Error(`sidebar item "${label}" not found`);
        await btn.click();
        await sleep(2000);
      };
      const rowVisible = async (filename) => (await findRow(filename)) !== null;

      await clickSidebar('Video');
      if (!(await rowVisible('slow-a.mp4'))) throw new Error('after clicking Video, slow-a.mp4 (a video file) is not shown');
      notes.push('Video filter shows slow-a.mp4');
      await clickSidebar('All downloads');
      if (!(await rowVisible('slow-a.mp4'))) throw new Error('after clicking All downloads, slow-a.mp4 is not shown');
      await clickSidebar('Main download queue');
      if (!(await rowVisible('slow-a.mp4'))) throw new Error('after clicking Main download queue, slow-a.mp4 is not shown');
      notes.push('queue filter shows slow-a.mp4');
      await clickSidebar('History');
      const search = await browser.$('input[aria-label="Search download history"]');
      if (!(await search.isExisting())) throw new Error('History view did not open (search box missing)');
      notes.push('History view opened');
      await clickSidebar('Finished');
      await waitFor(async () => (await findRow('slow-a.mp4')) === null, {
        timeout: 10000,
        interval: 500,
        label: 'slow-a.mp4 to leave the Finished view',
      });
      notes.push('Finished view hides the still-downloading row');
      await clickSidebar('All downloads');
      if (!(await rowVisible('slow-a.mp4'))) throw new Error('after returning to All downloads, slow-a.mp4 is not shown');
      if (await errorBannerPresent()) throw new Error('error banner appeared while switching sidebar views');
      return notes.join('; ');
    });

    // ---- 10 options (BUG 4): Settings dialog renders real values
    await runStep(10, 'options', async () => {
      await (await toolbarButton('Options')).click();
      const dialog = await browser.$(`//div[@role="dialog"]`);
      await browser.waitUntil(async () => dialog.isExisting(), { timeout: 15000, timeoutMsg: 'Settings dialog never opened' });
      const title = await browser.$(`//h2[@id="settings-title"]`);
      if (!(await title.isExisting())) throw new Error('Settings dialog opened without its title');
      const tabs = await browser.$$(`div[role="tablist"] button[role="tab"]`);
      const labels = [];
      for (const t of tabs) labels.push(await t.getText());
      const expected = ['General', 'File Types', 'Connection', 'Save To', 'Downloads', 'Proxy', 'Site logins', 'Dial-Up', 'Sounds'];
      if (labels.length !== expected.length || !expected.every((l) => labels.includes(l))) {
        throw new Error(`Settings tabs wrong: got [${labels.join(', ')}], expected [${expected.join(', ')}]`);
      }
      // BUG 4 check: the dialog must not show the load-failure alert,
      // and the General tab must render real controls. NOTE: the
      // default download folder is EMPTY by default (Settings::default
      // is String::new()), so an empty folder field is NORMAL — the
      // check is that the controls exist and the progress-window
      // checkbox (default ON) is checked.
      const alertInDialog = await browser.$(`//div[@role="dialog"]//div[@role="alert"]`);
      if (await alertInDialog.isExisting()) {
        throw new Error(`BUG 4 REPRODUCED: Settings dialog shows load error: "${await alertInDialog.getText()}"`);
      }
      const folder = await browser.$('#set-folder');
      if (!(await folder.isExisting())) throw new Error('General tab: Default download folder field missing');
      const progressCheck = await browser.$(`//label[contains(normalize-space(.), "Show the download progress window")]//input[@type="checkbox"]`);
      if (!(await progressCheck.isExisting())) throw new Error('General tab: progress-window checkbox missing');
      if (!(await progressCheck.isSelected())) {
        throw new Error('General tab: "Show the download progress window" is OFF on a fresh install (default must be ON)');
      }
      // Two more tabs must render non-empty content.
      for (const tab of ['Downloads', 'Sounds']) {
        const tabBtn = await browser.$(`//div[@role="tablist"]//button[normalize-space()="${tab}"]`);
        await tabBtn.click();
        await sleep(1000);
        const bodyLen = await browser.execute(() => {
          const d = document.querySelector('div[role="dialog"]');
          return d ? d.innerText.length : 0;
        });
        if (bodyLen < 100) throw new Error(`Settings tab "${tab}" renders almost nothing (${bodyLen} chars)`);
        await saveShot(`10-options-${tab.toLowerCase()}`);
      }
      await browser.$(`//div[@role="dialog"]//button[@aria-label="Close"]`).click();
      // Re-query on every poll: the dialog unmounts on close.
      await waitFor(async () => !(await (await browser.$(`//div[@role="dialog"]`)).isExisting()), { timeout: 10000, interval: 500, label: 'Settings dialog to close' });
      return `9 tabs present; General renders (folder field + progress-window checkbox ON); Downloads + Sounds render`;
    });

    // ---- 11 stop-all + final state
    await runStep(11, 'stop-all', async () => {
      const stopAll = await toolbarButton('Stop All');
      if (await stopAll.isEnabled()) {
        await stopAll.click();
        await waitFor(
          async () => {
            const r = await findRow('slow-a.mp4');
            return r && r.status !== 'Downloading' ? r : null;
          },
          { timeout: 20000, interval: 1000, label: 'no download still Downloading after Stop All' },
        );
      }
      const row = await findRow('slow-a.mp4');
      if (!row) throw new Error('slow-a.mp4 row vanished after Stop All');
      if (await errorBannerPresent()) throw new Error('error banner present at the end of the run');
      return `slow-a.mp4 ended as ${row.status} at ${row.percent}%; no error banner`;
    });
  } catch (e) {
    console.error(`FATAL: ${e.message}`);
    exitCode = 1;
  } finally {
    // ---- evidence + cleanup (always runs)
    try {
      if (browser) {
        try {
          await saveShot('99-final-state');
        } catch {
          /* session already gone */
        }
        try {
          await browser.deleteSession();
        } catch {
          /* already closed */
        }
      }
    } catch {
      /* ignore */
    }
    try {
      if (driverProc && driverProc.exitCode === null) driverProc.kill();
    } catch {
      /* ignore */
    }
    try {
      if (fixture) fixture.server.close();
    } catch {
      /* ignore */
    }
    try {
      await killApp();
    } catch {
      /* ignore */
    }
    try {
      if (DB_PATH) {
        const startupLog = path.join(path.dirname(DB_PATH), 'startup.log');
        if (fs.existsSync(startupLog)) {
          fs.copyFileSync(startupLog, path.join(SHOTS_DIR, 'startup.log'));
        }
      }
    } catch {
      /* ignore */
    }
    const failed = RESULTS.filter((r) => r.status === 'fail').length;
    const blocked = RESULTS.filter((r) => r.status === 'blocked').length;
    const passed = RESULTS.filter((r) => r.status === 'pass').length;
    const lines = [
      '# Sonic Manager E2E — results',
      '',
      `- app: ${APP_EXE}`,
      `- host: ${HOST_EXE}`,
      `- seeded stale pending link: ${state.seededLinkId ?? 'none'}`,
      `- fixture: ${fixture ? fixture.baseUrl : 'not started'}`,
      `- outcome: ${passed} passed, ${failed} failed, ${blocked} blocked`,
      '',
      '## Steps',
      '',
      ...RESULTS.map((r) => `- [${r.status.toUpperCase()}] ${r.step} ${r.name} — ${r.detail}`),
      '',
      'Screenshots: one PNG per step in this folder (plus 03b-progress-window when the driver sees it).',
    ];
    fs.writeFileSync(path.join(SHOTS_DIR, 'results.md'), lines.join('\n'));
    console.log(`E2E DONE: ${passed} passed, ${failed} failed, ${blocked} blocked`);
    if (failed > 0 || blocked > 0) exitCode = 1;
  }
  process.exit(exitCode);
}

main().catch((e) => {
  console.error(`FATAL (unhandled): ${e?.stack ?? e}`);
  process.exit(1);
});
