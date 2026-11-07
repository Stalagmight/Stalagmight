/**
 * Problem oracles. Two sweeps over the running app:
 *
 *  - cheapSweep: runs after every mutating tool call (goal < ~50 ms of extra
 *    WebDriver traffic). Crash markers (fs only), a React error-boundary probe,
 *    and new error/warn ring lines since an internal cursor.
 *  - fullSweep: cheapSweep plus project-integrity, leftover write-staging files,
 *    a memory-growth check against a session baseline, dialog residue, and a
 *    read-only responsiveness probe (command-lock deadlock detector).
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
import { redactCorpusText, redactCorpusValue } from './fixtureCorpus';
import { shareableFinding } from '../../../scripts/lib/shareable-finding.mjs';

export type Severity = 'fatal' | 'error' | 'warning';

export interface Finding {
  severity: Severity;
  kind: string;
  /**
   * Where the finding came from, as the APP'S CODE names it - for a `log-warn` /
   * `log-error`, the log target (`logLineCode`). Never text from the data, so a
   * summary line can carry it where it cannot carry `message` (#576 HFI2-F2).
   */
  code?: string;
  message: string;
  detail?: unknown;
}

/**
 * The code-defined source of a ring log line: its tracing target
 * (`WARN stalagmight_parsers::tml: Section ...` -> `stalagmight_parsers::tml`),
 * else the frontend crash tag the line carries. Nothing after the target is
 * read: that is the message, and a message quotes the data it is about - a
 * section, a project, a station (#576 HFI2-F2).
 */
