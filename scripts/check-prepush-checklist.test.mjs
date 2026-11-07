// Fixture tests for the pre-push checklist drift gate (scripts/check-prepush-checklist.mjs).
//
// A drift gate that never fires proves nothing, so each normalisation rule is
// pinned with a synthetic workflow + checklist pair that MUST fail, next to the
// matching pair that must pass. The reader is injected, so no case touches the
// real tree except the last one, which runs the gate on the live repo.
//
// Pure Node, no test framework (mirrors scripts/check-no-em-dash.test.mjs).
// Run: node scripts/check-prepush-checklist.test.mjs  (also a CI leak-scan step).

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import {
  checkDrift, extractSteps, extractJobUses, splitCommands, normalize, scriptIndex, parseChecklist, REPO_ROOT,
  PLUMBING_STEPS, SHELL_GATE_STEPS, PLUMBING_ACTIONS, ACTION_GATE_STEPS, isShellGlue,
} from './check-prepush-checklist.mjs';

let passed = 0;
const failures = [];
function test(name, fn) {
  try {
    fn();
    passed++;
  } catch (e) {
    failures.push(`${name}: ${e.message}`);
  }
}

const WF = '.github/workflows/fixture.yml';
const PKG = JSON.stringify({
  scripts: {
    'lint:foo': 'node scripts/check-foo.mjs',
    'lint:foo:test': 'node scripts/check-foo.test.mjs',
    'test:ui:smoke': 'playwright test --grep @smoke',
    'typecheck:agent': 'tsc -p tsconfig.agent.json',
    test: 'vitest',
  },
});

const workflow = (steps) => `name: Fixture
on:
  pull_request:
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
${steps}
`;
const checklist = (lines) => `# Rules

### Pre-Push Checklist

All must pass before pushing:

\`\`\`bash
${lines.join('\n')}
\`\`\`

Prose after the block.
`;

// Run the gate over an in-memory tree with empty plumbing/shell tables unless
// a case passes its own; the only plumbing action is the checkout every
// fixture workflow starts with.
const CHECKOUT_ONLY = [{ action: 'actions/checkout', why: 'x' }];
function run(files, opts = {}) {
  const tree = { 'package.json': PKG, ...files };
  const read = (rel) => {
    if (!(rel in tree)) throw Object.assign(new Error(`ENOENT ${rel}`), { code: 'ENOENT' });
    return tree[rel];
  };
  return checkDrift(read, {
    workflows: [WF], plumbingSteps: [], shellGateSteps: [], plumbingActions: CHECKOUT_ONLY, actionGateSteps: [], aliases: [], ...opts,
  });
}

// --- the gate fires on a missing command -----------------------------------------

test('a CI gate command that the checklist lacks fails and is named', () => {
  const r = run({
    [WF]: workflow('      - run: pnpm lint:foo\n      - run: pnpm typecheck:agent'),
    'CLAUDE.md': checklist(['pnpm lint:foo']),
  });
  assert.equal(r.problems.length, 1);
  assert.match(r.problems[0], /missing from the checklist: pnpm typecheck:agent/);
});

test('the same pair passes once the checklist carries the command', () => {
  const r = run({
    [WF]: workflow('      - run: pnpm lint:foo\n      - run: pnpm typecheck:agent'),
    'CLAUDE.md': checklist(['pnpm lint:foo', 'pnpm typecheck:agent   # trailing comment is fine']),
  });
  assert.deepEqual(r.problems, []);
  assert.equal(r.gates, 2);
});

test('a :test sibling CI runs first is required too (the drift E6 found)', () => {
  const steps = '      - name: Foo\n        run: |\n          pnpm lint:foo:test\n          pnpm lint:foo';
  const r = run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(r.problems.length, 1);
  assert.match(r.problems[0], /pnpm lint:foo:test/);
  assert.deepEqual(run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['pnpm lint:foo:test && pnpm lint:foo']) }).problems, []);
});

test('a command named only in a checklist COMMENT does not count', () => {
  const r = run({
    [WF]: workflow('      - run: pnpm lint:foo'),
    'CLAUDE.md': checklist(['pnpm tsc --noEmit   # and pnpm lint:foo']),
  });
  assert.equal(r.problems.length, 1);
});

// --- normalisation ------------------------------------------------------------------

