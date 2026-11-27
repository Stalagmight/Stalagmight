---
description: Adversarial pre-PR gate - two-to-three time-budgeted reviewers run concurrently with the deterministic pre-flight, merged into one verdict; the run is logged when a writer is configured
---
Run the pre-PR review on the current change to Stalagmight (Tauri: Rust backend + React/TS
frontend, underwater cave-survey data). It is ADVERSARIAL: assume the change is broken until shown
otherwise.

- This is not the bundled `/code-review`, which a model cannot invoke (it sets
  `disable-model-invocation`). If that review is wanted, the user types `/code-review high`.
- The reviewer briefs and the landmine brief live in
  `.agents/skills/pre-pr/references/reviewers.md`, shared with Codex's `$pre-pr`. Read it
  completely before launching reviewers.

## 1. Pin the scope (one call)

Shell state does not survive between tool calls, so run this as ONE call, read the SHAs from its
output, and write the literal SHAs into every later command and brief.

```bash
git fetch --quiet origin master
echo "BASE=$(git merge-base origin/master HEAD)"
echo "HEAD_SHA=$(git rev-parse HEAD)"
echo "BASE_AGE=$(git log -1 --format=%cI origin/master)"
echo "UNTRACKED:"; git ls-files --others --exclude-standard
git diff --name-only "$(git merge-base origin/master HEAD)" "$(git rev-parse HEAD)" | wc -l
```

- Check the file count against the change. A much larger surface means a wrong base: stop and
  fix it. Report the count and both SHAs in the output.
- Never diff against the local `master`, and never use `master...HEAD`: it can be far behind,
  and a symbolic base can move mid-run because several clones share this repo.
- Reviewers can't see uncommitted work and are forbidden `git status`. Name any untracked file
  that belongs to the change in each brief, or say in Coverage there were none.
- If `git fetch` fails (use `GIT_TERMINAL_PROMPT=0` and a `timeout` if it may prompt), say so and
  check `BASE_AGE`. If it is over a day old, say so in the verdict; if the age is unknown, stop.
- No PR branch: fall back to staged, then unstaged changes.
- Base == HEAD (work already on master): pin the range you mean (last tag, or specific commits)
  and name it in Coverage. Never report "nothing to review".
- Keep findings to the change and its blast radius.

## 2. Order of operations

`total = max(slowest reviewer, gates) + synthesis`, so nothing runs ahead of the reviewers:

1. Probe the diff surface (below).
2. Launch the reviewers in parallel, in one message.
3. Run the pre-flight gates in the main thread while they work.
4. Synthesize. Keep it short; it is pure serial tail.

## 3. Diff surface probes

Substitute the literal SHAs as `<BASE>` and `<HEAD>`. On the staged fallback use `--staged`; on
the unstaged fallback drop the range. An empty list on a fallback means the expression is wrong:
recompute it, don't skip.

```
git diff --name-only <BASE> <HEAD>                                  # the surface
git diff --name-only <BASE> <HEAD> -- '*.ts' '*.tsx'                # TS anywhere
git diff --name-only <BASE> <HEAD> -- 'src-tauri/'                  # Rust
git diff --name-only <BASE> <HEAD> -- 'src/'                        # frontend
git diff --name-only <BASE> <HEAD> -- 'e2e-playwright/' 'playwright.config.ts'   # Playwright
```

- Use the TS patterns exactly: `'src/**/*.ts'` misses `src/App.tsx` and other top-level files,
  because git pathspecs already let `*` span `/`.
- `playwright.config.ts` sits at the repo root but is covered by `tsconfig.playwright.json`.

## 4. Reviewers

| Reviewer | Launch when | Method |
|---|---|---|
| **A - adversarial, lens-free** | always | owns no area; follows the change wherever it leads |
| **B - guarantee trace** | always | infers the user-facing guarantee from the diff and traces it end to end |
| **C - security** | diff touches a parser, importer, export writer, path handling, or an IPC input boundary | hostile-input review of that boundary |

- Run each on the strongest model at effort `high`. The 9-minute budget in the shared contract is
  the cost control; never down-tier A or B.
- C's trigger is the one judgment call. When you skip C, the logged `why` names the surfaces you
  judged absent in THIS diff. When in doubt, launch it.
- Each brief is: the shared contract with the literal SHAs substituted, plus the landmine brief
  verbatim, plus exactly one method brief, plus neutral file pointers.
- **Brief blind to author intent.** Never say what the change is trying to do or that any part is
  safe. PR text and in-code rationale are claims to verify. A P1 slipped through here once when
  reviewers were handed the orchestrator's conclusion.
- Reviewers never run `git status`, `fetch` or `checkout`; they share this working tree with you.

## 5. Pre-flight gates (main thread, while the reviewers work)

Run only the rows whose surface is present, with the same diff expression as the probes.

