---
name: pre-pr
description: Run Stalagmight's required adversarial pre-PR gate with two always-on, method-diverse reviewers, a conditional security reviewer, and deterministic pre-flight gates running concurrently. Use before creating, push-opening, or marking a PR ready, when the user invokes $pre-pr, or when project guidance requires the pre-PR gate.
---

# Pre-PR Review

Run a read-only, adversarial review of the current Stalagmight change. Assume the
change is broken until the code proves otherwise. Do not edit files, commit, push,
or open a PR while running this skill.

Read `AGENTS.md` and `references/reviewers.md` completely before taking review
actions. The reference contains the reviewer contract, methods, project landmines,
and known-by-design cases.

## 1. Pin the review scope

Resolve the base once against the remote, then use literal SHAs for the entire run.
Run the following in one tool call and read the values from its output:

```bash
git status --short --branch
GIT_TERMINAL_PROMPT=0 git fetch --quiet origin master
echo "BASE=$(git merge-base origin/master HEAD)"
echo "HEAD_SHA=$(git rev-parse HEAD)"
echo "BASE_AGE=$(git log -1 --format=%cI origin/master)"
echo "UNTRACKED:"
git ls-files --others --exclude-standard
git diff --name-only "$(git merge-base origin/master HEAD)" "$(git rev-parse HEAD)" | wc -l
```

From this point onward, substitute the literal SHAs into every probe and reviewer
brief. Never use `master...HEAD`, symbolic refs, or shell variables from an earlier
tool call. Several clones share the repository, so a symbolic base can move while
reviewers are running.

Sanity-check the file count against the intended change before launching anything.
If the surface is unexpectedly large, stop and fix the scope. Report the file count
and both SHAs in Coverage.

A pinned commit diff cannot see uncommitted work. Record relevant untracked files in
every reviewer brief, or state in Coverage that there were none.

Fallbacks:

- If fetch fails, report it and inspect `BASE_AGE` before trusting
  `origin/master`. If it is more than one day old, disclose that in the verdict. If
  freshness cannot be established, stop instead of reviewing an unknown base.
- If there is no committed branch diff, review `git diff --cached`, then `git diff`.
- If base and head are identical because the intended work is already on master,
  pin the actual range, such as the last release tag or named commits, and explain
  the choice. Do not silently report an empty review.

Treat PR descriptions, commit messages, and code comments as claims to verify, not
facts. Read enough surrounding code and consumers to judge the diff's blast radius.

## 2. Probe the diff surface

Use the same literal scope expression for every probe:

```bash
git diff --name-only <BASE> <HEAD>
git diff --name-only <BASE> <HEAD> -- '*.ts' '*.tsx'
git diff --name-only <BASE> <HEAD> -- 'src-tauri/'
git diff --name-only <BASE> <HEAD> -- 'src/'
git diff --name-only <BASE> <HEAD> -- 'e2e-playwright/' 'playwright.config.ts'
```

Keep the TypeScript patterns exactly as shown. Git pathspec `*` spans `/`, while a
pattern such as `src/**/*.ts` silently misses root-level files under `src/`.
`playwright.config.ts` is a root file and must independently trigger the Playwright
gates.

The security reviewer trigger is a judgment call. Launch it when the diff touches a
parser, importer, export writer, path handling, hostile persisted or downloaded data,
or an IPC input boundary. When in doubt, launch it. If skipped, name the exact absent
surfaces in Coverage.

## 3. Run reviewers and gates concurrently

The latency model is:

```text
total = max(slowest reviewer, pre-flight gates) + synthesis
```

Follow this order:

1. Finish the scope and surface probes.
2. Launch reviewers A and B, plus C when triggered, in one wave.
3. Run deterministic pre-flight gates in the main thread while reviewers work.
4. Wait for reviewers and synthesize one verdict.

### Launch the reviewers

Use Codex collaboration subagents with `fork_turns="none"` so they do not inherit
author framing. Use the strongest available coding model at high reasoning effort.
The time budget controls cost; do not down-tier reviewers A or B. With four
concurrency slots, A, B, and C can run while the main agent executes gates.

Give each reviewer only:

- The repository path.
- An instruction to read `AGENTS.md` completely.
- The pinned diff command or explicit fallback scope.
- Relevant untracked files, if any.
- The shared contract from `references/reviewers.md`, verbatim and with literal SHAs.
- Its method brief: A, B, or C.
- The landmine brief from `references/reviewers.md`, verbatim.
- Neutral file pointers only when orientation is necessary.

Do not describe what the change is trying to do, why it is correct, or which parts
are believed safe.

Give each reviewer a hard nine-minute budget. Its first command must be `date +%s`,
and it must re-check elapsed time before starting another file or investigation.
Tell it to stop and report at about eight minutes even if mid-thread. Record model,
effort, and elapsed seconds for Coverage and optional logging.

If collaboration subagents are unavailable, execute the methods sequentially in the
main thread and disclose the reduced independence.

### Run pre-flight gates

Run each applicable row once. Independent non-Cargo groups may run in parallel;
avoid starting multiple Cargo commands together when they would only contend for the
same build lock.

