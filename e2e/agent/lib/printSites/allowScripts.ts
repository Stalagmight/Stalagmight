/**
 * #576 W51: the print and write sites of the repo's Node tooling under scripts/, checked by
 * `printSites.test.ts` against `rules.ts`. Every entry says why its value may
 * reach its sink; see `rules.ts` for the classes and the reasons.
 *
 * Three kinds of tool live here:
 * - the repro report and the replay generator, which read a session's
 *   actions.jsonl back: report.md is pasted into public issues, so what it prints
 *   is checked against the form the harness writes (`id`, `num`, `word`, `code`,
 *   `argShape`), never trusted raw;
 * - the lint and release gates (check-*.mjs, gen-third-party-licenses), which
 *   print repo-relative paths, line numbers and text of the checkout, and the
 *   output of git, eslint, cargo and pnpm;
 * - the hand-run developer tools (heap-profile, slice-ts, pw-timing-*), whose
 *   output is what the developer asked them for.
 */
import { R, type SiteGroup } from './rules';

const REPORT = 'scripts/generate-repro-report.mjs';
const REPLAY = 'scripts/generate-replay.mjs';
const NO_SLEEP = 'scripts/check-e2e-no-sleep.mjs';
const PARALLEL = 'scripts/check-e2e-parallel-safe.mjs';
const ESLINT = 'scripts/check-eslint.mjs';
const I18N = 'scripts/check-i18n-literals.mjs';
const CYCLES = 'scripts/check-import-cycles.mjs';
const EM_DASH = 'scripts/check-no-em-dash.mjs';
const PREPUSH = 'scripts/check-prepush-checklist.mjs';
const LEAKS = 'scripts/check-private-leaks.mjs';
const READONLY = 'scripts/check-readonly-gates.mjs';
const RUST_SIZE = 'scripts/check-rust-file-size.mjs';
const SMOKE = 'scripts/check-smoke-features.mjs';
const TEST_IDS = 'scripts/check-test-ids.mjs';
const THIRD_PARTY_CHECK = 'scripts/check-third-party-licenses.mjs';
const TS_SIZE = 'scripts/check-ts-file-size.mjs';
const IPC_MOCK = 'scripts/check-vitest-ipc-mock.mjs';
const THIRD_PARTY_GEN = 'scripts/gen-third-party-licenses.mjs';
const UPDATER_PUBKEY = 'scripts/check-updater-pubkey.mjs';
const UPDATER_MANIFEST = 'scripts/gen-updater-manifest.mjs';
const HEAP = 'scripts/heap-profile.mjs';
const MERGE_COVERAGE = 'scripts/merge-coverage.mjs';
const PW_REPORTER = 'scripts/pw-timing-reporter.mjs';
const PW_SUMMARY = 'scripts/pw-timing-summary.mjs';
const SLICE_TS = 'scripts/lib/slice-ts.mjs';
const FOLD = 'scripts/lib/fold-local-paths.mjs';
const BRIDGE = 'scripts/lib/vite-agent-bridge.mjs';

/** A gate's own `log`/`err` parameters, defaulting to console.log/console.error. */
const CONSOLE_PARAMS =
  "the gate's `log`/`err` parameters default to console.log/console.error so its test can capture them; every call of them is a sink checked here (`wrappers`)";

// Reasons shared by several gates.
const REL = 'a repo-relative path of the checkout, from walkFiles / listSrcFiles / git ls-files (scripts/lib)';
const LINE = 'a line or column number in a file of the checkout, counted by the gate';
const SYMLINK =
  "symlinkProblem: the link's repo-relative path and its target as written when it stays in the checkout, else resolved and passed through foldLocalPaths by scripts/lib/src-files.mjs walkFiles";
const LINK_TARGET =
  'a symlink target from walkFiles: as written when it resolves inside the checkout, else resolved and passed through foldLocalPaths (scripts/lib/src-files.mjs)';
const RULE_WHY = "a rule's id or its why: fixed text of the gate's own rule table";
const SCANNED = 'the number of tracked text files the gate scanned';
const GIT_FIRST_LINE =
  "the first line of listTracked's error (execFileSyncFolded, shaped as execFileSync's): `Command failed: git ls-files -s -z` or `spawnSync git ENOENT`; the child's stderr after it is not printed again";
const SKIP_REASON =
  "a skip reason from scripts/lib/tracked-files.mjs toScanText (fixed words and a size in MB) or `unreadable (<fs error code>)`";
const BASELINE_JSON =
  "a tracked baseline JSON's parse error (its text quotes a span of that public file); an fs error prints its code instead (`e.code || e.message`)";
const FS_CODE = "Node's fs error code (EACCES, EISDIR, ENOENT, ...): a fixed set, never the message that names the absolute path";
const NO_CODE_FALLBACK =
  'reached only for an error without a code; readFileSync throws system errors, which all carry one, so this is never the fs message that names the absolute path';

