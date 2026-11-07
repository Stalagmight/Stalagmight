---
description: Adversarial pre-PR gate - two-to-three time-budgeted reviewers run concurrently with the deterministic pre-flight, merged into one verdict; the run is logged when a writer is configured
---
Run the project's pre-PR review on the current change to Stalagmight (Tauri: Rust
backend + React/TS frontend, underwater cave-survey data). This is an ADVERSARIAL
review: assume the change is broken until you've shown otherwise. No rubber-stamping.

This command does NOT replace the bundled `/code-review` - it keeps a different name
on purpose so `/code-review ultra`, `--comment`, and `--fix` stay reachable.

⚠ **`/code-review` cannot be invoked from this command at all.** It ships compiled
into the CLI with `disable-model-invocation`, which blocks BOTH a subagent (`Skill`
tool → "cannot be used ... due to disable-model-invocation") **and** the main thread -
the restriction is on the model, not on subagents, and the `/config` toggle is locked
by the skill's own frontmatter. There is no setting to flip and no file to edit. If you
want that voice on a change, **the user must type `/code-review high` themselves**;
this command cannot summon it. Attempting it has twice burned a full reviewer's tokens
before failing.

## Scope

**Resolve the base ONCE, against the remote, and pin it to SHAs before anything else
runs.** Two failures live in this one line and both are silent:

- **A stale local ref.** `git diff master...HEAD` reads the *local* `master`, which in a
  clone that has not pulled can be a hundred commits behind. Seen live: a 130-file
  surface for a 14-file change, carrying the entire SpeleoDB integration, the
  column-width work and CI changes - none of it the author's. Every reviewer would have
  been briefed on other people's already-merged code and spent its budget there.
- **A base that moves mid-run.** Several clones share this repo, so a merge can land
  while the reviewers are reading. A symbolic base is not stable for the ~9 minutes they
  run, and a reviewer that re-resolves it mid-review aborts on a tree that moved.

⚠ **Run this as ONE call and read the SHAs out of its output.** Shell state does not
survive between tool calls, so `BASE=$(...)` set in one call is an empty string in the
next - and `git diff --name-only "" ""` yields an empty surface, which is precisely the
false-skip this file warns about below. **From here on, write the literal SHAs into
every probe and every brief.** There are no variables to reuse.

```bash
git fetch --quiet origin master   # writes FETCH_HEAD + remote-tracking refs, nothing else
echo "BASE=$(git merge-base origin/master HEAD)"
echo "HEAD_SHA=$(git rev-parse HEAD)"
echo "BASE_AGE=$(git log -1 --format=%cI origin/master)"
echo "UNTRACKED:"; git ls-files --others --exclude-standard
git diff --name-only "$(git merge-base origin/master HEAD)" "$(git rev-parse HEAD)" | wc -l
```

⚠ **Check that count against the change you just made, before launching anything.** If
the surface is much larger, the base is wrong - stop and fix it; do not review. Report
the count and both SHAs in the run's output, so a wrong base is visible afterwards
instead of being discovered in the findings.

⚠ **A commit-pinned diff cannot see uncommitted work, and reviewers are forbidden
`git status`** - so an unstaged edit or a forgotten `git add` is invisible to the whole
review unless you look. That is why `git ls-files --others --exclude-standard` is in the
block above: name any untracked file that belongs to the change in each brief, or say
in Coverage that there were none.

`merge-base` gives the two-dot form the same semantics as three-dot
`origin/master...HEAD`, while being immutable for the whole run.

Fallbacks, in order, and **never** silently to the local `master` ref:

- **`git fetch` fails** (offline, no remote, or it hangs on a credential prompt - use
  `GIT_TERMINAL_PROMPT=0` and a `timeout` if you suspect one): say so explicitly, then
  check `BASE_AGE` before trusting `origin/master`. **A never-pulled `origin/master` is
  exactly as stale as the local `master` this section exists to avoid** - the real axis
  is freshness, not remote-versus-local. If it is more than a day old, say so in the
  verdict; if you cannot establish the base's age at all, stop rather than review
  against an unknown base.
- **No PR branch at all**: fall back to staged, then unstaged.
- **The base and HEAD are the same commit** (reviewing work already on master, which is
  normal here since changes land directly): pin to the range you actually mean - the
  last release tag, or the specific commits - and say in Coverage which range you chose
  and why. Do not report "nothing to review".

Read enough surrounding code to judge correctness; keep findings scoped to the change +
its blast radius.

