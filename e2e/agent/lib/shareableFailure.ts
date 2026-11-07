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
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';

import { redactCorpusText } from './fixtureCorpus';

/** The checkout: the session directories live in it (`.agent-artifacts/sessions/<id>`). */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

/**
 * `text` with the checkout folded to `<repo>` and the home directory to `~`
 * (#576 RV18-W23-4). A kept line can carry a path - a `reproduce:` line's
 * fixture or `REPLAY of <trace>`, a headline naming the session directory - and
 * a path under either root names the user. What follows the folded root is the
 * harness's own layout, or a corpus id the corpus fold already put there. Read
 * at call time, so a changed `HOME` is honoured; a root of `/` folds nothing.
 */
export function foldLocalPaths(text: string): string {
  const roots = [
    { root: REPO_ROOT, say: '<repo>' },
    { root: os.homedir(), say: '~' },
  ]
    .filter(({ root }) => path.isAbsolute(root) && path.resolve(root) !== path.parse(root).root)
    .map(({ root, say }) => ({ root: path.resolve(root), say }))
    .sort((a, b) => b.root.length - a.root.length);
  let out = text;
  for (const { root, say } of roots) {
    if (!out.includes(root)) continue;
    const literal = root.replace(/[.*+?^$()|[\]{}\\]/g, '\\$&');
    // Not inside a longer name: `/home/bob` must leave `/home/bobby` alone.
    out = out.replace(new RegExp(`${literal}(?![\\w.-])`, 'g'), say);
  }
  return out;
}

/** Lines the harness writes from its own vocabulary, kept whole. */
const KEPT_LINES: readonly RegExp[] = [
  /^\s*reproduce: /, // the command that re-runs it
  /^\s*trace: /, // where the trace went
  // A diff path - a code location in the data model: field names, indices.
  /^\s*at [A-Za-z_]\w*(?:\[(?:\d+|\*)\]|\.(?:[A-Za-z_]\w*|\d+))*$/,
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

/** `headline` with the app's text after an `APP_ERROR_HEADLINES` prefix replaced by its length. */
function shareableHeadline(headline: string): string {
  for (const shape of APP_ERROR_HEADLINES) {
    const app = shape.exec(headline);
    if (!app) continue;
    const [, prefix = '', text = ''] = app;
    return WITHHELD_TEXT.test(text) ? headline : `${prefix}: (${text.length} characters of the app's text withheld)`;
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
 * `message` with the corpus and path folds applied and every line the harness did not
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
  const [first = '', ...rest] = foldLocalPaths(redactCorpusText(message)).split('\n');
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
 * failure, whose first line is the headline the matrix reports.
 */
export function scenarioFailOutput(name: string, err: unknown): string {
  return `FAIL: ${name} threw:\n${shareableFailureText(err instanceof Error ? err.message : String(err))}\n`;
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
  if (!(err instanceof Error)) return shareableFailureText(String(err), [], CRASH_WITHHELD);
  const frames = (err.stack ?? '').split('\n').filter((line) => STACK_FRAME.test(line));
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
    const line = foldLocalPaths(redactCorpusText(raw));
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
