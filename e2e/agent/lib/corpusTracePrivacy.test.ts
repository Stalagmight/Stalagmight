/**
 * A corpus run's trace, and everything printed from it, never names a corpus
 * file (#576 HFI-13).
 *
 * Builds a trace the way `importDialogFuzz` does - a real draw from a synthetic
 * corpus, the real materialization, the real step builders - out of files whose
 * names are recognisable, and looks for those names in the trace FILE and in
 * what `fuzz-shrink` prints from it. The synthetic corpus lives in a temp
 * directory and is deleted afterwards; nothing here is a fixture.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { shortId } from './crc32';
import {
  CORPUS_DIR_ENV,
  corpusEntryFor,
  discoverCorpus,
  materializeCorpusEntry,
  redactCorpusText,
  registerCorpusRedactions,
  type CorpusEntry,
} from './fixtureCorpus';
import {
  CORPUS_ENTRY_SEGMENT,
  TraceRecorder,
  corpusEntryRel,
  describeSignature,
  describeSteps,
  failureSignature,
  readTrace,
  writeTrace,
  type FuzzTrace,
  type TraceStep,
} from './fuzzTrace';
import { drawFileSource, type FileSource, type FuzzMove } from '../scenarios/importDialogGenerators';
import { CorpusEntryPaths, moveTraceStep, sourceTraceStep } from '../scenarios/importDialogTrace';

/** Recognisable on purpose: if it shows up anywhere, the test sees it. */
const SECRET = 'ZzSecretCaveQ7x';
const PROJECT_DIR = `${SECRET}-expedition`;
const ENTRY = `${SECRET}.wpj`;
const SURVEY = `${SECRET}-north.srv`;

const saved: Record<string, string | undefined> = {};
const KNOBS = [CORPUS_DIR_ENV, 'STALAGMIGHT_FUZZ_CORPUS_DRAW_P', 'STALAGMIGHT_FUZZ_IMPORT_FORMAT'];
let scratch = '';
let corpusRoot = '';
let sessionDir = '';

beforeAll(() => {
  scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-corpus-privacy-'));
  corpusRoot = path.join(scratch, 'corpus');
  sessionDir = path.join(scratch, 'session');
  // A Walls project: the .wpj names its survey, which sits in a subfolder.
  fs.mkdirSync(path.join(corpusRoot, PROJECT_DIR, SECRET), { recursive: true });
  fs.writeFileSync(path.join(corpusRoot, PROJECT_DIR, ENTRY), `.BOOK ${SECRET}\n.PATH ${SECRET}\n.SURVEY ${SURVEY}\n.ENDBOOK\n`);
  fs.writeFileSync(path.join(corpusRoot, PROJECT_DIR, SECRET, SURVEY), 'A1 A2 10 90 0\n');
  for (const knob of KNOBS) saved[knob] = process.env[knob];
  process.env[CORPUS_DIR_ENV] = corpusRoot;
  process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = '1';
  process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = 'walls';
});

afterAll(() => {
  for (const knob of KNOBS) {
    if (saved[knob] === undefined) delete process.env[knob];
    else process.env[knob] = saved[knob];
  }
  fs.rmSync(scratch, { recursive: true, force: true });
});

/** Every recognisable piece of the synthetic corpus, and its root. */
const forbidden = (): string[] => [SECRET, PROJECT_DIR, ENTRY, SURVEY, corpusRoot];

const expectNoName = (text: string, where: string): void => {
  for (const name of forbidden()) {
    const hits = text.split(name).length - 1;
    expect(hits, `${where} names the corpus ${hits} time(s)`).toBe(0);
  }
};

/** Draw the project file the way a run does, and materialize it where a run would. */
function drawAndMaterialize(fileSeq: number): { source: Extract<FileSource, { origin: 'corpus' }>; opened: string } {
  const inventory = discoverCorpus();
  const corpus = { wpj: inventory.byFormat.wpj ?? [], srv: [] as readonly CorpusEntry[] };
  const env = {
    ctx: { rng: () => 0.01, randInt: () => 0 },
    digest: {},
    filesRel: 'fuzz-files',
    fileSeq,
    corpus,
  } as unknown as Parameters<typeof drawFileSource>[0];
  const source = drawFileSource(env);
  if (source.origin !== 'corpus') throw new Error(`expected a corpus draw, got ${source.origin}`);
  const entry = corpusEntryFor(source.corpusRel) as CorpusEntry;
  const destination = path.join(sessionDir, source.dirRel);
  const copied = materializeCorpusEntry(entry, destination);
  expect(copied.skipped).toBe(false);
  registerCorpusRedactions(source.id, destination, copied.names);
  return { source, opened: copied.path };
}

