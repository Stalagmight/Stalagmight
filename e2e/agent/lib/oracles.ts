/**
 * Problem oracles. Two sweeps over the running app:
 *
 *  - cheapSweep: runs after every mutating tool call (goal < ~50 ms of extra
 *    WebDriver traffic). Crash markers (fs only), a React error-boundary probe,
 *    and new error/warn ring lines since an internal cursor.
 *  - fullSweep: cheapSweep plus leftover write-staging files, a memory-growth
 *    check against a session baseline, dialog residue, and a read-only
 *    responsiveness probe (command-lock deadlock detector).
 *
 * The project's own integrity is not probed: the app runs `Project::validate`
 * after every edit, import, load and file-decoded history step, and rolls back
 * with an error log (which the cheap sweep reports) whatever would break it.
 * The `check_project_integrity` command this sweep used to call could only
 * ever answer "sound", and is gone.
 *
 * Oracles take a structural OracleTarget (AppSession satisfies it) so this
 * module does not import session.ts (no cycle). Findings are deduplicated by
 * kind+message; the first time a fatal/error finding is seen, runSweep captures
 * a screenshot + finding.json + any crash markers into the recorder's findings
 * dir and emits a `finding` event to the action log.
 */
import * as fs from 'fs';
import * as path from 'path';
import type { Browser } from 'webdriverio';
import { invokeInApp, evalInApp } from './page';
import { readStore } from './stores';
import { isErrorLine, isWarnLine } from './logs';

export type Severity = 'fatal' | 'error' | 'warning';

export interface Finding {
  severity: Severity;
  kind: string;
  message: string;
  detail?: unknown;
}

export interface OracleTarget {
  readonly browser: Browser;
  exclusive<T>(fn: () => Promise<T>): Promise<T>;
  findCrashMarkers(): string[];
}

export interface RecorderLike {
  readonly findingsDir: string;
  event(name: string, detail?: unknown): void;
}

export interface OracleState {
  reportedCrashMarkers: Set<string>;
  logCursor: number;
  memoryBaselineRssKb: number | null;
  reported: Set<string>;
  findingCount: number;
}

export function newOracleState(): OracleState {
  return {
    reportedCrashMarkers: new Set(),
    logCursor: 0,
    memoryBaselineRssKb: null,
    reported: new Set(),
    findingCount: 0,
  };
}

/**
 * Suffixes of the write-staging files that a crashed/aborted write can strand
 * beside the project. Every producer of one belongs here:
 *
 *  - `.stalx.tmp`        - the atomic `.stalx` v2 write
 *    (`persistence::stalx_v2::write_atomic`), `<file>.tmp`: every save, Save a
 *    Copy, salvage and SpeleoDB copy goes through it.
 *  - `.download.tmp`     - 0.6.0's SpeleoDB dump extract, which appended to the
 *    WHOLE file name, so an open remote project staged at
 *    `project.stalx.download.tmp` - a name that does not end in `.stalx.tmp`,
 *    and in exactly the directory this oracle scans (an open SpeleoDB
 *    project's path IS `<cache>/speleodb/<uuid>/project.stalx`).
 *  - `.vacuum.tmp`       - 0.6.0's push snapshot, a SQLite `VACUUM INTO`
 *    staged at `snapshot.stalx.vacuum.tmp` in that same directory.
 *
 * The last two have no producer in this build (the native transfer and the
 * v2 file replaced both); they stay listed, so a regression back to either, or
 * a leftover reaching a run's project folder, is still reported.
 *
 * Note the app's own recovery scanner (`commands/recovery.rs`) treats ANY trailing
 * `.tmp` as leftover, so it needs no such list; this narrower allowlist is the
 * harness's alone, and is the only one that can silently fall behind a producer.
 *
 * ## EXPORTS ADD NOTHING HERE, because no export stages
 *
 * Audited for `export-roundtrip-fuzz` (wave 4), since a leaked staging file is
 * exactly what a fuzzer that aims exports at hostile paths would produce. Every
 * export writer opens the DESTINATION and writes into it, so a failure mid-write
 * leaves a TRUNCATED artifact at the real path rather than a `.tmp` beside it -
 * which is why `exportOracles.ts`'s X1 checks the destination's own size and
 * shape and does not look for a staging suffix that cannot exist:
 *
 *   export/csv.rs:9        `std::fs::File::create(path)`, unbuffered `writeln!`
 *                          per row, no flush, no rename
 *   export/json.rs:14      `std::fs::write(path, json)`
 *   export/tml.rs:275,276  `std::fs::write(path, xml)` (TMLU, and the
 *                          any-other-extension fallback)
 *   export/tml.rs:460      `std::fs::File::create(path)` -> `ZipWriter` (TML)
 *   export/image.rs:400    `std::fs::write(path, svg)`
 *   export/image.rs:443    `pixmap.save_png(path)`
 *   export/image.rs:469    `std::fs::write(path, pdf)`
 *   commands/export.rs:441 `std::fs::write(&path, rendered)` (dive profile,
 *                          both wire formats)
 *
 * Functions read and confirmed staging-free: `export::csv::export_csv`,
 * `export::json::export_json`, `export::tml::{export_tml, export_with_profile,
 * write_output, write_zip}`, `export::image::{export_image,
 * export_image_with_stations, export_svg, export_png, export_pdf}`,
 * `commands::export::export_dive_profile`. `commands::export::preview_svg_export`
 * writes no file at all.
 *
 * So a `.tmp` seen during an export run belongs to a SAVE, not to the export -
 * `write_atomic` (`.stalx.tmp`, for the save, Merge and every other project
 * write) - and the suffixes below already cover it. This paragraph is the record that
 * the question was asked; if an export ever grows a staging write, its suffix
 * goes in the list and this note comes out.
 */