export function logLineCode(line: string): string {
  const target = /^(?:ERROR|WARN)\s+([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*):\s/.exec(line);
  if (target?.[1]) return target[1];
  if (line.includes('[unhandled-error]')) return 'unhandled-error';
  if (line.includes('[unhandled-rejection]')) return 'unhandled-rejection';
  if (line.includes('PANIC:')) return 'panic';
  return 'unattributed';
}

/**
 * A sweep's findings as a run's summary line prints them: a count per
 * severity, kind and code, and nothing else (#576 HFI2-F2).
 *
 * The PASS/FAIL line used to carry the findings as JSON, message and detail
 * included, and a `log-warn` message IS the app's log line - `Section "<name>"
 * has no shots after filtering`, a project's name in a datum warning. The
 * corpus fold hides corpus FILE names only, so every name inside a file reached
 * a run log, and run logs are what a campaign publishes. The full text stays in
 * the session directory (`actions.jsonl`, `findings/`), which
 * docs/agent-harness.md says not to attach, and in the app's own log files.
 * Every key is built from harness-defined `kind` and the code-defined `code`,
 * and is still reduced to identifier characters.
 */
export function findingsSummary(findings: readonly Finding[]): string {
  const label = (text: string): string => text.replace(/[^\w:.-]/g, '?').slice(0, 120);
  const counts = new Map<string, number>();
  for (const f of findings) {
    const key = `${f.severity}:${label(f.kind)}${f.code === undefined ? '' : `@${label(f.code)}`}`;
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return JSON.stringify(Object.fromEntries([...counts].sort(([a], [b]) => a.localeCompare(b))));
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
 *  - `.stalx.tmp`        - the atomic save (`core/save.rs`), `<file>.tmp`.
 *  - `.download.tmp`     - the SpeleoDB dump extract
 *    (`speleodb/dump.rs::download_temp_path`), which appends to the WHOLE file
 *    name, so an open remote project stages at `project.stalx.download.tmp`.
 *    That does not end in `.stalx.tmp`, so matching only the save suffix let a
 *    leaked download temp pass `check_problems` in silence - and the directory
 *    it lands in is exactly the one this oracle scans (an open SpeleoDB
 *    project's path IS `<cache>/speleodb/<uuid>/project.stalx`).
 *  - `.vacuum.tmp`       - the push snapshot's SQLite compaction
 *    (`persistence/sqlite/repair.rs::vacuum_temp_path`), which likewise appends to
 *    the whole file name, so the push stages at `snapshot.stalx.vacuum.tmp` in
 *    that same directory. Cleaned on every in-process exit from `VACUUM INTO`
 *    (success renames it away, both failure arms unlink it) and self-healed by the
 *    next push, so it only survives a crash or kill mid-compaction - which is
 *    precisely what the adversarial runs provoke, and what this oracle is for.
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
 * `project_io.rs:535` (`.stalx.tmp`), `project_merge.rs:311`, `speleodb/dump.rs:194`
 * (`.download.tmp`), `sqlite/repair.rs:387` (`.vacuum.tmp`) - and the three
 * suffixes below already cover all of them. This paragraph is the record that
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

/**
 * `text` through the corpus fold, THEN cut to `limit` (#576 RV17-2). Cut first,
 * a corpus name straddling the limit was no longer whole, no fold matched it,
 * and its beginning stayed in the finding.
 */
function cutAfterFold(text: string, limit: number): string {
  return redactCorpusText(text).slice(0, limit);
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
      // The page sends the whole text: Node folds it, THEN cuts it (`cutAfterFold`).
      // A cut in the page could split a corpus name the fold then cannot match
      // (#576 RV19b-P3).
      const dom = await evalInApp<{ errorBoundary: boolean; text?: string }>(
        target.browser,
        `
        const el = document.querySelector('[data-testid="error-boundary-fallback"]');
        return { errorBoundary: !!el, text: el ? el.textContent || '' : undefined };
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
        detail: probe.dom.text === undefined ? undefined : cutAfterFold(probe.dom.text, 300),
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
        findings.push({ severity: 'error', kind: 'log-error', code: logLineCode(e.line), message: cutAfterFold(e.line, 400) });
      } else if (isWarnLine(e.line) && !isAllowedWarn(e.line)) {
        findings.push({ severity: 'warning', kind: 'log-warn', code: logLineCode(e.line), message: cutAfterFold(e.line, 400) });
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

  // 4. Project integrity.
  try {
    const integrity = await target.exclusive(() =>
      invokeInApp<{ messages: string[]; save_blocked: boolean }>(target.browser, 'check_project_integrity'),
    );
    if (integrity.save_blocked) {
      findings.push({
        severity: 'fatal',
        kind: 'integrity-save-blocked',
        message: 'project integrity: saving is blocked (unresolvable cross-references)',
        detail: integrity.messages,
      });
    } else if (integrity.messages.length > 0) {
      findings.push({
        severity: 'error',
        kind: 'integrity-messages',
        message: `project integrity reported ${integrity.messages.length} issue(s)`,
        detail: integrity.messages,
      });
    }
  } catch (err) {
    findings.push({ severity: 'warning', kind: 'integrity-probe-failed', message: String(err) });
  }

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
  opts: { full?: boolean; recorder?: RecorderLike | undefined } = {},
): Promise<SweepResult> {
  // Redacted at the SOURCE (#576 RV12-F1): a `log-error` finding's message is the
  // backend's log line, and an import that failed logs the corpus copy it was
  // handed. Everything below - the dedupe key, the action log event,
  // finding.json, the caller's printout - only ever sees the publishable text.
  const swept = opts.full ? await fullSweep(target, state) : await cheapSweep(target, state);
  const findings = swept.map(publishableFinding);
  const newFindings: Finding[] = [];
  for (const f of findings) {
    const key = `${f.kind}:${f.message}`;
    if (state.reported.has(key)) continue;
    state.reported.add(key);
    newFindings.push(f);
    // The whole finding, detail included, goes to the action log - the private
    // record (#576 W21-NF2); finding.json carries the shareable projection.
    opts.recorder?.event('finding', {
      severity: f.severity,
      kind: f.kind,
      code: f.code,
      message: f.message,
      ...(f.detail === undefined ? {} : { detail: f.detail }),
    });
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

/**
 * `finding` with every corpus name the process has copied replaced by its tag
 * (`fixtureCorpus.redactCorpusText`), in the message and in the detail. The
 * detail is folded string by string (`redactCorpusValue`, #576 RV15-1): over its
 * JSON text, a name JSON escapes (a quote, a backslash) was never found - a
 * crash marker's detail is the parsed marker, and report.md prints it.
 */
export function publishableFinding(finding: Finding): Finding {
  const out: Finding = { ...finding, message: redactCorpusText(finding.message) };
  if (finding.detail !== undefined) out.detail = redactCorpusValue(finding.detail);
  return out;
}

/**
 * A crash marker's text as it may be copied into a finding (#576 NF-W17-2,
 * RV15-1). A marker is serde_json output, so it is parsed and folded value by
 * value: folding its text never finds a name JSON escapes. A marker that does not
 * parse was cut off mid-write, and is withheld whole, its length kept
 * (#576 RV16-2): the cut can fall inside a name, which no fold then matches, so
 * the copy kept the name's beginning; and a torn marker has no field worth
 * reading anyway.
 */
export function publishableCrashMarker(text: string): string {
  try {
    return JSON.stringify(redactCorpusValue(JSON.parse(text) as unknown), null, 2);
  } catch {
    return `(withheld: the crash marker is not JSON - cut off mid-write? - so a corpus name in it could not be folded; ${text.length} characters)\n`;
  }
}

async function captureFinding(
  target: OracleTarget,
  recorder: RecorderLike,
  index: number,
  finding: Finding,
): Promise<void> {
  const dir = path.join(recorder.findingsDir, String(index).padStart(3, '0'));
  fs.mkdirSync(dir, { recursive: true });
  // Codes, counts and code locations only (#576 W21-NF2): finding.json is what
  // gets attached; the full finding is the `finding` event in actions.jsonl.
  fs.writeFileSync(path.join(dir, 'finding.json'), JSON.stringify(shareableFinding(finding), null, 2));
  // Screenshot (best-effort).
  try {
    const b64 = await target.exclusive(() => target.browser.takeScreenshot());
    fs.writeFileSync(path.join(dir, 'screenshot.png'), Buffer.from(b64, 'base64'));
  } catch {
    // Webview may be dead; the finding is still recorded.
  }
  // Copy any crash markers, through the corpus fold (#576 NF-W17-2): a marker
  // carries the panic message, which can quote a corpus copy path.
  for (const marker of target.findCrashMarkers()) {
    try {
      const text = fs.readFileSync(marker, 'utf8');
      fs.writeFileSync(path.join(dir, path.basename(marker)), publishableCrashMarker(text));
    } catch {
      // Best-effort.
    }
  }
}
