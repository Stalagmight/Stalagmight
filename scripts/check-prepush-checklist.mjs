// Pre-push checklist drift gate (#576 E6).
//
// The Pre-Push Checklist in CLAUDE.md (AGENTS.md is a symlink to it) is the
// list every contributor and agent runs before pushing. It is hand-written, and
// it has been re-synced with the workflows by hand at least four times; each
// time it had silently fallen behind (the release-profile `cargo check` that
// cost rc4 prep nine hours, #530, was missing from it). This gate makes the
// drift a CI failure instead of a surprise.
//
// WHAT IT CHECKS - every gate command a PR workflow runs (WORKFLOWS below) must
// appear in the checklist's ```bash block, after normalisation:
//   - `node scripts/X.mjs` counts as `pnpm <name>` when package.json maps a
//     script to exactly that command (CI calls the scripts directly where it
//     has no `pnpm install`, the checklist uses the pnpm names);
//   - `pnpm exec <cmd>` counts as `pnpm <name>` when a script is exactly
//     `<cmd>` (reporter flags dropped - they change output, not the verdict);
//   - cargo commands compare as a SET of flags, so argument order and a
//     leading `TELEMETRY_ENDPOINT=...` do not matter, but a missing `--locked`
//     or `--release` does;
//   - ALIASES covers the one command CI and a local shell spell differently.
// A step that runs no recognised gate command (apt installs, cache stats, a
// pure-shell check) must be classified in PLUMBING_STEPS or SHELL_GATE_STEPS,
// so a NEW shell-only gate cannot slip past by not looking like one. The same
// holds for a MIXED step (#576 RV3-FE-12): one that runs a recognised command
// (even plumbing like `pnpm install`) next to a command this gate cannot
// compare (`python3 scripts/x.py`, `bash scripts/x.sh`, `make lint`) fails
// unless the step is classified; plain shell glue (SHELL_GLUE: `if`, `echo`,
// `exit`, assignments, ...) is not a command to compare - unless a command
// substitution in it (`$(...)`, backticks) runs something that is not glue
// itself, as `[ -z "$(git status --porcelain)" ]` or `OUT=$(python3 x.py)` do
// (#576 RV4-FE-FEB-4). Rows in either table
// that match no step are themselves a failure (stale allowlist).
// A `uses:` step (an action) carries its command inside the action, so there is
// nothing in the YAML to compare (#576 NF-FIX3B-1): it passes only when its
// action is listed in PLUMBING_ACTIONS (setup, caches, uploads) or the step is
// classified by name - PLUMBING_STEPS, or ACTION_GATE_STEPS for an action that
// IS a gate, paired with its checklist stand-in - so a gate added as an action
// (cargo-deny-action, a linter action) cannot slip past by not running `run:`.
// The reverse direction is checked for names only: a `pnpm <script>` in the
// checklist that package.json does not define (a renamed script) fails too.
// A JOB can call a reusable workflow instead of running steps (`jobs.<id>.uses:
// ./.github/workflows/x.yml`, #576 NF-W4M-5): its steps run as part of the
// caller, so a local one is read and checked like a listed workflow (each file
// once, so a cycle ends), its steps classified under its own path. A reusable
// workflow from another repository cannot be read, so calling one fails.
//
// WHAT IT DOES NOT CHECK - the hand-written comments, ordering, or commands the
// checklist adds on top of CI (e.g. the touched-specs Playwright line). The
// checklist may say MORE than CI; it may not say less.
//
// WHERE IT RUNS - the leak-scan workflow, the only one with no paths-ignore:
// `check` and `smoke` both ignore `**/*.md`, so a CLAUDE.md-only edit would
// never run a gate placed there. Pure Node, no dependencies (leak-scan has no
// `pnpm install`), so the YAML is read with a small line-based step parser
// that understands exactly the subset the workflows use (list-item steps with
// `name:` / `run:` / `uses:` keys, `run: |` block scalars).
//
// Usage:  node scripts/check-prepush-checklist.mjs   (or: pnpm lint:prepush-checklist)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// The workflows that gate a PR. release.yml's verify job repeats a subset of
// `check`, nightly-e2e.yml is the full Playwright suite (deliberately not a
// pre-push item, see the checklist's Playwright line) and agent-harness.yml is
// on-demand, so none of those define what a contributor must run. Nor does
// os-helper.yml: a path-filtered macOS-only job for one standalone crate that
// most contributors cannot build (its commands live in e2e/agent/os/README.md).
// Nor does telemetry-worker.yml: a path-filtered type-check of the standalone
// Cloudflare worker, outside the root install (its command is in
// docs/telemetry.md). Local reusable workflows these call from a job
// (pick-runner.yml) are followed, not listed here.
export const WORKFLOWS = [
  '.github/workflows/ci.yml',
  '.github/workflows/e2e-smoke.yml',
  '.github/workflows/leak-scan.yml',
];
export const CHECKLIST_FILE = 'CLAUDE.md';
export const CHECKLIST_HEADING = '### Pre-Push Checklist';

