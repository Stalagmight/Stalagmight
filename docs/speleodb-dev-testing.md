# SpeleoDB testing

Stalagmight uses the published `speleodb-api` dependency declared in
`src-tauri/Cargo.toml`. Its own Cargo workspace and lockfile remain independent.
The API adapter and its tests are in
`src-tauri/crates/speleodb/src/speleodb/client/` and `client.rs`.

## Configuration

Copy `.env.example` to `.env` at the Stalagmight repository root. The live Rust
suite loads this file automatically with dotenvy. An explicitly exported
variable takes precedence. These variables configure tests only; the desktop
application continues to use its saved endpoint and credential storage.

| Variable | Purpose |
| --- | --- |
| `SPELEODB_INSTANCE_URL` | Test server base URL |
| `SPELEODB_OAUTH_TOKEN` | Existing bearer token, validated independently |
| `SPELEODB_EMAIL`, `SPELEODB_PASSWORD` | Email login, followed by profile validation |
| `SPELEODB_API_TIMEOUT_MS` | Timeout in milliseconds, including file transfers; defaults to `120000` (120 seconds) |
| `SPELEODB_API_RETRY_COUNT` | Additional attempts for safe reads and token exchange; defaults to `3` |
| `SPELEODB_API_TEST_ENABLED` | Exactly `true` enables live requests; defaults to `false` |

Defaults apply when neither the exported variable nor `.env` supplies a value.
Set `SPELEODB_API_TEST_ENABLED=false` to disable live requests explicitly.
Both authentication paths are required when live testing is enabled. Missing,
blank or placeholder credentials skip before any network operation, naming only the
missing variable names. Diagnostics distinguish disabled, incomplete configuration,
and execution; authentication/API/assertion/cleanup failures after validation fail
the test. Credentials are never printed.
The live suite uses a 120-second timeout for API requests and uploads, and a
120-second read-idle timeout for downloads (no total download deadline).
`SPELEODB_API_TIMEOUT_MS` overrides these budgets. Connections retain the
application's 30-second timeout. Uploads are not automatically retried.

## Commands

Run from the standalone Stalagmight checkout:

```sh
# Offline SDK contracts and all application regression tests.
TELEMETRY_ENDPOINT=http://localhost/dummy SPELEODB_API_TEST_ENABLED=false \
  cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry

# Live native-file round trips using BOTH email login and the provided token.
SPELEODB_API_TEST_ENABLED=true TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-speleodb \
  --features telemetry live_used_api_calls_with_email_and_token -- --nocapture

pnpm typecheck
pnpm exec vitest run
pnpm test:mock-contract
pnpm typecheck:playwright
```

When checking registry resolution from inside an enclosing checkout with a Cargo
path override, run Cargo from an external temporary directory and pass the
absolute Stalagmight manifest path. For a standalone reproduction, copy this
checkout outside that enclosing tree and run its locked build there. Do not add
Stalagmight to the enclosing repository's workspace, overrides or submodules.

## GitHub Actions

Offline workspace tests run on every applicable PR and push, without credential
secrets. Both workflows set `SPELEODB_API_TEST_ENABLED=false` and explicitly skip
`speleodb::client::tests::live_used_api_calls_with_email_and_token`:

```sh
SPELEODB_API_TEST_ENABLED=false TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry \
  -- --skip speleodb::client::tests::live_used_api_calls_with_email_and_token
```

The separate "SpeleoDB live API tests" step runs after all deterministic checks in its job, including
release-profile compilation and CI's generated-bindings/clippy checks. Its condition is
`github.event_name != 'pull_request'`:

| Event | Offline tests | Live tests |
|-------|---------------|------------|
| Fork, same-repository, or Dependabot PR | Required | Skipped |
| Master push | Required | Required |
| Release tag or manual rehearsal | Required | Required |

Only the live step receives `SPELEODB_INSTANCE_URL`, `SPELEODB_OAUTH_TOKEN`,
`SPELEODB_EMAIL`, and `SPELEODB_PASSWORD` from repository/organization secrets.
It sets `SPELEODB_API_TEST_ENABLED=true`, preserves telemetry and isolated config,
and runs exactly the test exercising both EMAIL and TOKEN authentication:

```sh
SPELEODB_API_TEST_ENABLED=true TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-speleodb --lib \
  --features telemetry speleodb::client::tests::live_used_api_calls_with_email_and_token \
  -- --exact --nocapture
```

Missing credentials visibly skip enabled verification; `--nocapture` exposes the
diagnostic. A skipped run is not live API validation. Live failures fail the job.
The manual runner benchmark stays offline. Each live run deletes its two synthetic projects.
Deterministic frontend regressions cover request supersession, warning cancellation,
and close-operation ownership using synthetic data and deferred promises.