export const GROUP: SiteGroup = {
  covers: ['scripts/'],
  programs: {
    'scripts/build-android.sh': { class: 'tooling', reason: 'builds, signs and serves an arm64 APK of the public tree for sideloading: build tool output' },
    'scripts/ci-cargo-test-unprivileged.sh': { class: 'tooling', reason: "CI wrapper that runs the cargo test step as an unprivileged user; prints the user name and setpriv/permission diagnostics" },
    'scripts/deploy-android.sh': { class: 'tooling', reason: 'builds the APK and deploys it to a tablet over network ADB: build and adb output' },
    'scripts/dev.ps1': { class: 'tooling', reason: 'Windows twin of dev.sh: stops old dev processes and starts `pnpm tauri dev`' },
    'scripts/dev.sh': { class: 'tooling', reason: 'restarts the Tauri dev server (kills old processes, loads .env, runs `pnpm tauri dev`)' },
    'scripts/lib/release-version.sh': { class: 'tooling', reason: 'sourced by release.sh / release-tag.sh: checks the version files of the public tree agree' },
    'scripts/lib/slice-rs/main.rs': { class: 'tooling', reason: 'the Rust half of scripts/slice.sh: prints the declarations or source lines of the .rs file the developer asks for' },
    'scripts/mac-build.sh': { class: 'tooling', reason: 'builds and tests the app on macOS: build tool output' },
    'scripts/mac-setup.sh': { class: 'tooling', reason: 'installs the macOS development tools: installer output' },
    'scripts/profile-build.sh': { class: 'tooling', reason: 'times cargo, tsc and vite builds of the public tree into a local build-profile-<stamp>.log' },
    'scripts/ralph/flowctl': { class: 'tooling', reason: 'bash wrapper that execs flowctl.py' },
    'scripts/ralph/flowctl.py': { class: 'tooling', reason: "CLI over the repo's .flow/ task JSON (epics, tasks, specs) for the Ralph agent loop" },
    'scripts/ralph/ralph.sh': { class: 'tooling', reason: 'the Ralph autonomous agent loop over .flow/ tasks; its run state goes to scripts/ralph/runs/ and *.log, both gitignored (scripts/ralph/.gitignore)' },
    'scripts/ralph/ralph_once.sh': { class: 'tooling', reason: 'runs one Ralph iteration (MAX_ITERATIONS=1) through ralph.sh' },
    'scripts/ralph/watch-filter.py': { class: 'tooling', reason: "formats the agent's stream-json tool calls for the local terminal while Ralph runs" },
    'scripts/release-notes.sh': { class: 'shareable', reason: 'prints the GitHub release body for a tag from CHANGELOG.md, a tracked file of the public tree (release.yml publishes it)' },
    'scripts/release-tag.sh': { class: 'tooling', reason: 'tags an already-bumped tree after the version-file checks' },
    'scripts/release.sh': { class: 'tooling', reason: 'bumps the version files, updates CHANGELOG.md, commits and tags' },
    'scripts/slice.sh': { class: 'tooling', reason: 'code slicer: prints the table of contents or a declaration of the source file the developer asks for (slice-ts / slice-rs)' },
    'scripts/smaps-breakdown.sh': { class: 'tooling', reason: "buckets a PID's /proc smaps mappings by purpose: sizes, counts and the kernel's anon labels" },
    'scripts/smaps-histogram.sh': { class: 'tooling', reason: "histogram of a PID's anonymous mapping sizes from /proc smaps: counts and sizes" },
    'scripts/test-release.sh': { class: 'tooling', reason: 'exercises release.sh against a throwaway clone' },
    'scripts/test.ps1': { class: 'tooling', reason: 'Windows cargo test runner (sets STALAGMIGHT_TEST_MANIFEST and the telemetry feature)' },
    'scripts/verify-macos-signature.sh': { class: 'tooling', reason: "checks the code signature of the .app inside a built DMG: codesign/spctl output" },
  },
  allow: {
    // report.md (#576 W49-NF1): every value it reads back from actions.jsonl or a
    // finding.json goes through a projection by construction.
    [REPORT]: {
      "describeAction: list.map((v) => argShape(v)).join(', ')": "store_action's positional arguments, each as its argShape",
      'getFlag: args[i + 1]': "the --title / --summary flag: the reporter's own words for the issue they are filing",
      '<module>: args[0]': 'the session dir the caller gave, folded by foldLocalPaths (`no actions.jsonl in ...`)',
      "<module>: path.join(sessionDir, 'report.md')": 'the report.md path in the session dir the caller gave, folded by foldLocalPaths (`wrote ...`)',
      '<module>: f.severity': "shareableFinding's label(): identifier characters only, any other replaced by ?, at most 120",
      '<module>: f.kind': "shareableFinding's label(): identifier characters only, any other replaced by ?, at most 120",
      '<module>: f.code': "shareableFinding's label(): identifier characters only, any other replaced by ?, at most 120",
      '<module> > steps.forEach: s':
        'a step line, at scrub(s) for each of `steps`: id(a.seq), code(a.tool), describeAction (a checked builder) and a fixed failed flag',
      '<module>: f.message':
        "shareableFinding's message, at scrub(f.message): the oracle's one-line wording, a crash marker name or fixed text (scripts/lib/shareable-finding.mjs)",
      "scrub: out.replace(CORPUS_COPY, '$1/<corpus file>')":
        'scrub: the same text with the session dir as <artifacts>, the home directory as $HOME and a corpus copy as <corpus file>',
      "<module>: scrub(JSON.stringify(f.detail, null, 2)).split('\\n').join('\\n ')":
        "shareableFinding's detail (counts, code locations, harness labels) as indented JSON in a ```json fence; JSON.stringify escapes every newline inside a value",
      '<module>: process.platform': "Node's platform word (linux, darwin, win32, ...): a fixed set",
      // What P(...) pushes into md, which report.md is written from: each P call is a checked stream site of its own.
      'P: s': R.FORWARD,
    },
    // The replay generator names its input and output by the caller's paths, folded (#576 W58).
    [REPLAY]: {
      '<module>: args[0]': 'the session dir or actions.jsonl the caller gave, folded by foldLocalPaths (`no actions.jsonl at ...`)',
      "<module>: path.join(input, 'actions.jsonl')": 'the actions.jsonl path in the session dir the caller gave, folded by foldLocalPaths',
      'getFlag: args[i + 1]': 'the --out path the caller gave, folded by foldLocalPaths (`wrote ...`)',
      "<module>: path.join(sessionDir, 'replay.spec.ts')": 'the default replay.spec.ts beside the actions.jsonl the caller gave, folded by foldLocalPaths (`wrote ...`)',
    },
    // The lint gates: repo-relative paths, line numbers, rule text, counts.
    [NO_SLEEP]: {
      'main: settles': R.COUNT,
      'main: v.rel': REL,
      'main: v.lineNo': LINE,
      'main: l.rel': REL,
      'main: l.target': LINK_TARGET,
    },
    [PARALLEL]: {
      'main: v.rel': "a repo-relative path of e2e-playwright/ from walkFiles, or CONFIG_FILE (a constant)",
      'main: v.lineNo': LINE,
      'main: v.ruleId': RULE_WHY,
      'main: v.why': RULE_WHY,
      'main: v.snippet':
        'the trimmed source line of e2e-playwright/ the rule matched (test code of the checkout), or `-> <symlink target>` from walkFiles (inside the checkout as written, else resolved and passed through foldLocalPaths)',
    },
    [ESLINT]: {
      'main: JSON.stringify(baselinePayload(warnings), null, 2)':
        "the baseline: the gate's fixed README, the total, and warning counts per ESLint rule and repo-relative file",
      'printErrors: e.file': REL,
      'printErrors: e.line': LINE,
      'printErrors: e.column': LINE,
      'printErrors: e.ruleId': 'an ESLint rule id from eslint.config.js, or `(no rule)` for a parse error',
      "printErrors: e.message.split('\\n')[0]": "the first line of ESLint's message about a source line of the checkout (it can quote an identifier of that code)",
      'loadBaseline: e.message': BASELINE_JSON,
      'loadBaseline: e.code': FS_CODE,
      'main: g.file': R.REPO,
      'main: g.rule': 'an ESLint rule id, measured or recorded in the tracked eslint-baseline.json',
      'main: g.have': R.COUNT,
      'main: g.allowed': 'a warning count recorded in the tracked eslint-baseline.json',
      'main: l.file': R.REPO,
      'main: l.rule': 'an ESLint rule id, measured or recorded in the tracked eslint-baseline.json',
      'main: l.have': R.COUNT,
      'main: l.allowed': 'a warning count recorded in the tracked eslint-baseline.json',
      "main: LINT_TARGETS.join(', ')": "the gate's own constant directory list (src, e2e-playwright, e2e)",
      'total: Object.values(counts).reduce((sum, perFile) => sum + Object.values(perFile).reduce((a, b) => a + b, 0), 0)':
        'total(): the sum of the warning counts',
    },
    [I18N]: {
      'main: symlinkProblem(link)': SYMLINK,
      'main: v.file': R.REPO,
      'main: v.line': LINE,
      'main: v.kind': "the kind of literal the analyzer found (JSX text, an attribute, a setter argument): the gate's own words",
      'main: JSON.stringify(v.text)': 'a hard-coded UI string of the dialog layer under src/ (source text of the checkout), quoted',
      'main: s.file': 'a repo-relative path from the tracked allowlist scripts/i18n-literals-allowlist.json',
      'main: JSON.stringify(s.text)': 'a literal from the tracked allowlist scripts/i18n-literals-allowlist.json, quoted',
      'main: m': "a PINNED_COMMON path: the gate's own constant list of repo-relative files",
    },
    [CYCLES]: {
      'main: files': R.COUNT,
      'main: p': SYMLINK,
      "main: comp.join(', ')": 'the repo-relative module paths of one import cycle under src/',
      'main: e.from': R.REPO,
      'main: e.line': LINE,
      'main: e.to': R.REPO,
    },
    [EM_DASH]: {
      'report: scanned': SCANNED,
      'report: safePath(v.rel)': 'a tracked path (git ls-files), JSON-quoted by safePath',
      'report: safePath(s.rel)': 'a tracked path (git ls-files), JSON-quoted by safePath',
      'report: v.lineNo': LINE,
      'report: v.col': LINE,
      'report: v.name': "the banned character's Unicode name from the gate's own table (EM DASH, EN DASH, ...)",
      'report: s.reason': SKIP_REASON,
      "main: e.message.split('\\n')[0]": GIT_FIRST_LINE,
    },
    [PREPUSH]: {
      'main: gates': R.COUNT,
      'main: workflowsRead': R.COUNT,
      'main: p':
        'a drift problem: workflow and step names, commands and checklist rows of tracked files (.github/workflows/, CLAUDE.md, package.json); an unreadable file by its fs error code',
    },
    [LEAKS]: {
      'report: scanned': SCANNED,
      'report: v.rel': 'a tracked path (git ls-files): the public tree',
      'report: v.lineNo': LINE,
      'report: v.ruleId': RULE_WHY,
      'report: v.why': RULE_WHY,
      'report: v.match':
        "the matched marker only (m[0] of one of the gate's own fixed patterns, a case variant of a word in this script's source), never the line around it",
      'report: s.rel': 'a tracked path (git ls-files): the public tree',
      'report: s.reason': SKIP_REASON,
      "main: e.message.split('\\n')[0]": GIT_FIRST_LINE,
    },
    [READONLY]: {
      'main: rel': REL,
      'main: e.code': FS_CODE,
      'main: e.message': NO_CODE_FALLBACK,
      'main: v': 'a violation: symlinkProblem, or a repo-relative path with a count and the gate\'s fixed advice',
    },
    [RUST_SIZE]: {
      'writeBaseline: JSON.stringify(payload, null, 2)': "the baseline: the gate's fixed README, its thresholds, and LOC counts per repo-relative .rs file",
      'loadBaseline: relPosix(BASELINE_PATH)': R.REPO,
      'main: relPosix(BASELINE_PATH)': R.REPO,
      'main: relPosix(primary)': R.REPO,
      'loadBaseline: e.message': BASELINE_JSON,
      'loadBaseline: e.code': FS_CODE,
      'main: w': "a strip-heuristic warning: the gate's fixed words with a line number and a context type",
      'main: key': "a threshold key from the gate's own list (warn_loc, fail_loc, ...)",
      'main: baseline[key]': 'a threshold value recorded in the tracked src-tauri/file-size-baseline.json',
      'main: want': "the gate's own threshold constant",
      'main: m': 'a classify() verdict: a repo-relative path, LOC counts and fixed words',
      'main: label': "`production` or `test`: the gate's own section names",
      'main: c.lowered': R.COUNT,
      'main: c.added': R.COUNT,
      'main: c.pruned': R.COUNT,
      "main: stale.slice(0, 5).join(', ')": 'repo-relative paths recorded in the tracked baseline',
      "main: unrecorded.slice(0, 5).join(', ')": 'repo-relative paths of measured .rs files',
      'main: e': 'a classify() verdict: a repo-relative path, LOC counts and fixed words',
    },
    [SMOKE]: {
      'main: p': 'a coverage problem: feature titles and spec paths of tracked files (the feature list, e2e-playwright specs); an unreadable list by its fs error code',
    },
    [TEST_IDS]: {
      '<module>: path.relative(REPO_ROOT, finding.fileName)': R.REPO,
      '<module>: finding.line': LINE,
      '<module>: finding.message': 'the gate\'s fixed words with a JSX tag name or a literal test ID from source under src/, or symlinkProblem',
    },
    [THIRD_PARTY_CHECK]: {
      'main: p': 'a bundle problem: THIRD-PARTY-LICENSES.md or a repo-relative lockfile path, a byte count and fixed words',
    },
    [TS_SIZE]: {
      'writeBaseline: JSON.stringify(payload, null, 2)': "the baseline: the gate's fixed README, its thresholds, and LOC counts per repo-relative TS file",
      'loadBaseline: relPosix(BASELINE_PATH)': R.REPO,
      'main: relPosix(BASELINE_PATH)': R.REPO,
      'main: relPosix(SRC_DIR)': R.REPO,
      'loadBaseline: e.message': BASELINE_JSON,
      'loadBaseline: e.code': FS_CODE,
      'main: w': "a strip-heuristic warning: the gate's fixed words with a context type",
      'main: key': "a threshold key from the gate's own list (warn_loc, fail_loc)",
      'main: baseline[key]': 'a threshold value recorded in the tracked ts-file-size-baseline.json',
      'main: want': "the gate's own threshold constant",
      "main: stale.slice(0, 5).join(', ')": 'repo-relative paths recorded in the tracked baseline',
      'main: e': 'a classify() verdict: a repo-relative path, LOC counts and fixed words',
    },
    [IPC_MOCK]: {
      'main: v.rel': REL,
      'main: v.lineNo': LINE,
      'main: rel': "a repo-relative path from the gate's own ALLOWLIST",
      'main: symlinkProblem(l)': SYMLINK,
    },
    // The hand-run developer tools: what the developer asked them for.
    [HEAP]: {
      'findTarget: res.status': "the inspector's HTTP status code, a number",
      'main > ws.addEventListener: msg.error.message': "the WebKit inspector's protocol error text for a Heap/Runtime command this tool sent",
      'main > ws.addEventListener: JSON.stringify(msg.error)': "the WebKit inspector's protocol error object for a command this tool sent",
      'reportSnapshot: agg.totalNodes.toLocaleString()': 'a node count, formatted',
      'reportSnapshot: dN.toLocaleString()': 'a node count difference, formatted',
      'reportSnapshot: r.count.toLocaleString()': 'a node count, formatted',
      'reportSnapshot: r.dCount': R.COUNT,
      'reportSnapshot: fmtBytes(agg.totalBytes)': 'a byte count formatted by fmtBytes (B/K/M/G)',
      'reportSnapshot: fmtBytes(dB)': 'a byte count formatted by fmtBytes (B/K/M/G)',
      'reportSnapshot: fmtBytes(r.bytes)': 'a byte count formatted by fmtBytes (B/K/M/G)',
      'reportSnapshot: fmtBytes(r.dBytes)': 'a byte count formatted by fmtBytes (B/K/M/G)',
      'reportSnapshot: fmtBytes(-r.dBytes)': 'a byte count formatted by fmtBytes (B/K/M/G)',
      'reportSnapshot: r.k.padEnd(38).slice(0, 38)':
        "a heap class key aggregate() builds: the node type and the constructor or closure name the engine records (class and function names of the code), never a string's value ('string' nodes share one key)",
      'main: p': 'a --file snapshot path the caller gave, folded by foldLocalPaths (`parsing ...`)',
      'reportSnapshot: label':
        "the snapshot label snapAndReport or the --file loop passes (see `snapAndReport: label`, and `main: p` for the --file path the loop passes through foldLocalPaths)",
      'snapAndReport: label':
        "the tool's own words (`initial`, `auto +Ns`, `manual`) or a label the developer typed at this tool's prompt to name their own checkpoint",
      'snapAndReport: new Date().toISOString().slice(11, 19)': 'the wall-clock time HH:MM:SS of a snapshot',
      'main: target.webSocketDebuggerUrl': "the inspector's ws://127.0.0.1:<port>/... endpoint for the page",
      'main: target.url': "the inspected page's URL (tauri://localhost or the Vite dev server); the app keeps no state in its URL (no router)",
      'errText: e.code': 'a system error code (ENOENT, ECONNREFUSED, ...): a fixed set',
      'errText: e.name': "an error's class name (SyntaxError)",
      'errText: e?.message': "the message of an error with no code that is not a SyntaxError: this tool's own (`/json returned 404`, `Heap.snapshot failed: ...`) or the inspector's protocol error",
      'main > setInterval > snapAndReport().catch: e':
        "a snapshot's error at errText(e): an fs or network error as its code, a SyntaxError as its name, else the profiler's own message (`Heap.snapshot failed: ...`) or a WebSocket error event's `[object Event]`",
      'main > rl.on > snapAndReport().catch: e':
        "a snapshot's error at errText(e): an fs or network error as its code, a SyntaxError as its name, else the profiler's own message (`Heap.snapshot failed: ...`) or a WebSocket error event's `[object Event]`",
      '<module> > main().catch: e':
        "the profiler's fatal error at errText(e): an fs or network error as its code, a SyntaxError as its name, else the profiler's own message (`/json returned N`, `no inspectable page`) or `[object Event]`",
      'takeSnapshot: e.message': "the Web Inspector protocol's error for Heap.snapshot (WebKit's text about the command, not the page's)",
      'takeSnapshot: e2.message': "the Web Inspector protocol's error for HeapProfiler.takeHeapSnapshot (WebKit's text about the command, not the page's)",
    },
    [SLICE_TS]: {
      '<module>: args[0]': 'the source file path the developer gave, folded by foldLocalPaths (`File not found: ...`)',
      '<module>: args[1]': 'the mode flag the developer typed, echoed by `Unknown mode:`',
      '<module>: args[2]': 'the declaration name or line range the developer asked this slicer for, echoed by `... not found:`',
      '<module>: path.basename(filePath)': 'the name of the source file the developer asked this slicer to print (scripts/slice.sh), in its --toc heading',
      '<module>: d.name': 'a declaration name of the source file the developer asked this slicer to print',
      '<module>: d.parent': 'the enclosing class or object name of a declaration in that file',
      '<module>: d.startLine': LINE,
      '<module>: d.endLine': LINE,
      "<module>: { 'interface': 'Interfaces', 'type': 'Types', 'enum': 'Enums', 'class': 'Classes', 'component': 'Components', 'function': 'Functions', 'method': 'Methods' }[type]":
        "a heading from the slicer's own fixed table",
      '<module>: extractWithLineNumbers(node.getFullStart(), node.getEnd())':
        'the source lines of the declaration the developer asked for: printing source text of the checkout is what this slicer is for (scripts/slice.sh)',
      "<module>: extracted.map((line, i) => { const lineNum = String(startLine + i).padStart(lineNumWidth); return `${lineNum}| ${line}`; }).join('\\n')":
        'the source lines of the range the developer asked for, numbered: printing source text of the checkout is what this slicer is for',
    },
    [PW_REPORTER]: {
      'onStepEnd: JSON.stringify(step.title)': "a top-level hook step title Playwright names (Before Hooks, After Hooks, Worker Cleanup, ...): the test runner's own words",
      'onEnd: JSON.stringify(out, null, 2)':
        'the timing rows: spec paths relative to the cwd (the repo root, where Playwright runs), test titles of the public e2e suite, durations, attempts, statuses',
      'onEnd: process.env.PW_TIMING_OUTPUT':
        'the timing.json path the caller set in $PW_TIMING_OUTPUT, folded by foldLocalPaths (unset, it is the repo-relative playwright-report/timing.json)',
    },
    // Regenerated tracked files: their whole content is the public tree's.
    [THIRD_PARTY_GEN]: {
      'collectRust: foldLocalPaths(String(e.stderr || e.message)).slice(0, 4000)':
        "cargo about's stderr (or spawn error) when it fails, folded by foldLocalPaths: a developer tool's text about Cargo.lock and the crates' license data, uncaught so Node prints it whole",
      'collectJs: parsed.error.message': "pnpm licenses' own error text about the lockfile's packages (a developer tool's output)",
      "collectJs: unvetted.map((u) => ` - ${u}`).join('\\n')":
        'the names and versions of npm packages that ship no license file (public registry data from the lockfile)',
      "main: HASHED_INPUTS.map((rel) => `${path.posix.basename(rel)}=${inputHash(rel)}`).join('\\n')":
        "the basenames of the generator's own HASHED_INPUTS (the lockfiles, the generator, about.toml), each with its sha256",
      "sha256: createHash('sha256').update(buf).digest('hex')": 'a sha256 hex digest',
      "main: renderSection( 'npm packages', 'The production JavaScript dependencies bundled into the frontend, with the license/notice files each package ships.', js, )":
        'the npm section: package names, versions and the license texts they ship, from pnpm licenses over the lockfile (third-party public data)',
      "main: renderSection( 'Rust crates', 'The dependency graph of the release binary (telemetry features included, dev-dependencies excluded), as harvested by cargo-about.', rust, )":
        'the Rust section: crate names, versions and license texts, from cargo about over Cargo.lock (third-party public data)',
    },
    // #579 updater gates (master). The pubkey gate reads the tracked tauri.conf.json.
    [UPDATER_PUBKEY]: {
      'main: p':
        "a problem checkUpdaterConfig found in the tracked src-tauri/tauri.conf.json: the file's repo-relative name, the offending pubkey or endpoint text of that public file, and the gate's fixed wording",
    },
    // The manifest generator runs in the release job over the release's own artifacts; every value
    // is a release fact (version, tag, artifact and signature names) or the job's own paths.
    [UPDATER_MANIFEST]: {
      'getFlag: argv[i + 1]':
        'a flag value the caller gave: in CI the release version and tag, the repo-relative artifact dir and the runner-temp notes and output paths release.yml passes; a bad --include-darwin value is quoted back',
      'findOne: label': "a platform label from the generator's own PLATFORMS table (Linux, Windows, macOS)",
      'findOne: dir': "the --dir the caller gave: release.yml's repo-relative release-artifacts directory",
      "findOne: entries.join(', ')": 'the file names of release artifacts the bundler built from the public tree, as uploaded to the GitHub release',
      'readSig: label': "a platform label from the generator's own PLATFORMS table",
      'readSig: artifactFile': 'a release artifact file name the bundler built, as uploaded to the GitHub release',
      'readSig: dir': "the --dir the caller gave: release.yml's repo-relative release-artifacts directory",
      'readSig: path.join(dir, sigFile)':
        'the repo-relative path of a release artifact signature: the --dir the caller gave and the artifact file name the bundler built',
      'notMinisign: label': "a platform label from the generator's own PLATFORMS table",
      'notMinisign: why':
        "the generator's own fixed wording for which minisign check the release signature failed (a missing header, a bad signature line, ...)",
      'buildManifest: version': 'the --version the caller gave: the release version from the tag release.yml builds',
      'buildManifest: tag': 'the --tag the caller gave: the git tag release.yml builds',
      'buildManifest: plat.label': "a platform label from the generator's own PLATFORMS table",
      'buildManifest: dir': "the --dir the caller gave: release.yml's repo-relative release-artifacts directory",
      'buildManifest: plat.artifactRe': "an artifact name pattern from the generator's own PLATFORMS table",
      'resolveNotes: opts.notesFile':
        "the --notes-file the caller gave: in CI the release body release.yml wrote under the GitHub runner's RUNNER_TEMP, not a user's machine",
      'resolveNotes: e.message':
        "Node's fs error reading that --notes-file: its code and the runner-temp path the caller gave (a CI-only release step; no user data)",
      'main: e.message':
        "an Error this generator threw from parseArgs, resolveNotes or buildManifest, each built from the values above, or Node's fs error on --dir naming the release-artifacts directory",
      'main: opts.out': "the --out the caller gave: in CI a manifest path under the GitHub runner's RUNNER_TEMP",
      'main: manifest.version': 'the --version the caller gave, checked against VERSION_RE: the release version',
      'main: JSON.stringify(manifest, null, 2)':
        'the manifest buildManifest made: the release version, the release notes (the GitHub release body, from CHANGELOG.md), a timestamp, and per platform the public download URL and minisign signature of a release artifact',
    },
    [PW_SUMMARY]: {
      "<module>: render(JSON.parse(readFileSync(file, 'utf8')), { top })":
        'the markdown timing table render() builds from a timing.json pw-timing-reporter wrote: spec paths and test titles of the public e2e suite, durations, attempts and statuses',
    },
    // execFileSyncFolded: a developer tool's stderr, captured and folded (#576 W58).
    [FOLD]: {
      'execFileSyncFolded: ret.stderr':
        "the stderr of git ls-files (listTracked) or cargo about / pnpm licenses (gen-third-party-licenses run()) over the public tree, folded by foldLocalPaths: written to our stderr, and after `Command failed:` in the error's message",
      'execFileSyncFolded: file': 'the program a caller runs (git, cargo, pnpm or pnpm.cmd), folded by foldLocalPaths in `Command failed: <file> <args>`',
      'execFileSyncFolded: ...args':
        "the caller's arguments (static flags, and gen-third-party's --manifest-path under the checkout), folded by foldLocalPaths in `Command failed: <file> <args>`",
    },
  },
  fileSinks: {
    [REPORT]: {
      '<module> -> outPath': {
        class: 'shareable',
        what: 'report.md',
        reason: 'pasted into public issues: md.join of the lines pushed into md, each push a value of this write (P(...) checked at its calls)',
      },
    },
    [REPLAY]: {
      '<module> -> outPath': {
        class: 'private',
        what: 'replay.spec.ts',
        reason: "the replay spec records each step's exact params (typed text, scripts, IPC arguments, the seeded fixture), so it reproduces the session",
      },
    },
    // Tracked files of the public tree the gates regenerate (`--update`).
    [ESLINT]: {
      'main -> baselinePath': {
        class: 'shareable',
        what: 'eslint-baseline.json, a tracked file of the public tree',
        reason: 'ESLint warning counts per rule and repo-relative file, and the gate\'s fixed README text',
      },
    },
    [RUST_SIZE]: {
      'writeBaseline -> BASELINE_PATH': {
        class: 'shareable',
        what: 'src-tauri/file-size-baseline.json, a tracked file of the public tree',
        reason: "LOC counts per repo-relative .rs file, the gate's thresholds and its fixed README text",
      },
    },
    [TS_SIZE]: {
      'writeBaseline -> BASELINE_PATH': {
        class: 'shareable',
        what: 'ts-file-size-baseline.json, a tracked file of the public tree',
        reason: "LOC counts per repo-relative TS file, the gate's thresholds and its fixed README text",
      },
    },
    [UPDATER_MANIFEST]: {
      'main -> opts.out': {
        class: 'shareable',
        what: 'latest-<track>.json, the updater manifest the release job uploads to the public `updater` release',
        reason:
          'the release version, the release notes (the GitHub release body, from CHANGELOG.md), a timestamp, and per platform the public download URL and the minisign signature of a release artifact',
      },
    },
    [THIRD_PARTY_GEN]: {
      'main -> OUT_FILE': {
        class: 'shareable',
        what: 'THIRD-PARTY-LICENSES.md, a tracked file shipped with the app',
        reason: 'license texts and the names and versions of the third-party crates and npm packages, from cargo about and pnpm licenses over the lockfiles, with lockfile hashes',
      },
    },
    [PW_REPORTER]: {
      'onEnd -> path': {
        class: 'shareable',
        what: 'playwright-report/timing.json, the Playwright timing CI artifact',
        reason: 'per test of the public e2e suite: its spec path relative to the repo root, line, title, durations, attempts and status',
      },
    },
    [PW_SUMMARY]: {
      '<module> -> process.env.GITHUB_STEP_SUMMARY': {
        class: 'shareable',
        what: 'the GitHub Actions job summary',
        reason: 'the timing table render() builds (see the allow entry): spec paths, test titles, durations',
      },
    },
    // The dev server's agent bridge: the app's console and eval results.
    [BRIDGE]: {
      'configureServer -> TOKEN_PATH': {
        class: 'private',
        what: '.agent-bridge/',
        reason: "the bridge's per-start access token (mode 0600) under the gitignored .agent-bridge/",
      },
      'appendLine -> file': {
        class: 'private',
        what: '.agent-bridge/',
        reason: "console.jsonl / results.jsonl under .agent-bridge/: the dev webview's mirrored console and eval results, the app's own text",
      },
    },
    // Local coverage merge: istanbul JSON, keyed by absolute source paths.
    [MERGE_COVERAGE]: {
      "writeReport -> path.join(tmpDir, 'out.json')": {
        class: 'private',
        what: 'docs/coverage/',
        reason: 'a copy of a merged coverage JSON into the gitignored docs/coverage/.nyc_output/ for nyc report; istanbul keys it by absolute source paths',
      },
      '<module> -> e2eOut': {
        class: 'private',
        what: 'docs/coverage/',
        reason: 'the merged E2E coverage JSON under the gitignored docs/coverage/; istanbul keys it by absolute source paths',
      },
      "<module> -> path.join(mergeDir, 'vitest.json')": {
        class: 'private',
        what: 'docs/coverage/',
        reason: 'a copy of the Vitest coverage JSON into the gitignored docs/coverage/.merge/; istanbul keys it by absolute source paths',
      },
      "<module> -> path.join(mergeDir, 'e2e.json')": {
        class: 'private',
        what: 'docs/coverage/',
        reason: 'a copy of the E2E coverage JSON into the gitignored docs/coverage/.merge/; istanbul keys it by absolute source paths',
      },
    },
  },
  passthrough: {
    [MERGE_COVERAGE]: {
      "runNyc -> 'inherit'":
        "pnpm exec nyc merge / report: the coverage tool's own output, a table of the public source's file names and percentages",
    },
    [NO_SLEEP]: { 'main -> console.log': CONSOLE_PARAMS, 'main -> console.error': CONSOLE_PARAMS },
    [PARALLEL]: { 'main -> console.log': CONSOLE_PARAMS, 'main -> console.error': CONSOLE_PARAMS },
    [EM_DASH]: { 'report -> console.log': CONSOLE_PARAMS, 'report -> console.error': CONSOLE_PARAMS },
    [LEAKS]: { 'report -> console.log': CONSOLE_PARAMS, 'report -> console.error': CONSOLE_PARAMS },
    [SMOKE]: { 'main -> console.log': CONSOLE_PARAMS, 'main -> console.error': CONSOLE_PARAMS },
    [IPC_MOCK]: { 'main -> console.log': CONSOLE_PARAMS, 'main -> console.error': CONSOLE_PARAMS },
  },
  wrappers: {
    [REPORT]: { P: 'stream' },
    [NO_SLEEP]: { log: 'stream', err: 'stream' },
    [PARALLEL]: { log: 'stream', err: 'stream' },
    [EM_DASH]: { log: 'stream', err: 'stream' },
    [LEAKS]: { log: 'stream', err: 'stream' },
    [SMOKE]: { log: 'stream', err: 'stream' },
    [IPC_MOCK]: { log: 'stream', err: 'stream' },
  },
  builders: {
    [REPORT]: ['describeAction'],
  },
  fileHelpers: {
    [REPORT]: {
      id: 'an integer as itself (Number.isInteger), any other value as its argShape',
      num: 'a finite number as itself, any other value as its argShape',
      word: "a member of the step's fixed set (BUTTONS, DIALOG_KINDS) as itself, any other value as its argShape",
      code: 'a string matching its shape (NAME /^[\\w.:-]{1,80}$/, GIT_HEAD, DISPLAY, VERSION) as itself, any other value as its argShape',
      ipcCommand:
        'an ipc command: `plugin:<[\\w-]{1,80}>|<NAME>` with the `|` escaped (PLUGIN_COMMAND, two shape-checked captures), else code()',
      argShape: "a value's shape, never its value: a string as its length, a number as its type, an object's field-name keys",
      redactPath: 'a chosen file as <file.ext>: the extension only, whitelisted to /^\\.[A-Za-z0-9]{1,12}$/',
      describeKeys: 'press_keys keys: WebDriver key names from a fixed set, U+ code points, other keys counted in characters',
      describeChord: 'an os_keys chord: modifiers and named keys from a fixed set, any other key counted in characters',
    },
  },
};