/** The steps a run records for one corpus file opened three ways. */
function recordRun(recorder: TraceRecorder): { source: Extract<FileSource, { origin: 'corpus' }>; opened: string } {
  const drawn = drawAndMaterialize(3);
  const moves: FuzzMove[] = [
    { kind: 'openFile', source: drawn.source, via: 'route', trigger: 'routeOpenedFile', label: 'import via router' },
    { kind: 'openFile', source: drawn.source, via: 'dialog', trigger: 'toolbar-import-button', label: 'import via toolbar' },
    { kind: 'keyFile', chord: 'Control+o', source: drawn.source, label: 'import via Ctrl+O' },
  ];
  for (const move of moves) {
    const sourceStep = sourceTraceStep(drawn.source);
    if (sourceStep !== null) recorder.record(sourceStep);
    const step = moveTraceStep(move);
    if (step !== null) recorder.record(step);
  }
  return drawn;
}

function buildTrace(recorder: TraceRecorder, failure: string): FuzzTrace {
  return recorder.build({
    scenario: 'importDialogFuzz',
    seed: 19,
    iterations: 4,
    fixture: 'fractal_plant_iter3.stalx',
    stream: 'import-v3',
    failure: { message: failure },
  });
}

describe('a corpus trace names no corpus file (#576 HFI-13)', () => {
  it('records the opened entry as a token, and the trace FILE carries no name, path or root', () => {
    const recorder = new TraceRecorder();
    const { source, opened } = recordRun(recorder);
    expect(path.basename(opened)).toBe(ENTRY);
    // What the app says when it cannot import a file: it quotes the path.
    const failure = redactCorpusText(
      `S4 UNEXPECTED ERROR DIALOG: a move with no business failing raised one\n` +
        `  error: could not read ${opened}: missing survey ${SURVEY}`,
    );
    const file = path.join(scratch, 'trace.json');
    writeTrace(file, buildTrace(recorder, failure));
    const json = fs.readFileSync(file, 'utf8');
    expectNoName(json, 'the trace file');
    expect(json).toContain(corpusEntryRel(source.dirRel));
    expect(json).toContain(`"corpusId": "${source.id}"`);
  });

  it('prints no name in what fuzz-shrink shows of the trace', () => {
    const recorder = new TraceRecorder();
    recordRun(recorder);
    const opened = path.join(sessionDir, 'fuzz-files', 'corpus-3', ENTRY);
    const failure = redactCorpusText(`F3 ACCOUNTING: the receipt does not match\n  error: ${opened} parsed 0 shots`);
    const file = path.join(scratch, 'shrink-input.json');
    writeTrace(file, buildTrace(recorder, failure));
    const trace = readTrace(file);
    // `fuzz-shrink.ts` prints exactly these for a trace and its baseline.
    const printed = [describeSteps(trace.steps), describeSignature(failureSignature(failure))].join('\n');
    expectNoName(printed, 'the shrink output');
    // The session path prints as its tag, as every path the repo does not track (#576 W74-NF2).
    expect(printed).toContain(`<private-path#${shortId(`fuzz-files/corpus-3/${CORPUS_ENTRY_SEGMENT}`)}>`);
  });

  it('resolves the token back to the copy on replay, and to nothing without its corpusFile step', () => {
    const recorder = new TraceRecorder();
    const { source, opened } = recordRun(recorder);
    const routed = recorder.snapshot().find((step): step is Extract<TraceStep, { kind: 'route' }> => step.kind === 'route');
    expect(routed?.rel).toBe(corpusEntryRel(source.dirRel));
    const paths = new CorpusEntryPaths();
    expect(paths.resolve(routed?.rel ?? '')).toBeNull();
    paths.bind(source.dirRel, opened);
    expect(paths.resolve(routed?.rel ?? '')).toBe(opened);
    expect(paths.resolve('fuzz-files/fuzz-2.dmp')).toBeUndefined();
  });

  it('refuses to write or read a trace that opens a corpus copy by its name, without quoting it', () => {
    const recorder = new TraceRecorder();
    const { source } = recordRun(recorder);
    const leaky: FuzzTrace = buildTrace(recorder, 'x');
    leaky.steps.push({ kind: 'route', rel: `${source.dirRel}/${ENTRY}`, label: 'a hand-edited trace' });
    let refusal = '';
    try {
      writeTrace(path.join(scratch, 'leaky.json'), leaky);
    } catch (err) {
      refusal = err instanceof Error ? err.message : String(err);
    }
    expect(refusal).toMatch(/opens a file inside a corpus copy by its name/);
    expectNoName(refusal, 'the write refusal');

    const onDisk = path.join(scratch, 'leaky-on-disk.json');
    fs.writeFileSync(onDisk, JSON.stringify(leaky));
    let readRefusal = '';
    try {
      readTrace(onDisk);
    } catch (err) {
      readRefusal = err instanceof Error ? err.message : String(err);
    }
    expect(readRefusal).toMatch(/re-record it on this build/);
    expectNoName(readRefusal, 'the read refusal');
  });

  it('redacts a printed line that quotes the copy, whole paths first', () => {
    const { opened } = drawAndMaterialize(4);
    const line = `findings=[{"message":"ERROR import failed for ${opened}"},{"message":"${SURVEY} not found"}]`;
    const redacted = redactCorpusText(line);
    expectNoName(redacted, 'a redacted findings line');
    expect(redacted).toContain('<corpus #');
  });
});