## Order of operations - this is the latency model

`total = max(slowest reviewer, pre-flight gates) + synthesis`, so nothing serializes
ahead of the reviewers:

1. Compute the **diff surface** (the five probes below). Seconds, and it decides
   whether reviewer C launches.
2. **Launch the reviewers FIRST** - in parallel, in a single message.
3. Run the **pre-flight gates yourself, in the main thread, while the reviewers work.**
   The reviewers are told not to run gates, and they do not need gate output to review.
4. **Synthesize**: fold gate results and reviewer findings into one verdict. Synthesis
   is pure serial tail - keep it short.

## Diff surface probes

⚠ **Substitute the literal SHAs Scope printed** - shown below as `<BASE>` and `<HEAD>` -
into every probe and every reviewer brief. `$BASE`/`$HEAD_SHA` are NOT available here:
shell state does not survive between tool calls, and an empty expansion makes
`git diff --name-only "" ""` return an empty surface. On the staged fallback substitute
`--staged`, on the unstaged fallback drop the range entirely. Get this wrong and every
probe returns empty on a real change, gated work is skipped, and the skip is logged with
a `why` that is false - in the single field the log exists to make trustworthy. An empty
surface list on a fallback path means **recompute with the right expression**, not *skip*.

⚠ **Never write `master...HEAD` here.** That reads the local ref, which is exactly the
stale-base failure Scope exists to prevent, and it re-resolves on every probe so two
probes can disagree if a merge lands between them.

```
git diff --name-only <BASE> <HEAD>                                  # the surface
git diff --name-only <BASE> <HEAD> -- '*.ts' '*.tsx'                # TS?
git diff --name-only <BASE> <HEAD> -- 'src-tauri/'                  # Rust crate?
git diff --name-only <BASE> <HEAD> -- 'src/'                        # frontend?
git diff --name-only <BASE> <HEAD> -- 'e2e-playwright/' 'playwright.config.ts'   # Playwright?
```

⚠ Match the TS patterns exactly. `'src/**/*.ts'` looks equivalent and is **not**: git
pathspecs are fnmatch without `:(glob)`, so `*` already spans `/` and the literal slash
before `*.ts` forces at least one directory level - it silently misses `src/App.tsx`,
`src/main.tsx`, and `src/globals.d.ts`. The bare `'*.ts' '*.tsx'` form also correctly
catches TS outside `src/`, such as `e2e-playwright/mocks/tauri-ipc.ts`.

⚠ `playwright.config.ts` is at the repo **root**, not under `e2e-playwright/`, yet
`tsconfig.playwright.json` includes it - so a config-only change is exactly the case
where `typecheck:playwright` matters and a naive `e2e-playwright/`-only probe reports
it irrelevant.

## The reviewers

Two reviewers always, a third conditionally. They are distinguished by **method**, not
by area or bug class - method diversity is what has produced disjoint find sets on this
repo; fixed area-lenses shared their blind spots.

| Reviewer | Launch when | Method |
|---|---|---|
| **A - adversarial, lens-free** | always | owns no area; follows the change wherever it leads |
| **B - guarantee trace** | always | infers the user-facing guarantee from the diff alone and traces it end-to-end |
| **C - security** | diff touches a parser, importer, export writer, path handling, or an IPC input boundary | hostile-input review of that boundary |

Run each on the strongest available model at effort `high` - the binding constraint is
time, and higher effort spends it in thinking. The **budget is the cost control; the
tier is not** - do not down-tier A or B. Record every reviewer you don't launch (C on
a non-matching diff) for the log's `lenses_skipped`.

⚠ C's trigger is the ONE judgment call in this file - every other gate is a pathspec.
When you skip C, the logged `why` must name the surfaces you judged absent for THIS
diff, not restate the rule; when in doubt, launch it.

