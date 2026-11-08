/**
 * A scenario failure as a SHAREABLE artifact prints it: codes, counts and code
 * locations, never the app's own text (#576 W21-NF1).
 *
 * A failure message mixes the harness's words with the app's: an INTEGRITY
 * failure lists `check_project_integrity` messages, an S6 IPC CONTRACT failure
 * quotes the backend's deserialization lines (argument values included), an
 * undo diff prints `expected:` / `actual:` values out of the project, an
 * `error:` line is a command's error, and a move note can quote either. Those
 * name sections, stations and projects of the loaded data, which no corpus fold
 * knows. The FAIL line, a fuzz trace and `fuzz-shrink`'s output are what a
 * campaign attaches to an issue, so they carry this form; the full text stays
 * in the session's `actions.jsonl` (docs/agent-harness.md lists what is not for
 * sharing).
 *
 * Fail-closed: a line is kept only if its SHAPE is one the harness writes from
 * its own vocabulary, and everything else is withheld and counted - so a new
 * oracle that quotes the app is covered without anyone remembering to mark it.
 */
import * as path from 'path';
import { fileURLToPath } from 'url';

import { foldLocalPaths } from '../../../scripts/lib/fold-local-paths.mjs';

import { redactCorpusText } from './fixtureCorpus';
import { foldPrivateFixtures } from './privateFixtures';

/** The checkout: `harnessBuilt` keeps an error built in its sources (#576 W58 Q3). */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

/**
 * `text` with the checkout folded to `<repo>`, the home directory to `~`,
 * `CARGO_TARGET_DIR` to `<target>` and the temp directory to `<tmp>` (#576
 * RV18-W23-4, W58). A kept line can carry a path - a `reproduce:` line's
 * fixture or `REPLAY of <trace>`, a headline naming the session directory - and
 * a path under any of these roots names the user or their machine. One
 * implementation, `scripts/lib/fold-local-paths.mjs`, so the repo's Node tools
 * under `scripts/` fold the paths they echo the same way.
 */
export { foldLocalPaths };

/**
 * Lines the harness writes from its own vocabulary, kept whole. Each shape is
 * the harness's, not only its label (#576 RV19b-P2): a line of the app's text
 * can start `trace: `, `reproduce: ` or `at ` too.
 */
