# SpeleoDB integration and native-file contract

Stalagmight uses the published `speleodb-api` crate. The dependency version and
registry checksum are authoritative in `src-tauri/Cargo.toml` and `Cargo.lock`.
The desktop application's Cargo workspace is standalone, including when the
checkout is located below another repository.

## Boundaries

The `stalagmight-speleodb` crate adapts generated SDK responses to the application's
small project, revision and error projections. All SpeleoDB HTTP operations go
through that SDK. A shared Tokio runtime bridges the async SDK to the existing
blocking command boundary; frontend IPC and the core sync engine stay synchronous.
Core orchestration still owns cache paths, credentials, project identity, local
edit protection, lock ownership, revision ancestry and undo policy.

The frontend lists only `STALAGMIGHT` projects. It displays read-only permission and
lock ownership independently and warns before opening a read-only or locked
project. This includes cold recents, file-picker, drag/drop and OS file opens: a
read-only SQLite metadata preflight checks access before replacing the open project.
Unavailable verification produces its own notice, distinct from verified read-only
permission. Own-lock warnings are suppressed only for matching endpoint and normalized
account email. Local-only inspection is available for keep-lock exits. Opening does not grant write access. Start editing must acquire the
server mutex and reconcile the local base before enabling edits.

Collaborator/team management and remote project deletion are absent. Local file
management, local imports and local exports remain separate application features.

## Operations used by the app

All paths below are relative to `/api/v2/`.

| Method | Path | Use |
| --- | --- | --- |
| POST | `user/auth-token/` | Exchange email/password for a token |
| GET | `user/` | Validate pasted tokens and resolve their account email |
| GET | `projects/` | List accessible native projects and lock states |
| GET | `projects/{id}/` | Metadata, permission, mutex and short lock probes |
| POST | `projects/` | Create a private/public project from publish metadata |
| PATCH | `projects/{id}/` | Metadata, rendering and managed-type updates |
| GET | `projects/{id}/revisions/` | Revision history and ancestry checks |
| POST | `projects/{id}/acquire/` | Acquire the write lock |
| POST | `projects/{id}/release/` | Release the individual project lock |
| PUT | `projects/{id}/upload/stalagmight/` | Upload one native SQLite project |
| GET | `projects/{id}/download/stalagmight/` | Download the native project at HEAD |
| GET | `projects/{id}/download/stalagmight/{hexsha}/` | Download a pinned native revision |

The request and response schemas come from the SDK. Project permissions are
`WEB_VIEWER`, `READ_ONLY`, `READ_AND_WRITE` and `ADMIN`; absent permissions do not
grant access. Lock ownership comes from `active_mutex`, independently of permission.
A narrow profile compatibility case accepts the email from a successful response
with an empty country: existing accounts can have no country, while the SDK country
enum requires a country code. Non-success responses and other malformed profiles
still fail authentication.

Project-list decoding alone tolerates future enum values: unknown types are omitted
from the native browser, unknown permissions grant nothing, and unknown optional
metadata does not invalidate other entries. Required IDs/names/structure, HTTP errors,
invalid JSON and all mutation responses remain strict.

The SDK is `AGPL-3.0-only`; first-party code remains `AGPL-3.0-or-later`. Version 3
is available under the app's grant, so the combined distribution uses AGPL version 3.
This does not grant later-version rights for the SDK or combined distribution.
SDK attribution remains in `THIRD-PARTY-LICENSES.md`; neither project is relicensed.

## Native file transfer

`project.stalx` is a SQLite database. Upload sends one multipart `artifact` with
that filename and `application/vnd.sqlite3`, plus a `message`. Download returns
the raw SQLite file. There is no archive extraction, candidate selection, companion
Ariane upload, or remote Ariane conversion. The server may use a different stored
filename; callers identify the artifact through the dedicated format endpoint.

Before uploading, core saves the live database, creates its existing consistent
snapshot, and sanitizes local state according to the project's undo policy. The
snapshot remains independent of subsequent local edits. The authoritative upload
result is its top-level `hexsha`; an HTTP 304 remains the existing no-change outcome.
An uncertain upload does not clear dirty state or automatically release the lock.

Downloads stream to an owned temporary file, with a 512 MiB limit. The cache
installer and downloaded-revision policy probes open private copies through
`open_existing_untrusted`, with a 64 MiB SQLite value limit and a 30-second
processing deadline. The installer checks the schema, stages the replacement,
strips inherited undo unless an explicit Unwind requires it, and preserves local
display preferences.
The downloaded copy receives its validated SpeleoDB link before replacement. A
failed download, validation, link write or staged-file synchronization leaves the
existing cache intact. Finished staging files are synchronized before atomic rename;
the parent directory is synchronized where supported. Ordinary pulls capture backend
session ownership before downloading, then check it under the project lock before
replacement. Native load cancellation invalidates old downloads even if the newer
open fails; pending native readers also exclude replacement. Replacements
refuse to overwrite unuploaded edits unless the user explicitly confirms discard.
When a revision is known, the download is pinned to that SHA; an undated HEAD
download cannot be stamped with a stale project-list hint.

