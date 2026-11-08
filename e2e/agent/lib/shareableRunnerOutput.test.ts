/**
 * #576 W23-NF1..NF3, RV18-W23-4: what the runners print around a verdict is
 * shareable too - the lines a matrix keeps of a cell that died, a harness
 * crash, the progress lines a scenario prints, and the paths in kept lines.
 * Codes, counts and code locations only; the app's text stays out.
 *
 * Everything here is synthetic: token names (`qz...`) stand in for a user, a
 * station and a project.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import ts from 'typescript';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { foldLocalPaths, shareableFailureText } from './shareableFailure';

const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');
const HOME = '/home/qzbob';
const SESSION = `${REPO}/.agent-artifacts/sessions/s1`;
/** The synthetic user, project, section and station a leak would show. */
const names = (text: string): string[] =>
  ['qzbob', 'qzproject', 'qzsection', 'qzstation'].filter((name) => text.includes(name));

let savedHome: string | undefined;
beforeEach(() => {
  savedHome = process.env.HOME;
  process.env.HOME = HOME;
});
afterEach(() => {
  if (savedHome === undefined) delete process.env.HOME;
  else process.env.HOME = savedHome;
});

describe('kept lines carry no local path or name (#576 RV18-W23-4)', () => {
  it('folds the home directory and the checkout out of the headline and every kept line', () => {
    const shared = shareableFailureText(
      [
        `generated file path "g/a.dat" escapes ${SESSION}`,
        `  reproduce: STALAGMIGHT_FUZZ_FIXTURE=${HOME}/fixtures/qzfixture.stalx pnpm agent:scenario s --seed 3` +
          ` (REPLAY of ${SESSION}/traces/t.json)`,
        `  trace: ${SESSION}/traces/t.json`,
      ].join('\n'),
    );
    expect(shared.split('\n')).toEqual([
      'generated file path "g/a.dat" escapes <repo>/.agent-artifacts/sessions/s1',
      '  reproduce: STALAGMIGHT_FUZZ_FIXTURE=~/fixtures/qzfixture.stalx pnpm agent:scenario s --seed 3' +
        ' (REPLAY of <repo>/.agent-artifacts/sessions/s1/traces/t.json)',
      '  trace: <repo>/.agent-artifacts/sessions/s1/traces/t.json',
    ]);
    expect(shared).not.toContain('qzbob');
    expect(shared).not.toContain(REPO);
  });

  it('folds a root only where it ends, never a longer name that starts with it', () => {
    expect(foldLocalPaths(`${HOME}/a ${HOME}by/b ${HOME}`)).toBe(`~/a ${HOME}by/b ~`);
  });

  // #576 W58 Q2: a build's `Built application at:` line names CARGO_TARGET_DIR,
  // and a save under the temp dir names it, both outside the home dir and the
  // checkout as often as not.
  describe('CARGO_TARGET_DIR and the temp dir', () => {
    let saved: Record<'CARGO_TARGET_DIR' | 'TMPDIR', string | undefined>;
    let scratch: string;
    beforeEach(() => {
      saved = { CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR, TMPDIR: process.env.TMPDIR };
      scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'fold-roots-'));
    });
    afterEach(() => {
      for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      fs.rmSync(scratch, { recursive: true, force: true });
    });

    it('folds CARGO_TARGET_DIR to <target> and the temp dir to <tmp>, outside the home dir and the checkout', () => {
      process.env.CARGO_TARGET_DIR = '/qzbuild/target';
      process.env.TMPDIR = '/qzscratch/t';
      expect(foldLocalPaths('Built application at: /qzbuild/target/release/stalagmight')).toBe(
        'Built application at: <target>/release/stalagmight',
      );
      expect(foldLocalPaths('saved /qzscratch/t/stalagmight-fractal-1/qzproject.stalx')).toBe(
        'saved <tmp>/stalagmight-fractal-1/qzproject.stalx',
      );
      // Not inside a longer name, and folding twice changes nothing.
      expect(foldLocalPaths('/qzbuild/targets/x /qzscratch/tt/y')).toBe('/qzbuild/targets/x /qzscratch/tt/y');
      expect(foldLocalPaths('<target>/release <tmp>/a')).toBe('<target>/release <tmp>/a');
    });

    it('folds the longest root first: a target under the home dir is <target>, the checkout under the temp dir is <repo>', () => {
      process.env.CARGO_TARGET_DIR = `${HOME}/.cache/qztarget`;
      expect(foldLocalPaths(`${HOME}/.cache/qztarget/debug/app ${HOME}/x`)).toBe('<target>/debug/app ~/x');
      process.env.TMPDIR = path.dirname(REPO);
      expect(foldLocalPaths(`${REPO}/a ${path.dirname(REPO)}/b`)).toBe('<repo>/a <tmp>/b');
    });

    it('folds the temp dir by its real path too', () => {
      const real = path.join(scratch, 'qzreal');
      fs.mkdirSync(real);
      const link = path.join(scratch, 'qzlink');
      fs.symlinkSync(real, link);
      process.env.TMPDIR = link;
      expect(foldLocalPaths(`${link}/a ${fs.realpathSync(real)}/b`)).toBe('<tmp>/a <tmp>/b');
    });

    it('folds nothing for an unset, relative or root CARGO_TARGET_DIR', () => {
      process.env.TMPDIR = '/qzscratch/t';
      for (const target of [undefined, '', 'target', '/']) {
        if (target === undefined) delete process.env.CARGO_TARGET_DIR;
        else process.env.CARGO_TARGET_DIR = target;
        expect(foldLocalPaths('target/debug /qzbuild/target/debug')).toBe('target/debug /qzbuild/target/debug');
      }
    });
  });

  it('keeps a move-tail heading only in the words the harness writes', () => {
    const shared = shareableFailureText(
      ['U3 undo diff', '  last 3 moves into qzstation:', '  m1', '  last 2 action(s) this iteration:', '  a1'].join(
        '\n',
      ),
    );
    expect(shared).not.toContain('qzstation');
    expect(shared).toContain('  last 2 action(s) this iteration:');
  });

  it("withholds the page's message after a WebDriver `javascript error:`", () => {
    const message = 'Station "qzstation" vanished';
    expect(shareableFailureText(`javascript error: ${message}`)).toBe(
      `javascript error: (${message.length} characters of the app's text withheld)`,
    );
  });
});