## Coverage and cleanup

Only operations used by the application are exercised: token exchange, profile,
project list/detail/create/update, revisions, individual lock acquire/release,
and native upload/download at HEAD and at a revision. There are no collaborator,
team, release-all or archive endpoints in the suite. The SDK deletion endpoint is
used only by test cleanup.

Offline HTTP tests check request methods, paths, bearer headers, login payloads,
permission and mutex projection, nullable metadata patches, one multipart
`project.stalx` artifact, commit messages, upload SHA selection, status errors,
malformed successful responses, retry boundaries and bounded streamed downloads.
Core tests keep the existing local-edit fences, atomic cache replacement, SQLite
schema checks, undo sanitization, revision fencing and lock-ownership races.
Credential-file tests cover malformed/unreadable input without resetting general
preferences, preservation during unrelated saves, rollback after a failed settings
write, owner-only permissions and token-safe diagnostics. Native installation tests
refuse a failed link write before replacing the cache.

The live test generates synthetic SQLite databases in temporary directories. For
each authentication path it creates a unique private `STALAGMIGHT` project, verifies
metadata and locking, uploads two native revisions, checks the unchanged-file
response, and compares downloaded bytes at HEAD and at the first revision. It
attempts lock release and deletion of only IDs returned by this run, after success,
returned failure or an assertion panic. Both cleanup operations are attempted even
if release fails; cleanup failures fail the test and are reported separately from
the original failure. It never sweeps by name/prefix. If creation completes remotely
but its response is lost, the project ID is unknown and cannot safely be deleted
automatically. All test content is synthetic; no private cave fixtures are used.

UI regression coverage must include:

- Opening a SpeleoDB project automatically attempts to acquire its editing lock.
  Read-only permissions, another editor's lock, and connection failures remain
  distinct. A failed acquisition leaves the project open read-only and explains
  why; a competing editor is identified by their email address.
- A project with unsaved local changes offers Discard changes or Cancel before
  switching or closing. Cancelling performs no download, release or replacement.
  Saved but unuploaded work stays in the local file and does not trigger an exit prompt.
- Deferred open/create setup, remote downloads, and reconciliation cannot replay
  older selections over newer mutations. Cancelling a newer preflight warning does
  not strand an older already-dispatched load; it reconciles and settles loading.
  Stale successes and failures leave the newer loading state, link, dialog and
  telemetry intact. Backend session fences prevent late downloads from closing
  a newer same-cache open.
- Routine session refreshes do not abort close operations. Login/logout, project
  replacement, and dialog replacement invalidate close and older-version confirmation
  continuations before further side effects. Cancelled or superseded remote opens
  restore the owning prompt's controls without resetting replacement prompts.
- Manual SpeleoDB Save collects a commit message. Creating a project uses
  `Project Creation` automatically. Autosave remains local; failures keep the
  message and local edits available for retry.
- Switching projects, closing a project, or quitting releases the editing lock
  automatically. These exits do not upload changes. The SpeleoDB workspace offers
  Switch to Read Only Mode only when locally saved and uploaded; Resume editing
  retries editing when it becomes available. A different holder is never released.
  Local release/acquire/refresh operations are serialized.
- Read-only users cannot acquire or upload; another user's mutex does not become
  a lock owned by this process. Offline/failed metadata checks never grant editing.

Use selected Playwright cases for the affected workflows, including
`window-close-protection.spec.ts` and `speleodb-managed-file.spec.ts`. The tests
mock IPC and therefore complement, rather than replace, the Rust HTTP contracts
and live round trips.

## Workspace acceptance

Use synthetic controlled projects. Check Sections-first default, logo + text at
narrow widths and UI zoom, Survey/Plan/Export navigation, keyboard tab arrows,
focus retention, pin/hover/collapse, resizing, and fullscreen table state. Check
metadata and connection drafts across tab/section/dock hiding, including invalid
drafts on switch, close, sign-out and Quit. Exercise upload, cancel, retry, missing
lock, contention naming the editor, sign-in recovery, conflicts, uncertainty and
release failure. Verify ordinary upload retains the lock, Save and Close waits for
confirmed upload/release, and autosave retry remains local-only. Inspect light and
dark themes. Record real-app and live-server results separately from mocked browser
checks in [the workspace acceptance record](plans/2026-11-08_speleodb-workspace.md).

For the reduced workspace, verify Save Changes, Reload Project, Close Project and History,
and absence of a Projects tab and metadata, rendering, undo-policy, account and admin forms. Sign-in
must open application Settings and preserve the originating description and
Save-and-Close intent without automatically uploading on return. Local settings
remains available and direct IPC cannot rename a linked project.