## Authentication and transport

Email/password login exchanges the supplied credentials for a token; pasted-token
login validates the token by reading its profile. Live tests validate the profile
for both authentication paths. Saved tokens are managed by the backend and do not
cross the frontend as profile data. Session generations
and per-account lock claims continue to guard late responses and account changes.

The existing HTTP policy permits loopback/private deployments and requires HTTPS
elsewhere unless explicitly configured otherwise. Redirects and implicit Reqwest
retries are disabled. Safe reads and token exchange retry transient transport/5xx
failures twice, after 200 ms and 500 ms. Mutations are never automatically replayed.
Metadata requests use 30 seconds and lock probes five seconds with no retries.
Downloads have a 30-second connection limit and a 120-second read inactivity limit,
with no total transfer deadline. Buffered multipart uploads have a size-aware total
budget of 120 seconds plus transfer time at 16 KiB/s. Response-read timeouts are not
upload-progress timeouts. JSON responses are capped at 10 MiB. Production settings
do not read the live-test environment variables.

HTTP statuses retain their existing typed frontend actions: re-login on an expired
session, read/fence handling for permissions, retryable read errors, and dirty-state
retention for failed writes. Error text is for display; it does not determine sync
or ownership decisions. Diagnostic transport errors omit request URLs.

## Save, switching and exit

Manual Save in a normal SpeleoDB edit session opens the commit-message dialog and
saves locally before uploading after confirmation. Only confirmed `pushed` is
success. Failure says "Saved locally, but not uploaded to SpeleoDB."; uncertainty
says "Saved locally; upload could not be confirmed." The typed message, saved file
and unpushed state remain available for retry, and the lock stays held. Publish also collects a message. Autosave writes only
to the local SQLite file. Force-editing without a mutex remains local until the user
reconnects and reconciles.

Switching projects, creating a new project, closing a project and quitting pass
through the existing unsaved/unuploaded-work guards. Cancellation keeps the project
and lock. The default Keep lock action saves locally when requested and closes
without contacting the current server. Quit first completes this local close, so
backend exit cleanup has no open project to release. An available destination file
can be inspected locally; a remote-only project still requires downloading.
The dialogs also allow uploading before release, or explicitly confirmed release
without upload. The latter retains local edits and warns that collaborators may
advance the remote project. Backend consent is default-false and scoped to the
expected project, authenticated account and backend session generation. A failed save, upload or explicit release
prevents the confirmed exit from continuing. Clean shutdown also has a bounded
best-effort backend release; an unreachable server or forcibly terminated process
cannot guarantee that the server has observed a release. Confirmed close/quit
re-checks ownership before releasing and serializes the release with local lock
acquisition/status polling. A lock already held by another account is left alone.
Explicit administrator lock recovery remains separate. The server has no atomic
compare-and-release operation, so ownership changes between the probe and POST
remain a protocol limitation.

## Verification

See [SpeleoDB testing](speleodb-dev-testing.md) for automatic `.env` loading, both
required authentication paths, synthetic native-file round trips, test-project
deletion and the offline/UI coverage matrix. Generated IPC types are regenerated
from Rust with `pnpm gen:ipc-types`; removed commands and their generated types must
also disappear from browser mocks and telemetry.

## Opening and closing request ownership

Project switches reserve a shared request generation before access metadata or
replacement queues are awaited. Newer preflight requests suppress older probes,
warnings and undispatched work, even if the newer warning is cancelled. A confirmed
operation that already dispatched backend work reconciles its result until a newer
mutation begins. Thus cancelling a newer warning cannot leave a completed native
load hidden behind an indefinite loading overlay. Same-path opens share execution
only while current; A-B-A selects the final A. Tree replacement retains its ordering
contract while independent access probes can proceed concurrently.
Remote opens carry ownership through warning, download and file load. A newer
confirmed native transition, successful login, rejected authentication, or newer
download reservation invalidates the backend download's cache-write authority
as well as its frontend continuation, so a late pull cannot close the newer cache.
Create-project operations distinguish preflight cancellation from dispatched-work
reconciliation in the same way.

The close dialog becomes busy before probing its destination and stays busy through
saving, upload/release, and hand-back. Dismissal and editable choices are disabled;
duplicate activation is ignored synchronously. Escape from outside the dialog
routes through the visible dialog's dismissal guard as well. Cancelling the native
access warning restores the parent controls without saving, releasing, or closing. Replacing the
prompt, project, or authenticated session stops stale continuations. Ordinary saves
may refresh the project object without invalidating the operation. Routine session
status refreshes are also allowed; login/logout changes the session generation and
invalidates the operation. Older-version upload confirmation carries request/session
ownership with the pending exit and checks it before upload/release or hand-back,
while retaining its cancel-return flow.

Cancelling read-only or locked-project warnings in the browser or managed-file prompt
restores retry and dismissal without an error. A replaced managed-file prompt retains
ownership of its own controls. A superseded discard-and-open in the unpushed-local
prompt similarly restores retry and dismissal without resetting a replacement prompt's
operation.
