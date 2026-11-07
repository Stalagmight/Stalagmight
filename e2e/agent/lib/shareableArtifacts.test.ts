/**
 * #576 W21-NF1 / W21-NF2: what a run SHARES carries codes, counts and code
 * locations, never the app's text about the loaded data.
 *
 * Shareable: the runners' summary and FAIL lines, a fuzz trace, `fuzz-shrink`'s
 * and `fuzz-matrix`'s output, `report.md`, and `findings/<n>/finding.json`. The
 * full text stays in the session's `actions.jsonl` (docs/agent-harness.md lists
 * what is not for sharing). Before, an INTEGRITY or S6 IPC CONTRACT failure
 * quoted integrity messages and the backend's argument values into the FAIL
 * line and the trace, and finding.json / report.md printed panic messages, the
 * error boundary's text, integrity messages and leftover temp file names.
 *
 * Everything here is synthetic: three token names and text the app writes about
 * them.
 */
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { newOracleState, runSweep, type OracleTarget } from './oracles';
import { Recorder } from './recorder';
import { TraceRecorder, failureSignature, readTrace, sameSignature, writeTrace } from './fuzzTrace';

const PROJECT = 'qznf2project';
const SECTION = 'qznf2section';
const STATION = 'qznf2station';
const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');
const namesIn = (text: string): string[] => [PROJECT, SECTION, STATION].filter((name) => text.includes(name));

let scratch = '';
beforeAll(() => {
  scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-shareable-'));
});
afterAll(() => {
  fs.rmSync(scratch, { recursive: true, force: true });
});

/** Every file named `name` under `dir`, as text. */
function filesNamed(dir: string, name: string): string[] {
  return fs
    .readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile() && e.name === name)
    .map((e) => fs.readFileSync(path.join(e.parentPath, e.name), 'utf8'));
}

/**
 * An app in trouble over the synthetic project: a panic marker, the error
 * boundary up, an error log line, a save-blocking integrity verdict and a
 * leftover temp file named after the project.
 */
function troubledApp(): OracleTarget {
  const projectDir = path.join(scratch, 'project');
  fs.mkdirSync(projectDir, { recursive: true });
  fs.writeFileSync(path.join(projectDir, `${PROJECT}.stalx`), '');
  fs.writeFileSync(path.join(projectDir, `${PROJECT}.stalx.tmp`), '');
  const marker = path.join(scratch, 'crash-41-42-0.json');
  fs.writeFileSync(
    marker,
    JSON.stringify({
      message: `called \`Option::unwrap()\` on a \`None\` value: section ${SECTION} has no station ${STATION}`,
      location: 'crates/models/src/project/offsets.rs:120:9',
      thread: 'main',
    }),
  );
  const line = `ERROR stalagmight_lib::commands::station: Failed to rename ${STATION} in ${SECTION} of ${PROJECT}`;
  const ipc: Record<string, unknown> = {
    get_recent_logs_since: { entries: [{ seq: 1, ts_ms: 0, line }], latest_seq: 1, dropped: false },
    check_project_integrity: { messages: [`Station ${STATION} in ${SECTION} has a non-finite coordinate`], save_blocked: true },
    get_memory_stats: { total_rss_kb: 1000, webkit_processes: [], backend: { rss_kb: 500 } },
  };
  const browser = {
    executeAsync: async (_fn: unknown, first: unknown) =>
      typeof first === 'string' ? { value: ipc[first] } : { value: { errorBoundary: true, text: `Could not draw ${PROJECT}` } },
    execute: async () => ({ value: path.join(projectDir, `${PROJECT}.stalx`), truncated: false }),
    takeScreenshot: async () => {
      throw new Error('no screen in a unit test');
    },
  };
  return { browser: browser as unknown as OracleTarget['browser'], exclusive: (fn) => fn(), findCrashMarkers: () => [marker] };
}