Acquisition tests must observe a live revisions request after acquire and no download
for equal revisions. Cover stale listing hints, verified descendants, ambiguous
history, failed downloads and status refresh never opening editing. Reload tests
must cover cancellation, acknowledged discard, historical checkouts, mode preservation
and safe failure. Direct read-only/reload calls and concurrent project/account/edit
changes must reject stale consent. Use only synthetic data. For focused verification,
run newly created or modified cases with explicit file and test-name filters, plus
static gates. Run the full suites when requested; selected acceptance does not imply
full-suite acceptance.

### Focused workspace verification (2026-11-09)

Selected new or modified cases passed: 23 Rust backend tests, 24 Vitest cases and
6 headless Chromium cases. The browser coverage includes the reduced workspace,
Close Project cancellation and return to the listing, lock release/resume, Save,
and Settings authentication preserving the description and close intent without
uploading. Backend coverage includes live revision verification, safe replacement,
pending-work guards and edits arriving during a reload's network request.

Main, agent and Playwright typechecks, all nine non-test lint gates, root and
workspace-member formatting checks, and workspace Clippy with telemetry and all
targets passed. Node 24 and the pinned pnpm were used. Existing repository/toolchain
warnings remain; no full-suite or live-server acceptance is claimed. One overly
broad Vitest rerun filter also matched one unchanged reload case, which passed;
subsequent filters were narrowed. No external fixtures were used or committed.

### Local conversion and transfer progress verification (2026-11-09)

Selected new or modified cases passed: 25 Vitest cases, 7 headless Chromium cases,
and one Rust integration test using a synthetic HTTP server and native SQLite
files. Coverage includes local conversion, fresh server reopening, partial failures,
Settings draft recovery, centered layouts at 1280px and 540px, and app-blocking
progress for publishing and uploads from menu Save, Ctrl+S and Cmd+S. Fake-clock
cases verify the exact 120-second expiration, preserved descriptions, and prevention
of duplicate submission. Layout and progress screenshots were inspected.

Main, agent and Playwright typechecks, all nine non-test lint gates, root and
workspace-member formatting, and workspace Clippy with telemetry and all targets
passed. Existing repository/toolchain warnings remain. No unchanged test cases,
external fixtures or live-server tests were run for this change; these selected
results do not claim full-suite acceptance.

### Full automated verification (2026-11-09)

At the user's request, verification was broadened to the full automated suites:

- Vitest: 408 files, 5,698 tests passed, including the IPC mock contract. Seven stale
  expectations from the first run were corrected before the clean full rerun.
- Rust workspace with telemetry: 3,590 tests passed, 41 ignored, including doc tests.
  Corrected retired telemetry entries, source inventories and old acquisition/load
  assertions; made accepted synthetic HTTP sockets explicitly blocking on macOS.
- Vendored Compass: 119 unit tests and 3 doc tests passed.
- Headless Chromium: all 1,135 cases executed, initially 1,127 passed and 8 failed.
  Updated dialog-scoped focus, retained-but-hidden workspace and two-row toolbar
  assertions, and disabled fling inertia in the painted pan-position test. All
  eight failed cases passed on targeted reruns. The three gas-planning cases
  passed unchanged after navigation interruptions in the initial run. A second
  full browser run was not performed.
- All three typechecks, all nine lint gates, all standalone JavaScript script-test
  files, root and workspace-member Rust formatting checks, and workspace Clippy
  with telemetry and all targets passed. The Playwright typecheck and parallel
  lint were repeated after the browser-test corrections.

Clippy retains existing repository warnings. Ignored and environment-gated checks
are not equivalent to executed coverage; native desktop UAT and live-server
acceptance were not separately verified. No external fixtures were added or
committed. Logs and browser failure artifacts are under `/tmp/stalagmight-*`.

## Protocol fuzzer

