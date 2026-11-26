# Pre-PR Reviewer Briefs

The single source for both `/pre-pr` (Claude Code) and `$pre-pr` (Codex). Read this file
completely before launching reviewers. Give the shared contract and landmine brief to every
reviewer verbatim. Give each reviewer exactly one method brief. Do not pass author intent or
conclusions.

## Shared contract

Substitute literal SHAs before sending this contract:

> **The change under review is exactly `git diff <BASE> <HEAD_SHA>`** - both are
> pinned SHAs, so use that expression verbatim and resolve nothing yourself.
> `master`, `origin/master`, and `HEAD` are wrong here because they move. Read-only
> git only: no `status`, `fetch`, or `checkout`.
>
> **You have nine minutes, hard.** Make your first tool call `date +%s`. Re-check
> elapsed time before starting each new file or investigation. Triage by risk, read
> the riskiest mechanism first, and stop and report at about eight minutes even if
> mid-thread. An honest partial review that names what it did not reach is better
> than a late review.
>
> Do not run build, test, lint, typecheck, or formatting gates. The main thread runs
> deterministic gates while you review the code.
>
> Return findings as `file:line | why | fix-or-test`. Separate VERIFIED facts from
> ASSUMED claims. Never assert a bug you did not trace. Apply the same burden of
> proof to safety: quote the line that proves a load-bearing safety claim. Report an
> unverified load-bearing membership, enumeration, configuration, or consumer as
> `ASSUMPTION (unverified): ... | what to check`. Naming a risk and dismissing it
> without tracing is not allowed.
>
> Verify consumers, not just producers. For every changed state, flag, or field,
> grep its consumers and quote the gating line that delivers the intended effect for
> the specific route or state.
>
> For every new or changed test in the diff that you inspect, state the controlled
> code or input change that would make it fail. Flag tests for which you cannot name
> a credible failure injection.
>
> Put the highest-signal findings first. End with two mandatory sections of at most
> about five lines each: **What I did not reach** and **Test injections**. Be terse:
> no restating the diff, no narrative.

## Reviewer A - adversarial, lens-free

Own no fixed area. Follow the change wherever its highest risk leads. Look for
correctness, behavioral regression, data loss, provenance errors, duplicated rules,
dead code, generated-file drift, dependency or cost implications, and comments or
documentation that assert behavior the code does not provide.

Treat PR text and in-code rationale as claims. This reviewer exists to find issues
that fixed checklists do not have a category for.

## Reviewer B - guarantee trace

Infer the user-facing guarantee from the diff alone. Trace each guarantee end to end:

```text
trigger -> state written -> consumer -> user-visible or persisted effect
```

Read every link. Confirm the exact route, dialog, state, enum membership, or gating
condition touched by the change is included in what the consumer checks. Quote the
consumer line. A broken or unverifiable chain is a finding even when individual
hunks look plausible.

## Reviewer C - security

Assume hostile input at every changed boundary because the repository is public.
Review validation, path traversal, unsafe Rust, secret handling, command injection,
telemetry PII, hostile SQLite or archive content, and resource exhaustion from
attacker-controlled sizes or counts.

## Landmine brief

### State and async

- Invalidate or re-derive cached state when a mutation, reset, reply, edit, or view
  switch makes it stale.
- Rehydrate mutable objects from live data by identity key such as shot id or UUID,
  never by an index captured before insertion or deletion.
- Version-guard async IPC replies that paint or mutate. Capture project identity at
  request time using the `projectIdentityKey` pattern in `src/hooks/useMapTools.ts`
  and drop replies after a switch or reset.
- Geometry readiness is separate from project identity. The measured-to-adjusted
  swap does not bump `modified_at`; inspect readiness-specific consumers in
  `src/components/Map/MapCanvas.tsx`.

### Geometry and multi-section Rust

- Key every UUID or positional read, write, and delete by `section_id` too. UUIDs
  can collide across sections.
- Treat section and shot ids as immutable foreign keys for merges, closures, and
  comments.
- State range boundaries as inclusive or exclusive and demand boundary tests.
- Metadata-only comment or LRUD edits must touch the project and push undo but must
  not call `reconcile_geometry()`. The dirty signal is the `modified_at` timestamp;
  there is no `is_dirty` boolean.

### Persistence

- Do not persist transient defaults or freshly minted in-memory values before they
  are real.
- Keep saves atomic through staged temporary files and rename. Check read-only,
  legacy, missing-table, and `INSERT OR REPLACE` collision paths.
- Treat every new log, cache, or sibling file as persistence surface: atomic write,
  cleanup, and gitignore status apply.
- Persist station references through `StationKey` and `AfterShot(uuid)`, not
  positional references.

### Guard parity

- Every conditionally disabled UI mutation needs a backend guard that fails safe.
- Nullable frontend guards must match the backend condition they imply, such as
  `databaseOpen` rather than a non-null `file_path`.
- Read-only gating is `appMode !== 'survey'` through `useEditingBlocked()` or
  `isEditingBlockedNow()`, never geometry mode alone.

### Tests and e2e

- Require adversarial boundary, duplicate-UUID, legacy-load, and hostile-input cases,
  not only happy paths.
- Parser tests should use golden fixtures under `e2e/fixtures` when appropriate.
- Flag new `waitForTimeout` use as a likely Playwright flake source. It is discouraged
  but not lint-enforced.
- Check for silently dropped coverage or fixtures.
- The e2e `get_project_state` mock must return a fresh identity per call, such as via
  `structuredClone`, or reference-keyed memos can freeze.

### Project conventions

- Telemetry is self-describing JSON plus gzip v3. New commands, tools, dialogs, and
  sources must be in both `EVENT_NAMES`/`SOURCE_NAMES`
  (`src-tauri/crates/telemetry/src/telemetry/registry.rs`) and the lists in
  `src/utils/telemetry.ts`, or `registry_covers_all_*` fails. New commands use
  `command.<name>`.
- IPC TypeScript types are generated. Never hand-edit `src/types/ipc/` or
  `src/types/ipc.generated.ts`.
- Put heavy O(N) checks behind `#[cfg(debug_assertions)]`. Use `debugLog()` rather
  than `console.log` on hot paths.
- Check cross-platform paths and tile URLs. Windows Rust tests require
  `STALAGMIGHT_TEST_MANIFEST=1`.
- Split a production file already at or above 1500 lines instead of rebaselining it.
- `PositiveNumberField.onCommit` fires on every valid keystroke, so expensive work
  behind it needs a separate debounce.
- Route all Plan-mode gas-config disk writes through `src/utils/gasConfigDisk.ts`.

## Known by design

Do not flag these without evidence that the change alters established behavior:

- Mid-section shot insertion intentionally renumbers later stations. See
  `insert_shot_after` and `insert_shot_before` in `src-tauri/crates/models/src/section.rs`
  and `src-tauri/crates/models/src/project/shots.rs`.
- Existing `src-tauri/crates/models/src/project.rs` formatting.
- `#[serde(default)]` config tolerance of unknown fields.
