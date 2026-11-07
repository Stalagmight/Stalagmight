/**
 * #576 HFI2-F2: what a run publishes quotes nothing the app logged about the
 * loaded data.
 *
 * The oracle sweep turns every new WARN/ERROR ring line into a finding whose
 * message IS the line, and the app's lines name the data they are about:
 * `Section "<name>" has no shots after filtering`, a project in a datum
 * warning. `agent:scenario`'s PASS/FAIL line printed the findings as JSON, and
 * the corpus fold hides corpus FILE names only - so on a private-fixture
 * campaign, run logs carried project and section names. The summary line now
 * carries counts per severity, kind and code (the log TARGET, which the app's
 * code names), the repro report the code instead of the line, and the full line
 * stays in the session directory, which docs/agent-harness.md says not to attach.
 *
 * Everything here is synthetic: two token names and the lines the backend
 * writes about them.
 */
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { checkIpcContract } from './importFlowOracles';
import { findingsSummary, logLineCode, newOracleState, runSweep, type OracleTarget } from './oracles';
import { Recorder } from './recorder';

const PROJECT = 'qzhfi2project';
const SECTION = 'qzhfi2section';
const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');

const WARN_TML = `WARN stalagmight_parsers::tml: Section "${SECTION} north" has no shots after filtering`;
const WARN_IMPORT = `WARN stalagmight_lib::import_runner: Section skipped {section=${SECTION}, project=${PROJECT}}`;
const ERROR_DATUM = `ERROR stalagmight_lib::commands::station: Failed to set zero datum in ${PROJECT}: ${SECTION} has no station 0`;

let scratch = '';

beforeAll(() => {
  scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-finding-text-'));
});

afterAll(() => {
  fs.rmSync(scratch, { recursive: true, force: true });
});

/** A webview whose log ring holds `lines`, and no error boundary. */
function webviewLogging(lines: readonly string[]): OracleTarget {
  const browser = {
    executeAsync: async (_fn: unknown, first: unknown) => {
      if (first === 'get_recent_logs_since') {
        const entries = lines.map((line, i) => ({ seq: i + 1, ts_ms: 0, line }));
        return { value: { entries, latest_seq: lines.length, dropped: false } };
      }
      return { value: { errorBoundary: false } };
    },
    takeScreenshot: async () => {
      throw new Error('no screen in a unit test');
    },
  };
  return {
    browser: browser as unknown as OracleTarget['browser'],
    exclusive: (fn) => fn(),
    findCrashMarkers: () => [],
  };
}

const namesIn = (text: string): string[] => [PROJECT, SECTION].filter((name) => text.includes(name));

/** Sweep `lines` into a fresh session, write its repro report, return both. */
async function sweptSession(id: string, lines: readonly string[]) {
  const recorder = new Recorder(id, scratch);
  const sweep = await runSweep(webviewLogging(lines), newOracleState(), { recorder });
  const session = path.join(scratch, id);
  const printed = execFileSync('node', [path.join(REPO, 'scripts', 'generate-repro-report.mjs'), session], {
    encoding: 'utf8',
  });
  return {
    sweep,
    report: fs.readFileSync(path.join(session, 'report.md'), 'utf8') + printed,
    actions: fs.readFileSync(path.join(session, 'actions.jsonl'), 'utf8'),
  };
}

describe('a log line naming survey data reaches no published artifact (#576 HFI2-F2)', () => {
  it('names a log finding by its target, never by its text', () => {
    expect([WARN_TML, WARN_IMPORT, ERROR_DATUM].map(logLineCode)).toEqual([
      'stalagmight_parsers::tml',
      'stalagmight_lib::import_runner',
      'stalagmight_lib::commands::station',
    ]);
    // A line without a target says so rather than quoting itself.
    expect(logLineCode(`WARN ${SECTION} has no shots`)).toBe('unattributed');
    expect(logLineCode(`ERROR webview: [unhandled-rejection] ${PROJECT}`)).toBe('webview');
    expect(logLineCode(`[unhandled-error] ${PROJECT} is undefined`)).toBe('unhandled-error');
  });

  it('prints the summary line as counts per severity, kind and code', async () => {
    const { sweep } = await sweptSession('summary', [WARN_TML, WARN_TML, WARN_IMPORT, ERROR_DATUM]);
    // The sweep itself still has the text: the session keeps it for triage.
    expect(sweep.findings.map((f) => f.kind)).toEqual(['log-warn', 'log-warn', 'log-warn', 'log-error']);

    const summary = findingsSummary(sweep.findings);

    expect(JSON.parse(summary)).toEqual({
      'error:log-error@stalagmight_lib::commands::station': 1,
      'warning:log-warn@stalagmight_lib::import_runner': 1,
      'warning:log-warn@stalagmight_parsers::tml': 2,
    });
    expect(namesIn(summary)).toEqual([]);
  });

  it('keeps every key to identifier characters, whatever a kind or code holds', () => {
    const summary = findingsSummary([
      { severity: 'warning', kind: `log-warn ${SECTION}"}`, code: `x ${PROJECT}`, message: SECTION },
    ]);
    expect(Object.keys(JSON.parse(summary) as object)).toEqual([
      `warning:log-warn?${SECTION}??@x?${PROJECT}`,
    ]);
  });

  it('withholds the lines from the repro report, which keeps the kind and the target', async () => {
    // Warnings only: the report lists the sweep's finding events.
    const warned = await sweptSession('warned', [WARN_TML, WARN_IMPORT]);
    // An error: the report lists the captured findings/<n>/finding.json.
    const errored = await sweptSession('errored', [ERROR_DATUM]);

    expect({ warned: namesIn(warned.report), errored: namesIn(errored.report) }).toEqual({ warned: [], errored: [] });
    expect(warned.report).toContain('log-warn');
    expect(warned.report).toContain('stalagmight_parsers::tml');
    expect(errored.report).toContain('log-error');
    expect(errored.report).toContain('stalagmight_lib::commands::station');
    // Where the text IS kept: the session's action log, which is not attached.
    expect(namesIn(warned.actions)).toEqual([PROJECT, SECTION]);
  });

  it('turns no such line into a failure message, the only text a fuzz trace carries from the app', () => {
    // The import fuzzer reads error lines into a failure (and so into the trace)
    // only through the S6 IPC-contract oracle, for deserialization rejections.
    expect(() => checkIpcContract([WARN_TML, WARN_IMPORT, ERROR_DATUM], 'a move')).not.toThrow();
  });

  it('has every run summary print findings through findingsSummary', () => {
    const runners = ['scenario-run.ts', 'scenarios-smoke.ts', 'journeys-run.ts'].map((file) => ({
      file,
      source: fs.readFileSync(path.join(REPO, 'e2e', 'agent', file), 'utf8'),
    }));
    const verdicts = runners.map(({ file, source }) => ({
      file,
      viaSummary: /(?:findings|sweep)=\$\{findingsSummary\(/.test(source),
      rawJson: /JSON\.stringify\((?:sweep\.findings|breaches)\)/.test(source),
    }));
    expect(verdicts).toEqual(runners.map(({ file }) => ({ file, viaSummary: true, rawJson: false })));
  });
});