test('`node scripts/X.mjs` in CI matches the pnpm script name in the checklist', () => {
  const r = run({
    [WF]: workflow('      - run: node scripts/check-foo.test.mjs\n      - run: node scripts/check-foo.mjs'),
    'CLAUDE.md': checklist(['pnpm lint:foo:test && pnpm lint:foo']),
  });
  assert.deepEqual(r.problems, []);
});

test('`pnpm exec playwright test --grep @smoke --reporter=...` is `pnpm test:ui:smoke`', () => {
  const steps = '      - run: pnpm exec playwright test --grep @smoke --reporter=github,./scripts/x.mjs';
  assert.deepEqual(run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['pnpm test:ui:smoke']) }).problems, []);
  const bare = run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['pnpm exec playwright test']) });
  assert.match(bare.problems[0], /pnpm test:ui:smoke/);
});

test('cargo flags compare as a set: order and an env prefix do not matter', () => {
  const steps = '      - run: cargo clippy --workspace --manifest-path src-tauri/Cargo.toml --features telemetry --all-targets\n'
    + '        env:\n          TELEMETRY_ENDPOINT: "https://test.invalid"';
  const line = 'TELEMETRY_ENDPOINT=http://localhost/dummy cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features telemetry --workspace';
  assert.deepEqual(run({ [WF]: workflow(steps), 'CLAUDE.md': checklist([line]) }).problems, []);
});

test('a cargo flag CI passes and the checklist omits (--locked, --release) fails', () => {
  const locked = '      - run: cargo test --locked --manifest-path vendor/x/Cargo.toml';
  assert.equal(run({ [WF]: workflow(locked), 'CLAUDE.md': checklist(['cargo test --manifest-path vendor/x/Cargo.toml']) }).problems.length, 1);
  const release = '      - run: cargo check --release --workspace --manifest-path src-tauri/Cargo.toml';
  assert.equal(run({ [WF]: workflow(release), 'CLAUDE.md': checklist(['cargo check --workspace --manifest-path src-tauri/Cargo.toml']) }).problems.length, 1);
});

test('a for-loop body in the checklist matches the loop body CI runs', () => {
  const steps = '      - name: fmt\n        run: |\n          for m in crates/*/Cargo.toml; do\n            [ -e "$m" ] || continue\n'
    + '            cargo fmt --manifest-path "$m" --check\n          done';
  const r = run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['for m in crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done']) });
  assert.deepEqual(r.problems, []);
});

test('an alias row maps the CI spelling to the local one', () => {
  const opts = { aliases: [{ ci: 'pnpm test', checklist: 'pnpm exec vitest run', why: 'x' }] };
  assert.deepEqual(run({ [WF]: workflow('      - run: pnpm test'), 'CLAUDE.md': checklist(['pnpm exec vitest run']) }, opts).problems, []);
  assert.equal(run({ [WF]: workflow('      - run: pnpm test'), 'CLAUDE.md': checklist(['pnpm exec vitest run']) }).problems.length, 1);
});

test('plumbing commands (pnpm install, playwright install) are not gates', () => {
  const steps = '      - run: pnpm install --frozen-lockfile\n      - run: pnpm exec playwright install --with-deps chromium';
  const r = run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['pnpm tsc --noEmit']) });
  assert.deepEqual(r.problems, []);
  assert.equal(r.gates, 0);
});

// --- steps that run no recognised gate command ------------------------------------------

const SHELL_STEP = '      - name: Bindings clean\n        run: |\n          if ! git diff --exit-code bindings; then\n            exit 1\n          fi';

test('a pure-shell step nobody classified fails (a new shell gate cannot hide)', () => {
  const r = run({ [WF]: workflow(SHELL_STEP), 'CLAUDE.md': checklist(['pnpm tsc --noEmit']) });
  assert.equal(r.problems.length, 1);
  assert.match(r.problems[0], /runs no recognised gate command/);
});

test('a PLUMBING_STEPS row clears it; a SHELL_GATE_STEPS row demands its checklist command', () => {
  const plumbing = { plumbingSteps: [{ workflow: WF, step: 'Bindings clean', why: 'x' }] };
  assert.deepEqual(run({ [WF]: workflow(SHELL_STEP), 'CLAUDE.md': checklist(['pnpm tsc --noEmit']) }, plumbing).problems, []);
  const shell = { shellGateSteps: [{ workflow: WF, step: 'Bindings clean', checklist: 'git status --porcelain bindings', why: 'x' }] };
  assert.match(run({ [WF]: workflow(SHELL_STEP), 'CLAUDE.md': checklist(['pnpm tsc --noEmit']) }, shell).problems[0], /git status --porcelain bindings/);
  assert.deepEqual(run({ [WF]: workflow(SHELL_STEP), 'CLAUDE.md': checklist(['git status --porcelain bindings']) }, shell).problems, []);
});