// Gate commands that are CI plumbing, not something a contributor runs.
export const PLUMBING_COMMANDS = [
  { re: /^pnpm install(\s|$)/, why: 'dependency install' },
  { re: /^pnpm exec playwright install(-deps)?(\s|$)/, why: 'browser / system-dependency install' },
  { re: /^node scripts\/pw-timing-summary\.mjs(\s|$)/, why: 'run-page timing table; it cannot fail the job' },
];

// Scripts that run the command they are given (CI plumbing around a gate):
// stripped, so the wrapped command is compared with the checklist like any
// other.
export const WRAPPER_SCRIPTS = [
  {
    re: /^(\.\/)?scripts\/ci-cargo-test-unprivileged\.sh$/,
    why: "on a root runner the wrapped command's Rust test binaries run as a non-root user (#576 NF-ROOT-PERM, RV5-PI-1)",
  },
];

// Steps that run no gate command at all, keyed by workflow + step name.
export const PLUMBING_STEPS = [
  { workflow: '.github/workflows/ci.yml', step: 'Install tauri apt deps (hosted only)', why: 'runner setup' },
  { workflow: '.github/workflows/ci.yml', step: 'sccache zero-stats (self-hosted only)', why: 'cache observability' },
  { workflow: '.github/workflows/ci.yml', step: 'sccache stats (self-hosted only)', why: 'cache observability' },
  { workflow: '.github/workflows/e2e-smoke.yml', step: 'Resolve Playwright version', why: 'browser-cache key' },
  {
    workflow: '.github/workflows/pick-runner.yml',
    step: 'Choose runner - hekatrine when online & idle, else cloud',
    why: 'runner routing for ci.yml (called as a job-level reusable workflow); it gates nothing',
  },
];

// Pure-shell gate steps, each paired with the checklist command that stands
// in for it locally.
export const SHELL_GATE_STEPS = [
  {
    workflow: '.github/workflows/ci.yml',
    step: 'ts-rs bindings tree matches the checkout',
    checklist: 'git status --porcelain src-tauri/bindings',
    why: 'the bindings tree is a side effect of cargo test; CI fails on modified or untracked files under it',
  },
  {
    workflow: '.github/workflows/ci.yml',
    step: 'Generated IPC types match the checkout',
    checklist: 'git status --porcelain src/types/ipc src/types/ipc.generated.ts',
    why: 'the frontend IPC types are written by pnpm gen:ipc-types; CI fails on modified or untracked files under them',
  },
  {
    workflow: '.github/workflows/ci.yml',
    step: 'crates.io packaging check (stalx-reader)',
    checklist: 'cargo package --no-verify --manifest-path src-tauri/Cargo.toml -p stalagmight-models -p stalagmight-persistence -p stalx-reader',
    why: 'a shell loop over the three published crates: cargo package, then a LICENSE check on each package listing',
  },
  {
    workflow: '.github/workflows/ci.yml',
    step: 'SQLite authorizer on a build without the repo\'s flags (#619)',
    checklist: 'STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE=1 CARGO_TARGET_DIR=target-unstripped-sqlite cargo --config \'env.LIBSQLITE3_FLAGS.value=""\' --config \'env.LIBSQLITE3_FLAGS.force=true\' test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence -p stalx-reader --lib -- --exact persistence::sqlite::untrusted_sql_tests::every_untrusted_open_refuses_what_the_build_flags_strip tests::the_inspect_connection_refuses_what_the_build_flags_strip',
    why: 'two named tests on a SQLite built without the repo flags (#619); the test names after `--` are filters this gate cannot compare as commands',
  },
  {
    workflow: '.github/workflows/ci.yml',
    step: 'MSRV check (stalx-reader and its crates, Rust 1.89.0)',
    checklist: 'cargo +1.89.0 check --locked --manifest-path src-tauri/Cargo.toml -p stalx-reader -p stalagmight-persistence -p stalagmight-models',
    why: 'installs the 1.89.0 toolchain (rustup, plumbing), then checks the three published crates on it (#625)',
  },
];