/** An error thrown in session setup, as `page.ts` rethrows an IPC rejection. */
function setupCrash(): Error {
  const err = new Error('ipc load_project: section "qzsection" of qzproject is locked\nholder: qzbob');
  err.stack =
    `Error: ${err.message}\n` +
    `    at ipc (${REPO}/e2e/agent/lib/page.ts:120:11)\n` +
    `    at async AppSession.start (file://${REPO}/e2e/agent/session.ts:301:7)\n` +
    '    at Array.map (<anonymous>)\n' +
    '    at process.processTicksAndRejections (node:internal/process/task_queues:105:5)';
  return err;
}

describe('a harness crash prints codes, counts and code locations (#576 W23-NF2)', () => {
  it('keeps the name, the headline prefix and the frames, and withholds the app text', async () => {
    const { shareableErrorText } = await import('./shareableFailure');
    const shared = shareableErrorText(setupCrash());
    const appText = 'section "qzsection" of qzproject is locked';
    expect(shared.split('\n')).toEqual([
      `Error: ipc load_project: (${appText.length} characters of the app's text withheld)`,
      "  (1 line(s) withheld: they can quote the app's text - the rest of the error is not printed)",
      '    at ipc (<repo>/e2e/agent/lib/page.ts:120:11)',
      '    at async AppSession.start (file://<repo>/e2e/agent/session.ts:301:7)',
      '    at Array.map (<anonymous>)',
      '    at process.processTicksAndRejections (node:internal/process/task_queues:105:5)',
    ]);
    expect(names(shared)).toEqual([]);
  });

  it('projects a thrown non-Error value like a failure message', async () => {
    const { shareableErrorText } = await import('./shareableFailure');
    expect(names(shareableErrorText('ipc save_project: qzproject\nqzbob'))).toEqual([]);
  });

  it('is what every runner prints after `fatal: ` and `shrink failed: `', () => {
    // The runners call `main()` at module scope, so their crash lines are
    // checked in the source: each prints the shareable form, none a raw stack.
    for (const runner of ['scenario-run.ts', 'scenarios-smoke.ts', 'journeys-run.ts', 'fuzz-matrix.ts', 'fuzz-shrink.ts']) {
      const source = fs.readFileSync(path.join(REPO, 'e2e/agent', runner), 'utf8');
      const crashLines = source.split('\n').filter((line) => /`(?:fatal|shrink failed): /.test(line));
      expect(crashLines.length, runner).toBeGreaterThan(0);
      for (const line of crashLines) expect(line, runner).toContain('${shareableErrorText(err)}');
      expect(source, runner).not.toMatch(/err\.stack/);
    }
  });
});

/**
 * What the scenarios interpolate into a `ctx.log` line, each a value the
 * harness owns: a count, a seed, a duration, a knob, a fixture or corpus id, a
 * format list, a path (folded), or a harness table's words. A new
 * interpolation joins these lists only once someone has checked it cannot carry
 * the app's text; one that can goes at the end of a `... failed: ${...}` line,
 * where the runner withholds it.
 *
 * An expression that says what it reads - a field path, a length, a join of a
 * harness list, a call of a harness function - is allowed anywhere
 * (`HARNESS_VALUES`). A bare identifier says nothing on its own: `name` is a
 * scenario's name in `scenarios/index.ts` and could be a section's in the next
 * file (#576 RV19b-P7), so it is allowed only in the file it was checked in
 * (`HARNESS_IDENTIFIERS`). A value inside an allowed interpolation - `name` in
 * the `Object.entries(UNREACHABLE_VARIANTS)` one - was checked with it.
 */
const HARNESS_VALUES = new Set([
  'counts.sections',
  'counts.stations',
  'shareableTestId(counts.view)',
  "Object.entries(UNREACHABLE_VARIANTS) .map(([name, why]) => `${name} (${why})`) .join('; ')",
  'ctx.seed',
  "[...seen.entries()].map(([v, c]) => `${v}=${c}`).sort().join(' ')",
  'sections.length',
  'merges.length',
  "enabledFormats().join(',')",
  'digest.missingUuids',
  'digest.duplicateUuids',
  'replay.steps.length',
  "enabledFamilies().join(',')",
  "formats.join(',')",
  'describeCorpus(inventory)',
  'noProjectEpisodeProbability()',
  'Math.round(probeMax)',
  'recorder.length',
  "replay ? `REPLAY of ${replayPath} (${replay.steps.length} recorded step(s)) ` : ''",
  'streamName()',
  'swarm.name',
  'ctx.metrics.mutations ?? 0',
  'ctx.metrics.undos ?? 0',
  'ctx.metrics.redos ?? 0',
  'ctx.metrics.rejected ?? 0',
  'ctx.metrics.noops ?? 0',
  // fuzzDeterminismProbe.ts: the plant's state-record number and a literal.
  'this.states',
  "found ? '' : ' (no number found)'",
]);

/**
 * The bare identifiers (and `episode + 1`, one with arithmetic) each file
 * interpolates, keyed `<directory>/<file>`: both `scenarios/` and `journeys/`
 * have an `index.ts` and a `framework.ts`. Not by line, which every edit moves.
 */
const HARNESS_IDENTIFIERS: ReadonlyMap<string, readonly string[]> = new Map([
  ['scenarios/dialogCycling.ts', ['iterations', 'nodesBefore', 'nodesAfter']],
  ['scenarios/exportRoundtripFuzz.ts', ['replayPath', 'episode + 1', 'moves', 'written']],
  ['scenarios/fractalStress.ts', ['iterations', 'totalShots']],
  ['scenarios/importDialogFuzz.ts', ['CORPUS_DIR_ENV', 'suggestiveDrawn', 'written']],
  ['scenarios/importRepeatedly.ts', ['iterations', 'finalDepth', 'UNDO_LIMIT']],
  // The scenario's own name, seed, iteration count and duration.
  ['scenarios/index.ts', ['name', 'seed', 'iterations', 'durationMs']],
  ['scenarios/mapStorm.ts', ['iterations', 'maxJumpMs']],
  ['scenarios/rapidProjectCycle.ts', ['iterations']],
  ['scenarios/saveUnderChurn.ts', ['iterations']],
  ['scenarios/toolSwitchSpam.ts', ['iterations']],
  ['scenarios/undoPersistenceFuzz.ts', ['fixture', 'iterations', 'totalEntries', 'tag', 'variant']],
  // A count of create/undo/redo cycles.
  ['scenarios/undoRedoChurn.ts', ['created']],
  ['scenarios/undoRedoFuzz.ts', ['written', 'fixture', 'strippedRows', 'deepest', 'maxBytes']],
  // The journey framework's check label and checkpoint name (literals, checked
  // below) and a checkpoint's path (folded).
  ['journeys/framework.ts', ['label', 'name', 'file']],
]);

interface Interpolation {
  /** `<directory>/<file>`, as `HARNESS_IDENTIFIERS` keys it. */
  file: string;
  where: string;
  text: string;
  /** At the very end of a template whose text before it ends in ` failed: `. */
  afterFailed: boolean;
  /** The interpolated value whose own template this one is in, if any. */
  parent: Interpolation | undefined;
}

/** Every value a `ctx.log(...)` (or other `callees`) call in `file` interpolates, nested templates included. */
function logInterpolations(file: string, callees: readonly string[] = ['ctx.log']): Interpolation[] {
  const source = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
  const found: Interpolation[] = [];
  const key = `${path.basename(path.dirname(file))}/${path.basename(file)}`;
  const where = (node: ts.Node): string => `${key}:${source.getLineAndCharacterOfPosition(node.getStart()).line + 1}`;
  const text = (node: ts.Node): string => node.getText(source).replace(/\s+/g, ' ');
  // A template nested in an interpolated value: its own values are values of the line too.
  const nested = (node: ts.Node, parent: Interpolation): void =>
    node.forEachChild(function walk(child: ts.Node): void {
      if (ts.isTemplateExpression(child)) collect(child, parent);
      else child.forEachChild(walk);
    });
  const collect = (node: ts.Node, parent?: Interpolation): void => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return;
    if (ts.isParenthesizedExpression(node)) return collect(node.expression, parent);
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
      collect(node.left, parent);
      collect(node.right, parent);
      return;
    }
    if (ts.isTemplateExpression(node)) {
      node.templateSpans.forEach((span, i) => {
        const before = i === 0 ? node.head.text : (node.templateSpans[i - 1] as ts.TemplateSpan).literal.text;
        const last = i === node.templateSpans.length - 1 && span.literal.text === '';
        const afterFailed = last && before.endsWith(' failed: ');
        const value = { file: key, where: where(span), text: text(span.expression), afterFailed, parent };
        found.push(value);
        nested(span.expression, value);
      });
      return;
    }
    const value = { file: key, where: where(node), text: text(node), afterFailed: false, parent };
    found.push(value);
    nested(node, value);
  };
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && callees.includes(text(node.expression))) {
      for (const arg of node.arguments) collect(arg);
    }
    node.forEachChild(visit);
  };
  visit(source);
  return found;
}

/**
 * The `values` no allowlist covers, as `<where> ${<value>}`: neither allowed
 * anywhere (`anywhere`) or in their own file (`HARNESS_IDENTIFIERS`), nor inside
 * an interpolation that is, nor the app text at the end of a `failed:` line.
 */
function uncheckedValues(values: readonly Interpolation[], anywhere: ReadonlySet<string>): string[] {
  const allowed = (value: Interpolation): boolean =>
    anywhere.has(value.text) || (HARNESS_IDENTIFIERS.get(value.file)?.includes(value.text) ?? false);
  const covered = (value: Interpolation | undefined): boolean =>
    value !== undefined && (allowed(value) || covered(value.parent));
  return values
    .filter((value) => !value.afterFailed && !covered(value))
    .map((value) => `${value.where} \${${value.text}}`);
}

describe("a scenario's progress lines share no app text (#576 W23-NF3)", () => {
  it('withholds the app text a progress line reports, after the corpus and path folds', async () => {
    const { shareableProgressLine } = await import('./shareableFailure');
    const appError = 'Station "qzstation" of qzproject vanished';
    expect(shareableProgressLine(`x5: first import failed: ${appError}`)).toBe(
      `x5: first import failed: (${appError.length} characters of the app's text withheld)`,
    );
    expect(shareableProgressLine(`trace written to ${SESSION}/traces/t.json (12 step(s))`)).toBe(
      'trace written to <repo>/.agent-artifacts/sessions/s1/traces/t.json (12 step(s))',
    );
    const counts = 'fixture=demo.stalx seed=3: 4 mutations, 2 undos; max depth 7; snapshot <=1024B';
    expect(shareableProgressLine(counts)).toBe(counts);
    // An app error that runs over several lines: only the prefix and a count stay.
    expect(names(shareableProgressLine('x7: createSection failed: qzsection\nexists in qzproject'))).toEqual([]);
  });

  it('is what the runners print, while the full line goes to the recorder', async () => {
    const { progressLogger } = await import('./shareableFailure');
    const printed: string[] = [];
    const events: unknown[] = [];
    const log = progressLogger((line) => printed.push(line), { event: (event, detail) => events.push({ event, detail }) });
    const appError = 'qzsection exists';
    log(`x7: createSection failed: ${appError}`);
    expect(printed).toEqual([`x7: createSection failed: (${appError.length} characters of the app's text withheld)`]);
    expect(events).toEqual([{ event: 'scenario-progress', detail: { line: 'x7: createSection failed: qzsection exists' } }]);
    for (const runner of ['scenario-run.ts', 'scenarios-smoke.ts']) {
      const source = fs.readFileSync(path.join(REPO, 'e2e/agent', runner), 'utf8');
      expect(source, runner).toMatch(/log: progressLogger\(/);
    }
  });

  it('every ctx.log value is a harness value, or the app text at the end of a `failed:` line', () => {
    // The scenarios, and the harness modules that log through a scenario's
    // context (the determinism probe's self-test line).
    const all = ['e2e/agent/scenarios', 'e2e/agent/lib'].flatMap((dir) =>
      fs
        .readdirSync(path.join(REPO, dir))
        .filter((name) => name.endsWith('.ts') && !name.endsWith('.test.ts'))
        .flatMap((name) => logInterpolations(path.join(REPO, dir, name))),
    );
    // The walk found the calls: a parse that saw none would pass vacuously.
    expect(all.length).toBeGreaterThan(40);
    expect(all.some((value) => value.where.startsWith('lib/fuzzDeterminismProbe.ts:'))).toBe(true);
    expect(uncheckedValues(all, HARNESS_VALUES)).toEqual([]);
  });

  it('allows a bare identifier only in the file it was checked in (#576 RV19b-P7)', () => {
    // Synthetic scenarios: here `name` could be a section's name, `created` a station's.
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'rv19b-p7-'));
    try {
      const write = (rel: string, body: string): string => {
        const file = path.join(root, rel);
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, body);
        return file;
      };
      const values = [
        write('scenarios/renameFuzz.ts', 'ctx.log(`renamed ${name}`);\n'),
        write('scenarios/index.ts', 'ctx.log(`created ${created}`);\n'),
        // Both directories have an index.ts: `name` is checked in the scenarios' one only.
        write('journeys/index.ts', 'ctx.log(`renamed ${name}`);\n'),
        // Where `created` was checked (a count of cycles), it still passes.
        write('scenarios/undoRedoChurn.ts', 'ctx.log(`churned ${created} cycles`);\n'),
      ].flatMap((file) => logInterpolations(file));
      expect(uncheckedValues(values, HARNESS_VALUES)).toEqual([
        'scenarios/renameFuzz.ts:1 ${name}',
        'scenarios/index.ts:1 ${created}',
        'journeys/index.ts:1 ${name}',
      ]);
      // And no bare identifier is allowed everywhere.
      expect([...HARNESS_VALUES].filter((value) => /^[A-Za-z_$][\w$]*$/.test(value))).toEqual([]);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
});

describe('a matrix cell that died without a FAIL line shares no app text (#576 W23-NF1)', () => {
  it('keeps the verdict-free tail as codes, counts and code locations', async () => {
    const { failureDetail } = await import('./fuzzMatrixReport');
    // What a child prints that dies in setup: its session line, its progress
    // lines (one reporting the app's error), an uncaught error Node printed
    // itself, and the runner's own crash line.
    const outLines = [
      'session started (debug build reused)',
      '   . scenario export-roundtrip-fuzz start (seed=3, iterations=2)',
      '   . x5: first import failed: Station "qzstation" vanished',
    ];
    const errLines = [
      `${HOME}/src/qzproject/e2e/agent/lib/page.ts:120`,
      '    throw new Error(`ipc ${command}: ${message}`);',
      '          ^',
      'Error: ipc load_project: qzproject is locked by qzbob',
      `    at ipc (${REPO}/e2e/agent/lib/page.ts:120:11)`,
      'Node.js v24.11.0',
      'fatal: Error: ipc load_project: qzproject is locked',
      `    at async main (file://${REPO}/e2e/agent/scenario-run.ts:114:19)`,
    ];
    const detail = failureDetail({ outLines, allLines: outLines.concat(errLines) });
    expect(names(detail)).toEqual([]);
    expect(detail).not.toContain(REPO);
    const locked = 'qzproject is locked';
    expect(detail.split('\n')).toEqual([
      'session started (debug build reused)',
      '   . scenario export-roundtrip-fuzz start (seed=3, iterations=2)',
      `   . x5: first import failed: (${'Station "qzstation" vanished'.length} characters of the app's text withheld)`,
      "  (4 line(s) withheld: they can quote the app's text - the matrix does not keep them)",
      '    at ipc (<repo>/e2e/agent/lib/page.ts:120:11)',
      "  (1 line(s) withheld: they can quote the app's text - the matrix does not keep them)",
      `fatal: Error: ipc load_project: (${locked.length} characters of the app's text withheld)`,
      '    at async main (file://<repo>/e2e/agent/scenario-run.ts:114:19)',
    ]);
  });

  it('prints a FAIL block as the child wrote it', async () => {
    const { failureDetail } = await import('./fuzzMatrixReport');
    const { scenarioFailOutput } = await import('./shareableFailure');
    const block = scenarioFailOutput('s', new Error('ipc save_project: qzproject')).trimEnd().split('\n');
    expect(failureDetail({ outLines: block, allLines: block })).toBe(block.join('\n'));
  });
});

describe('a two-session determinism failure is kept where its FAIL block says (#576 FZ-SYNC)', () => {
  const divergence = [
    'determinism: two replays of seed 5 diverged at trace step 2 (record #4, state "after step 2")',
    '  first differing path: $.project.ok.sections[0]["qzstation"]',
    '  replay A: "qzproject"',
    '  replay B: "qzsection"',
    '  reproduce: STALAGMIGHT_FUZZ_DETERMINISM=2sessions pnpm agent:scenario s --seed 5 --iterations 3',
  ].join('\n');

  it("records the full text in every replay's action log, which the withheld count names", async () => {
    const { pairFailOutput } = await import('./shareableFailure');
    const events: { log: string; event: string; detail: unknown }[] = [];
    const replay = (log: string) => ({
      event: (event: string, detail?: unknown) => {
        events.push({ log, event, detail });
      },
    });
    const printed = pairFailOutput('s', new Error(divergence), [replay('A'), replay('B')]);
    expect(names(printed)).toEqual([]);
    expect(printed.split('\n')).toEqual([
      'FAIL: s threw:',
      'determinism: two replays of seed 5 diverged at trace step 2 (record #4, state "after step 2")',
      "  (3 line(s) withheld: they can quote the app's text - the full failure is in each replay session's actions.jsonl)",
      '  reproduce: STALAGMIGHT_FUZZ_DETERMINISM=2sessions pnpm agent:scenario s --seed 5 --iterations 3',
      '',
    ]);
    const detail = { scenario: 's', determinism: '2sessions', message: divergence };
    expect(events).toEqual([
      { log: 'A', event: 'scenario-failure', detail },
      { log: 'B', event: 'scenario-failure', detail },
    ]);
  });

  it('says the text is nowhere when no replay had an action log', async () => {
    const { pairFailOutput } = await import('./shareableFailure');
    expect(pairFailOutput('s', new Error(divergence), [])).toContain(
      "(3 line(s) withheld: they can quote the app's text - no replay session had an action log to keep the full failure)",
    );
  });

  it("is what agent:scenario prints for the pair, with each replay's action log", () => {
    const source = fs.readFileSync(path.join(REPO, 'e2e/agent/scenario-run.ts'), 'utf8');
    expect(source).toMatch(/if \(own\.hasRecorder\) replayLogs\.push\(own\.recorder\);/);
    expect(source).toMatch(/process\.stdout\.write\(pairFailOutput\(args\.name, err, replayLogs\)\);/);
  });
});

describe('agent:journeys prints its progress lines and abort reason shareable (#576 W33-NF1)', () => {
  let sessionDir = '';
  beforeEach(() => {
    sessionDir = fs.mkdtempSync(path.join(os.tmpdir(), 'w33-nf1-'));
  });
  afterEach(() => fs.rmSync(sessionDir, { recursive: true, force: true }));
  /** A session whose screenshot fails with the page's text, as a WebDriver error can. */
  const target = () => ({
    browser: {
      saveScreenshot: async (): Promise<never> => {
        throw new Error('no screenshot of qzproject');
      },
    } as never,
    sessionDir,
    exclusive: <T>(fn: () => Promise<T>): Promise<T> => fn(),
  });

  it("withholds a failed check's observed detail and a failed checkpoint's error, and records them whole", async () => {
    const { runJourney } = await import('../journeys/framework');
    const { progressLogger } = await import('./shareableFailure');
    const printed: string[] = [];
    const events: unknown[] = [];
    const log = progressLogger((line) => printed.push(line), { event: (event, detail) => events.push({ event, detail }) });
    const observed = 'error=Station "qzstation" of qzproject vanished';
    const result = await runJourney(
      {
        name: 'demo-journey',
        description: '',
        async run(ctx) {
          ctx.expect('save reported success', false, observed);
          ctx.expect('project closed', true);
          await ctx.checkpoint('saved');
        },
      },
      target(),
      log,
    );
    expect(names(printed.join('\n'))).toEqual([]);
    expect(printed).toEqual([
      `FAIL: save reported success - failed: (${observed.length} characters of the app's text withheld)`,
      'ok: project closed',
      `checkpoint saved (non-gating) failed: (${'no screenshot of qzproject'.length} characters of the app's text withheld)`,
    ]);
    // The full lines go to actions.jsonl, the full detail to the run record.
    expect(names(JSON.stringify(events))).toEqual(['qzproject', 'qzstation']);
    expect(result.checks[0]?.detail).toBe(observed);
  });

  it('prints the abort reason in the headline form and counts its further lines', async () => {
    const { journeyVerdictLine } = await import('../journeys/framework');
    const result = { journey: 'demo-journey', verdict: 'fail' as const, durationMs: 12, checks: [{ label: 'a', ok: true }] };
    const appError = 'qzsection has no shots';
    const line = journeyVerdictLine('fail', { ...result, aborted: `parseTml failed: ${appError}\nin qzproject` }, ' sweep=error=1');
    expect(names(line)).toEqual([]);
    expect(line.split('\n')).toEqual([
      `FAIL: demo-journey (12ms, 1/1 checks) aborted: parseTml failed: (${appError.length} characters of the app's text withheld) sweep=error=1`,
      "  (1 line(s) withheld: they can quote the app's text - the full reason is in the session's uat/run.jsonl)",
      '',
    ]);
    expect(names(journeyVerdictLine('fail', { ...result, aborted: 'ipc load_project: qzproject is locked' }))).toEqual([]);
    // A reason the harness words itself stays whole.
    const harness = 'journey recorded zero checks - a journey must assert something';
    expect(journeyVerdictLine('fail', { ...result, checks: [], aborted: harness })).toBe(
      `FAIL: demo-journey (12ms, 0/0 checks) aborted: ${harness}\n`,
    );
    expect(journeyVerdictLine('pass', result)).toBe('PASS: demo-journey (12ms, 1/1 checks)\n');
  });

  it('is what the runner prints', () => {
    const source = fs.readFileSync(path.join(REPO, 'e2e/agent/journeys-run.ts'), 'utf8');
    expect(source).toMatch(/log = progressLogger\(/);
    expect(source).toMatch(/journeyVerdictLine\(/);
    expect(source).not.toMatch(/result\.aborted\b/);
  });

  it('every journey progress value is a harness value, or the app text at the end of a `failed:` line', () => {
    const dir = path.join(REPO, 'e2e/agent/journeys');
    const files = fs
      .readdirSync(dir)
      .filter((name) => name.endsWith('.ts') && !name.endsWith('.test.ts'))
      .map((name) => path.join(dir, name));
    // The framework's own lines: the verdict word, and a failed check's detail
    // at the end of a `failed:` line. Its bare identifiers are in HARNESS_IDENTIFIERS.
    const journeyValues = new Set(["ok ? 'ok' : 'FAIL'", "!ok && detail ? ` - failed: ${detail}` : ''"]);
    const all = files.flatMap((file) => logInterpolations(file, ['ctx.log', 'base.log']));
    expect(all.length).toBeGreaterThan(4);
    expect(uncheckedValues(all, new Set([...HARNESS_VALUES, ...journeyValues]))).toEqual([]);
    // Every check label and checkpoint name a journey passes is a literal.
    const named: string[] = [];
    for (const file of files) {
      const source = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
      const visit = (node: ts.Node): void => {
        if (ts.isCallExpression(node) && ['ctx.expect', 'ctx.checkpoint'].includes(node.expression.getText(source))) {
          const first = node.arguments[0];
          const literal = first !== undefined && (ts.isStringLiteral(first) || ts.isNoSubstitutionTemplateLiteral(first));
          named.push(literal ? 'literal' : `${path.basename(file)}: ${node.getText(source).slice(0, 60)}`);
        }
        node.forEachChild(visit);
      };
      visit(source);
    }
    expect(named.length).toBeGreaterThan(20);
    expect(named.filter((what) => what !== 'literal')).toEqual([]);
  });
});

describe('the other harness entry points print a crash in the shareable form (#576 W33-NF6)', () => {
  it('prints shareableErrorText after `fatal: ` and `probe failed: `', () => {
    // They call `main()` at module scope too, so their crash lines are checked in the source.
    for (const entry of ['smoke.ts', 'ipc-payload-probe.ts', 'mcp/bridge.ts', 'mcp/smoke-client.ts', 'mcp/server.ts']) {
      const source = fs.readFileSync(path.join(REPO, 'e2e/agent', entry), 'utf8');
      const crashLines = source.split('\n').filter((line) => /`(?:\[[\w-]+\] )?(?:fatal|probe failed): /.test(line));
      expect(crashLines.length, entry).toBeGreaterThan(0);
      for (const line of crashLines) expect(line, entry).toContain('${shareableErrorText(err)}');
    }
  });

  it("prints agent:mcp:smoke's unexpected exception in the shareable form (#576 W39-NF3)", () => {
    // The smoke's check line for a throw mid-run; the error can quote the app as a crash can.
    const source = fs.readFileSync(path.join(REPO, 'e2e/agent/mcp/smoke-client.ts'), 'utf8');
    const lines = source.split('\n').filter((line) => line.includes("'unexpected exception'"));
    expect(lines.length).toBe(1);
    expect(lines[0]).toContain('shareableErrorText(err)');
    expect(source).not.toMatch(/err\.message/);
  });

  it('leaves the raw stack to shareableErrorText alone, in every file of the harness', () => {
    const dir = path.join(REPO, 'e2e/agent');
    const files = fs
      .readdirSync(dir, { recursive: true, encoding: 'utf8' })
      .filter((rel) => rel.endsWith('.ts') && !rel.endsWith('.test.ts') && !rel.startsWith('replays'));
    expect(files.length).toBeGreaterThan(50);
    const raw = files.filter((rel) => /\.stack\b/.test(fs.readFileSync(path.join(dir, rel), 'utf8')));
    expect(raw).toEqual([path.join('lib', 'shareableFailure.ts')]);
  });
});

describe('the committed UAT ledger record carries codes, counts and code locations (#576 W39-NF1)', () => {
  /** A failed run as `agent:journeys` collects it: the app's text in a check, the abort and the sweep. */
  function failedRun() {
    const observed = 'error=Station "qzstation" of qzproject vanished';
    const appError = `qzsection has no shots in ${HOME}/caves/qzproject.stalx`;
    return {
      observed,
      appError,
      record: {
        kind: 'uat-journeys' as const,
        date: '2026-11-07T10:00:00.000Z',
        gitSha: 'abc1234',
        platform: 'linux-x64',
        verdict: 'fail' as const,
        journeys: [
          {
            journey: 'save-reopen-identity',
            verdict: 'fail' as const,
            durationMs: 12,
            checks: [
              { label: 'save reported success', ok: false, detail: observed },
              { label: 'project closed', ok: true },
            ],
            aborted: `parseTml failed: ${appError}\nholder: qzbob`,
            sweepFindings: [
              {
                severity: 'error' as const,
                kind: 'log-error',
                code: 'stalagmight_lib::commands::station',
                message: 'Station "qzstation" gone',
                detail: { holder: 'qzbob' },
              },
              { severity: 'warning' as const, kind: 'leftover-temp', message: `${HOME}/qzproject.stalx.tmp` },
            ],
          },
        ],
      },
    };
  }

  it("keeps a check detail's length, the abort reason's headline form and the sweep's codes", async () => {
    const { ledgerRecord } = await import('../journeys/ledger');
    const { observed, appError, record } = failedRun();
    const shared = ledgerRecord(record);
    expect(names(JSON.stringify(shared))).toEqual([]);
    expect(shared).toEqual({
      kind: 'uat-journeys',
      date: '2026-11-07T10:00:00.000Z',
      gitSha: 'abc1234',
      platform: 'linux-x64',
      verdict: 'fail',
      journeys: [
        {
          journey: 'save-reopen-identity',
          verdict: 'fail',
          durationMs: 12,
          checks: [
            { label: 'save reported success', ok: false, detail: `(${observed.length} characters of the app's text withheld)` },
            { label: 'project closed', ok: true },
          ],
          // What the verdict line prints after `aborted:`, its further lines counted.
          aborted:
            // (The home directory is folded to `~` before the text is counted.)
            `parseTml failed: (${appError.replace(HOME, '~').length} characters of the app's text withheld)\n` +
            "  (1 line(s) withheld: they can quote the app's text - the full reason is in the session's uat/run.jsonl)",
          sweepFindings: [
            {
              severity: 'error',
              kind: 'log-error',
              code: 'stalagmight_lib::commands::station',
              message: '<scrubbed - see the session run.jsonl>',
            },
            { severity: 'warning', kind: 'leftover-temp', message: '<scrubbed - see the session run.jsonl>' },
          ],
        },
      ],
    });
    // The verbatim record, for the session's uat/run.jsonl, is left as it was.
    expect(record.journeys[0]?.checks[0]?.detail).toBe(observed);
  });

  it('keeps the fields it knows only, so a field added later does not reach the public tree', async () => {
    const { ledgerRecord } = await import('../journeys/ledger');
    const { record } = failedRun();
    const extra = {
      ...record,
      note: 'qzbob',
      journeys: record.journeys.map((j) => ({
        ...j,
        observedName: 'qzstation',
        checks: j.checks.map((c) => ({ ...c, expected: 'qzproject' })),
      })),
    };
    expect(names(JSON.stringify(ledgerRecord(extra)))).toEqual([]);
  });

  it('is what the runner writes to docs/uat/runs/', () => {
    const source = fs.readFileSync(path.join(REPO, 'e2e/agent/journeys-run.ts'), 'utf8');
    expect(source).toMatch(/JSON\.stringify\(ledgerRecord\(record\)\)/);
    // No second, weaker scrub of its own beside it.
    expect(source).not.toMatch(/scrubText|sanitizeForLedger|\(\?:Users\|home\)/);
  });
});