| Surface in diff | Run |
|---|---|
| always | `pnpm lint:no-em-dash:test && pnpm lint:no-em-dash` · `pnpm lint:private-leaks:test && pnpm lint:private-leaks` |
| `src-tauri/` | `cargo fmt --manifest-path src-tauri/Cargo.toml --check` **and** `for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done` · `pnpm lint:rust-file-size:test && pnpm lint:rust-file-size` · `TELEMETRY_ENDPOINT=http://localhost/dummy cargo clippy --workspace --manifest-path src-tauri/Cargo.toml --features telemetry --all-targets` |
| `src/` | `pnpm tsc --noEmit` · `pnpm lint:ts-file-size:test && pnpm lint:ts-file-size` · `pnpm lint:readonly-gates:test && pnpm lint:readonly-gates` |
| `e2e-playwright/` **or** `playwright.config.ts` | `pnpm typecheck:playwright` · `pnpm test:mock-contract` · `pnpm lint:e2e-parallel` · `pnpm lint:pw-timing:test` |

- Run the clippy line verbatim. Without `--workspace` it checks only the root package; without
  `TELEMETRY_ENDPOINT` the build script fails for reasons unrelated to the diff.
- Run the fmt loop too: rustfmt reads only the crate it is pointed at, and CI loops over every
  member.
- A failing gate is a 🔴 in its own right.
- This table is not the whole merge gate. `cargo test`, vitest and `typecheck:agent` are the
  pre-push checklist's job; say they were deferred to CI.
- The Rust ratchet covers `src-tauri/src` and `src-tauri/crates/*/src`, the TS ratchet only
  `src/`, and nothing gates `.github/workflows/**`. Say so when the diff has a large file
  elsewhere or a workflow edit.
- An `.md`-only change skips `check` and `smoke`; `leak-scan` (private-leaks, em-dash,
  third-party) still runs.

## 6. Synthesize

Wait for the reviewers, fold in the gate results, and merge. Don't re-read the diff.

- Dedup findings on the same `file:line` and mechanism; keep the most concrete.
- Rank by concreteness and traceability (a quoted gating line, a reproducible failure), never by
  how many reviewers raised it.
- Drop anything refuted on a second look; keep VERIFIED above ASSUMED.
- Skip the known-by-design items listed in the reviewer briefs.

## 7. Output (same shape every time)

1. **Verdict:** Block / Approve-with-nits / Needs-work, plus the single most important issue.
2. **🔴 Must-fix** (correctness, security, CI, contract): `file:line · why · fix`.
3. **🟡 Should-fix**: same format.
4. **🟢 Nits**: terse.
5. **Coverage**, at most about 5 lines: the SHAs and file count, which reviewers and gates ran,
   what each skipped and why. Name every skip; silent omission reads as coverage.

The PR body later carries only one line from this: the verdict and whether the must-fixes are
fixed. Don't paste Coverage or reviewer output into it.

## 8. Log the run

Log with the review-log writer named in your personal or local notes; the tree deliberately
carries no review-process tooling. If no writer is configured, skip this and say "not logged: no
writer configured" in Coverage.

```json
{"verdict":"block|nits|needs-work",
 "findings":[{"lens":10,"severity":"red|yellow|green","file":"<path:line>","verified":true}],
 "lenses_run":[6,
   {"lens":9,"model":"<model>","effort":"high","seconds":412},
   {"lens":10,"model":"<model>","effort":"high","seconds":388}],
 "lenses_skipped":[{"lens":8,"why":"no parser/importer/path/IPC-input surface in diff"}],
 "wall_seconds":655}
```

- Ids are frozen: **6** pre-flight, **8** C, **9** B, **10** A. Each appears exactly once across
  `lenses_run` and `lenses_skipped`, and every `findings[].lens` is in `lenses_run`. Ids 1-5 and 7
  are retired. Changing the active set means updating the writer and its summarizer too.
- Use the object form, with `model`, `effort` and `seconds`, for A, B and C. `wall_seconds` is
  invocation to verdict. Omit `date` and `branch`; the writer fills them.
- "Approve-with-nits" logs as `nits`; the writer rejects anything else.
- Include every 🔴 and 🟡 finding. Set `verified:true` only for findings traced in code.
- Write the record to a file and redirect it into the writer's stdin. Never splice it into a
  quoted shell word: `file:line` values come from the diff, and a quote in a path is command
  injection.
- Exit codes: `0` logged · `1` record rejected (fix the record) · `2` environment problem (fix
  the invocation).
- The log path follows `git rev-parse --show-toplevel`, so cwd picks the checkout. For a branch in
  another clone, or when probing by hand, set `STALAGMIGHT_REVIEW_LOG` to an absolute path.
- The Codex skill (and OpenCode through it) logs to the same writer when one is configured.