A stateful fuzzer drives the app's SpeleoDB verbs against a **private** SpeleoDB
instance: `src-tauri/crates/core/src/core/state/speleodb_fuzz/`. Each run builds a
fresh world on the instance (an owner project the collaborator also administers, a
collaborator project shared read-write with the owner, a second owner project, and
a project uploaded through the generic route as v0.5.x builds did when the server
accepts it), signs an `AppState` in as the owner through an in-process fault proxy,
and plays a seeded sequence of the frontend's flows in its IPC order: open with
auto-acquire and the un-pushed-local prompt, the file-open path, edits, autosave,
Save Changes, Save and Close, Close, project switch, Quit (the exit hook's bounded
release), crash (optionally leaving a hot journal), restart-to-update, Switch to
Read Only, Reload, status refresh, Take remote, Discard, History View / Revert /
Unwind / Rewind, Save copy, local projects and Publish, sign-in as another account
or to another deployment, sign-out. A second client (the collaborator, or another
session of the app's own account) takes and drops locks, pushes competing or
newer-schema revisions, renames or deletes projects and changes the app account's
access. The proxy answers 4xx / 5xx, a status after the server applied the request,
latency, a dropped upload, a lost reply, a truncated, halved or scrambled body, a
connection reset, and a hang past the 5 s lock-probe budget. Interleaving steps hold
one request (or reply) of a verb at the proxy while a second verb runs, then let it
go, or drop it when the second verb ends the process.

After every step an oracle checks the invariants documented in
`speleodb_fuzz/oracle.rs` and its `oracle_*.rs` and `interleave.rs` siblings: lock
release on every exit path, the edit gate against the server's lock holder and
permission, other accounts' locks, BASE and the dirty bit against the server's
history and content, no silent loss of an edit, no silent overwrite of a revision,
newer-schema refusals, cache installs and failed transfers, the offline fallback,
late replies after a session move, and synchronous commands waiting on a held
network span. Ids that reproduce a finding of the master review name it (`R1-...`,
`R3-...`, `P2-...`). Each run writes a JSONL trace; a failing run is delta-debugged
to a short step list.

It refuses to start unless every condition below holds, before it sends anything:

- `STALAGMIGHT_SPELEODB_FUZZ_URL` is `http://<loopback IP literal>:<port>/`;
- every account email is under the reserved `.invalid` domain;
- `DEV_SPELEODB_URL` is unset (the app would prefer it over its own endpoint).

| Env var | Meaning |
|---------|---------|
| `STALAGMIGHT_SPELEODB_FUZZ_URL` | The instance base URL (loopback only). |
| `STALAGMIGHT_SPELEODB_FUZZ_EMAIL` / `_PASSWORD` | The app's account. |
| `STALAGMIGHT_SPELEODB_FUZZ_EMAIL2` / `_PASSWORD2` | The second account (second client, account switches). |
| `STALAGMIGHT_SPELEODB_FUZZ_ADMIN_EMAIL` / `_ADMIN_PASSWORD` | A superuser: the oracle's view of locks, history and every commit. |
| `STALAGMIGHT_SPELEODB_FUZZ_RO_EMAIL` / `_RO_PASSWORD` | Optional read-only account (both or neither). |
| `STALAGMIGHT_SPELEODB_FUZZ_SEEDS` | Seed range `a..b` (default `1..4`). |
| `STALAGMIGHT_SPELEODB_FUZZ_STEPS` | Steps per seed (default 40). |
| `STALAGMIGHT_SPELEODB_FUZZ_FAULT_PCT` | Percent of app steps with a fault (default 25). |
| `STALAGMIGHT_SPELEODB_FUZZ_MINUTES` | Stop starting new seeds after this long (default: no limit). |
| `STALAGMIGHT_SPELEODB_FUZZ_SHRINK` / `_SHRINK_BUDGET` | Shrink failures (default on) / at most this many replays (40). |
| `STALAGMIGHT_SPELEODB_FUZZ_MUTE` | Comma-separated invariant ids or id prefixes that do not fail a seed; each is printed once per seed as `seed <N>: muted <id> at step <K>` and the seed goes on. |
| `STALAGMIGHT_SPELEODB_FUZZ_SKIP_ACTS` | Comma-separated act names the generator never draws (e.g. `Interleave,RestartUpdate`). |
| `STALAGMIGHT_SPELEODB_FUZZ_DIR` | The app's folders for the run (default: a temp dir per process). |
| `STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR` | Where traces and step lists go (default: a private temp dir per process, kept). |
| `STALAGMIGHT_SPELEODB_FUZZ_REPLAY` | A saved step list for the replay entry point. |
| `STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT` / `_SEED_SECTIONS` | Instance tooling: where to write a synthetic seed `.stalx` / its sections (`A:3,B:2,C:2`). |

```bash
cargo test --manifest-path src-tauri/crates/core/Cargo.toml --lib \
  core::state::speleodb_fuzz::speleodb_fuzz_campaign -- --exact --ignored --nocapture
```

Run it alone (`--exact`): it sets the process-wide app folders. Parallel workers
need one account pair each. Each seed prints `seed <N>: ok (...)`, `seed <N>: SETUP
ERROR ...` or `seed <N>: FAIL at step <K>: <violations as JSON>`. The other entry
points are `speleodb_fuzz_replay`, `speleodb_fuzz_regression_seeds` (the seeds in
`REGRESSION_SEEDS` plus the directed step lists) and
`speleodb_fuzz_write_seed_stalx`. Normal `cargo test` runs only the offline unit
tests (allowlist, generator, proxy, stub, raw HTTP, oracle helpers); the entry points
are `#[ignore]` and fail naming the missing variable when run without one.

The instance itself (deployment, accounts, reset to a clean snapshot) is tooling
on the maintainer's harness host and is not part of this repo.
