/**
 * #576 RV13-4: no line of the session's action log names a corpus file.
 *
 * The MCP server recorded a successful tool call's params and result as they
 * came: `file_dialog_answer` carries the path of the corpus copy it picks, and
 * the `logs` tool returns the backend's line for a failed parse, which quotes
 * that path, real names and all. Only findings and error outcomes went through
 * the corpus redaction (RV12-F1). Now the recorder folds every line it writes,
 * and the server caps a result only after redacting it.
 *
 * NF-W16FE-1: the same holds for the session's `state/` snapshots. A store
 * snapshot taken after a corpus import carries the copy's path, and
 * `Recorder.writeState` wrote it as given.
 *
 * NF-W16FE-2: `run_scenario` folded its findings through a JSON-text pass of its
 * own before `textResult`, which folds every answer anyway. That pass is gone;
 * the answer is the same.
 *
 * Everything here is synthetic: a token-named corpus entry built in a temp
 * directory, and the lines the app would echo back for it.
 */
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import {
  CORPUS_DIR_ENV,
  corpusEntryFor,
  discoverCorpus,
  materializeCorpusEntry,
  redactCorpusText,
  redactCorpusValue,
  registerCorpusRedactions,
} from './fixtureCorpus';
import { describeRunError } from './fuzzHarness';
import { publishableFinding, type Finding } from './oracles';
import { capJson } from './page';
import { Recorder } from './recorder';

const MARK = 'qzactionlogmark';
const TRIP = `${MARK}-trip`;
const FILE = `${MARK}-broken.dmp`;
const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');

let scratch = '';
let sessionsRoot = '';
let copyPath = '';
const savedCorpusDir = process.env[CORPUS_DIR_ENV];

beforeAll(() => {
  scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-corpus-actionlog-'));
  const corpusRoot = path.join(scratch, 'corpus');
  sessionsRoot = path.join(scratch, 'sessions');
  fs.mkdirSync(path.join(corpusRoot, TRIP), { recursive: true });
  fs.writeFileSync(path.join(corpusRoot, TRIP, FILE), 'not an mnemo dump\n');
  process.env[CORPUS_DIR_ENV] = corpusRoot;
  const entry = corpusEntryFor(discoverCorpus().entries[0]?.relPath ?? '');
  if (entry === null) throw new Error('the synthetic corpus entry was not discovered');
  // Materialized and registered exactly as the import-dialog fuzzer does it.
  const destination = path.join(sessionsRoot, 'run-1', 'fuzz-files', 'corpus-0');
  const copied = materializeCorpusEntry(entry, destination);
  registerCorpusRedactions(entry.id, destination, copied.names);
  copyPath = copied.path;
});

afterAll(() => {
  if (savedCorpusDir === undefined) delete process.env[CORPUS_DIR_ENV];
  else process.env[CORPUS_DIR_ENV] = savedCorpusDir;
  fs.rmSync(scratch, { recursive: true, force: true });
});

/** The backend's line for a failed parse (`commands/import.rs`), quoting the copy. */
const failedParseLine = () =>
  `ERROR stalagmight_lib::commands::import: Failed to parse DMP file for import preview path="${copyPath}" reason="not a dump"`;