// #576 RV3-FE-12: a recognised command used to vouch for the whole step.
test('a MIXED step (recognised command + one the gate cannot compare) fails unless classified', () => {
  const cases = {
    'plumbing + python gate': '      - name: Lint\n        run: pnpm install && python3 scripts/gate.py',
    'node gate + bash gate': '      - name: Lint\n        run: node scripts/check-foo.mjs && bash scripts/new-gate.sh',
    'gate behind if': '      - name: Lint\n        run: |\n          pnpm lint:foo\n          if ! make lint; then\n            exit 1\n          fi',
  };
  for (const [name, step] of Object.entries(cases)) {
    const r = run({ [WF]: workflow(step), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
    assert.equal(r.problems.length, 1, `${name}: ${JSON.stringify(r.problems)}`);
    assert.match(r.problems[0], /step "Lint" also runs ".*", which this gate cannot compare/, name);
    const plumbing = { plumbingSteps: [{ workflow: WF, step: 'Lint', why: 'x' }] };
    assert.deepEqual(run({ [WF]: workflow(step), 'CLAUDE.md': checklist(['pnpm lint:foo']) }, plumbing).problems, [], name);
  }
});

test('shell glue next to a gate (loops, tests, echo, exit, assignments) is not a mixed step', () => {
  const step = [
    '      - name: Fmt',
    '        run: |',
    '          f="report.json"',
    '          for m in crates/*/Cargo.toml; do',
    '            [ -e "$m" ] || continue',
    '            cargo fmt --manifest-path "$m" --check',
    '          done',
    '          if [ ! -f "$f" ]; then echo "none" >> "$GITHUB_STEP_SUMMARY"; exit 0; fi',
  ].join('\n');
  const r = run({ [WF]: workflow(step), 'CLAUDE.md': checklist(['cargo fmt --manifest-path "$m" --check']) });
  assert.deepEqual(r.problems, []);
  for (const glue of ['f=1', 'then', 'done', 'if [ -f x ]', 'echo hi', 'exit 1', 'export A=1']) assert.ok(isShellGlue(glue), glue);
  for (const tool of ['python3 x.py', 'if ! make lint', 'bash scripts/x.sh', 'A=1 ruby x.rb']) assert.ok(!isShellGlue(tool), tool);
});

// #576 RV4-FE-FEB-4: glue was judged by its first word, so a gate inside a command
// substitution passed as glue next to a recognised command.
const HIDDEN_IN_SUBSTITUTION = [
  'test -z "$(python3 scripts/gate.py)"',
  '[ -z "$(git status --porcelain)" ]',
  'echo "$(python3 x.py)"',
  'exit $(python3 x.py)',
  'OUT=$(python3 scripts/gate.py)',
  'test -z "`python3 scripts/gate.py`"',
  '[ -z "`git status --porcelain`" ]',
  'echo "`python3 x.py`"',
  'exit `python3 x.py`',
  'OUT=`python3 scripts/gate.py`',
];
for (const form of HIDDEN_IN_SUBSTITUTION) {
  test(`a gate in a command substitution is not glue: ${form}`, () => {
    const step = `      - name: Lint\n        run: |\n          pnpm lint:foo\n          ${form}`;
    const r = run({ [WF]: workflow(step), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
    assert.equal(r.problems.length, 1, JSON.stringify(r.problems));
    assert.match(r.problems[0], /step "Lint" also runs ".*", which this gate cannot compare/);
    const plumbing = { plumbingSteps: [{ workflow: WF, step: 'Lint', why: 'x' }] };
    assert.deepEqual(run({ [WF]: workflow(step), 'CLAUDE.md': checklist(['pnpm lint:foo']) }, plumbing).problems, []);
  });
}

test('glue whose substitutions run only glue stays glue; a single-quoted $(...) expands nothing', () => {
  for (const glue of ['echo "v=$(echo 1)"', "echo '$(python3 x)'", "echo '`python3 x`'", 'v=$(printf a | true; echo b)','echo "$((1 + 2))"', 'echo "\\$(python3 x)"']) {
    assert.ok(isShellGlue(glue), glue);
  }
  for (const tool of ['echo "$(echo "$(python3 x)")"', 'echo "$(true && python3 x)"', 'echo "$((1 + $(python3 x)))"', 'v=$(echo ")"; python3 x)']) {
    assert.ok(!isShellGlue(tool), tool);
  }
});

test('a classification row whose step no longer exists is reported stale', () => {
  const opts = { plumbingSteps: [{ workflow: WF, step: 'Renamed away', why: 'x' }] };
  const r = run({ [WF]: workflow('      - run: pnpm lint:foo'), 'CLAUDE.md': checklist(['pnpm lint:foo']) }, opts);
  assert.equal(r.problems.length, 1);
  assert.match(r.problems[0], /stale PLUMBING_STEPS row/);
});

// --- `uses:` steps (#576 NF-FIX3B-1): an action carries its command inside it ----------------

const DENY_STEP = '      - name: Deny\n        uses: EmbarkStudios/cargo-deny-action@0123456789abcdef0123456789abcdef01234567 # v2\n'
  + '        with:\n          command: check bans';

test('an action step nobody classified fails (a gate added as an action cannot hide)', () => {
  const r = run({ [WF]: workflow(`${DENY_STEP}\n      - run: pnpm lint:foo`), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(r.problems.length, 1, JSON.stringify(r.problems));
  assert.match(r.problems[0], /step "Deny" uses the action "EmbarkStudios\/cargo-deny-action@[0-9a-f]+", whose command this gate cannot compare/);
});

test('an unnamed action step is named by its action, and a local action is not exempt', () => {
  const r = run({ [WF]: workflow('      - uses: ./.github/actions/lint\n      - run: pnpm lint:foo'), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(r.problems.length, 1, JSON.stringify(r.problems));
  assert.match(r.problems[0], /step "\.\/\.github\/actions\/lint" uses the action/);
});

test('a PLUMBING_ACTIONS row (any ref) or a PLUMBING_STEPS row clears an action step', () => {
  const files = { [WF]: workflow(DENY_STEP), 'CLAUDE.md': checklist(['pnpm lint:foo']) };
  const byAction = { plumbingActions: [...CHECKOUT_ONLY, { action: 'EmbarkStudios/cargo-deny-action', why: 'x' }] };
  assert.deepEqual(run(files, byAction).problems, []);
  assert.equal(run(files, byAction).gates, 0);
  assert.deepEqual(run(files, { plumbingSteps: [{ workflow: WF, step: 'Deny', why: 'x' }] }).problems, []);
});

test('an ACTION_GATE_STEPS row makes the action a gate that demands its checklist command', () => {
  const opts = { actionGateSteps: [{ workflow: WF, step: 'Deny', checklist: 'cargo deny check bans', why: 'x' }] };
  const missing = run({ [WF]: workflow(DENY_STEP), 'CLAUDE.md': checklist(['pnpm lint:foo']) }, opts);
  assert.equal(missing.problems.length, 1, JSON.stringify(missing.problems));
  assert.match(missing.problems[0], /step "Deny" is an action gate; the checklist must carry: cargo deny check bans/);
  const carried = run({ [WF]: workflow(DENY_STEP), 'CLAUDE.md': checklist(['cargo deny check bans']) }, opts);
  assert.deepEqual(carried.problems, []);
  assert.equal(carried.gates, 1);
});

test('a PLUMBING_ACTIONS or ACTION_GATE_STEPS row that matches no step is reported stale', () => {
  const opts = {
    plumbingActions: [...CHECKOUT_ONLY, { action: 'actions/cache', why: 'x' }],
    actionGateSteps: [{ workflow: WF, step: 'Gone', checklist: 'pnpm lint:foo', why: 'x' }],
  };
  const r = run({ [WF]: workflow('      - run: pnpm lint:foo'), 'CLAUDE.md': checklist(['pnpm lint:foo']) }, opts);
  assert.deepEqual(r.problems.map((p) => p.replace(/:.*/, '')), ['stale ACTION_GATE_STEPS row', 'stale PLUMBING_ACTIONS row']);
});

// --- reverse direction: checklist names a script package.json does not define -------------

test('a checklist `pnpm <script>` that package.json lacks fails (renamed script)', () => {
  const r = run({ [WF]: workflow('      - run: pnpm lint:foo'), 'CLAUDE.md': checklist(['pnpm lint:foo', 'pnpm lint:renamed-away']) });
  assert.equal(r.problems.length, 1);
  assert.match(r.problems[0], /"pnpm lint:renamed-away"/);
});

// --- fail closed ------------------------------------------------------------------------------

test('no checklist heading, no code block, or an empty block each fail', () => {
  const wf = { [WF]: workflow('      - run: pnpm lint:foo') };
  assert.match(run({ ...wf, 'CLAUDE.md': '# nothing here\n' }).problems[0], /no "### Pre-Push Checklist" heading/);
  assert.match(run({ ...wf, 'CLAUDE.md': '### Pre-Push Checklist\n\njust prose\n' }).problems[0], /no fenced code block/);
  assert.match(run({ ...wf, 'CLAUDE.md': checklist(['# only a comment']) }).problems[0], /has no commands/);
});

test('a missing workflow file or one the parser reads no steps from fails', () => {
  assert.match(run({ 'CLAUDE.md': checklist(['pnpm lint:foo']) }).problems[0], /unreadable/);
  assert.match(run({ [WF]: 'name: x\n', 'CLAUDE.md': checklist(['pnpm lint:foo']) }).problems[0], /no run steps found/);
});

// --- job-level reusable workflows (#576 NF-W4M-5) ------------------------------------------------

// A job that runs no steps of its own: it CALLS another workflow, whose steps run
// as part of this one (`jobs.<id>.uses:`, how ci.yml calls pick-runner.yml).
const REUSABLE = '.github/workflows/reusable.yml';
const caller = (uses) => `name: Caller
on:
  pull_request:
jobs:
  pick:
    uses: ${uses}
    with:
      mode: fast
  check:
    needs: pick
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - run: pnpm lint:foo
`;
const reusable = (extraJobs = '') => `name: Reusable
on:
  workflow_call:
jobs:
  inner:
    runs-on: ubuntu-latest
    steps:
      - run: pnpm typecheck:agent
${extraJobs}`;

test('a gate in a local reusable workflow a job calls is required too', () => {
  const tree = { [WF]: caller('./.github/workflows/reusable.yml'), [REUSABLE]: reusable() };
  const r = run({ ...tree, 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(r.problems.length, 1, r.problems.join('\n'));
  assert.match(r.problems[0], /missing from the checklist: pnpm typecheck:agent .*reusable\.yml/);
  const ok = run({ ...tree, 'CLAUDE.md': checklist(['pnpm lint:foo', 'pnpm typecheck:agent']) });
  assert.deepEqual(ok.problems, []);
  assert.equal(ok.gates, 2);
});

test('a reusable workflow called back (a cycle) is read once, and its steps classify under its own path', () => {
  const back = '  again:\n    uses: ./.github/workflows/fixture.yml\n';
  const tree = { [WF]: caller('./.github/workflows/reusable.yml'), [REUSABLE]: reusable(back) };
  const r = run({ ...tree, 'CLAUDE.md': checklist(['pnpm lint:foo', 'pnpm typecheck:agent']) });
  assert.deepEqual(r.problems, []);
  assert.equal(r.gates, 2);
  // A classification row names the reusable file, not the caller.
  const shell = '      - name: Route\n        run: echo routed';
  const routed = { [WF]: caller('./.github/workflows/reusable.yml'), [REUSABLE]: reusable().replace('      - run: pnpm typecheck:agent', shell) };
  assert.match(run({ ...routed, 'CLAUDE.md': checklist(['pnpm lint:foo']) }).problems[0] ?? '', /reusable\.yml \(inner\) step "Route" runs no recognised gate command/);
  const rows = [{ workflow: REUSABLE, step: 'Route', why: 'x' }];
  assert.deepEqual(run({ ...routed, 'CLAUDE.md': checklist(['pnpm lint:foo']) }, { plumbingSteps: rows }).problems, []);
});

test('a reusable workflow from another repository, or a missing local one, fails', () => {
  const remote = run({ [WF]: caller('octo/ci/.github/workflows/x.yml@v1'), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(remote.problems.length, 1, remote.problems.join('\n'));
  assert.match(remote.problems[0], /calls the reusable workflow "octo\/ci\/\.github\/workflows\/x\.yml@v1", which this gate cannot read/);
  const gone = run({ [WF]: caller('./.github/workflows/gone.yml'), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(gone.problems.length, 1, gone.problems.join('\n'));
  assert.match(gone.problems[0], /\.github\/workflows\/gone\.yml unreadable/);
});

test('extractJobUses reads job-level uses only, not a step-level one', () => {
  assert.deepEqual(extractJobUses(caller('"./.github/workflows/reusable.yml" # pinned')), [
    { job: 'pick', uses: './.github/workflows/reusable.yml', line: 6 },
  ]);
});

// --- parser units -----------------------------------------------------------------------------

test('extractSteps reads named, unnamed and block-scalar run steps with their job', () => {
  const steps = extractSteps(workflow('      - name: A\n        run: |\n          one\n          two\n      - run: three\n        env:\n          X: y'));
  assert.deepEqual(steps.map((s) => [s.job, s.name, s.run]), [
    ['check', 'actions/checkout@v7', undefined],
    ['check', 'A', 'one\ntwo'],
    ['check', 'three', 'three'],
  ]);
});

test('extractSteps reads uses steps, dropping a trailing pin comment, and not a nested `run:`', () => {
  const steps = extractSteps(workflow(`${DENY_STEP}\n      - name: B\n        uses: "org/act@v1"\n        with:\n          run: pnpm lint:foo`));
  assert.deepEqual(steps.map((s) => [s.name, s.uses, s.run]), [
    ['actions/checkout@v7', 'actions/checkout@v7', undefined],
    ['Deny', 'EmbarkStudios/cargo-deny-action@0123456789abcdef0123456789abcdef01234567', undefined],
    ['B', 'org/act@v1', undefined],
  ]);
});

test('splitCommands keeps quoted separators and $(...) together', () => {
  assert.deepEqual(splitCommands('a && b || c; d | e'), ['a', 'b', 'c', 'd', 'e']);
  assert.deepEqual(splitCommands('echo "x && y" && z'), ['echo "x && y"', 'z']);
  assert.deepEqual(splitCommands('v=$(p | q) && r'), ['v=$(p | q)', 'r']);
  assert.deepEqual(splitCommands('one \\\n  two'), ['one    two']);
});

test('normalize ignores shell plumbing and assignments', () => {
  const by = scriptIndex(JSON.parse(PKG));
  for (const c of ['echo hi', 'if [ -n "$x" ]', 'fi', 'untracked=$(git ls-files x)', 'sudo apt-get install y', 'exit 1']) {
    assert.equal(normalize(c, by), null, c);
  }
  assert.equal(normalize('./scripts/test-release.sh', by), './scripts/test-release.sh');
});

test('parseChecklist only reads the block under the heading', () => {
  const md = `\`\`\`bash\npnpm before-heading\n\`\`\`\n${checklist(['pnpm lint:foo'])}\n\`\`\`bash\npnpm after-block\n\`\`\`\n`;
  const r = parseChecklist(md, new Map());
  assert.deepEqual(r.commands.map((c) => c.key), ['pnpm lint:foo']);
});

// --- wrapper scripts (#576 RV5-PI-1) ------------------------------------------------------------

test('a command run through the unprivileged-test wrapper is the wrapped gate', () => {
  const steps = '      - run: ./scripts/ci-cargo-test-unprivileged.sh cargo test --workspace --manifest-path src-tauri/Cargo.toml';
  const missing = run({ [WF]: workflow(steps), 'CLAUDE.md': checklist(['pnpm lint:foo']) });
  assert.equal(missing.problems.length, 1);
  assert.match(missing.problems[0], /missing from the checklist: cargo test --manifest-path=src-tauri\/Cargo.toml --workspace/);
  const covered = run({
    [WF]: workflow(steps),
    'CLAUDE.md': checklist(['cargo test --manifest-path src-tauri/Cargo.toml --workspace']),
  });
  assert.deepEqual(covered.problems, []);
  assert.equal(covered.gates, 1);
  assert.equal(
    normalize('./scripts/ci-cargo-test-unprivileged.sh cargo test --workspace', new Map()),
    normalize('cargo test --workspace', new Map()),
  );
});

// The runner must reach the wrapped `cargo test` and nothing after it: `cargo run`
// honours it too, and `pnpm gen:ipc-types` failed as the test user (#576 RV5-PI-1).
test('the live workflows hand the unprivileged runner to one wrapped command, never to GITHUB_ENV', () => {
  const script = 'scripts/ci-cargo-test-unprivileged.sh';
  const source = fs.readFileSync(path.join(REPO_ROOT, script), 'utf8');
  assert.doesNotMatch(source, />>\s*"?\$\{?GITHUB_ENV/, `${script} must not configure later steps through GITHUB_ENV`);
  let calls = 0;
  for (const wf of ['.github/workflows/ci.yml', '.github/workflows/runner-bench.yml']) {
    for (const step of extractSteps(fs.readFileSync(path.join(REPO_ROOT, wf), 'utf8'))) {
      for (const cmd of splitCommands(step.run ?? '')) {
        const tokens = cmd.trim().split(/\s+/);
        if (!/^(\.\/)?scripts\/ci-cargo-test-unprivileged\.sh$/.test(tokens[0])) continue;
        calls++;
        assert.ok(tokens.length > 1 && tokens[1] === 'cargo', `${wf} step "${step.name}" calls ${script} without the cargo command it wraps`);
      }
    }
  }
  assert.ok(calls >= 2, `expected the wrapper in ci.yml and runner-bench.yml, found ${calls} call(s)`);
  // Unprivileged already (every dev machine, the hosted runner): it runs the command.
  if (typeof process.getuid === 'function' && process.getuid() !== 0) {
    const ran = spawnSync('bash', [path.join(REPO_ROOT, script), 'node', '-e', 'process.stdout.write("wrapped command ran")'], { encoding: 'utf8' });
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stdout, /wrapped command ran/);
  }
});

// "Lints first, tests still run" (#576 E18(h)) must hold for a FORMAT failure too, the
// most common lint failure: with an implicit success() a failed `cargo fmt --check`
// skipped Clippy (its findings hidden until the next run) and the hosted Rust cache
// restore (every later Rust step compiled cold) (#576 RV4-FE-FEB-5). Every step of the
// `check` job after the fmt gate carries !cancelled() (or always()), except the
// PLUMBING_STEPS rows (observability that gates nothing).
test('in ci.yml `check`, every gate and cache step after cargo fmt still runs when fmt fails', () => {
  const wf = '.github/workflows/ci.yml';
  const lines = fs.readFileSync(path.join(REPO_ROOT, wf), 'utf8').split(/\r?\n/);
  const steps = [];
  let inCheck = false;
  for (const line of lines) {
    if (/^ {2}[\w-]+:\s*$/.test(line)) inCheck = line.trim() === 'check:';
    if (!inCheck) continue;
    const item = /^ {6}- (.*)$/.exec(line);
    const key = item ? item[1] : (/^ {8}(\S.*)$/.exec(line)?.[1] ?? null);
    if (item) steps.push({ name: null, uses: null, cond: null });
    if (key === null || steps.length === 0) continue;
    const kv = /^(name|uses|if):\s*(.*)$/.exec(key);
    if (kv) steps.at(-1)[kv[1] === 'if' ? 'cond' : kv[1]] = kv[2].trim();
  }
  const fmt = steps.findIndex((s) => s.name === 'Rust formatting (cargo fmt --check)');
  assert.ok(fmt >= 0 && steps.length - fmt > 10, `the fmt step or the steps after it are gone (${steps.length} steps read)`);
  const plumbing = new Set(PLUMBING_STEPS.filter((s) => s.workflow === wf).map((s) => s.step));
  const skippedOnFmtFailure = steps
    .slice(fmt + 1)
    .filter((s) => !plumbing.has(s.name) && !/!cancelled\(\)|always\(\)/.test(s.cond ?? ''))
    .map((s) => s.name ?? s.uses);
  assert.deepEqual(skippedOnFmtFailure, [], 'these steps keep the implicit success() and are skipped when cargo fmt fails');
});

// --- the live repo -------------------------------------------------------------------------------

test('the live tree passes: every CI gate is in the CLAUDE.md checklist', () => {
  const r = checkDrift((rel) => fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8'));
  assert.deepEqual(r.problems, []);
  assert.ok(r.gates >= 25, `only ${r.gates} gates seen - the parser stopped reading a workflow?`);
});

test('every live classification row carries a written reason', () => {
  for (const row of [...PLUMBING_STEPS, ...SHELL_GATE_STEPS, ...PLUMBING_ACTIONS, ...ACTION_GATE_STEPS]) {
    assert.ok(row.why && row.why.trim(), row.step ?? row.action);
  }
});

// --- report -------------------------------------------------------------------------------------

if (failures.length > 0) {
  console.error(`✗ check-prepush-checklist.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-prepush-checklist.test: ${passed} checks passed.`);