// Actions that gate nothing: runner setup, caches and report uploads. Matched
// on the action name before `@<ref>`; every `uses:` step outside this list must
// be classified by step name (PLUMBING_STEPS / ACTION_GATE_STEPS).
export const PLUMBING_ACTIONS = [
  { action: 'actions/checkout', why: 'checkout' },
  { action: 'actions/setup-node', why: 'toolchain setup' },
  { action: 'pnpm/action-setup', why: 'toolchain setup' },
  { action: 'dtolnay/rust-toolchain', why: 'toolchain setup' },
  { action: 'Swatinem/rust-cache', why: 'build cache' },
  { action: 'actions/cache', why: 'cache restore and save' },
  { action: 'actions/upload-artifact', why: 'report upload; it cannot fail the job' },
];

// Action steps that ARE gates, each paired with the checklist command that
// stands in for it locally. An action's `with:` inputs are its command line,
// but every action spells them differently (`command:`, `args:`, `arguments:`),
// so the pairing is written down here rather than guessed. None today.
export const ACTION_GATE_STEPS = [];

// CI spelling -> checklist spelling, for commands that differ only because CI
// runs with CI=true.
export const ALIASES = [
  { ci: 'pnpm test', checklist: 'pnpm exec vitest run', why: 'vitest runs once under CI=true; locally `pnpm test` watches' },
];

// `git` counts so a checklist stand-in like `git status --porcelain <dir>` is a
// command, and a bare `git diff --exit-code` CI step would be a gate.
const GATE_TOOLS = new Set(['pnpm', 'node', 'npx', 'cargo', 'git']);
// Value-taking cargo flags, so `--manifest-path X` compares as one unit.
const CARGO_VALUE_FLAGS = new Set(['--manifest-path', '--features', '-p', '--package', '--target-dir', '--bin', '--example', '--test', '-F']);
// Shell glue in a gate step: control flow and builtins that run no tool of
// their own. Words that prefix a command (`if`, `!`, `then`, ...) are dropped
// first, so `if python3 x.py; then` is judged by `python3`.
const SHELL_PREFIX_WORDS = new Set(['if', 'elif', 'while', 'until', '!', 'then', 'do', 'else', 'time']);
export const SHELL_GLUE = new Set([
  'for', 'done', 'fi', 'case', 'esac', 'in', '[', '[[', 'test', 'continue', 'break', 'exit', 'return',
  'echo', 'printf', 'true', 'false', ':', 'set', 'export', 'shift', 'local',
]);

// Is `cmd` shell glue (nothing a gate could hide in)? Assignments and the
// prefix words are skipped; an empty remainder is a pure assignment. The
// first word is not the whole story: every command substitution in `cmd` must
// run only glue too (#576 RV4-FE-FEB-4), or `test -z "$(python3 gate.py)"`
// would hide a gate behind `test`.
export function isShellGlue(cmd) {
  const tokens = tokenize(cmd);
  while (tokens.length && (/^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[0]) || SHELL_PREFIX_WORDS.has(tokens[0]))) tokens.shift();
  if (tokens.length > 0 && !SHELL_GLUE.has(tokens[0])) return false;
  return commandSubstitutions(cmd).every((body) => splitCommands(body).every(isShellGlue));
}

