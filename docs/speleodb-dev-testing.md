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
| `SPELEODB_API_TIMEOUT_MS` | Per-request timeout, including file transfers; defaults to `20000` |
| `SPELEODB_API_RETRY_COUNT` | Additional attempts for safe reads and token exchange; defaults to `3` |
| `SPELEODB_API_TEST_ENABLED` | Exactly `true` enables live requests; defaults to `false` |

Defaults apply when neither the exported variable nor `.env` supplies a value.
Set `SPELEODB_API_TEST_ENABLED=false` to disable live requests explicitly.
Both authentication paths are required when live testing is enabled. Missing,
blank or placeholder credentials skip before any network operation, naming only the
missing variable names. Diagnostics distinguish disabled, incomplete configuration,
and execution; authentication/API/assertion/cleanup failures after validation fail
the test. Credentials are never printed.
Set a timeout long enough for the test server's native-file processing; a process
override can be used without editing `.env`.

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

- Only Stalagmight projects appear; read-only permission and lock ownership remain
  distinct and visible. Remote opens warn before downloading; cold recents and
  direct file opens inspect SQLite metadata and warn before replacing the project.
- Cancelling an open warning performs no download, lock release or project
  replacement, including a switch initiated through the close dialog.
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
- Manual SpeleoDB Save and publishing collect a commit message. Autosave remains
  local; failures keep the message and local edits available for retry.
- Switching projects or closing the app warns about unsaved/unuploaded work.
  Cancelling preserves the current project and lock. Keep-lock exits close locally without contacting the current server. Confirmed
  release exits check the current owner; a different holder is not released.
  Local release/acquire/poll operations are serialized. A failed upload or explicit
  release keeps the exit dialog open.
- Read-only users cannot acquire or upload; another user's mutex does not become
  a lock owned by this process. Offline/failed metadata checks never grant editing.

Use selected Playwright cases for the affected workflows, including
`window-close-protection.spec.ts` and `speleodb-managed-file.spec.ts`. The tests
mock IPC and therefore complement, rather than replace, the Rust HTTP contracts
and live round trips.