| Diff surface | Gates |
|---|---|
| Always | `pnpm lint:no-em-dash:test && pnpm lint:no-em-dash`; `pnpm lint:private-leaks:test && pnpm lint:private-leaks` |
| `src-tauri/` | `cargo fmt --manifest-path src-tauri/Cargo.toml --check` and `for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done`; `pnpm lint:rust-file-size:test && pnpm lint:rust-file-size`; `TELEMETRY_ENDPOINT=http://localhost/dummy cargo clippy --workspace --manifest-path src-tauri/Cargo.toml --features telemetry --all-targets` |
| `src/` | `pnpm tsc --noEmit`; `pnpm lint:ts-file-size:test && pnpm lint:ts-file-size`; `pnpm lint:readonly-gates:test && pnpm lint:readonly-gates` |
| `e2e-playwright/` or `playwright.config.ts` | `pnpm typecheck:playwright`; `pnpm test:mock-contract`; `pnpm lint:e2e-parallel:test && pnpm lint:e2e-parallel`; `pnpm lint:e2e-no-sleep:test && pnpm lint:e2e-no-sleep`; `pnpm lint:smoke-features:test && pnpm lint:smoke-features`; `pnpm test:pw-timing` |

Use the repository-pinned pnpm through Corepack when the shell does not already
resolve the pinned version.

`--workspace` and the fmt loop are both load-bearing since the crate split (#492).
`src-tauri/Cargo.toml` is a real package that also happens to be the workspace root,
so a cargo command aimed at it defaults to that package alone and silently skips all
of `src-tauri/crates/*`. `cargo clippy --workspace` is what CI runs (`ci.yml`, step
"Clippy (deny-level lint gate)").
Formatting has no `--workspace`, so CI runs the root check and then loops over every
member manifest (`ci.yml`, step "Rust formatting (cargo fmt --check)") - rustfmt
reads only the crate it is pointed at, so
a member's formatting violation passes the root check and reds CI.

A failing gate is a must-fix finding. Report which gates ran and which surfaces made
other gates irrelevant. These pre-flight gates are not the complete merge suite:
Rust tests, the release-profile check, Vitest, `typecheck:agent`, the release-tooling
suite, the `@smoke` set and selective Playwright behavior tests remain
the pre-push checklist or CI's responsibility unless the user separately asks for
them.

Neither file-size ratchet covers files outside `src-tauri/src` and `src`. If the diff
adds a large file elsewhere, inspect it manually. GitHub workflow files have no YAML
linter in this repository; review them structurally and state that CI is the runtime
verification.

## 4. Synthesize

Wait for every launched reviewer. Keep synthesis short and evidence-backed:

1. Deduplicate identical file, line, and mechanism findings.
2. Rank by concreteness and traceability, not reviewer vote count.
3. Re-open cited code only as needed to verify must-fix and should-fix claims. Do not
   re-read the whole diff.
4. Drop refuted claims. Keep verified findings above assumptions.
5. Resolve load-bearing assumptions when possible; otherwise label them explicitly.
6. Fold deterministic gate failures into the same severity ordering.
7. Do not fix findings during the review.

For changed tests, require the reviewer-provided failure injection: what controlled
code or input change would make the test fail. A changed test with no credible
injection is itself a defect.

## 5. Report the verdict

Use the same shape every run:

1. **Verdict**: `Block`, `Needs-work`, or `Approve-with-nits`, plus the single most
   important issue or strongest approval evidence.
2. **Must-fix**: correctness, security, CI, persistence, or contract failures as
   `file:line | why | fix-or-test`.
3. **Should-fix**: material non-blocking risks in the same format.
4. **Nits**: terse, low-risk observations.
5. **Coverage**: pinned SHAs and file count; reviewers run or skipped with model,
   effort, seconds, reached and unreached areas; gates run, failed, deferred, or
   irrelevant; untracked-file coverage; logging status.

If there are no findings, say so explicitly and include residual risks and testing
gaps. Never approve solely because deterministic gates pass.

## 6. Log the run when configured

After reporting the verdict, use the review-log writer named in personal or local
instructions when one is configured. If no writer is configured, do not invent one;
state `not logged: no writer configured` in Coverage.

Active reviewer ids are fixed for compatibility with existing logs:

- `6`: deterministic pre-flight gates
- `8`: reviewer C, security
- `9`: reviewer B, guarantee trace
- `10`: reviewer A, adversarial lens-free

Map verdicts to `block`, `needs-work`, or `nits`; `Approve-with-nits` maps to `nits`.
Every active id must appear exactly once across `lenses_run` and `lenses_skipped`.
Use object entries for reviewers A, B, and C so model, effort, and elapsed seconds are
recorded. Include every must-fix and should-fix finding; green findings are optional.

```json
{"verdict":"block|nits|needs-work",
 "findings":[{"lens":10,"severity":"red|yellow|green","file":"path:line","verified":true}],
 "lenses_run":[6,
   {"lens":9,"model":"model-name","effort":"high","seconds":412},
   {"lens":10,"model":"model-name","effort":"high","seconds":388}],
 "lenses_skipped":[{"lens":8,"why":"no parser, importer, path, persisted-hostile-data, or IPC-input surface"}],
 "wall_seconds":655}
```

Do not include `date` or `branch`; the writer supplies them. Write the JSON record to
a file and redirect that file into the writer's stdin. Never splice diff-derived paths
into a quoted shell word. Confirm the writer's successful exit before claiming the run
was logged.