describe('a full sweep shares no app text (#576 W21-NF2)', () => {
  it('writes finding.json and report.md with codes, counts and code locations only', async () => {
    const recorder = new Recorder('nf2', scratch);
    const sweep = await runSweep(troubledApp(), newOracleState(), { full: true, recorder });
    const session = path.join(scratch, 'nf2');
    const printed = execFileSync('node', [path.join(REPO, 'scripts', 'generate-repro-report.mjs'), session], { encoding: 'utf8' });
    const report = fs.readFileSync(path.join(session, 'report.md'), 'utf8') + printed;
    const findingJson = filesNamed(path.join(session, 'findings'), 'finding.json');
    const actions = fs.readFileSync(path.join(session, 'actions.jsonl'), 'utf8');

    expect(sweep.findings.map((f) => f.kind).sort()).toEqual(
      ['crash-marker', 'error-boundary', 'integrity-save-blocked', 'leftover-temp', 'log-error'].sort(),
    );
    expect(findingJson).toHaveLength(5);
    expect({ findingJson: namesIn(findingJson.join('\n')), report: namesIn(report) }).toEqual({ findingJson: [], report: [] });
    // Still says what happened and where in the code.
    expect(report).toContain('crates/models/src/project/offsets.rs:120:9');
    expect(report).toContain('from `stalagmight_lib::commands::station`');
    expect(report).toContain('1 leftover write-staging temp file(s)');
    expect(report).toContain('".stalx.tmp": 1');
    expect(report).toContain('"messages": 1');
    expect(report).toContain('"textChars"');
    // The private record keeps the full text, detail included, for triage.
    expect(namesIn(actions)).toEqual([PROJECT, SECTION, STATION]);
  });
});

/** A failure as the import fuzzer and the undo fuzzers word them, app text and all. */
const FULL_FAILURE = [
  'INTEGRITY: an import left the project in a state the app refuses to save',
  '  reproduce: pnpm agent:scenario import-dialog-fuzz --seed 7 --iterations 40',
  '  fixture=empty.stalx flowsPerEpisode=3 maxFlowSteps=12',
  '  after: open import dialog',
  '  save_blocked=true',
  `  Station ${STATION} in ${SECTION} has a non-finite coordinate`,
  `  ERROR tauri: invalid args \`name\` for command \`rename_section\`: invalid type: string "${SECTION}"`,
  '  at project.sections[3].name',
  `    expected: "${SECTION}"`,
  `    actual:   "${STATION}"`,
  `  error: Section ${SECTION} not found in ${PROJECT}`,
  `  INVARIANT: station ${STATION} is orphaned`,
  '  ATTRIBUTED [rv9-merge]: a known merge drift',
  '  last 2 move(s):',
  '    open import dialog [idle]',
  `    rename to ${SECTION} -> FAILED [Section ${SECTION} exists]`,
  '  trace: /tmp/session/traces/import-dialog-fuzz-seed7.json',
].join('\n');