const KEPT_LINES: readonly RegExp[] = [
  // The command that re-runs it: `pnpm agent:<runner>` or the MCP tool, after
  // any environment it needs. What follows is the harness's own words, seeds
  // and the fixture, which the corpus and path folds above already covered.
  /^\s*reproduce: (?:[A-Z][A-Z0-9_]*=\S+ )*(?:pnpm agent:[a-z][\w:-]* |run_scenario\(")/,
  // Where the trace went: one path, to a JSON trace.
  /^\s*trace: \S+\.json$/,
  // A diff path - a code location in the data model: field names (lower-case
  // led, as the snapshot's are) and indices, a root and at least one step.
  /^\s*at [a-z][A-Za-z0-9_]*(?:\[(?:\d+|\*)\]|\.(?:[a-z][A-Za-z0-9_]*|\d+))+$/,
  // The move tail's heading, a count. The headings the scenarios write
  // (`fuzzHarness.failureText`'s callers, `exportRoundtripFuzz`), and only
  // those: lower-case words are not enough, `last 3 moves into <a name>:` is
  // lower-case words too.
  /^\s*last \d+ (?:action\(s\)(?: this iteration)?|move\(s\)):$/,
  /^ {2}\(\d+ line\(s\) withheld: /, // a count this function wrote: projecting twice changes nothing
];

/** A classifier the message names - an id from a harness table, kept as the id alone. */
const CLASSIFIER = /(KNOWN OPEN APP BUG|ATTRIBUTED) \[([a-z0-9-]+)\]/g;

/**
 * Headlines that end in the app's or the page's own text, as `prefix: text`:
 * the prefix names the code path, the rest is the app's words.
 * - `page.ts` puts `ipc <command>:` / `eval compile:` / `eval:` in front of an
 *   error it rethrows, and WebDriver words a script that threw in the page as
 *   `javascript error: <the page's message>`;
 * - a scenario words a refused action as `<action> failed: <its error>`
 *   (`loadProject(<fixture>) failed: ...`, `createSection 3 failed: ...`);
 * - a headline that ends in a JSON array or object carries values, such as
 *   `E2 INTEGRITY: ... at the start of episode 3: [<integrity messages>]`.
 */
const APP_ERROR_HEADLINES: readonly RegExp[] = [
  /^(ipc \w+|eval compile|eval|javascript error): (.*)$/,
  /^(.*? failed): (.*)$/,
  /^(.*?): ([[{].*)$/,
];

/** What `shareableFailureText` leaves of the app's text in a headline. */
const WITHHELD_TEXT = /^\(\d+ characters of the app's text withheld\)$/;

/**
 * What a shareable artifact keeps of the app's own `text`: its length. A failed
 * journey check's observed detail is all app text, so the committed UAT record
 * keeps it this way (#576 W39-NF1). Projecting twice changes nothing.
 */
export function withheldAppText(text: string): string {
  return WITHHELD_TEXT.test(text) ? text : `(${text.length} characters of the app's text withheld)`;
}

/**
 * A value read back from a file someone else may have written - a trace, a
 * recorded call - as an error's headline prints it (#576 W51): a number, a
 * boolean, `null` or `undefined` as itself, anything else as its type and size.
 * Never its text: a forged copy chooses that, and the headline is pasted into
 * issues.
 */
export function valueShape(value: unknown): string {
  if (value === null || value === undefined || typeof value === 'number' || typeof value === 'boolean') {
    return String(value);
  }
  if (typeof value === 'string') return value === '' ? 'an empty string' : `a string of ${value.length} character(s)`;
  if (Array.isArray(value)) return `an array of ${value.length} item(s)`;
  return typeof value === 'object' ? 'an object' : `a ${typeof value}`;
}

/** A token shaped like a flag the runners take: `--seed`, `-h`, lower-case words and dashes. */
const FLAG_SHAPED = /^--?[a-z][a-z0-9-]{0,40}$/;
/** A token shaped like a number as typed: an optional sign, digits, a decimal point, an exponent. */
const NUMBER_SHAPED = /^[-+]?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?$/i;

/** Is `value` a command-line token shaped like a flag or like a number (#576 W58 Q4)? */
export function cliShaped(value: unknown): boolean {
  return typeof value === 'string' && (FLAG_SHAPED.test(value) || (value.length <= 24 && NUMBER_SHAPED.test(value)));
}

/**
 * A value a runner refuses on its command line, as the refusal prints it (#576
 * W58 Q4): a token shaped like a flag (`--sed`) or like a number (`1.5`, `-3`)
 * as itself, quoted, and anything else - a stray path, a name - by its shape
 * (`valueShape`: its length). Neither shape can spell a path or a sentence.
 */
export function cliValueShape(value: unknown): string {
  return cliShaped(value) ? JSON.stringify(value) : valueShape(value);
}

/**
 * Error classes whose message the engine or Node words: a `TypeError` can quote
 * a value (`Cannot create property 'x' on string '<text>'`), a `SyntaxError`
 * from `JSON.parse` quotes the text it could not parse.
 */
const ENGINE_ERRORS = [TypeError, RangeError, SyntaxError, ReferenceError, EvalError, URIError];

/** The file a stack frame (`at fn (file:line:col)`, `at file:line:col`) names, or null. */
function frameFile(frame: string): string | null {
  const location = /\(([^()]*)\)\s*$/.exec(frame)?.[1] ?? frame.trim().replace(/^at (?:async )?/, '');
  const file = location.replace(/(?::\d+){1,2}$/, '');
  if (file.startsWith('file://')) {
    try {
      return fileURLToPath(file);
    } catch {
      return null;
    }
  }
  return path.isAbsolute(file) ? file : null;
}

/**
 * Did the harness build `err` (#576 W58 Q3)? Its message is then one the
 * print-site guard checked where it was built (`printSites/`), and its first
 * line prints as a headline. An error is the harness's when it is a plain
 * `Error` (or a harness subclass) built in a source file of the checkout
 * outside node_modules - the first frame of its stack - with nothing that
 * says Node made it: no `code`, `errno` or `syscall`.
 *
 * Fail-closed: an error with no frame (a WebDriver error carries the driver's
 * stack instead of ours), one an engine class words, one built in node_modules
 * or Node's internals (webdriverio, undici, `fs`), and one whose stack
 * webdriverio spliced (`waitUntil` joins two stacks with a `---` line and drops
 * its own frames) is third-party.
 */
export function harnessBuilt(err: unknown): boolean {
  if (!(err instanceof Error)) return false;
  if (ENGINE_ERRORS.some((cls) => err instanceof cls)) return false;
  if (['code', 'errno', 'syscall'].some((key) => Object.prototype.hasOwnProperty.call(err, key))) return false;
  // The stack opens with `<name>: <message>`, as many lines as the message has:
  // a message line that reads like a frame (`  at project.sections[3]`) is not one.
  const lines = typeof err.stack === 'string' ? err.stack.split('\n').slice(err.message.split('\n').length) : [];
  if (lines.some((line) => /^\s*---\s*$/.test(line))) return false;
  const frame = lines.find((line) => /^\s+at /.test(line));
  const file = frame === undefined ? null : frameFile(frame);
  if (file === null) return false;
  const rel = path.relative(REPO_ROOT, file);
  return rel !== '' && !rel.startsWith('..') && !path.isAbsolute(rel) && !rel.split(/[\\/]/).includes('node_modules');
}

/** A third-party error's first line as the projections see it: `a third-party <Name>: <its text>`. */
const THIRD_PARTY_HEADLINE = /^(a third-party \w+): (.*)$/;
/** What `shareableFailureText` leaves of a third-party error's first line. */
const WITHHELD_THIRD_PARTY = /^\(\d+ characters withheld\)$/;

/**
 * An error's message as the shareable projections take it (#576 W58 Q3). One
 * the harness built (`harnessBuilt`) is its message. Any other - webdriverio's,
 * Node's, a thrown non-Error - gets `a third-party <Name>: ` in front of its
 * first line, which every projection prints as that prefix and the line's
 * length: the line is not a shape the harness writes, and it can quote a
 * selector, a path or the page. WebDriver's `javascript error: <the page's
 * message>` is one the harness knows, and keeps its prefix the same way
 * (`APP_ERROR_HEADLINES`). The full text is what a runner records in the
 * session's actions.jsonl.
 */
export function errorMessageOf(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err);
  if (harnessBuilt(err)) return message;
  const [first = '', ...rest] = message.split('\n');
  if (APP_ERROR_HEADLINES[0]?.test(first)) return message;
  const name = err instanceof Error && /^\w+$/.test(err.name) ? err.name : 'error';
  return [`a third-party ${name}: ${first}`, ...rest].join('\n');
}

/**
 * `headline` with a third-party error's first line, or the app's text after an
 * `APP_ERROR_HEADLINES` prefix, replaced by its length.
 */
function shareableHeadline(headline: string): string {
  const thirdParty = THIRD_PARTY_HEADLINE.exec(headline);
  if (thirdParty) {
    const [, prefix = '', text = ''] = thirdParty;
    return `${prefix}: ${WITHHELD_THIRD_PARTY.test(text) ? text : `(${text.length} characters withheld)`}`;
  }
  for (const shape of APP_ERROR_HEADLINES) {
    const app = shape.exec(headline);
    if (!app) continue;
    const [, prefix = '', text = ''] = app;
    return `${prefix}: ${withheldAppText(text)}`;
  }
  return headline;
}

/**
 * The count line for a run of withheld lines. `where` says where the full text
 * is: a scenario failure's is in the session's `actions.jsonl`; a crash's and a
 * dead cell's are nowhere a run keeps.
 */
function withheldLine(count: number, where = "the full failure is in the session's actions.jsonl"): string {
  return `  (${count} line(s) withheld: they can quote the app's text - ${where})`;
}

/**
 * `message` with the corpus, path and private-fixture folds applied (a fixture
 * the repo does not track prints as its tag, #576 W58 Q1) and every line the harness did not
 * word itself withheld, a run of them replaced by one count line. The headline
 * (line 1) stays - a scenario writes it from harness words, numbers and names
 * (scenario, fixture, move, screen) - except the app's text an
 * `APP_ERROR_HEADLINES` shape ends in, which is replaced by its length.
 */
export function shareableFailureText(
  message: string,
  alsoKept: readonly RegExp[] = [],
  withheldWhere?: string,
): string {
  const [first = '', ...rest] = foldPrivateFixtures(foldLocalPaths(redactCorpusText(message))).split('\n');
  const out = [shareableHeadline(first)];
  let withheld = 0;
  const flush = (): void => {
    if (withheld > 0) out.push(withheldLine(withheld, withheldWhere));
    withheld = 0;
  };
  for (const line of rest) {
    if (KEPT_LINES.some((shape) => shape.test(line)) || alsoKept.some((shape) => shape.test(line))) {
      flush();
      out.push(line);
      continue;
    }
    const classifiers = [...line.matchAll(CLASSIFIER)].map((found) => `${found[1]} [${found[2]}]`);
    if (classifiers.length > 0) {
      flush();
      out.push(`  ${classifiers.join(' ')}`);
      continue;
    }
    withheld++;
  }
  flush();
  return out.join('\n');
}

/**
 * What `agent:scenario` / `agent:scenarios` print for a scenario that threw:
 * the `FAIL: <name> threw:` line `fuzz-matrix` anchors on, then the shareable
 * failure, whose first line is the headline the matrix reports. A third-party
 * error's first line prints as its length (`errorMessageOf`, #576 W58 Q3).
 */
export function scenarioFailOutput(name: string, err: unknown): string {
  return `FAIL: ${name} threw:\n${shareableFailureText(errorMessageOf(err))}\n`;
}

/** Where a two-session failure's full text is: every replay session with an action log. */
const PAIR_RECORDED = "the full failure is in each replay session's actions.jsonl";
const PAIR_NOT_RECORDED = 'no replay session had an action log to keep the full failure';

/**
 * What `agent:scenario` prints for a `STALAGMIGHT_FUZZ_DETERMINISM=2sessions`
 * failure, after recording its full text (#576 FZ-SYNC). The pair's verdict - a
 * divergence, or one replay failing alone - is raised after both replays'
 * sessions stopped, so no session logged it: it goes into every replay's
 * `actions.jsonl` now, as a `scenario-failure` event, and the FAIL block's
 * withheld-lines count says where it is only when it is there.
 */
export function pairFailOutput(
  name: string,
  err: unknown,
  replays: readonly { event(event: string, detail?: unknown): void }[],
): string {
  const message = err instanceof Error ? err.message : String(err);
  for (const recorder of replays) {
    recorder.event('scenario-failure', { scenario: name, determinism: '2sessions', message });
  }
  const shared = shareableFailureText(message, [], replays.length > 0 ? PAIR_RECORDED : PAIR_NOT_RECORDED);
  return `FAIL: ${name} threw:\n${shared}\n`;
}

/**
 * A stack frame - a code location: `at fn (file:line:col)`, `at file:line:col`,
 * `at fn (<anonymous>)`, `at async fn (node:internal/...:1:2)`. The function
 * part is identifiers, `.`, `<anonymous>` and `[as name]` only, and the
 * location has no spaces, so a frame cannot carry a sentence.
 */
export const STACK_FRAME =
  /^\s+at (?:(?:async |new )?[\w$.<>]+(?: \[as [\w$]+\])? \()?(?:[^\s()]+:\d+:\d+|<anonymous>|native)\)?$/;

/** No session records a crash's message: what is withheld is not printed anywhere. */
const CRASH_WITHHELD = 'the rest of the error is not printed';

/**
 * A harness CRASH as the runners print it after `fatal: ` or `shrink failed: `
 * (#576 W23-NF2): the error's name, its headline as a FAIL headline is printed
 * (`ipc <command>: <the app's error>` keeps the prefix and the length), and its
 * stack frames; every other line of the message is withheld and counted. A
 * crash in session setup can carry the app's text - an IPC rejection, a page
 * script's error - so the raw stack is not for sharing either.
 */
export function shareableErrorText(err: unknown): string {
  if (!(err instanceof Error)) return shareableFailureText(errorMessageOf(err), [], CRASH_WITHHELD);
  const frames = (err.stack ?? '').split('\n').filter((line) => STACK_FRAME.test(line));
  // A third-party error's message carries its name in front (`a third-party
  // TypeError: <its length>`, #576 W58 Q3), so it is not named twice.
  if (!harnessBuilt(err)) {
    return shareableFailureText([errorMessageOf(err), ...frames].join('\n'), [STACK_FRAME], CRASH_WITHHELD);
  }
  const name = /^\w+$/.test(err.name) ? err.name : 'Error';
  return `${name}: ${shareableFailureText([err.message, ...frames].join('\n'), [STACK_FRAME], CRASH_WITHHELD)}`;
}

/**
 * A scenario's progress line (`ctx.log`) as the runners print it (#576
 * W23-NF3): the corpus and path folds, and the line treated as a headline, so
 * the app's text after an `APP_ERROR_HEADLINES` prefix (`<what> failed: <the
 * app's error>`) is replaced by its length. The progress lines are part of what
 * people paste with a FAIL block. A scenario words a line from counts, seeds,
 * fixture and corpus ids and paths; where one reports the app's text it uses
 * the `... failed: <text>` shape, and `shareableRunnerOutput.test.ts` checks
 * every `ctx.log` call against that rule.
 */
export function shareableProgressLine(line: string): string {
  // A progress line is one line; an app error it ends in can run over several,
  // and those go the way of any other line the harness did not word.
  return shareableFailureText(line);
}

/**
 * The `log` a runner hands `runScenario`: the full line goes to the session's
 * `actions.jsonl` as a `scenario-progress` event (not for sharing; the
 * recorder folds it), and `print` gets the shareable line.
 */
export function progressLogger(
  print: (line: string) => void,
  recorder?: { event(event: string, detail?: unknown): void },
): (line: string) => void {
  return (line) => {
    recorder?.event('scenario-progress', { line });
    print(shareableProgressLine(line));
  };
}

/** A dead cell's output lives only in this process's memory: nothing keeps what is withheld. */
const TAIL_WITHHELD = 'the matrix does not keep them';

/** Lines of a runner's output that are shareable as printed: verdicts, the session line, frames, counts. */
const TAIL_KEPT: readonly RegExp[] = [
  /^session \w+ \([^()]*\)$/, // `session <state> (<mode> build <action>)`
  /^(?:PASS|PASS\+findings|SKIP): /, // a verdict: counts per severity, kind and code (#576 HFI2-F2)
  STACK_FRAME,
  /^ {2}\(\d+ line\(s\) withheld: /,
];

/**
 * The tail of a child that died without a `FAIL:` line, as `fuzz-matrix`
 * prints it (#576 W23-NF1): fail-closed per line. A progress line and a
 * `fatal:` / `shrink failed:` headline are projected again (projecting twice
 * changes nothing), the `TAIL_KEPT` shapes stay, and everything else - an
 * uncaught error Node printed itself with its source excerpt, a build's
 * output, a driver's log - is withheld, a run of them replaced by one count
 * line.
 */
export function shareableTail(lines: readonly string[]): string {
  const out: string[] = [];
  let withheld = 0;
  const flush = (): void => {
    if (withheld > 0) out.push(withheldLine(withheld, TAIL_WITHHELD));
    withheld = 0;
  };
  for (const raw of lines) {
    const line = foldPrivateFixtures(foldLocalPaths(redactCorpusText(raw)));
    const progress = /^(\s+\S )(.*)$/.exec(line);
    const crash = /^(fatal|shrink failed): (?:(\w*Error): )?(.*)$/.exec(line);
    let kept: string | null = null;
    if (crash) {
      const [, label = '', name, rest = ''] = crash;
      kept = `${label}: ${name ? `${name}: ` : ''}${shareableHeadline(rest)}`;
    } else if (TAIL_KEPT.some((shape) => shape.test(line))) kept = line;
    else if (progress && /^\s{3}[.\u00b7] $/.test(progress[1] ?? '')) {
      kept = `${progress[1]}${shareableProgressLine(progress[2] ?? '')}`;
    }
    if (kept === null) {
      withheld++;
      continue;
    }
    flush();
    out.push(kept);
  }
  flush();
  return out.join('\n');
}