**Brief reviewers blind to author intent - this is load-bearing.** Give each reviewer
ONLY the diff/scope above + the files to read. Do NOT tell them what the change "is
trying to do," why it's correct, or that any part is safe - author framing biases
reviewers into confirming the author's mental model, and is exactly how a P1 has
slipped through here (the orchestrator handed reviewers its own conclusion and they
confirmed it). The PR description and in-code "why" comments are **claims to verify,
not facts.** If a reviewer needs orientation, give neutral file pointers ("the
close-guard lives in `useBeforeUnload.ts`") - never a verdict ("the close-guard
protects this").

### Shared contract (give it to every reviewer verbatim)

⚠ **Substitute the real SHAs into the first line before sending it.** A reviewer given a
symbolic base re-resolves it mid-review, and in a shared clone the tree can move under
it - that has aborted reviewers outright, one of them after burning its whole budget.
Reviewers must also never run `git status`, `git fetch` or `git checkout`: they share
this working tree with you and with the gates you are running in parallel.

> **The change under review is exactly `git diff <BASE> <HEAD_SHA>`** - both are pinned
> SHAs, so use that expression verbatim and resolve nothing yourself. `master`,
> `origin/master` and `HEAD` are all wrong here: they move, and they are not what this
> review is scoped to. Read-only git only - no `status`, no `fetch`, no `checkout`.
> **You have 9 minutes, hard.** Make your FIRST tool call `date +%s` and re-check
> elapsed time before starting each new file or thread - you cannot otherwise observe
> the clock. Triage by risk, read the riskiest thing first, and stop
> and report at ~8 even if mid-thread. An honest partial review that names what it did
> not reach is worth more than a complete one that arrives late.
> Do not run build/test/lint gates - the main thread runs them; review the code.
> Return findings as `file:line · why · fix-or-test`. Separate what you VERIFIED
> (traced in the code) from what you ASSUMED; never assert a bug you didn't trace.
> **Apply that same burden of proof to SAFETY:** a "this is correct/safe because X"
> conclusion needs the line that proves X quoted. An unverified load-bearing
> assumption - a membership/enumeration/config/consumer you did not actually read -
> is itself a finding: report it as `ASSUMPTION (unverified): … · what to check`.
> Naming a risk and then dismissing it without tracing is NOT allowed.
> **Verify consumers, not just producers:** for any state/flag/field the change
> writes, grep its consumers and confirm at least one delivers the change's intended
> effect for the specific route/state in question - quote the consumer's gating line.
> **Tests:** for every new or changed test in the diff, state what injection would
> make it fail. Flag any you cannot answer for - a test that cannot fail is a defect.
> End with two mandatory sections: **What I did not reach** (files/questions skipped,
> and why) and **Test injections** (the per-test answers above).
> Highest-signal findings first.

### Reviewer A - adversarial, lens-free (no assigned area; that is the point)

Give it the diff, the shared contract, and the landmine brief, but **no lens**. Tell it
explicitly that it owns no area and must follow the change wherever it leads -
build/lint output, generated files, dependency and cost implications, docs and release
text that ship alongside the code, dead code, a formula duplicated in a second
language, a comment that asserts something the code does not do. Its recurring value is
the class of finding no fixed checklist has a column for: provenance, duplicated rules,
and prose that ships with the code. It is a judgment voice, not a checklist walk.

### Reviewer B - guarantee trace (end-to-end; cross-cutting)

- From the DIFF ALONE infer what user-facing guarantee the change must deliver
  (data-safety, close/unsaved guard, persistence, undo, UI↔backend parity), then
  trace each guarantee end-to-end: **trigger → state written → consumer that reads
  it → effect.** Do not assume any link in that chain; read each one.
- Confirm the SPECIFIC route/dialog/state this change touches is actually in the set
  the consumer checks - read the enumerated list/condition, never assume membership.
  (The canonical miss: a flag was set correctly but no consumer read it for that
  dialog state, so the guard never fired.) Quote the consumer's gating line.
- A broken or unverifiable guarantee chain is a high-signal finding even when every
  individual hunk is correct. If the change's own claims can't be traced to the code
  that delivers them, say so explicitly.

### Reviewer C - security (conditional)

Going open-source - assume hostile input at the boundary the diff touches: input
validation, path traversal, `unsafe` Rust, secret handling, no PII in telemetry,
resource exhaustion on attacker-controlled sizes/counts.

### Landmine brief (include verbatim in every reviewer's brief)

Project-specific knowledge no model derives from first principles. One check each -
whichever reviewer's thread touches the area owns it.

**State & async**
- Cached derived state (snap markers, ruler readout, warning identity, clipboard match)
  must be invalidated or re-derived on the mutation that stales it (reset, IPC reply,
  project edit, view switch).
- Rehydrate mutable objects from live data by identity key (shot_id/uuid), never an
  index captured before a delete/insert.
- Version-guard every async IPC reply that paints or mutates: capture identity at
  request time (`projectIdentityKey`, `src/hooks/useMapTools.ts`) and drop the reply if
  state moved. An in-flight request whose context was reset must not apply its response.
- Geometry-readiness is a **separate** guard: the async measured→adjusted swap does not
  bump `modified_at` (see the ruler-clearing effect in `src/components/Map/MapCanvas.tsx`).

**Geometry & multi-section (Rust)**
- `section_id` in every keyed read/write/delete - UUIDs collide across sections.
  `section_id`/`shot_id` are immutable FKs for merges/closures/comments; flag any path
  that regenerates or reassigns them.
- Range/loop ops: state inclusive vs exclusive boundaries explicitly; an off-by-one here
  silently displaces or deletes the wrong sections. Demand boundary unit tests.
- Metadata-only edits (comment/LRUD) must NOT trigger `reconcile_geometry()` - but they
  DO still `touch()` and push undo. The dirty signal is the `modified_at` timestamp;
  there is no `is_dirty` boolean.

**Persistence**
- Never persist transient in-memory values (freshly-minted UUIDs, `Config::default()`)
  before they're real - they orphan on next open.
- Saves are atomic (temp `.stalx.tmp` → rename); flag direct overwrites. Test read-only /
  legacy / missing-table load paths and `INSERT OR REPLACE` collisions.
- Any NEW on-disk artifact the change introduces (a log, a cache, a sibling file) is
  persistence surface regardless of language - atomic write, cleanup, and gitignore
  status all apply.
- Persistence references use `StationKey` / `AfterShot(uuid)` anchors, never positional
  refs - a positional ref goes stale on the first insert/delete.

**Guard parity**
- A conditionally-disabled UI action needs a **backend guard** that fails safe - never
  the disabled state alone. A nullable-field UI guard must match the backend gate it
  implies (`databaseOpen`, not `file_path != null`).
- Read-only gating is `appMode !== 'survey'` via `useEditingBlocked()` /
  `isEditingBlockedNow()`, never geometryMode alone (`pnpm lint:readonly-gates` blocks
  reintroduction).

**Tests & e2e**
- Adversarial case tested (boundary, duplicate-UUID, legacy load), not just the happy
  path; golden fixtures for parsers (`e2e/fixtures/*.dmp`).
- Flag NEW `waitForTimeout` uses - the #1 Playwright flake source (`docs/TESTING.md`);
  discouraged but NOT lint-enforced, so don't assume a guard exists.
- Did the change silently drop coverage or a fixture a test depends on?
- The e2e mock's `get_project_state` must return a fresh identity per call
  (`structuredClone`) or reference-keyed memos freeze - "panel empty, footer populated".

**Conventions**
- Telemetry is self-describing JSON+gzip (v3, #137); event names go out verbatim. A new
  command/tool/dialog/source must be added to `EVENT_NAMES`/`SOURCE_NAMES` in
  `src-tauri/src/telemetry/registry.rs` AND the lists in `src/utils/telemetry.ts`, or
  `registry_covers_all_*` fails. New commands use `command.<name>`. (`docs/telemetry.md`)
- IPC types are **generated**: never hand-edit `src/types/ipc/`; regen with
  `pnpm gen:ipc-types`. (`docs/ipc-typegen.md`)
- Heavy O(N) cross-checks must be `#[cfg(debug_assertions)]`; hot-path logs via
  `debugLog()`, not `console.log`. (`docs/debug-gates.md`, `docs/performance.md`)
- Cross-platform: macOS/Linux vs Windows tile-URL forms; Windows tests need
  `STALAGMIGHT_TEST_MANIFEST=1`. (`docs/architecture.md`, `CLAUDE.md`)
- A file already at/above 1500 prod LOC must be **split**, not rebaselined (#157).
- `PositiveNumberField.onCommit` fires on EVERY valid keystroke, not on blur/Enter -
  an expensive handler behind it needs its own debounce.
- All Plan-mode gas-config disk writes go through `src/utils/gasConfigDisk.ts` - a
  bespoke second writer merges a pre-write snapshot and silently reverts fields.

## Pre-flight gates (you, main thread, WHILE the reviewers work) = lens 6

Deterministic gates, run once. A gate that returns the same answer regardless of which
model runs it does not need a model - and it does not need to finish before the
reviewers start. Use the same diff expression as the probes. Run only the rows whose
surface is present:

| Surface in diff | Run |
|---|---|
| always | `pnpm lint:no-em-dash:test && pnpm lint:no-em-dash` · `pnpm lint:private-leaks:test && pnpm lint:private-leaks` |
| `src-tauri/` | `cargo fmt --manifest-path src-tauri/Cargo.toml --check` **and** `for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done` · `pnpm lint:rust-file-size:test && pnpm lint:rust-file-size` · `TELEMETRY_ENDPOINT=http://localhost/dummy cargo clippy --workspace --manifest-path src-tauri/Cargo.toml --features telemetry --all-targets` |
| `src/` | `pnpm tsc --noEmit` · `pnpm lint:ts-file-size:test && pnpm lint:ts-file-size` · `pnpm lint:readonly-gates:test && pnpm lint:readonly-gates` |
| `e2e-playwright/` **or** `playwright.config.ts` | `pnpm typecheck:playwright` · `pnpm test:mock-contract` · `pnpm lint:e2e-parallel:test && pnpm lint:e2e-parallel` · `pnpm lint:e2e-no-sleep:test && pnpm lint:e2e-no-sleep` · `pnpm lint:smoke-features:test && pnpm lint:smoke-features` · `pnpm test:pw-timing` |

Each ratchet is paired with its own `:test` because CI pairs them (`ci.yml`) - the
self-test pins the heuristic the gate depends on, and both are sub-second. **Run the
clippy line verbatim.** A bare `cargo clippy` is not the gate: without
`--features telemetry --all-targets` it misses what CI checks, and *with* the feature
but without `TELEMETRY_ENDPOINT` set, the telemetry crate's build script
(`src-tauri/crates/telemetry/build.rs`) fails - so
you report the gate red for a reason that has nothing to do with the diff.

⚠ **`--workspace` and the fmt LOOP are both load-bearing since the crate split
(#492), and omitting either reports green on code neither command read.**
`src-tauri/Cargo.toml` is a real package that also happens to be the workspace root,
so a cargo command aimed at it defaults to **that package alone** and silently skips
all of `src-tauri/crates/*`. `cargo clippy --workspace` (`ci.yml`, step "Clippy (deny-level lint gate)") is what CI
runs. Formatting has no `--workspace`, which is why CI runs the root check **and**
then loops over every member manifest (`ci.yml`, step "Rust formatting (cargo fmt --check)") - rustfmt reads only the
crate it is pointed at, so a member's formatting violation passes the root check and
reds CI. Both halves have been observed doing exactly that: a gate run without them
passed a diff whose only changes were in two member crates, one of which was
misformatted.

⚠ **This table is deliberately NOT the whole merge gate, and "gates green" must not be
reported as if it were.** The `check` job also runs the release-profile
`cargo check --release`, `cargo test --features telemetry` with the ts-rs bindings
drift check after it, the vendored `compass_data` tests, `./scripts/test-release.sh`,
`pnpm test` (vitest) and `pnpm typecheck:agent`, and `smoke` runs the `@smoke`
Playwright set; those take minutes and are the **Pre-Push Checklist's** job
(`CLAUDE.md`, which a leak-scan gate keeps complete). Say *which* gates you ran, and
that the test suites were deferred to CI.

⚠ A **failing gate is a 🔴 in its own right** - report it, and say which gates you ran
and which the diff made irrelevant. The Rust ratchet covers the crates' `src/`, the
build scripts, `src-tauri/examples`, `src-tauri/tests`, the agent OS helper and
`scripts/lib/slice-rs` (test files at their own, looser caps); the TS ratchet covers
`src/`, `e2e-playwright/` and `e2e/`. A large new file anywhere else is ungated; say
so rather than implying coverage.

⚠ **`.github/workflows/**` has no row above and no gate anywhere** - there is no YAML
linter in this repo, so a workflow edit can only be checked structurally (indentation
against a sibling step) until CI actually runs it. Say that plainly when the diff
touches one. Note also that `.claude/commands/*.md` matches `ci.yml`'s `paths-ignore`
(`**/*.md`), so a change to THIS file alone runs no CI at all.

## Synthesize (you, main thread - short; it is pure serial tail)

Wait for the reviewers, fold in the gate results, and merge. Do not re-read the diff.
- Dedup findings pointing at the same `file:line` + mechanism; keep the most concrete
  one. **Rank by concreteness and traceability - a quoted gating line, a reproducible
  failure - never by how many reviewers raised it.** The findings that mattered here
  have each been found by exactly one voice.
- Drop anything REFUTED on a second look; keep VERIFIED above ASSUMED.
- Don't re-flag known by-design: mid-section shot insert renumbering later stations
  (intentional - `insert_shot_after`/`insert_shot_before` in
  `src-tauri/src/models/project.rs`, and the `CreateShot` comment in
  `src-tauri/src/core/state/commands.rs`), `project.rs` formatting,
  `#[serde(default)]` config tolerance of unknown fields.

## Output (same shape every time, so reviews are comparable)
1. **Verdict** - Block / Approve-with-nits / Needs-work + the single most important issue.
   ⚠ When you log the run, these map to `block` / **`nits`** / `needs-work`. The middle
   one is not a case transform of its display label, and the logger rejects anything
   else - so "Approve-with-nits" written verbatim costs a wasted round.
2. **🔴 Must-fix** (correctness/security/CI/contract) - `file:line · why · fix`.
3. **🟡 Should-fix** - same format.
4. **🟢 Nits** - terse.
5. **Coverage** - which reviewers and gates ran, what each reviewed / skipped /
   couldn't verify (including each reviewer's "what I did not reach" section), and
   **why** anything did not run. Name skips explicitly; silent omission reads as
   coverage.

## Log the run (instrumentation - one line per review)

After printing the verdict, log the run with the review-log writer named in your
personal/local notes (it lives outside this repo, deliberately - the tree carries no
review-process tooling). **If no writer is configured in your context, skip this
section and say so in Coverage ("not logged: no writer configured")** - a silent skip
reads as a logged run; the review stands without it. The writer validates the record
and appends
exactly one JSON line to `<repo-root>/.claude/review-log.jsonl` (gitignored,
per-contributor).

What the log answers: which reviewers produce must-fixes, which run clean, and - via
`model`/`effort`/`seconds` - whether the review fits its wall-clock budget. Reviewer
ids are frozen for comparability with records already on disk: **6** = this pre-flight,
**8** = reviewer C, **9** = reviewer B, **10** = reviewer A. Ids 1-5 and 7 belong to
retired lenses: they remain valid in old records and are simply not logged anymore.
The active id set is coupled state with the out-of-repo writer (its ACTIVE set) and
its summarizer (its MANDATORY set): promoting C to always-on, or adding or retiring a
reviewer, means updating both alongside this file - otherwise the log will confidently
recommend deleting its own gates.

⚠ Only this definition logs at all, so the log is a 1-of-3 sample. The Codex
definition (`.agents/skills/pre-pr/SKILL.md`) has since moved onto this same A/B/C
reviewer shape, but `.opencode/command/pre-pr.md` still runs the old ten-lens one -
so that third path is not measuring the same thing (#358). Treat cross-path
comparisons with it accordingly.

Record shape:

```json
{"verdict":"block|nits|needs-work",
 "findings":[{"lens":10,"severity":"red|yellow|green","file":"<path:line>","verified":true}],
 "lenses_run":[6,
   {"lens":9,"model":"<model>","effort":"high","seconds":412},
   {"lens":10,"model":"<model>","effort":"high","seconds":388}],
 "lenses_skipped":[{"lens":8,"why":"no parser/importer/path/IPC-input surface in diff"}],
 "wall_seconds":655}
```

- `lenses_run` entries are a bare id or an object with per-reviewer `model`, `effort`,
  `seconds`; always use the object form for A/B/C so the budget stays measurable.
  `wall_seconds` is invocation-to-verdict.
- Ids 6, 8, 9, 10 must each appear exactly once across `lenses_run` +
  `lenses_skipped`; every `findings[].lens` must appear in `lenses_run`. The writer
  rejects a record that breaks the accounting.
- **Omit `date` and `branch`** - the writer fills them (UTC date; branch or short sha).
  Supplied values are recorded as given and not cross-checked.
- Include every 🔴/🟡 finding (🟢 optional); `verified:true` only for findings actually
  traced in code.
- **Write the record to a file and redirect it into the writer's stdin. Do not splice
  it into a quoted shell word:** a record embeds `file:line` values taken from the diff
  under review, and a path containing a single quote closes the quoting and turns the
  rest into shell syntax. On a hostile branch that is arbitrary command execution, and
  this command exists to review exactly such branches.
- Exit codes: `0` logged (not logged until you see it) · `1` record rejected, log
  untouched - fix the record · `2` environment problem (not in a repo, empty stdin,
  unwritable/symlinked log path) - fix the invocation, not the JSON.
- ⚠ The log path comes from `git rev-parse --show-toplevel`, so **cwd decides which
  checkout gets written** - a sibling clone or worktree keeps its own log, and the file
  is gitignored so a stray write is invisible to `git status`. When probing the writer
  by hand, set `STALAGMIGHT_REVIEW_LOG` to an absolute scratch path.