const LEFTOVER_TEMP_SUFFIXES = ['.stalx.tmp', '.download.tmp', '.vacuum.tmp'] as const;

/** Whether a directory entry is a leftover write-staging file. */
export function isLeftoverTempName(name: string): boolean {
  return LEFTOVER_TEMP_SUFFIXES.some((suffix) => name.endsWith(suffix));
}

/** Warn lines that are known-benign dev noise; kept tiny and explicit. */
const WARN_ALLOWLIST: string[] = [
  'source map',
  'Download the React DevTools',
];
function isAllowedWarn(line: string): boolean {
  return WARN_ALLOWLIST.some((s) => line.includes(s));
}

interface LogsSince {
  entries: { seq: number; ts_ms: number; line: string }[];
  latest_seq: number;
  dropped: boolean;
}

/**
 * Cheap post-action sweep. Never throws: a probe failure is itself surfaced as
 * a warning (an unresponsive webview is a finding, not a crash of the harness).
 */
export async function cheapSweep(target: OracleTarget, state: OracleState): Promise<Finding[]> {
  const findings: Finding[] = [];

  // 1. Backend crash markers (filesystem only; fatal even if the process survived).
  for (const marker of target.findCrashMarkers()) {
    if (state.reportedCrashMarkers.has(marker)) continue;
    state.reportedCrashMarkers.add(marker);
    let detail: unknown = { file: marker };
    try {
      detail = JSON.parse(fs.readFileSync(marker, 'utf8'));
    } catch {
      // Marker unreadable; keep the path.
    }
    findings.push({
      severity: 'fatal',
      kind: 'crash-marker',
      message: `backend panic marker: ${path.basename(marker)}`,
      detail,
    });
  }

  // 2 + 3. One exclusive block: DOM error-boundary probe, then new ring lines.
  try {
    const probe = await target.exclusive(async () => {
      const dom = await evalInApp<{ errorBoundary: boolean; text?: string }>(
        target.browser,
        `
        const el = document.querySelector('[data-testid="error-boundary-fallback"]');
        return { errorBoundary: !!el, text: el ? (el.textContent || '').slice(0, 300) : undefined };
      `,
      );
      const logs = await invokeInApp<LogsSince>(target.browser, 'get_recent_logs_since', {
        afterSeq: state.logCursor,
      });
      return { dom, logs };
    });

    if (probe.dom.errorBoundary) {
      findings.push({
        severity: 'error',
        kind: 'error-boundary',
        message: 'React error boundary is showing its fallback UI',
        detail: probe.dom.text,
      });
    }
    if (probe.logs.dropped) {
      findings.push({
        severity: 'warning',
        kind: 'ring-overflow',
        message: 'log ring rotated past the oracle cursor; some lines were lost',
      });
    }
    for (const e of probe.logs.entries) {
      if (isErrorLine(e.line)) {
        findings.push({ severity: 'error', kind: 'log-error', message: e.line.slice(0, 400) });
      } else if (isWarnLine(e.line) && !isAllowedWarn(e.line)) {
        findings.push({ severity: 'warning', kind: 'log-warn', message: e.line.slice(0, 400) });
      }
    }
    state.logCursor = Math.max(state.logCursor, probe.logs.latest_seq);
  } catch (err) {
    findings.push({
      severity: 'warning',
      kind: 'sweep-probe-failed',
      message: `oracle probe failed (webview may be unresponsive): ${err instanceof Error ? err.message : String(err)}`,
    });
  }

  return findings;
}