// The bodies of the command substitutions in `cmd`: `$(...)` (nested parens
// matched, quotes inside it skipped) and backticks, outside single quotes,
// where nothing expands. `$((...))` is arithmetic, not a command; its text is
// still scanned for a substitution inside it.
function commandSubstitutions(cmd) {
  const out = [];
  let dq = false;
  for (let i = 0; i < cmd.length; i++) {
    const c = cmd[i];
    if (c === '\\') { i++; continue; }
    if (c === "'" && !dq) {
      const close = cmd.indexOf("'", i + 1);
      i = close < 0 ? cmd.length : close;
      continue;
    }
    if (c === '"') { dq = !dq; continue; }
    if (c === '$' && cmd[i + 1] === '(' && cmd[i + 2] === '(') { i += 2; continue; }
    if (c === '$' && cmd[i + 1] === '(') {
      let depth = 0;
      let quote = null;
      let j = i + 1;
      for (; j < cmd.length; j++) {
        const d = cmd[j];
        if (quote) {
          if (d === '\\' && quote === '"') j++;
          else if (d === quote) quote = null;
          continue;
        }
        if (d === '\\') { j++; continue; }
        if (d === "'" || d === '"') { quote = d; continue; }
        if (d === '(') depth++;
        else if (d === ')' && --depth === 0) break;
      }
      out.push(cmd.slice(i + 2, j));
      i = j;
      continue;
    }
    if (c === '`') {
      let j = i + 1;
      while (j < cmd.length && cmd[j] !== '`') j += cmd[j] === '\\' ? 2 : 1;
      out.push(cmd.slice(i + 1, j));
      i = j;
    }
  }
  return out;
}

// pnpm first words that are not package.json scripts (built-ins and the one
// binary the checklist calls through pnpm).
const PNPM_NON_SCRIPTS = new Set(['exec', 'install', 'run', 'dlx', 'tsc']);

// --- YAML step extraction ------------------------------------------------------

const indentOf = (line) => line.length - line.trimStart().length;

