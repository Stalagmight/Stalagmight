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
- A project with unsaved local changes and no editing lock offers Save on this
  computer, Discard changes or Cancel before switching, closing or quitting.
  Cancelling performs no download, release or replacement.
- Closing, switching or quitting while holding the editing lock over work SpeleoDB
  has not seen (unsaved, or saved but not uploaded) raises the exit prompt:
  Upload, Keep the lock, Discard (confirmed) or Cancel. A clean project releases
  its lock silently. Signing out offers Upload and disconnect, Keep on this
  computer (the copy is kept, the lock handed back) or Discard (confirmed) for the
  open project's work; other copies with changes are always kept on this computer.
- Quitting while an upload, acquire, download or exit-prompt action is in flight
  waits for it with a toast and a waiting prompt (Keep Stalagmight open / Quit
  now behind a warning), then quits.
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
  automatically; a release that fails or is uncertain asks "Couldn't hand back the
  lock" (Retry / Close and keep the lock / Cancel) instead of leaving silently.
  These exits do not upload changes. The SpeleoDB workspace offers
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

## Immediate SpeleoDB connection settings

Connect saves changed preferences and authenticates immediately. Success closes
Settings and refreshes the project listing. Disconnect takes effect immediately,
clears the remote listing and stays on the SpeleoDB Settings tab. Neither action
requires a subsequent Save. While connected, the server endpoint and both reset
controls are disabled; backend requests to change instances are also refused.
Disconnect first, including when the current server is temporarily unavailable.

The SpeleoDB tab has a Close button instead of Save/Cancel. Closing validates and
saves preference changes, including changes in other categories, without connecting.
Connection and persistence errors keep Settings open. The ordinary Settings
unsaved-changes prompt offers only Cancel (red) and Discard Changes (blue).

HTTPS is required except for HTTP to the exact host `localhost`, the exact IPv4
address `127.0.0.1`, or valid IPv4 addresses in `192.168.0.0/16`. Valid ports are
allowed. For HTTP, other private ranges, IPv6, localhost aliases/subdomains, public hosts,
missing schemes and malformed addresses are rejected, without any DNS lookup. An
endpoint the policy refuses produces an error dialog when connecting, or when saving
a new server. `insecure_http` lifts only the HTTP rule (never certificate
verification); it stays in the file so existing configurations load, Settings has
no control for it, and its development-build-only handling is being finalised by a
follow-up commit. Certificate verification and redirect restrictions remain.

The verification notes below record PR #668 as reviewed on its own branch. On
the reconciliation with the review-fixes branch its close-release retry context,
pending-close state and close serialisation were dropped in favour of the
holder-checked release model (docs/SpeleoDB.md), and the `insecure_http` field
stayed in the configuration; its close scenarios were ported onto that model.

### Connection workflow verification (2026-11-09)

Full Vitest: 408 files and 5,701 tests passed; an additional immediate-disconnect
regression test passed separately. Full Rust workspace with telemetry: 3,581
passed and 41 ignored, including documentation tests. The narrower transport
policy replaces the former address-classification and override tests. Seven
selected headless Chromium cases passed for the two-action discard prompt and
SpeleoDB Close saving the endpoint without connecting. All three typechecks,
nine non-test lint gates, root/member formatting checks and workspace Clippy
with telemetry and all targets passed. Existing repository warnings remain.
No full browser-suite or native desktop acceptance is claimed for this change.

### Focused adversarial review (2026-11-09)

Reviewed all five commits above master for connection settings and automatic
transport policy. Corrected ownership checks around preference persistence and
Connect, category navigation during authentication, late save success/error
handling, newer endpoint edits, and rebasing account-owned fields after logout.
Nine focused component regression cases passed across the connection panel and
Settings dialog. No full test suite or browser suite was rerun for this review.

All three typechecks, nine non-test lint gates, root/member Rust formatting and
workspace Clippy with telemetry and all targets passed. Existing Clippy warnings
remain.

### Hamburger Close Project release verification (2026-11-09)

Three selected headless Chromium cases exercise Hamburger Menu -> Close Project:
clean close, unsaved-work cancellation/discard, and visible release failure with
retry. They verify `close_project` is invoked with `releaseLock: true`. Four
focused Rust cases exercise the shared close backend using synthetic HTTP: confirmed
release, rejected release/retry, a 200 response still reporting the owned mutex,
foreign ownership, closed editing gates, missing credentials, local close and
unchanged detached/bounded cleanup. Project and command guards
remain held through release; saved file bytes remain unchanged. One focused Vitest
case verifies IPC rejection preserves project/export state and resets the close guard.
Close failures must not navigate to the project listing or silently drop the session.

One mistakenly broad Rust filter also ran 102 SpeleoDB module cases, all passing.
The final confirmation used explicit `confirmed_close_` and
`lifecycle_release_preserves_saved_unpublished_files` filters. No full suite ran.

### Twelve-commit adversarial review (2026-11-09)

Reviewed the twelve commits above master. Uncertain close-release responses now
pause editing and retain a project/account/session-bound retry context, including
inherited locks without acquisition provenance. Resume verifies ownership before
editing reopens; unreadable retry state refuses teardown. Failed recent-reference
persistence leaves both in-memory lists unchanged so manual retry persists the
removal without touching the original project file.

Five focused Rust cases and three focused Vitest cases passed. They cover lost
release responses, foreign successors, inherited ownership, retry-context identity
and poisoned session guards, verified resume, failed recent-save/retry, immediate
frontend read-only state, and stale account replies. No full suites were rerun.
All three typechecks, nine non-test lint gates, root/member formatting and workspace
Clippy with telemetry and all targets passed; existing repository warnings remain.
`prek run -a` was attempted and exited 2 because neither this repository nor its
parents contains a prek/pre-commit configuration. Existing validation commands
were executed directly.