/** Full sweep: cheap sweep plus the expensive, less-frequent checks. */
export async function fullSweep(target: OracleTarget, state: OracleState): Promise<Finding[]> {
  const findings = await cheapSweep(target, state);

  // 4. (Project integrity: the app's own, see the module docs.)

  // 5. Leftover write-staging temp files next to the project (atomic save, and
  //     the SpeleoDB dump extract - see LEFTOVER_TEMP_SUFFIXES).
  try {
    const projectPath = await target.exclusive(async () => {
      const v = (await readStore(target.browser, 'project', 'projectPath')) as { value?: unknown };
      return typeof v.value === 'string' ? v.value : null;
    });
    if (projectPath) {
      const dir = path.dirname(projectPath);
      let leftovers: string[] = [];
      try {
        leftovers = fs.readdirSync(dir).filter(isLeftoverTempName);
      } catch {
        // Directory gone / unreadable.
      }
      if (leftovers.length > 0) {
        findings.push({
          severity: 'error',
          kind: 'leftover-temp',
          message: `leftover write-staging temp file(s) beside the project: ${leftovers.join(', ')}`,
          detail: { dir, leftovers },
        });
      }
    }
  } catch (err) {
    findings.push({ severity: 'warning', kind: 'temp-probe-failed', message: String(err) });
  }

  // 6. Memory growth vs the session baseline (first full sweep sets the baseline).
  try {
    const mem = await target.exclusive(() =>
      invokeInApp<{ total_rss_kb: number; webkit_processes: { role: string; rss_kb: number }[]; backend: { rss_kb: number } }>(
        target.browser,
        'get_memory_stats',
      ),
    );
    if (state.memoryBaselineRssKb === null) {
      state.memoryBaselineRssKb = mem.total_rss_kb;
    } else {
      const threshold = state.memoryBaselineRssKb * 1.5 + 300 * 1024;
      if (mem.total_rss_kb > threshold) {
        findings.push({
          severity: 'warning',
          kind: 'memory-growth',
          message: `total RSS ${mb(mem.total_rss_kb)} exceeds baseline ${mb(state.memoryBaselineRssKb)} * 1.5 + 300MB`,
          detail: {
            baselineMb: Math.round(state.memoryBaselineRssKb / 1024),
            currentMb: Math.round(mem.total_rss_kb / 1024),
            backendMb: Math.round(mem.backend.rss_kb / 1024),
            webkit: mem.webkit_processes.map((p) => ({ role: p.role, mb: Math.round(p.rss_kb / 1024) })),
          },
        });
      }
    }
  } catch (err) {
    findings.push({ severity: 'warning', kind: 'memory-probe-failed', message: String(err) });
  }

  // 7. Responsiveness probe: a read-only IPC under a hard timeout. A hang here
  // points at a command-lock deadlock. Read-only so it never mutates the repro.
  try {
    const responsive = await target.exclusive(() =>
      raceTimeout(invokeInApp(target.browser, 'get_recent_logs_since', { afterSeq: 0 }), 5000),
    );
    if (responsive === TIMEOUT) {
      findings.push({
        severity: 'fatal',
        kind: 'unresponsive',
        message: 'IPC did not return within 5s; possible command-lock deadlock',
      });
    }
  } catch (err) {
    findings.push({ severity: 'warning', kind: 'responsiveness-probe-failed', message: String(err) });
  }

  return findings;
}

const TIMEOUT = Symbol('timeout');
async function raceTimeout<T>(p: Promise<T>, ms: number): Promise<T | typeof TIMEOUT> {
  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<typeof TIMEOUT>((resolve) => {
    timer = setTimeout(() => resolve(TIMEOUT), ms);
  });
  try {
    return await Promise.race([p, timeout]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function mb(kb: number): string {
  return `${Math.round(kb / 1024)}MB`;
}

export interface SweepResult {
  findings: Finding[];
  newFindings: Finding[];
  worst: Severity | null;
}

const SEVERITY_RANK: Record<Severity, number> = { warning: 1, error: 2, fatal: 3 };

/**
 * Run a sweep, deduplicate against already-reported findings, and for each new
 * fatal/error finding capture a screenshot + finding.json + crash markers into
 * the recorder's findings dir. Emits a `finding` event per new finding.
 */
export async function runSweep(
  target: OracleTarget,
  state: OracleState,
  opts: { full?: boolean; recorder?: RecorderLike } = {},
): Promise<SweepResult> {
  const findings = opts.full ? await fullSweep(target, state) : await cheapSweep(target, state);
  const newFindings: Finding[] = [];
  for (const f of findings) {
    const key = `${f.kind}:${f.message}`;
    if (state.reported.has(key)) continue;
    state.reported.add(key);
    newFindings.push(f);
    opts.recorder?.event('finding', { severity: f.severity, kind: f.kind, message: f.message });
    if (opts.recorder && (f.severity === 'fatal' || f.severity === 'error')) {
      await captureFinding(target, opts.recorder, ++state.findingCount, f).catch(() => undefined);
    }
  }
  let worst: Severity | null = null;
  for (const f of findings) {
    if (!worst || SEVERITY_RANK[f.severity] > SEVERITY_RANK[worst]) worst = f.severity;
  }
  return { findings, newFindings, worst };
}

async function captureFinding(
  target: OracleTarget,
  recorder: RecorderLike,
  index: number,
  finding: Finding,
): Promise<void> {
  const dir = path.join(recorder.findingsDir, String(index).padStart(3, '0'));
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'finding.json'), JSON.stringify(finding, null, 2));
  // Screenshot (best-effort).
  try {
    const b64 = await target.exclusive(() => target.browser.takeScreenshot());
    fs.writeFileSync(path.join(dir, 'screenshot.png'), Buffer.from(b64, 'base64'));
  } catch {
    // Webview may be dead; the finding is still recorded.
  }
  // Copy any crash markers.
  for (const marker of target.findCrashMarkers()) {
    try {
      fs.copyFileSync(marker, path.join(dir, path.basename(marker)));
    } catch {
      // Best-effort.
    }
  }
}