describe('the action log names no corpus file (#576 RV13-4)', () => {
  it('folds the copy path out of params, results, resolved targets, events and keys', () => {
    expect(copyPath).toContain(MARK); // the input really does carry the name
    const recorder = new Recorder('run-1', sessionsRoot);
    recorder.sessionStart({
      sessionId: 'run-1',
      startedAt: '2026-01-01T00:00:00Z',
      gitHead: 'abc',
      binaryPath: '/bin/app',
      binaryMtime: 0,
      display: ':99',
      headed: false,
      profileDir: '/profile',
      freshProfile: true,
    });
    // A file dialog answered with the copy, then the `logs` tool reading the failure.
    recorder.action({
      seq: recorder.nextSeq(),
      ts: '2026-01-01T00:00:01Z',
      tool: 'file_dialog_answer',
      mutating: true,
      params: { paths: [copyPath] },
      resolved: { [copyPath]: 'picked' },
      outcome: { ok: true, durationMs: 3 },
      result: { answered: copyPath },
    });
    recorder.action({
      seq: recorder.nextSeq(),
      ts: '2026-01-01T00:00:02Z',
      tool: 'logs',
      params: { level: 'error' },
      outcome: { ok: true, durationMs: 1 },
      result: { newEntries: [failedParseLine()], errorCount: 1 },
    });
    recorder.action({
      seq: recorder.nextSeq(),
      ts: '2026-01-01T00:00:03Z',
      tool: 'file_dialog_answer',
      mutating: true,
      params: { paths: [copyPath] },
      outcome: { ok: false, durationMs: 2, error: 'refused' },
    });
    recorder.event('scenario-log', { message: `opened ${copyPath}` });

    const log = fs.readFileSync(path.join(sessionsRoot, 'run-1', 'actions.jsonl'), 'utf8');
    const lines = log.trim().split('\n');
    expect(lines).toHaveLength(5);
    // Every line is still JSON, and still says which entry it was.
    const records = lines.map((l) => JSON.parse(l) as Record<string, unknown>);
    expect(records.map((r) => r.type)).toEqual(['session-start', 'action', 'action', 'action', 'event']);
    expect(log).not.toContain(MARK);
    expect(log).toContain('<corpus #');
  });

  it('redacts a value string by string, so a name JSON would escape is folded too', () => {
    const odd = `${copyPath}"\\`;
    const redacted = redactCorpusValue({ list: [odd, 3, null, true], [copyPath]: { nested: copyPath } });
    expect(JSON.stringify(redacted)).not.toContain(MARK);
    expect(redacted).toEqual({
      list: [redactCorpusText(odd), 3, null, true],
      [redactCorpusText(copyPath)]: { nested: redactCorpusText(copyPath) },
    });
  });

  it('caps a recorded result only after redacting it: a cut through the name leaks no part of it', () => {
    // A result whose 4096-byte cap falls right after the name's first token.
    const line = failedParseLine();
    const opening = JSON.stringify({ newEntries: [''] }).indexOf('""') + 1; // `{"newEntries":["`
    // The line's JSON form up to the end of the first token, escapes included.
    const head = JSON.stringify(line.slice(0, line.indexOf(MARK) + MARK.length)).slice(1, -1);
    const result = { newEntries: [`${'x'.repeat(4096 - opening - head.length)}${line}`] };
    // Capping first leaves a stub the folding cannot recognise.
    expect(redactCorpusText(String(capJson(result, 4096).value))).toContain(MARK);
    expect(JSON.stringify(capJson(redactCorpusValue(result), 4096).value)).not.toContain(MARK);
    // ...and the MCP server records it in that order.
    const server = fs.readFileSync(path.join(REPO, 'e2e', 'agent', 'mcp', 'server.ts'), 'utf8');
    expect(server).toContain('result: capJson(redactCorpusValue(result), 4096).value,');
  });

  /**
   * #576 RV18-W23-6: `element_state`, `eval`, `ipc` and `driver_script` capped
   * their result at 16 KB before anything folded it. `record`'s own 4 KB cap
   * keeps a cut that far in out of the action log, but `textResult` returned the
   * capped text to the agent, and the start of a name across the cut with it.
   */
  it('caps a 16 KB tool result only after folding it, at every site the server caps one (#576 RV18-W23-6)', () => {
    const line = failedParseLine();
    const opening = JSON.stringify({ newEntries: [''] }).indexOf('""') + 1;
    const head = JSON.stringify(line.slice(0, line.indexOf(MARK) + MARK.length)).slice(1, -1);
    const result = { newEntries: [`${'x'.repeat(16384 - opening - head.length)}${line}`] };
    // What `run` hands `textResult` (which folds) for each order of fold and cap.
    const answer = (capped: { value: unknown; truncated: boolean }) => JSON.stringify(redactCorpusValue({ ...capped }));
    expect(answer(capJson(result, 16384))).toContain(MARK);
    expect(answer(capJson(redactCorpusValue(result), 16384))).not.toContain(MARK);
    // ...and the server caps a folded value wherever it caps one.
    const server = fs.readFileSync(path.join(REPO, 'e2e', 'agent', 'mcp', 'server.ts'), 'utf8');
    const lineOf = (index: number) => server.slice(0, index).split('\n').length;
    const unfolded = [...server.matchAll(/capJson\((?!redactCorpusValue\()/g)].map((m) => `server.ts:${lineOf(m.index)}`);
    expect(unfolded).toEqual([]);
    expect(server.match(/capJson\(redactCorpusValue\(/g)?.length).toBeGreaterThanOrEqual(2);
  });

  it("folds a stalled WebDriver call's error line before cutting it to 240 characters (#576 RV18-W23-6)", () => {
    // webdriver appends the in-page script to the message; here it quotes the
    // copy's file name across the cut, ten characters of it before 240.
    const stub = FILE.slice(0, 10);
    const opening = 'WebDriverError: The operation was aborted due to timeout when running "execute/async" with method "POST" and args "open ';
    const first = `${opening}${'p'.repeat(230 - opening.length)}${FILE} north"`;
    expect(first.indexOf(FILE)).toBe(230);
    const report = describeRunError(new Error(first), 'open the import dialog');
    expect(report.stall).toBe(true);
    const errorLine = report.detail.split('\n').find((l) => l.startsWith('  error: ')) ?? '';
    expect(errorLine.length).toBe('  error: '.length + 240 + '...'.length); // still cut, after the fold
    // The failure reaches the action log through the recorder, which folds again.
    const recorder = new Recorder('run-stall', sessionsRoot);
    recorder.event('scenario-failure', { scenario: 'importDialogFuzz', message: `${report.headline}\n${report.detail}` });
    const log = fs.readFileSync(path.join(sessionsRoot, 'run-stall', 'actions.jsonl'), 'utf8');
    expect({ detail: report.detail.includes(stub), actions: log.includes(stub) }).toEqual({ detail: false, actions: false });
    expect(errorLine).toContain('<corpus #');
  });

  it('the MCP server folds every tool result it returns, as it does its errors (the `logs` tool quotes the copy)', () => {
    const server = fs.readFileSync(path.join(REPO, 'e2e', 'agent', 'mcp', 'server.ts'), 'utf8');
    expect(server).toMatch(
      /function textResult\(value: unknown\): ToolResult \{\s*return \{ content: \[\{ type: 'text', text: JSON\.stringify\(redactCorpusValue\(value\) \?\? null\) \}\] \};/,
    );
  });
});

describe('a state snapshot names no corpus file (#576 NF-W16FE-1)', () => {
  it('folds the copy path out of every string and key of the snapshot, and keeps its shape', () => {
    const recorder = new Recorder('run-2', sessionsRoot);
    // What a store snapshot after a corpus import looks like: the open path, a
    // recent-projects list keyed by path, and a log line quoting the copy.
    const snapshot = {
      projectPath: copyPath,
      recentProjects: [{ path: copyPath, name: path.basename(copyPath) }],
      byPath: { [copyPath]: { opened: true } },
      lastError: failedParseLine(),
      count: 3,
      flags: [true, null],
    };
    const rel = recorder.writeState(recorder.nextSeq(), snapshot);
    expect(rel).toBe(path.join('state', '0001.json'));

    const text = fs.readFileSync(path.join(sessionsRoot, 'run-2', rel), 'utf8');
    expect(text).not.toContain(MARK);
    expect(text).toContain('<corpus #');
    // Still pretty-printed JSON, the same keys and non-string values.
    expect(text).toContain('\n  "projectPath": ');
    expect(JSON.parse(text)).toEqual(redactCorpusValue(snapshot));
    expect(JSON.parse(text)).toMatchObject({ count: 3, flags: [true, null] });
  });

  it('the repro report embeds no screenshot (they are app pixels and may show a corpus name): it names the folder only', () => {
    const recorder = new Recorder('run-3', sessionsRoot);
    const seq = recorder.nextSeq();
    const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
    const shot = recorder.writeScreenshot(seq, 'import-dialog', png);
    recorder.action({
      seq,
      ts: '2026-01-01T00:00:01Z',
      tool: 'screenshot',
      params: { name: 'import-dialog' },
      outcome: { ok: true, durationMs: 1 },
      result: { path: shot },
      artifacts: { screenshot: shot },
    });
    const session = path.join(sessionsRoot, 'run-3');
    execFileSync('node', [path.join(REPO, 'scripts', 'generate-repro-report.mjs'), session], { encoding: 'utf8' });
    const report = fs.readFileSync(path.join(session, 'report.md'), 'utf8');
    expect(report).toContain('`screenshots/`');
    expect(report).not.toMatch(/!\[|<img|data:image|iVBORw0KGgo/);
  });
});

describe('run_scenario returns its findings through textResult alone (#576 NF-W16FE-2)', () => {
  it('dropping the extra JSON-text fold leaves the answer unchanged', () => {
    // The findings as runSweep hands them over: already publishable.
    const raw: Finding[] = [
      { severity: 'error', kind: 'log-error', message: failedParseLine() },
      {
        severity: 'warning',
        kind: 'probe',
        message: `opened ${copyPath}`,
        detail: { path: copyPath, [copyPath]: [copyPath, 1, null], when: new Date(0), skipped: undefined },
      },
      { severity: 'fatal', kind: 'odd', message: 'escapes', detail: { name: `${copyPath}"\\`, list: [`"${copyPath}"`] } },
    ];
    const findings = raw.map(publishableFinding);
    // The pass server.ts ran before textResult, as it was.
    const jsonTextFold = (value: unknown): unknown => {
      try {
        return JSON.parse(redactCorpusText(JSON.stringify(value ?? null))) as unknown;
      } catch {
        return '(withheld: redacting a corpus name broke the encoding)';
      }
    };
    // textResult's text for run_scenario's answer.
    const answer = (f: unknown) =>
      JSON.stringify(redactCorpusValue({ scenario: 'probe', metrics: {}, ok: false, worst: 'fatal', findings: f }) ?? null);
    expect(answer(findings)).toBe(answer(jsonTextFold(findings)));
    expect(answer(findings)).toContain('<corpus #');
    expect(answer(findings)).not.toContain(MARK);

    const server = fs.readFileSync(path.join(REPO, 'e2e', 'agent', 'mcp', 'server.ts'), 'utf8');
    expect(server).not.toContain('redactedJson');
    expect(server).toContain('findings: sweep.findings,');
  });
});
