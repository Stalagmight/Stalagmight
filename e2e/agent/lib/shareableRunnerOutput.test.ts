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
 * interpolation joins this list only once someone has checked it cannot carry
 * the app's text; one that can goes at the end of a `... failed: ${...}` line,
 * where the runner withholds it.
 */
const HARNESS_VALUES = new Set([
  'counts.sections',
  'counts.stations',
  'counts.view',
  'tag',
  'variant',
  'fixture',
  'iterations',
  'finalDepth',
  'UNDO_LIMIT',
  'nodesBefore',
  'nodesAfter',
  'created',
  "Object.entries(UNREACHABLE_VARIANTS) .map(([name, why]) => `${name} (${why})`) .join('; ')",
  'name',
  'why',
  'ctx.seed',
  'totalEntries',
  "[...seen.entries()].map(([v, c]) => `${v}=${c}`).sort().join(' ')",
  'v',
  'c',
  'sections.length',
  'totalShots',
  'merges.length',
  "enabledFormats().join(',')",
  'digest.missingUuids',
  'digest.duplicateUuids',
  'replay.steps.length',
  'replayPath',
  'episode + 1',
  'moves',
  'written',
  "enabledFamilies().join(',')",
  "formats.join(',')",
  'describeCorpus(inventory)',
  'CORPUS_DIR_ENV',
  'noProjectEpisodeProbability()',
  'suggestiveDrawn',
  'maxJumpMs',
  'Math.round(probeMax)',
  'seed',
  'durationMs',
  'recorder.length',
  "replay ? `REPLAY of ${replayPath} (${replay.steps.length} recorded step(s)) ` : ''",
  'streamName()',
  'swarm.name',
  'strippedRows',
  'ctx.metrics.mutations ?? 0',
  'ctx.metrics.undos ?? 0',
  'ctx.metrics.redos ?? 0',
  'ctx.metrics.rejected ?? 0',
  'ctx.metrics.noops ?? 0',
  'deepest',
  'maxBytes',
]);

interface Interpolation {
  where: string;
  text: string;
  /** At the very end of a template whose text before it ends in ` failed: `. */
  afterFailed: boolean;
}

/** Every value a `ctx.log(...)` call in `file` interpolates, nested templates included. */
function logInterpolations(file: string): Interpolation[] {
  const source = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
  const found: Interpolation[] = [];
  const where = (node: ts.Node): string =>
    `${path.basename(file)}:${source.getLineAndCharacterOfPosition(node.getStart()).line + 1}`;
  const text = (node: ts.Node): string => node.getText(source).replace(/\s+/g, ' ');
  // A template nested in an interpolated value: its own values are values of the line too.
  const nested = (node: ts.Node): void =>
    node.forEachChild(function walk(child: ts.Node): void {
      if (ts.isTemplateExpression(child)) collect(child);
      else child.forEachChild(walk);
    });
  const collect = (node: ts.Node): void => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return;
    if (ts.isParenthesizedExpression(node)) return collect(node.expression);
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
      collect(node.left);
      collect(node.right);
      return;
    }
    if (ts.isTemplateExpression(node)) {
      node.templateSpans.forEach((span, i) => {
        const before = i === 0 ? node.head.text : (node.templateSpans[i - 1] as ts.TemplateSpan).literal.text;
        const last = i === node.templateSpans.length - 1 && span.literal.text === '';
        found.push({ where: where(span), text: text(span.expression), afterFailed: last && before.endsWith(' failed: ') });
        nested(span.expression);
      });
      return;
    }
    found.push({ where: where(node), text: text(node), afterFailed: false });
    nested(node);
  };
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && text(node.expression) === 'ctx.log') node.arguments.forEach(collect);
    node.forEachChild(visit);
  };
  visit(source);
  return found;
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
    const dir = path.join(REPO, 'e2e/agent/scenarios');
    const all = fs
      .readdirSync(dir)
      .filter((name) => name.endsWith('.ts') && !name.endsWith('.test.ts'))
      .flatMap((name) => logInterpolations(path.join(dir, name)));
    // The walk found the calls: a parse that saw none would pass vacuously.
    expect(all.length).toBeGreaterThan(40);
    const unchecked = all.filter((value) => !HARNESS_VALUES.has(value.text) && !value.afterFailed);
    expect(unchecked.map((value) => `${value.where} \${${value.text}}`)).toEqual([]);
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
