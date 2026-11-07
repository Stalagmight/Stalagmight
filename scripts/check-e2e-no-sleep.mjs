// No-unannotated-sleep gate for the Playwright tier (#576 E14).
//
// docs/TESTING.md has said "Do not sleep" for a long time: a hard
// `page.waitForTimeout()` is the single biggest flake source, and a sleep that
// stands in for a real condition makes a test slow when the condition is quick
// and wrong when it is slow. Nothing enforced it, and the count grew (41 -> 48
// lines by #576). This gate fails on every sleep in e2e-playwright/ that is not
// explicitly annotated as a deliberate settle. A sleep is (#576 RV3-FE-12):
//   - any reference to `waitForTimeout`, not just a direct call - so
//     `page.waitForTimeout.bind(page)`, `const { waitForTimeout } = page`,
//     `page['waitForTimeout'](...)` and a call split across lines count;
//   - a test-runner `setTimeout(` (`await new Promise((r) => setTimeout(r, 500))`),
//     i.e. one that is not inside a callback the BROWSER runs (page.evaluate,
//     addInitScript, waitForFunction, $eval, ...). Any bare reference to
//     `setTimeout` or `setInterval` counts, not only a call, so an alias
//     (`const st = setTimeout`), `promisify(setTimeout)` and a setInterval
//     sleep do too (#576 RV4-FE-FEB-2) - but not `typeof setTimeout` or an
//     object key named `setTimeout`, which name no timer (#576 RV9b-5);
//   - an import of `timers/promises`, whose `setTimeout` is a sleep under any
//     alias;
//   - a string expression that spells `waitForTimeout`: a lone literal or a
//     constant concatenation (`page['wait' + 'ForTimeout']`, a template whose
//     substitutions are literals) - the key of a computed call (#576 NF-W9FE-4);
//   - a wait whose failure is swallowed (#576 NF-W9FE-4): a Playwright wait
//     (`waitFor`, `waitForSelector`, `waitForFunction`, ...) or a web-first
//     assertion (`expect(...)`, `expect.poll`, `toPass`) chained into a
//     `.catch(...)` (or `?.catch?.(...)`, or a two-argument `.then(ok, fail)`)
//     whose handler does not always throw or assert, or awaited in a `try`
//     whose `catch` does not. When the condition never comes it waits out the
//     timeout and the test goes on as if it had - a sleep. "Always" (#576
//     RV10b-G2): an unconditional statement at the top of the handler - a
//     `throw`, or an `expect(...)` about something other than a literal - with
//     no `return` / `break` / `continue` before it. So a catch that rethrows
//     everything EXCEPT the timeout (`if (!(e instanceof errors.TimeoutError))
//     throw e;`, Playwright's optional-wait idiom) swallows the wait: settle-
//     annotate it if waiting it out is the point. A chain holding a wait in its
//     arguments or callee counts too (#576 RV10b-G3): `Promise.race([...])` /
//     `Promise.all([...])` or an async IIFE that waits, then `.catch(() => {})`;
//     and `Promise.allSettled([...waits])`, which never rejects, when its outcome
//     is thrown away. Not counted: a boolean probe
//     (`.then(() => true).catch(() => false)`, `.then(() => true, () => false)`)
//     or an `allSettled` whose outcome the test keeps - to branch or assert on
//     (one whose value is thrown away is a sleep) - and a swallowed action
//     (`click({ timeout })`), which is not a wait. A kept try/catch probe is not
//     exempt (write it as the .then form).
//
// THE ESCAPE HATCH - some waits are legitimate: proving that something does NOT
// happen ("press the key, give a buggy handler time to react, assert the tool is
// unchanged") cannot be written as a poll, because a poll accepts the
// pre-update value on its first read. Those are allowed when the `//` comment
// block directly above the call carries
//
//     // settle: <why a fixed wait is the only way to prove this>
//
// with a real reason (at least MIN_REASON characters). Everything else should
// wait for its condition: a web-first assertion, `expect.poll`, `toPass`, or a
// helper from e2e-playwright/helpers/app.ts (waitForMapIdle, waitForActiveTool,
// ...). See docs/testing-recipes.md, "Waiters".
//
// Out of scope by design: a timer inside page.evaluate and friends, or
// anywhere under e2e-playwright/mocks/ (the IPC mock is serialized into the
// page, so its timers are mock latency, not the test runner sleeping), and
// Playwright's own `test.setTimeout` / `testInfo.setTimeout` (a budget, not a
// wait). Comments are stripped with the
// shared string-aware scanner and same-line string literals are blanked, so a
// waitForTimeout named in prose or in a label does not count; a symlinked spec
// fails, as in every source gate.
//
// Runs in the `smoke` job (e2e-smoke.yml) before the browser install, after its
// fixture test (scripts/check-e2e-no-sleep.test.mjs).
//
// Usage:  node scripts/check-e2e-no-sleep.mjs   (or: pnpm lint:e2e-no-sleep)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { walkFiles } from './lib/src-files.mjs';
import { matchParen, stripComments } from './lib/js-comments.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const SCAN_DIR = 'e2e-playwright';
export const MIN_REASON = 12;