describe('a scenario failure shares no app text (#576 W21-NF1)', () => {
  it('keeps the headline, the reproduce and trace lines, diff paths and classifiers, and counts the rest', async () => {
    const { shareableFailureText } = await import('./shareableFailure');
    const shared = shareableFailureText(FULL_FAILURE);
    expect(namesIn(shared)).toEqual([]);
    expect(shared.split('\n')).toEqual([
      'INTEGRITY: an import left the project in a state the app refuses to save',
      '  reproduce: pnpm agent:scenario import-dialog-fuzz --seed 7 --iterations 40',
      "  (5 line(s) withheld: they can quote the app's text - the full failure is in the session's actions.jsonl)",
      '  at project.sections[3].name',
      "  (4 line(s) withheld: they can quote the app's text - the full failure is in the session's actions.jsonl)",
      '  ATTRIBUTED [rv9-merge]',
      '  last 2 move(s):',
      "  (2 line(s) withheld: they can quote the app's text - the full failure is in the session's actions.jsonl)",
      '  trace: /tmp/session/traces/import-dialog-fuzz-seed7.json',
    ]);
    // Projecting twice changes nothing, so a trace rewritten keeps its counts.
    expect(shareableFailureText(shared)).toBe(shared);
    // An error raised outside `fail` keeps the code path, not the app's words.
    const appError = `Section ${SECTION} already exists`;
    expect(shareableFailureText(`ipc rename_section: ${appError}`)).toBe(
      `ipc rename_section: (${appError.length} characters of the app's text withheld)`,
    );
    // So does a scenario's `<action> failed: <its error>` headline...
    const refused = shareableFailureText(`createSection 3 failed: ${appError}`);
    expect(refused).toBe(`createSection 3 failed: (${appError.length} characters of the app's text withheld)`);
    expect(shareableFailureText(refused)).toBe(refused);
    // ...and a headline that ends in values, such as the integrity messages E2 quotes.
    const messages = JSON.stringify([`Station ${STATION} in ${SECTION} has a non-finite coordinate`]);
    const e2 = 'E2 INTEGRITY: empty.stalx opened SAVE-BLOCKED at the start of episode 3';
    expect(shareableFailureText(`${e2}: ${messages}`)).toBe(
      `${e2}: (${messages.length} characters of the app's text withheld)`,
    );
    // A harness-worded headline is kept whole.
    const wedge = 'S5 WEDGE on "wiz-manual": no actionable control';
    expect(shareableFailureText(wedge)).toBe(wedge);
  });

  it('prints a FAIL block the matrix still reads: headline, reproduce line, detail', async () => {
    const { scenarioFailOutput } = await import('./shareableFailure');
    const { failHeadline, failureDetail } = await import('./fuzzMatrixReport');
    const printed = scenarioFailOutput('import-dialog-fuzz', new Error(FULL_FAILURE));
    const outLines = ['session started (debug build reused)', ...printed.split('\n')];
    const failIndex = outLines.findIndex((line) => line.startsWith('FAIL: '));

    expect(namesIn(printed)).toEqual([]);
    expect(failHeadline(outLines, failIndex)).toBe(
      'import-dialog-fuzz threw: INTEGRITY: an import left the project in a state the app refuses to save',
    );
    expect(outLines.find((line) => line.trim().startsWith('reproduce:'))?.trim()).toBe(
      'reproduce: pnpm agent:scenario import-dialog-fuzz --seed 7 --iterations 40',
    );
    expect(failureDetail({ outLines, allLines: outLines }).split('\n')[0]).toBe('FAIL: import-dialog-fuzz threw:');
  });

  it('reads the metrics off a PASS line whose findings are counts', async () => {
    const { parseMetrics } = await import('./fuzzMatrixReport');
    const line =
      'PASS: import-dialog-fuzz seed=7 iterations=40 mode=debug (81234ms) {"flows":12,"s4NoWork:busy":3,"rate":0.5} ' +
      'findings={"error:log-error@stalagmight_lib::commands::station":1}';
    expect(parseMetrics(line)).toEqual({ flows: 12, 's4NoWork:busy': 3, rate: 0.5 });
    expect(parseMetrics('PASS: x (12ms) {"a":1}')).toEqual({ a: 1 });
    expect(parseMetrics('PASS: x (12ms) not json findings={"k":1}')).toEqual({});
  });

  it('writes a trace with the shareable failure and its signature', () => {
    const recorder = new TraceRecorder();
    recorder.record({ kind: 'mutate', action: 'renameSection', args: [3, 'qzgenerated'], label: 'renameSection(s=3)' });
    const file = path.join(scratch, 'traces', 'nf1.json');
    writeTrace(
      file,
      recorder.build({
        scenario: 'import-dialog-fuzz',
        seed: 7,
        iterations: 40,
        fixture: 'empty.stalx',
        stream: 'default',
        failure: { message: FULL_FAILURE },
      }),
    );

    expect(namesIn(fs.readFileSync(file, 'utf8'))).toEqual([]);
    const trace = readTrace(file);
    expect(trace.failure?.signature).toEqual({
      headline: 'INTEGRITY: an import left the project in a state the app refuses to save',
      path: 'project.sections[*].name',
      error: null,
      classifiers: ['rv9-merge'],
    });
  });

  it('keeps the shrinker comparing like with like', async () => {
    const { shareableSignature } = await import('./fuzzTrace');
    // ddmin compares FULL signatures in memory: two failures that differ only
    // in the app's error text are still told apart there...
    const other = FULL_FAILURE.replace(`error: Section ${SECTION} not found`, 'error: Section other not found');
    expect(sameSignature(failureSignature(FULL_FAILURE), failureSignature(other))).toBe(false);
    // ...while what `fuzz-shrink` prints and checks against the trace header is
    // the shareable signature, the same for a full message and its trace copy.
    const file = path.join(scratch, 'traces', 'nf1.json');
    const recorded = readTrace(file).failure?.message ?? '';
    expect(sameSignature(shareableSignature(recorded), shareableSignature(FULL_FAILURE))).toBe(true);
    expect(namesIn(JSON.stringify(shareableSignature(FULL_FAILURE)))).toEqual([]);
    // A different defect still signs differently.
    const wedge = FULL_FAILURE.replace(/^INTEGRITY:[^\n]*/, 'S5 WEDGE on "wiz-manual": no actionable control');
    expect(sameSignature(shareableSignature(wedge), shareableSignature(FULL_FAILURE))).toBe(false);
  });
});