// Returns [{job, name, run, uses, line}] for every list-item step that has
// `run:` or `uses:` (exactly one of the two is set). `name` falls back to the
// run body's first line, or the `uses:` value, for unnamed steps.
export function extractSteps(yamlText) {
  const lines = yamlText.split(/\r?\n/);
  const steps = [];
  let job = null;
  let inJobs = false;
  let cur = null;
  const flush = () => {
    if (cur && cur.run !== undefined) {
      steps.push({ job: cur.job, name: cur.name ?? cur.run.split('\n')[0].trim(), run: cur.run, uses: undefined, line: cur.line });
    } else if (cur && cur.uses !== undefined) {
      steps.push({ job: cur.job, name: cur.name ?? cur.uses, run: undefined, uses: cur.uses, line: cur.line });
    }
    cur = null;
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim() || line.trimStart().startsWith('#')) continue;
    const ind = indentOf(line);
    if (ind === 0) {
      flush();
      inJobs = /^jobs:\s*$/.test(line);
      continue;
    }
    if (inJobs && ind === 2 && /^\s{2}[\w-]+:\s*$/.test(line)) {
      flush();
      job = line.trim().slice(0, -1);
      continue;
    }
    const item = /^(\s*)-\s+(.*)$/.exec(line);
    let keyText = null;
    if (item) {
      if (cur && ind <= cur.dashIndent) flush();
      if (!cur) {
        cur = { job, dashIndent: ind, keyIndent: ind + 2, line: i + 1 };
        keyText = item[2];
      }
    } else if (cur && ind === cur.keyIndent) {
      keyText = line.trim();
    } else if (cur && ind <= cur.dashIndent) {
      flush();
    }
    if (keyText === null || !cur) continue;
    const kv = /^([\w-]+):\s?(.*)$/.exec(keyText);
    if (!kv) continue;
    const [, key, rawVal] = kv;
    const val = rawVal.trim();
    if (key === 'name' && cur.name === undefined) cur.name = unquote(val);
    // A trailing ` # v7.0.1` pin comment is not part of the action reference.
    if (key === 'uses' && cur.uses === undefined) cur.uses = unquote(val.replace(/\s+#.*$/, ''));
    if (key === 'run') {
      if (/^[|>][-+]?$/.test(val)) {
        const body = [];
        let j = i + 1;
        let blockIndent = null;
        for (; j < lines.length; j++) {
          const l = lines[j];
          if (!l.trim()) { body.push(''); continue; }
          const li = indentOf(l);
          if (li <= cur.keyIndent) break;
          if (blockIndent === null) blockIndent = li;
          body.push(l.slice(Math.min(li, blockIndent)));
        }
        cur.run = body.join('\n').replace(/\n+$/, '');
        i = j - 1;
      } else {
        cur.run = unquote(val);
      }
    }
  }
  flush();
  return steps;
}

// Returns [{job, uses, line}] for every job that calls a reusable workflow
// (`jobs.<id>.uses:` - a job key, at the indent `extractSteps` reads job keys
// at), the one kind of job with no steps of its own. A step's `uses:` is an
// action, not this, and is left to `extractSteps`.
export function extractJobUses(yamlText) {
  const lines = yamlText.split(/\r?\n/);
  const calls = [];
  let inJobs = false;
  let job = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim() || line.trimStart().startsWith('#')) continue;
    const ind = indentOf(line);
    if (ind === 0) {
      inJobs = /^jobs:\s*$/.test(line);
      job = null;
      continue;
    }
    if (!inJobs) continue;
    if (ind === 2) {
      job = /^\s{2}([\w-]+):\s*$/.exec(line)?.[1] ?? null;
      continue;
    }
    const m = job !== null && ind === 4 ? /^\s{4}uses:\s*(.+)$/.exec(line) : null;
    if (m) calls.push({ job, uses: unquote(m[1].replace(/\s+#.*$/, '').trim()), line: i + 1 });
  }
  return calls;
}

function unquote(s) {
  if (s.length >= 2 && ((s[0] === '"' && s.at(-1) === '"') || (s[0] === "'" && s.at(-1) === "'"))) return s.slice(1, -1);
  return s;
}

// --- shell-ish splitting -------------------------------------------------------

// Split a run body / checklist block into logical commands: join `\` line
// continuations, drop `#` comments (outside quotes), and split at top-level
// `&&`, `||`, `;` and `|`. Quote- and `$(...)`-aware, nothing more.
export function splitCommands(body) {
  const joined = body.replace(/\\\r?\n/g, ' ');
  const out = [];
  for (const rawLine of joined.split(/\r?\n/)) {
    let cur = '';
    let quote = null;
    let depth = 0;
    const push = () => {
      if (cur.trim()) out.push(cur.trim());
      cur = '';
    };
    for (let i = 0; i < rawLine.length; i++) {
      const c = rawLine[i];
      const n = rawLine[i + 1];
      if (quote) {
        cur += c;
        if (c === '\\' && quote === '"') { cur += n ?? ''; i++; continue; }
        if (c === quote) quote = null;
        continue;
      }
      if (c === "'" || c === '"' || c === '`') { quote = c; cur += c; continue; }
      if (c === '$' && n === '(') { depth++; cur += '$('; i++; continue; }
      if (depth > 0) {
        if (c === '(') depth++;
        if (c === ')') depth--;
        cur += c;
        continue;
      }
      if (c === '#' && (i === 0 || /\s/.test(rawLine[i - 1]))) break; // comment to end of line
      if ((c === '&' && n === '&') || (c === '|' && n === '|')) { push(); i++; continue; }
      if (c === ';' || c === '|') { push(); continue; }
      cur += c;
    }
    push();
  }
  return out;
}

// Whitespace tokenizer that keeps quoted strings and `$(...)` whole.
export function tokenize(cmd) {
  const tokens = [];
  let cur = '';
  let quote = null;
  let depth = 0;
  for (let i = 0; i < cmd.length; i++) {
    const c = cmd[i];
    if (quote) { cur += c; if (c === quote) quote = null; continue; }
    if (c === "'" || c === '"' || c === '`') { quote = c; cur += c; continue; }
    if (c === '$' && cmd[i + 1] === '(') { depth++; cur += '$('; i++; continue; }
    if (depth > 0) { if (c === '(') depth++; if (c === ')') depth--; cur += c; continue; }
    if (/\s/.test(c)) { if (cur) tokens.push(cur); cur = ''; continue; }
    cur += c;
  }
  if (cur) tokens.push(cur);
  return tokens;
}

// --- normalisation -------------------------------------------------------------

// Build the reverse map "exact script command" -> script name.
export function scriptIndex(packageJson) {
  const byCommand = new Map();
  for (const [name, cmd] of Object.entries(packageJson?.scripts ?? {})) {
    if (!byCommand.has(cmd)) byCommand.set(cmd, name);
  }
  return byCommand;
}

// Normalise one command to a comparison key, or null if it is not a gate
// command (shell control flow, echo, assignments, apt, ...).
export function normalize(cmd, byCommand) {
  let tokens = tokenize(cmd);
  while (tokens.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[0])) tokens.shift(); // env prefix
  while (tokens.length && ['do', 'then', 'else'].includes(tokens[0])) tokens.shift();
  while (tokens.length > 1 && WRAPPER_SCRIPTS.some((w) => w.re.test(tokens[0]))) tokens.shift();
  if (tokens.length === 0) return null;
  const head = tokens[0];
  if (/^(\.\/)?scripts\/[\w.-]+\.(sh|mjs|js)$/.test(head)) {
    return [head.startsWith('./') ? head : `./${head}`, ...tokens.slice(1)].join(' ');
  }
  if (!GATE_TOOLS.has(head)) return null;
  if (head === 'cargo') {
    const sub = tokens[1] ?? '';
    const flags = [];
    const positional = [];
    for (let i = 2; i < tokens.length; i++) {
      const t = tokens[i];
      if (CARGO_VALUE_FLAGS.has(t) && i + 1 < tokens.length) { flags.push(`${t}=${tokens[++i]}`); continue; }
      if (t.startsWith('-')) flags.push(t);
      else positional.push(t);
    }
    return ['cargo', sub, ...flags.sort(), ...positional].join(' ');
  }
  if (head === 'node' && tokens.length === 2) {
    const name = byCommand.get(`node ${tokens[1]}`);
    if (name) return `pnpm ${name}`;
  }
  if (head === 'pnpm' && tokens[1] === 'run') tokens = ['pnpm', ...tokens.slice(2)];
  if (head === 'pnpm' && tokens[1] === 'exec') {
    const rest = tokens.slice(2).filter((t) => !t.startsWith('--reporter'));
    const name = byCommand.get(rest.join(' '));
    if (name) return `pnpm ${name}`;
    return ['pnpm', 'exec', ...rest].join(' ');
  }
  return tokens.join(' ');
}

// --- checklist parsing ---------------------------------------------------------

// Returns {commands: [{key, raw}], error}. The block is the first ```bash fence
// after CHECKLIST_HEADING.
export function parseChecklist(md, byCommand) {
  const lines = md.split(/\r?\n/);
  const h = lines.findIndex((l) => l.trim() === CHECKLIST_HEADING);
  if (h < 0) return { commands: [], error: `${CHECKLIST_FILE} has no "${CHECKLIST_HEADING}" heading` };
  const open = lines.findIndex((l, i) => i > h && /^```(bash|sh)?\s*$/.test(l.trim()));
  if (open < 0) return { commands: [], error: `no fenced code block after "${CHECKLIST_HEADING}"` };
  const close = lines.findIndex((l, i) => i > open && l.trim() === '```');
  if (close < 0) return { commands: [], error: `unterminated code block after "${CHECKLIST_HEADING}"` };
  const commands = [];
  for (const raw of splitCommands(lines.slice(open + 1, close).join('\n'))) {
    const key = normalize(raw, byCommand);
    if (key !== null) commands.push({ key, raw });
  }
  if (commands.length === 0) return { commands, error: `the checklist block under "${CHECKLIST_HEADING}" has no commands` };
  return { commands, error: null };
}

// --- the check -----------------------------------------------------------------

// `read(rel)` returns file text or throws. `opts` overrides the tables (the
// fixture test drives small synthetic workflows). Returns {problems, gates}.
export function checkDrift(read, opts = {}) {
  const {
    workflows = WORKFLOWS,
    plumbingSteps = PLUMBING_STEPS,
    shellGateSteps = SHELL_GATE_STEPS,
    plumbingActions = PLUMBING_ACTIONS,
    actionGateSteps = ACTION_GATE_STEPS,
    aliases = ALIASES,
  } = opts;
  const problems = [];
  let packageJson;
  try {
    packageJson = JSON.parse(read('package.json'));
  } catch (e) {
    return { problems: [`package.json unreadable: ${e.code || e.message}`], gates: 0 };
  }
  const byCommand = scriptIndex(packageJson);
  let md;
  try {
    md = read(CHECKLIST_FILE);
  } catch (e) {
    return { problems: [`${CHECKLIST_FILE} unreadable: ${e.code || e.message}`], gates: 0 };
  }
  const checklist = parseChecklist(md, byCommand);
  if (checklist.error) return { problems: [checklist.error], gates: 0 };
  const have = new Set(checklist.commands.map((c) => c.key));
  const aliasOf = new Map(aliases.map((a) => [normalize(a.ci, byCommand), normalize(a.checklist, byCommand)]));
  const usedPlumbing = new Set();
  const usedShell = new Set();
  const usedActions = new Set();
  const usedActionGates = new Set();
  let gates = 0;

  // The listed workflows, then every local reusable workflow one of their jobs
  // calls (#576 NF-W4M-5), each read once.
  const queue = [...workflows];
  const queued = new Set(queue);
  for (let qi = 0; qi < queue.length; qi++) {
    const wf = queue[qi];
    let text;
    try {
      text = read(wf);
    } catch (e) {
      problems.push(`${wf} unreadable: ${e.code || e.message}`);
      continue;
    }
    const calls = extractJobUses(text);
    for (const call of calls) {
      const local = /^\.\/(\.github\/workflows\/[^@\s]+\.ya?ml)$/.exec(call.uses);
      if (!local) {
        problems.push(`${wf} (${call.job}) calls the reusable workflow "${call.uses}", which this gate cannot read to compare with the checklist - it follows only local ones (./.github/workflows/...)`);
        continue;
      }
      if (!queued.has(local[1])) {
        queued.add(local[1]);
        queue.push(local[1]);
      }
    }
    const steps = extractSteps(text);
    if (steps.length === 0) {
      if (calls.length === 0) problems.push(`${wf}: no run steps found - the step parser no longer understands this file`);
      continue;
    }
    for (const step of steps) {
      const where = `${wf} (${step.job ?? '?'}) step "${step.name}"`;
      const action = step.uses === undefined ? undefined : plumbingActions.find((a) => a.action === step.uses.split('@')[0]);
      if (action) usedActions.add(action);
      const shell = shellGateSteps.find((s) => s.workflow === wf && s.step === step.name);
      const actionGate = step.uses === undefined ? undefined : actionGateSteps.find((s) => s.workflow === wf && s.step === step.name);
      const gateRow = shell ?? actionGate;
      if (gateRow) {
        (shell ? usedShell : usedActionGates).add(gateRow);
        gates++;
        if (!have.has(normalize(gateRow.checklist, byCommand))) {
          problems.push(`${where} is ${shell ? 'a shell' : 'an action'} gate; the checklist must carry: ${gateRow.checklist}`);
        }
        continue;
      }
      const plumbing = plumbingSteps.find((s) => s.workflow === wf && s.step === step.name);
      if (plumbing) usedPlumbing.add(plumbing);
      if (step.run === undefined) {
        if (!plumbing && !action) {
          problems.push(`${where} uses the action "${step.uses}", whose command this gate cannot compare with the checklist - list the action in PLUMBING_ACTIONS if it gates nothing, or classify the step in ACTION_GATE_STEPS / PLUMBING_STEPS (scripts/check-prepush-checklist.mjs)`);
        }
        continue;
      }
      const all = splitCommands(step.run).map((raw) => ({ raw, key: normalize(raw, byCommand) }));
      const cmds = all.filter((c) => c.key !== null);
      if (cmds.length === 0) {
        if (!plumbing) problems.push(`${where} runs no recognised gate command - classify it in PLUMBING_STEPS or SHELL_GATE_STEPS (scripts/check-prepush-checklist.mjs)`);
        continue;
      }
      if (!plumbing) {
        for (const c of all) {
          if (c.key !== null || isShellGlue(c.raw)) continue;
          problems.push(`${where} also runs "${c.raw}", which this gate cannot compare with the checklist - make it a recognised gate command or classify the step in SHELL_GATE_STEPS / PLUMBING_STEPS (scripts/check-prepush-checklist.mjs)`);
        }
      }
      for (const c of cmds) {
        if (PLUMBING_COMMANDS.some((p) => p.re.test(c.key) || p.re.test(c.raw))) continue;
        gates++;
        const key = aliasOf.get(c.key) ?? c.key;
        if (!have.has(key)) problems.push(`missing from the checklist: ${key}   (CI: ${where}: ${c.raw})`);
      }
    }
  }
  for (const s of plumbingSteps) if (!usedPlumbing.has(s)) problems.push(`stale PLUMBING_STEPS row: ${s.workflow} step "${s.step}" no longer exists`);
  for (const s of shellGateSteps) if (!usedShell.has(s)) problems.push(`stale SHELL_GATE_STEPS row: ${s.workflow} step "${s.step}" no longer exists`);
  for (const s of actionGateSteps) if (!usedActionGates.has(s)) problems.push(`stale ACTION_GATE_STEPS row: ${s.workflow} step "${s.step}" no longer exists`);
  for (const a of plumbingActions) if (!usedActions.has(a)) problems.push(`stale PLUMBING_ACTIONS row: no step uses ${a.action}`);

  const scripts = packageJson.scripts ?? {};
  for (const c of checklist.commands) {
    const t = tokenize(c.key);
    if (t[0] === 'pnpm' && t[1] && !PNPM_NON_SCRIPTS.has(t[1]) && !(t[1] in scripts)) {
      problems.push(`the checklist runs "pnpm ${t[1]}", which package.json does not define (renamed or removed?)`);
    }
  }
  return { problems, gates, workflowsRead: queue.length };
}

export function main(read = (rel) => fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8')) {
  const { problems, gates, workflowsRead } = checkDrift(read);
  if (problems.length === 0) {
    console.log(`✓ pre-push checklist: all ${gates} CI gate commands in ${workflowsRead} workflows (${WORKFLOWS.length} listed, the rest called from them) are in ${CHECKLIST_FILE}.`);
    return 0;
  }
  console.error(`\n✗ pre-push checklist drift - ${problems.length} problem(s):\n`);
  for (const p of problems) console.error(`  ${p}`);
  console.error(
    `\nAdd the command to the "${CHECKLIST_HEADING}" block in ${CHECKLIST_FILE} (CI's gates are the\n` +
      'floor of that list), or classify a non-gate step in scripts/check-prepush-checklist.mjs.\n',
  );
  return 1;
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