// Any mention of the name outside comments and strings: a call, a bind, a
// destructure. A string that spells it (`page['waitForTimeout']`,
// `page['wait' + 'ForTimeout']`) is found by spelledSleepLines before strings
// are blanked.
const SLEEP_RE = /\bwaitForTimeout\b/g;
const SLEEP_NAME = 'waitForTimeout';
// A bare or global `setTimeout` / `setInterval` reference - a call, an alias,
// an argument (`promisify(setTimeout)`); `test.setTimeout(` /
// `testInfo.setTimeout(` (budgets) are members of something else and do not
// match. Group 1 is the name.
const TIMER_RE = /(?:^|[^.\w$]|\b(?:globalThis|global)\.)(setTimeout|setInterval)(?![\w$])/g;
const TIMERS_PROMISES_RE = /(['"])(?:node:)?timers\/promises\1/g;
// Calls whose callback runs in the page, where a timer is not a runner sleep.
const BROWSER_CALL_RE = /\.(?:evaluate|evaluateHandle|evaluateAll|addInitScript|waitForFunction|\$eval|\$\$eval)\s*\(/g;
export const BROWSER_SIDE_DIRS = ['e2e-playwright/mocks/'];
const SETTLE_RE = /^\/\/\s*settle:\s*(.*)$/;

// The settle reason in the contiguous `//` comment block ending right above
// line `i` (0-based), or null. A block comment or a blank line ends the block.
export function settleReason(lines, i) {
  for (let j = i - 1; j >= 0; j--) {
    const t = lines[j].trim();
    if (!t.startsWith('//')) break;
    const m = SETTLE_RE.exec(t);
    if (m && m[1].trim().length >= MIN_REASON) return m[1].trim();
  }
  return null;
}

// Every sleep in one file's code: [{lineNo, reason}] where reason is the
// settle annotation or null. `rel` (repo-relative) decides whether runner-side
// timers are in scope (not under BROWSER_SIDE_DIRS).
// Same-line string literals are blanked too (stripComments keeps them, since
// other gates match INSIDE strings): a label like 'page.waitForTimeout(100)'
// is not a call. The blanking keeps each literal's length, so offsets into the
// blanked text are offsets into the stripped text.
const STRING_RE = /'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\])*`/g;
const blank = (lit) => lit[0] + ' '.repeat(lit.length - 2) + lit[lit.length - 1];

// [#576 NF-W9FE-4] 0-based lines where a string expression spells
// `waitForTimeout`: a lone literal, or literals joined by `+` (across lines),
// a template's `${'literal'}` substitutions folded in. Reported at the line of
// the first literal. Runs on `stripped`, which keeps the strings; what lies
// between two literals is code, so a `+` there joins them.
const TEMPLATE_LITERAL_SUB_RE = /\$\{\s*(['"])((?:\\.|(?!\1)[^\\])*)\1\s*\}/g;
function literalValue(lit) {
  const body = lit.slice(1, -1);
  return lit[0] === '`' ? body.replace(TEMPLATE_LITERAL_SUB_RE, '$2') : body;
}
function spelledSleepLines(stripped) {
  const text = stripped.join('\n');
  const lits = [];
  for (const m of text.matchAll(STRING_RE)) lits.push({ at: m.index, end: m.index + m[0].length, value: literalValue(m[0]) });
  const lines = [];
  for (let i = 0; i < lits.length; ) {
    let value = lits[i].value;
    let j = i;
    while (j + 1 < lits.length && /^\s*\+\s*$/.test(text.slice(lits[j].end, lits[j + 1].at))) value += lits[++j].value;
    if (value === SLEEP_NAME) lines.push(text.slice(0, lits[i].at).split('\n').length - 1);
    i = j + 1;
  }
  return lines;
}

// [#576 NF-W9FE-4] Waits whose failure is swallowed. All offsets are into
// `code` (comments stripped, same-line strings blanked, joined with '\n').
const WAIT_METHODS = new Set([
  'waitFor', 'waitForSelector', 'waitForFunction', 'waitForURL', 'waitForLoadState',
  'waitForEvent', 'waitForResponse', 'waitForRequest', 'waitForNavigation',
]);
const CHAIN_STOP_WORDS = new Set(['await', 'return', 'yield', 'void', 'typeof', 'new', 'in', 'of', 'throw', 'case', 'else', 'do']);
const CLOSERS = { ')': '(', ']': '[', '}': '{' };

// The index of the opener matching the closer at `close`, scanning back.
function matchBack(code, close) {
  const closeCh = code[close];
  const openCh = CLOSERS[closeCh];
  let depth = 0;
  for (let i = close; i >= 0; i--) {
    if (code[i] === closeCh) depth++;
    else if (code[i] === openCh && --depth === 0) return i;
  }
  return -1;
}

// The index after the closer matching the opener at `open` ({, ( or [), scanning forward.
function matchForward(code, open) {
  const openCh = code[open];
  const closeCh = openCh === '{' ? '}' : openCh === '[' ? ']' : ')';
  let depth = 0;
  for (let i = open; i < code.length; i++) {
    if (code[i] === openCh) depth++;
    else if (code[i] === closeCh && --depth === 0) return i + 1;
  }
  return code.length;
}

// The member chain ending right before `end` (`page.locator(x).waitFor(y)`):
// where it starts, and its names from the last back to the root.
function chainBefore(code, end) {
  const names = [];
  let start = end;
  for (;;) {
    let j = start - 1;
    while (j >= 0 && /\s/.test(code[j])) j--;
    if (j < 0) return { start, names };
    if (CLOSERS[code[j]] && code[j] !== '}') {
      const open = matchBack(code, j);
      if (open < 0) return { start, names };
      start = open;
      continue;
    }
    if (!/[\w$]/.test(code[j])) return { start, names };
    let k = j;
    while (k > 0 && /[\w$]/.test(code[k - 1])) k--;
    const name = code.slice(k, j + 1);
    if (CHAIN_STOP_WORDS.has(name)) return { start, names };
    names.push(name);
    start = k;
    let d = k - 1;
    while (d >= 0 && /\s/.test(code[d])) d--;
    if (d < 0 || code[d] !== '.') return { start, names };
    start = d > 0 && code[d - 1] === '?' ? d - 1 : d;
  }
}

// Whether the expression starting at `start` (after an optional `await`) is a
// whole statement - its value thrown away.
function isStatement(code, start) {
  const before = code.slice(0, start).replace(/\bawait\s*$/, '').trimEnd();
  return before === '' || /[;{}]$/.test(before);
}

// The index of the bracket that encloses `at`, scanning back, or -1.
function enclosingOpen(code, at) {
  let depth = 0;
  for (let i = at - 1; i >= 0; i--) {
    const c = code[i];
    if (c === '}' || c === ')' || c === ']') depth++;
    else if ((c === '{' || c === '(' || c === '[') && depth-- === 0) return i;
  }
  return -1;
}

// [#576 RV10b-G4] Whether the `{` / `[` at `open` is a destructuring pattern,
// which binds names: one declared (`const {...}` - whatever follows it: `=`, a
// type annotation, `of`), assigned (`({...} = value)`), a parameter (its `(`
// closes before `=>`, or follows `function name`), or nested in such a pattern.
function isPattern(code, open) {
  if (/\b(?:const|let|var)\s*$/.test(code.slice(0, open))) return true;
  if (/^\s*=(?![=>])/.test(code.slice(matchForward(code, open)))) return true;
  const outer = enclosingOpen(code, open);
  if (outer < 0) return false;
  if (code[outer] !== '(') return isPattern(code, outer);
  return (
    /^\s*(?::[^;{}()]*)?=>/.test(code.slice(matchForward(code, outer))) ||
    /\bfunction\s*[\w$]*\s*$/.test(code.slice(0, outer))
  );
}

// [#576 RV9b-5] A reference that names no timer: a type position (`typeof
// setTimeout`) or an object literal's key (`{ setTimeout: false }`). A
// destructuring key (`const { setTimeout: nap } = globalThis`) binds the timer, so
// it still counts - in a declaration, an assignment, a parameter list or a for-of,
// typed or not (#576 RV10b-G4) - as do a shorthand property and a value position.
function namesNoTimer(code, at, len) {
  if (/\btypeof\s+$/.test(code.slice(Math.max(0, at - 32), at))) return true;
  if (!/[{,]$/.test(code.slice(0, at).trimEnd()) || !/^:(?!:)/.test(code.slice(at + len).trimStart())) return false;
  const open = enclosingOpen(code, at);
  return open >= 0 && code[open] === '{' && !isPattern(code, open);
}

const TRY_WAIT_RE = new RegExp(`\\.\\s*(?:${[...WAIT_METHODS].join('|')})\\s*\\(|\\bawait\\s+(expect)\\b`, 'g');
const HAS_WAIT_RE = new RegExp(TRY_WAIT_RE.source);
// A chain is a wait when one of its links is (`loc.waitFor(...)`, `expect(...)`),
// or when it holds one in its arguments or its callee (#576 RV10b-G3):
// `Promise.race([a.waitFor()])`, `Promise.all([...])`, an async IIFE that waits.
const isWaitChain = (code, start, end, names) =>
  names.some((n) => WAIT_METHODS.has(n)) || names[names.length - 1] === 'expect' || HAS_WAIT_RE.test(code.slice(start, end));
// `() => true` / `(e) => false` / `() => { return false; }`: a boolean outcome.
const BOOLEAN_HANDLER_RE = /^\s*(?:async\s+)?(?:\([^)]*\)|[\w$]+)\s*=>\s*(?:(?:true|false)|\{\s*return\s+(?:true|false)\s*;?\s*\})\s*$/;
// `expect(true)` / `expect(1)` / `expect('x')`: asserts nothing about the page.
const TRIVIAL_EXPECT_RE = /^(?:await\s+)?expect\s*\(\s*(?:true|false|null|undefined|-?\d[\d.]*|(['"`])\s*\1)\s*\)/;
const ASSERT_RE = /^(?:await\s+)?expect\s*[.(]/;
const CONTROL_RE = /^(?:if|else|for|while|do|switch|case|default|with)\b/;
const CATCH_CALL_RE = /\??\.\s*catch\s*(?:\?\.\s*)?\(/g;
const THEN_CALL_RE = /\??\.\s*then\s*(?:\?\.\s*)?\(/g;
const ALL_SETTLED_RE = /\bPromise\s*\.\s*allSettled\s*\(/g;
const TRY_RE = /\btry\s*\{/g;
const CATCH_BLOCK_RE = /^\s*catch\s*(?:\([^)]*\)\s*)?\{/;

// The top-level arguments of the call whose `(` is at `open`, as source slices.
function callArgs(code, open) {
  const close = matchForward(code, open) - 1;
  const args = [];
  let depth = 0;
  let from = open + 1;
  for (let i = open + 1; i < close; i++) {
    const c = code[i];
    if (c === '(' || c === '[' || c === '{') depth++;
    else if (c === ')' || c === ']' || c === '}') depth--;
    else if (c === ',' && depth === 0) {
      args.push(code.slice(from, i));
      from = i + 1;
    }
  }
  if (code.slice(from, close).trim() !== '') args.push(code.slice(from, close));
  return args;
}

// [#576 RV10b-G2] Whether a rejection handler (its source: an arrow, a function,
// a reference) or a catch block (`{...}`) rethrows or asserts on EVERY path. Only
// an unconditional statement at the top level of its body counts - a `throw`, or
// an `expect(...)` that is not about a literal - with no `return` / `break` /
// `continue` before it; an expression body counts when it is such an expect. A
// throw under a condition (`if (!(e instanceof errors.TimeoutError)) throw e;`,
// Playwright's optional-wait idiom) lets the timeout through: a swallowed wait.
function rethrowsAlways(src) {
  let body = src.trim();
  if (body[0] !== '{') {
    const fn = /^(?:async\s+)?(?:function\b[^(]*)?/.exec(body)[0];
    let rest = body.slice(fn.length).trimStart();
    if (rest[0] === '(') rest = rest.slice(matchForward(rest, 0)).trimStart();
    else if (!fn.includes('function')) rest = rest.replace(/^[\w$]+\s*/, '');
    rest = rest.replace(/^(?::[^=;{]*)?=>\s*/, '').trimStart();
    if (rest[0] !== '{') return ASSERT_RE.test(rest) && !TRIVIAL_EXPECT_RE.test(rest);
    body = rest.slice(0, matchForward(rest, 0));
  }
  const block = body.slice(1, -1);
  let depth = 0;
  let stmt = 0;
  for (let i = 0; i < block.length; i++) {
    const c = block[i];
    const wordStart = !/[\w$.]/.test(block[i - 1] ?? ' ');
    if (wordStart && /^(?:return|break|continue)\b/.test(block.slice(i))) return false;
    if (c === '(' || c === '[' || c === '{') {
      depth++;
      continue;
    }
    if (c === ')' || c === ']' || c === '}') {
      if (--depth === 0 && c === '}') stmt = i + 1;
      continue;
    }
    if (depth === 0 && c === ';') stmt = i + 1;
    if (depth !== 0 || !wordStart) continue;
    const tail = block.slice(i);
    const isThrow = /^throw\b/.test(tail);
    if (!isThrow && !(ASSERT_RE.test(tail) && !TRIVIAL_EXPECT_RE.test(tail))) continue;
    const lead = block.slice(stmt, i);
    // A statement start: nothing before it in the statement, or (no semicolon)
    // a line break after an expression statement that is not a control header.
    const startsStatement = lead.trim() === '' || (/\n\s*$/.test(lead) && !CONTROL_RE.test(lead.trim()) && !/[?:&|,=(]\s*$/.test(lead));
    if (startsStatement) return true;
  }
  return false;
}

function swallowedWaitLines(code) {
  const lineOf = (at) => code.slice(0, at).split('\n').length - 1;
  const starts = new Set();
  let m;
  CATCH_CALL_RE.lastIndex = 0;
  while ((m = CATCH_CALL_RE.exec(code)) !== null) {
    const { start, names } = chainBefore(code, m.index);
    if (!isWaitChain(code, start, m.index, names)) continue;
    const open = m.index + m[0].length - 1;
    const handler = code.slice(open + 1, matchForward(code, open) - 1);
    if (rethrowsAlways(handler)) continue;
    if (names.includes('then') && BOOLEAN_HANDLER_RE.test(handler) && !isStatement(code, start)) continue;
    starts.add(start);
  }
  // [#576 RV10b-G3] `.then(onFulfilled, onRejected)`: the second argument is a
  // rejection handler, read like a `.catch`'s.
  THEN_CALL_RE.lastIndex = 0;
  while ((m = THEN_CALL_RE.exec(code)) !== null) {
    const { start, names } = chainBefore(code, m.index);
    if (!isWaitChain(code, start, m.index, names)) continue;
    const args = callArgs(code, m.index + m[0].length - 1);
    if (args.length < 2 || rethrowsAlways(args[1])) continue;
    if (args.every((a) => BOOLEAN_HANDLER_RE.test(a)) && !isStatement(code, start)) continue;
    starts.add(start);
  }
  // [#576 RV10b-G3] `Promise.allSettled([...waits])` never rejects: a sleep when
  // its outcome is thrown away (kept, its statuses are a probe).
  ALL_SETTLED_RE.lastIndex = 0;
  while ((m = ALL_SETTLED_RE.exec(code)) !== null) {
    const open = m.index + m[0].length - 1;
    if (!HAS_WAIT_RE.test(code.slice(open, matchForward(code, open)))) continue;
    if (isStatement(code, m.index)) starts.add(m.index);
  }
  TRY_RE.lastIndex = 0;
  while ((m = TRY_RE.exec(code)) !== null) {
    const tryOpen = m.index + m[0].length - 1;
    const tryEnd = matchForward(code, tryOpen);
    const catchHead = CATCH_BLOCK_RE.exec(code.slice(tryEnd));
    if (!catchHead) continue;
    const catchOpen = tryEnd + catchHead[0].length - 1;
    if (rethrowsAlways(code.slice(catchOpen, matchForward(code, catchOpen)))) continue;
    const body = code.slice(tryOpen, tryEnd);
    let w;
    TRY_WAIT_RE.lastIndex = 0;
    while ((w = TRY_WAIT_RE.exec(body)) !== null) {
      starts.add(w[1] ? tryOpen + w.index + w[0].length - w[1].length : chainBefore(code, tryOpen + w.index).start);
    }
  }
  return [...starts].map(lineOf);
}

export function findSleeps(text, rel = '') {
  const lines = text.split('\n');
  const stripped = stripComments(lines);
  const code = stripped.map((c) => c.replace(STRING_RE, blank));
  const lineHits = new Map(); // 0-based line -> number of sleeps on it
  const hit = (i, n = 1) => lineHits.set(i, (lineHits.get(i) ?? 0) + n);
  const timersPromisesLines = new Set();
  stripped.forEach((c, i) => {
    const imports = (c.match(TIMERS_PROMISES_RE) ?? []).length;
    if (imports) timersPromisesLines.add(i);
    if (imports) hit(i, imports);
  });
  for (const i of spelledSleepLines(stripped)) hit(i);
  code.forEach((c, i) => {
    const n = (c.match(SLEEP_RE) ?? []).length;
    if (n) hit(i, n);
  });
  for (const i of swallowedWaitLines(code.join('\n'))) hit(i);
  if (!BROWSER_SIDE_DIRS.some((d) => rel.startsWith(d))) {
    const joined = code.join('\n');
    const strippedJoined = stripped.join('\n');
    const browser = [];
    let m;
    BROWSER_CALL_RE.lastIndex = 0;
    while ((m = BROWSER_CALL_RE.exec(joined)) !== null) {
      const open = m.index + m[0].length - 1;
      browser.push([open, matchParen(strippedJoined, open)]);
    }
    TIMER_RE.lastIndex = 0;
    while ((m = TIMER_RE.exec(joined)) !== null) {
      const at = m.index + m[0].length - m[1].length;
      if (browser.some(([a, b]) => a < at && at < b)) continue;
      if (namesNoTimer(joined, at, m[1].length)) continue;
      const line = joined.slice(0, at).split('\n').length - 1;
      // `import { setTimeout as sleep } from 'timers/promises'` names that
      // module's export, and the line is already counted once above.
      if (!timersPromisesLines.has(line)) hit(line);
    }
  }
  const out = [];
  for (const i of [...lineHits.keys()].sort((a, b) => a - b)) {
    const reason = settleReason(lines, i);
    for (let k = 0; k < lineHits.get(i); k++) out.push({ lineNo: i + 1, reason });
  }
  return out;
}

// Scan repo-relative files; `readFile` is injected for the fixture test.
export function scan(files, readFile) {
  const violations = [];
  let settles = 0;
  for (const rel of files) {
    for (const s of findSleeps(readFile(rel), rel)) {
      if (s.reason) settles++;
      else violations.push({ rel, lineNo: s.lineNo });
    }
  }
  return { violations, settles };
}

export function main(root = REPO_ROOT, log = console.log, err = console.error) {
  if (!fs.existsSync(path.join(root, SCAN_DIR))) {
    err(`check-e2e-no-sleep: ${SCAN_DIR}/ not found (run from the repo root).`);
    return 1;
  }
  const { files, symlinks } = walkFiles(root, SCAN_DIR, { exts: ['.ts', '.tsx'] });
  const { violations, settles } = scan(files, (rel) => fs.readFileSync(path.join(root, rel), 'utf8'));
  if (violations.length === 0 && symlinks.length === 0) {
    log(`✓ e2e-playwright no-sleep: ${files.length} files, no unannotated sleep (${settles} annotated settle(s)).`);
    return 0;
  }
  err(`\n✗ e2e-playwright no-sleep check failed - ${violations.length + symlinks.length} issue(s):\n`);
  for (const v of violations) err(`  ${v.rel}:${v.lineNo}  a sleep (waitForTimeout, a runner-side setTimeout/setInterval, timers/promises or a swallowed wait) without a "// settle: <reason>" comment above it`);
  for (const l of symlinks) err(`  ${l.rel}  is a symlink (-> ${l.target}); not followed, so not scanned - replace it with a regular file`);
  err(
    '\nWait for the condition instead (web-first assertion, expect.poll, toPass, or a helper\n' +
      'in e2e-playwright/helpers/app.ts). If the wait proves that something does NOT happen,\n' +
      `annotate it: "// settle: <why>" (>= ${MIN_REASON} chars) directly above the call.\n` +
      'See scripts/check-e2e-no-sleep.mjs and docs/TESTING.md.\n',
  );
  return 1;
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
