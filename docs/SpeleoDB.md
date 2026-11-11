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

The frontend lists only `STALAGMIGHT` projects. It displays project name, SpeleoDB
identity, permission and lock owner. A lock held by the signed-in account is labeled
as yours, independently of whether editing is enabled yet, and opening requires no
additional same-account session check or warning. A read-only SQLite metadata
preflight checks access before replacing the open project, including cold recents,
file-picker, drag/drop and OS file opens. Opening automatically attempts to acquire
the server mutex and reconcile the local base. Only successful
acquisition/reconciliation enables editing. A lock held by another account keeps
the project read-only and the contention dialog identifies its current editor. Offline opening
and opening without a session stay read-only; a transient connection failure after
acquisition permits local editing. Confirmed lock loss, sign-out or permission loss
closes the editing gate.

The home screen uses the same card or table row for downloaded and undownloaded
SpeleoDB projects. Selecting one never exposes local Project Details, local open,
or local deletion actions. Opening while signed in always attempts a fresh download.
Only a DNS, connection-unavailable or timeout failure allows opening an existing
cache, whose stored project ID and endpoint must match. The UI announces that
offline fallback and preserves the cached unsynced-edit state. Authentication,
permission, missing remote project, TLS and invalid-download errors do not fall
back to the cache (but see the no-session rules below). An `http://` endpoint the
transport policy allows (see Authentication and transport) falls back exactly as an
`https://` endpoint does; one it refuses is refused before any request, without
DNS. Known gap (F1, #669): a DNS, refused or reset failure of the request itself
(the SDK's send) is currently classified as an unknown transport error, which does
not fall back, so today only a timeout - or an `http://` host that the transport
policy finds does not resolve - opens the cached copy. If a successful download would
overwrite unsynced edits, the user can cancel or explicitly confirm replacing
them; the prompt offers no online local-copy shortcut.

Without a usable session the cached copy opens read-only, with no network call:

- **Signed out** (no saved token): the cached copy of that project opens as it is,
  un-uploaded edits kept, without a lock attempt. Any instance's copy qualifies,
  since nothing is sent anywhere.
- **Expired token**: a 401, or a 403 that the whoami probe confirms is a dead token,
  drops the token (as on every authenticated call). The open then continues as a
  signed-out one, and the app shows the user as signed out.
- **Server-side denial keeps the no-fallback rule**: a 403 while the token is still
  valid, or a 404, means the server says this account may not have the project. No
  cached copy is opened. TLS and invalid-download errors do not fall back either.

A copy opened without a session says "Read-only copy - connect to SpeleoDB to edit" on
its home screen card or row, on the map's read-only pill (with a Connect action that
opens the SpeleoDB settings) and in the SpeleoDB workspace header. Signing in later does not
change an open project by itself; Resume editing then takes the lock as usual. A
signed-in account whose permission is below read-and-write sees "Read-only - your
SpeleoDB access does not allow editing" in the same three places. The SpeleoDB tab's
lock badge carries the same words as its tooltip (and accessible description), and,
for a project that could be edited but is not right now, "Read-only - you are not
editing this project".

Collaborator/team management and remote project deletion are absent. Local file
management, local imports and local exports remain separate application features.

## Cached copies on this computer

Every project opened from SpeleoDB lives at
`<cache>/speleodb/<project_id>/project.stalx`. The project ID must be a UUID in its
canonical spelling (lowercase and hyphenated, as SpeleoDB sends it; a braced,
`urn:uuid:`, hyphenless or uppercase spelling of the same UUID is refused, since it
would be a second directory or request path for one project) before it becomes a
path: opening a linked file whose ID is not one is refused, and the
cache path itself (`speleodb_cache_path`, used by downloads, revision restores and
the upload snapshot) refuses such an ID again for every caller, so a crafted
`speleo_link` row cannot steer a write out of the cache. The `.stalx` records its instance,
but not the account: the file rides to the server on every upload, so an account
email inside it would reach every collaborator. The account is recorded beside the
copy instead, in `owner.json`, which never leaves the machine. It is written when a
signed-in account opens the copy online (download, or the fast-forward local open
after the server showed that account the history) and when a project is published.
It is not written for offline or signed-out opens. It is only ever read or written
for the copy in the app's own cache (never for a file elsewhere that merely sits
in a `<project_id>/project.stalx` folder), through a temporary file the write
creates itself, so a symlink planted under the temporary name is never written
through.

Un-uploaded work belongs to the account that made it, and no other account ever
takes it over:

- The online open takes the fast-forward (local edits whose base is the server tip
  open as they are, with no download) only for a copy whose owner record names the
  signed-in account on the signed-in instance.
- Another account's copy with un-uploaded edits (or one recorded on another
  instance) goes through the guarded download instead, which refuses to replace the
  edits, and its owner record is left as it is. The prompt names that account
  instead of saying "your changes", and its non-destructive answer is "Open it
  read-only": the copy loads without downloading anything and without taking the
  lock, and a toast says whose changes they are. The user can look at them and save
  a copy of the project. Cancel, or Discard their changes after a confirmation, work
  as before (the discard takes the server copy, which deletes the other account's
  changes and records the signed-in account). Like every dialog, the prompt says
  in its body what each answer does, and Cancel / Discard (danger) / Open (primary)
  sit on one footer line.
- An offline open of such a copy opens it read-only too, with the same toast.
- Start Editing (and with it Resume editing and the held-lock prompt's resume)
  refuses an open cache copy whose owner record names another account while it
  holds un-uploaded edits, before any request is sent: "this copy holds changes
  <account> made that were never uploaded to SpeleoDB. Only <account> can edit and
  upload them...". The copy stays open read-only. A clean copy recorded for another
  account holds only the server's content, so Start Editing goes ahead and records
  the signed-in account as its owner.
- A copy with no owner record (downloaded before the record existed) is not claimed
  silently when it holds un-uploaded edits: nothing tells whose they are, so it
  takes the guarded download and its prompt (with "Open my local copy", which still
  opens it for editing as before). It keeps no owner until a download replaces it
  (once its work is uploaded, or after the confirmed take-the-server-copy); a clean
  owner-less copy just downloads and records the account as before.
- Signed out, every cached copy opens read-only anyway; signing in later does not
  let Start Editing take over another account's work, by the rule above.

The home screen lists the cached copies by owner:

- **Signed in**: the copies of the signed-in account on the signed-in instance.
  Copies with no owner record, downloaded before the record existed, are listed
  when their own link names the signed-in instance and hidden when it names another.
  Nothing tells whose they are, and hiding a pre-upgrade copy from the account that
  downloaded it would hide the way back to its work. The next online open records
  the owner. Other accounts' copies are hidden everywhere on the home screen,
  including the map. A project the account can see on the server is still listed
  from the server listing, and opening it downloads as that account (guarded against
  replacing un-uploaded edits).
- **Signed out**: every cached copy, of every account and instance, on the list and
  on the map. Signing out does not forget the recents rows of cached copies (the map
  is built from recents). It still forgets clean SpeleoDB references outside the
  cache, and keeps any reference whose copy holds un-uploaded edits.

A cached copy with un-uploaded edits carries the yellow leading edge on its card and
on its table row alike (from the copy's own state, so a copy that is in neither
recents nor the server listing is marked too). SpeleoDB cards and rows open on a
single click; the list hint says so whenever one is listed.

Right-click on a SpeleoDB card, table row or map marker opens the SpeleoDB project
menu: Open project, Zoom to system (when the project is on the map) and Remove local
copy. Removing a copy goes through an in-app confirmation that first re-reads the
copy. A copy with un-uploaded edits, or one whose state cannot be read, gets a
warning and a "Remove copy and lose my changes" button. Only that confirmation tells
the backend to remove such a copy; without it the backend refuses. Signed in, the
backend also refuses another account's (or another instance's) copy, which Home does
not list then anyway; signed out, any copy can be removed. The project open right
now is never removed: the delete itself asks again under the project lock, so a copy
opened after the confirmation's check is refused, never closed and trashed. Nor is a
`.stalx` in the cache that is no longer linked to SpeleoDB (an unlink keeps the file,
which may have been edited and saved since): no listing shows it, and Home's
removal, the purge and sign-out's Discard all refuse it, whatever the consent. A
project folder in the cache that is a symlink is never followed by a removal (it
would reach whatever it points at). Removal sends the `.stalx` to the OS trash where there is
one, drops its recents row, and deletes the rest of its directory (owner record and
SQLite sidecars). The file goes to the trash under the project's name (for example
`Synthetic Cave.stalx`, made a safe file name on every OS, with invisible Unicode
format characters such as a right-to-left override replaced) rather than as one more
`project.stalx`, so a user can tell which copy to restore; it is renamed in place
just before the trash, never over another file, and renamed back if the trash fails.
The exit prompt's and sign-out's Discard trash their copy under the same name.

Removing a copy does not strand a lock. After a successful removal, when the session
is the copy's account on its instance (owner recorded as this account, or no owner
recorded and the copy is from this instance), the short lock probe asks who holds
the project's lock. If the server says this account, the normal lock release hands
it back. The probe never drops the session. Signed out, for another account's copy,
or when the server cannot be asked, nothing else happens: a lock held on the server
stays until that account's next sign-in (whose held-lock prompt offers to release
it) or the SpeleoDB website. The confirmation says which of the two applies. A
probe or release that fails returns a warning saying so. Removal and the purge count
as exclusive SpeleoDB operations: their confirm buttons are disabled while another
lock, upload or account call runs, and the spinner shows while they run.

SpeleoDB settings has "Purge all cached files". It removes the cached copies the
home screen lists, except the open project: signed in, the account's own copies and
the owner-less ones from its instance; signed out, every copy of every account and
instance. Signed in, another account's copy (or another instance's) is never counted,
listed or removed, not even by "Delete everything": its un-uploaded work is not this
user's to throw away. The backend applies the same rule when it purges, and a session
it cannot read at all removes nothing. The purge reaches exactly the copies the
listing shows: an unlinked `.stalx` in the cache is skipped, and not reported, since
the confirmation never named it. Copies with un-uploaded or unreadable state are
listed in the confirmation and kept by default ("Keep those, remove the rest").
Deleting them too takes a second confirmation. The backend re-checks every copy when
it removes it. Each removed copy's lock is handled as above, and the result lists the
locks that were handed back and those that could not be checked. Once one probe
cannot reach the server, the remaining copies are removed without a probe each
(which would only wait out its timeout again) and their locks are listed as not
checked. The result is
announced (a live region) and scrolled into view. With nothing to remove (no copy, or
only the open project's), the button is disabled and says so.

## Operations used by the app

All paths below are relative to `/api/v2/`.

| Method | Path | Use |
| --- | --- | --- |
| POST | `user/auth-token/` | Exchange email/password for a token |
| GET | `user/` | Validate pasted tokens and resolve their account email |
| GET | `projects/` | List accessible native projects and lock states |
| GET | `projects/{id}/` | Metadata, permission, mutex and short lock probes |
| POST | `projects/` | Create a private/public project from publish metadata |
| PATCH | `projects/{id}/` | Internal managed-type updates and publishing compatibility |
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

The server accepts at most 24,657,920 bytes per upload and refuses a larger file
only after receiving all of it. That figure is an upstream bug (the check
multiplies the maximum file COUNT by `1024 * 1204`, a typo for `1024`); it lives in
one constant, `client::SERVER_UPLOAD_LIMIT_BYTES`, to raise once the server is
fixed. The sanitized snapshot is measured before anything is sent:

- within the limit, it uploads unchanged;
- over it with the undo history included (Full mode), the copy is re-sanitized on
  the upload-without-history path the other undo modes take. If that fits, the
  survey data uploads in full and the success toast becomes a warning that the
  revision was uploaded without undo history (`undo_history_omitted` on the
  `released` / `pushed` result, and on the publish result for the first upload,
  where the note rides the publish outcome's own toast or stands alone as a
  warning). Only the copy changes; after the landed push the
  local undo stack follows the same per-push rule as in every mode;
- still over, the push is refused before any request with a clear "too large"
  error naming both sizes. Nothing is uploaded, and the project stays dirty and
  locked. `upload_stalx` itself refuses an over-limit file the same way, so no
  caller can send one.

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

SQLite finds a database's `-journal`, `-wal` and `-shm` by name only, so a sidecar
left beside the cache copy by a kill mid-save would replay the old file's pages into
whatever is installed under that name. Every install therefore moves the outgoing
file's sidecars aside (durably, with no connection left on the file) before the
rename, deletes them after it, and puts them back if the install fails. A hot
rollback journal is part of the copy's committed state, so a copy that has one reads
as unknown local state: a protected replacement refuses it, and only the confirmed
discard removes it.

A revision View or Restore is bound to the project open when it started. Its
download can outlive a switch to another project, so it installs and reopens in one
commit under the project lock, re-checking the session generation, the open link
and the in-memory project there; if any moved, it replaces nothing, leaves the
project open now untouched, and the frontend treats the late reply as stale.

## Authentication and transport

Email/password login exchanges the supplied credentials for a token; pasted-token
login validates the token by reading its profile. Live tests validate the profile
for both authentication paths. Saved tokens are managed by the backend and do not
cross the frontend as profile data. Session generations
and per-account lock claims continue to guard late responses and account changes.

HTTPS is required (`speleodb/tls.rs`, `validate_endpoint`). Plain HTTP is allowed
only to the automatic exceptions, compared as written and without DNS: the exact
host `localhost` (any letter case), the exact IPv4 address `127.0.0.1`, and IPv4
literals in `192.168.0.0/16`, on any valid port. HTTP to anything else (hostnames,
public addresses, the other private ranges, IPv6 literals, localhost aliases or
subdomains) is refused before any request, as are a missing or unknown scheme, a
missing host, user info and an invalid port. The check runs before every client
call, on sign-in and when Settings saves a new server or a new insecure-HTTP choice
(development builds), so turning that choice off under a saved `http://` hostname is
refused rather than saved. A refused endpoint crosses IPC
with a typed `endpoint_refusal` (`PlainHttp`, or `InvalidAddress` for an unusable
address), and the UI shows a translated line that names the server and says what to
change instead of the backend's English rule. The exception set is defined once
(`is_automatic_http_exception`, described by `PLAIN_HTTP_RULE` beside it, and for the
UI by the one `speleodb.httpExceptions` string the refusal lines nest).

The persisted `insecure_http` field (`SpeleoDbConfig`) lifts only the HTTP rule, never
certificate verification, and only in a development build: the transport policy
(`tls::validate_endpoint` and the client's `enforce`) applies it only when
`debug_assertions` is on (`INSECURE_HTTP_HONOURED`), so a release build - betas and
release candidates included - ignores a stored `insecure_http = true` and any value a
caller passes. The policy is the one place the value takes effect, so no caller has
to remember the build. Settings shows the "Allow insecure HTTP" switch only in a
development build (`import.meta.env.DEV`), where Connect sends the switch on screen; a
release build sends no choice. The field stays in the configuration so existing files
load in every build. In a development build, a hostname it lets through over HTTP is
resolved before the client dials it, and one that does not resolve is the DNS transport
failure (offline, with the cached open), not a refusal. Redirects and implicit Reqwest
retries are disabled. Safe reads and token exchange retry transient transport/5xx
failures twice, after 200 ms and 500 ms. Mutations are never automatically replayed.
Metadata requests use 30 seconds and lock probes five seconds with no retries.
Downloads have a 30-second connection limit, a 120-second read inactivity limit
and a size-aware total deadline: 120 seconds plus the bytes received at 4 KiB/s
(the SDK's stream does not expose the reply's length, so a download of N bytes ends
within 120 s + N / 4 KiB/s), capped by an absolute ceiling: the deadline of the
largest file SpeleoDB accepts (24,657,920 bytes, see Native file transfer) at 4 KiB/s, 6,140 s or about
1 h 42 min (`DOWNLOAD_CEILING`). Every project the server can store still fits at
the minimum rate; without the cap, a server paying out just above 4 KiB/s could
hold a download for some 36 hours within the 512 MiB native limit. A server
trickling bytes inside the read limit is stopped as a timeout instead of holding
the transfer, and a quit waiting for it, for ever. Buffered multipart uploads have a size-aware total
budget of 120 seconds plus transfer time at 16 KiB/s. Response-read timeouts are not
upload-progress timeouts. JSON responses are capped at 10 MiB. Production settings
do not read the live-test environment variables.

HTTP statuses retain their existing typed frontend actions: re-login on an expired
session, read/fence handling for permissions, retryable read errors, and dirty-state
retention for failed writes. Error text is for display; it does not determine sync
or ownership decisions. The message of an error reply (its `detail`, `error` or
first `errors` entry) is the server's text, so it is cut to 240 characters (the
contract-mismatch bound below) where it is read, before it reaches the log, an
error dialog, a removal warning or the purge report. Diagnostic transport errors omit request URLs. A TLS
handshake or certificate failure is classified as TLS from the typed `rustls`
error. A 2xx body that does not match the SDK contract (for example a new server
enum value) is a parse error: it keeps the SDK decoder detail, with every
response value redacted (serde's quoted strings and its backticked numbers,
variants and field names; a backticked value is printed verbatim and may itself
hold a backtick or `, expected`, so its span runs to the last backtick before the
message's final `, expected`, and anything after a span that never closes is
dropped) and the length capped, is logged with the endpoint
kind, and is counted as its own `parse-error` traffic outcome (docs/telemetry.md).

Revision times arrive as ISO-8601 UTC instants
(`2026-11-10T08:18:23.123456Z`; SpeleoDB a679010c6, SDK 0.6.2, which decodes
them as UTC). The backend hands each on as UTC with milliseconds, and the
revision history shows the viewer's local time with its GMT offset, e.g.
`Nov 9, 2026, 10:15 AM GMT+1`, with the relative time as the tooltip.

An older server sends New York wall time with no zone (`YYYY/MM/DD HH:MM`) and
embeds a commit with such a date as every project's `latest_commit`. SDK 0.6.2
refuses that date, so the client (`client/commit_dates.rs`) decodes a refused
2xx reply again with those dates out of the SDK's way:

- an embedded `latest_commit` becomes its bare id, the only part the app reads;
- a revision keeps its place, and its wall time passes through to the frontend,
  which converts it to an instant with the webview's time-zone data (DST-aware).

Everything else in the reply is still held to the SDK contract, and a reply
with no such date stays a contract mismatch. A revision time in neither form is
not shown.

## Project workspace

The SpeleoDB workspace is the left dock's
SpeleoDB tab, for local and managed projects, including signed-out and read-only
projects. The tab is its only general entry point (the hamburger has no SpeleoDB
row): in Survey click it, after revealing the dock with its left-edge opener if the
dock is closed, in hover mode or squeezed out by a narrow window; from the
fullscreen table, go Back to Map (or Escape) first. Plan and Export keep the
SpeleoDB tab when Survey was showing it on entry; otherwise they show no left dock,
and the way to the tab is back through Survey. The push-failure banner's links and
the revision Revert flow still open the tab themselves (`openSpeleoWorkspace`),
which leaves the fullscreen table without clearing table selection or scroll. Read-only
puts a lock badge on its icon (see the read-only prose above for its tooltip).
Un-uploaded changes are not an error, so they get no badge and no "!": the tab's
icon and text both turn the warning colour (yellow), dimmed while another tab is
selected and full strength when the SpeleoDB tab is; the tooltip says the changes
are not uploaded yet. The tab is otherwise selected exactly like Sections (no wash
or rule of its own). Home marks a copy with un-uploaded changes the same way: a
yellow leading edge and a yellow mark, no badge. Survey
starts on Sections; Sections and SpeleoDB use accessible tabs with arrow-key
navigation, and returning to Survey restores its previous tab. Navigation never
acquires or releases a lock.

For managed projects, the persistent header shows identity, editing access, lock owner and
connection status. Everyday controls are Save Changes, Switch to Read Only Mode or
Resume editing (Connect to resume editing while signed out), Reload Project, Close
Project and Open on SpeleoDB, stacked as the app's ordinary full-width buttons (see
docs/style-guide.md, Buttons). At most one of them is `primary`, the action that
makes sense now (`speleoPrimaryAction`): Save Changes while there are changes not yet
uploaded, Resume editing when the account can edit, the Connect button while signed
out, and none when the access is view-only or when editing with nothing to save.
Every other button is `secondary`, and the browser ones carry an external-link icon.
A disabled control says why in
its tooltip (signed out, no access, view-only, or another SpeleoDB operation
running). Revision History is the only
expandable section; its "Revision History on SpeleoDB" button opens the project's
revision list on the web. Close Project sits below Reload Project,
warns about unsaved work, releases the editing lock on confirmed close and returns
to the project listing. Revision History includes inspection and restore/revert
actions; revision editing remains Survey-only. It has no Unwind/Rewind controls.

Account sign-in, endpoint and connection management live exclusively in application
Settings under SpeleoDB. Every sign-in link opens that category. Settings' buttons
say Connect and Disconnect (see "Immediate SpeleoDB connection settings" below), and
the rest of the SpeleoDB screens use the same words: "connect to SpeleoDB" for the
way back to editing, "Not connected" in the workspace header, "Disconnect" in the
sign-out dialog a Disconnect raises. Connect first runs the sign-out and
unsent-drafts questions an account change needs (`speleoStore.applyPendingAuth`),
then saves the staged settings (its `beforeApply` step, so a cancelled question saves
nothing), then signs in with the endpoint on screen (and, in a development build, the
insecure-HTTP switch on screen); the backend saves the connection with the session, a
failed sign-in saves no credentials, and saving a new server clears the previous
server's email and token. A connected session is bound to its server: a sign-in or a
settings write that would change servers is refused with its own error,
`ServerChangeWhileConnected` ("Disconnect from SpeleoDB before changing servers").
A failure is shown in an in-app error dialog; a transport error names the server on
screen, which is the one that was tried. Disconnect runs the sign-out flow below
before the session ends. A blocked save
retains its project, validated description and Save-and-Close intent while Settings
is open. Authentication returns to confirmation without saving or uploading.
Changing project or account invalidates the continuation.

Local settings retains project validation and elevation settings. Linked names are
server-managed and cannot be renamed through local-settings IPC. There are no
remote metadata, rendering, per-project undo-policy or administrative lock-recovery
controls. Existing persisted undo-policy values remain respected. Creation and
publishing retain their initial metadata inputs. The server's web-rendering flag
(`exclude_geojson`) is not surfaced: the former "Web rendering disabled" banner is
retired, so the only informational banner left is the dismissable no-zero-datum
notice.

The left dock reuses Sections' saved width and pin/hover preferences: one width for
both tabs, so switching tabs never resizes it, with a 230px minimum
(`LEFT_WORKSPACE_MIN_WIDTH`) and Sections' 400px maximum; constrained layouts float
the dock over the map. Programmatic opening focuses the selected tab without changing the saved
pin/hover preference. Keyboard focus inside keeps a floating dock accessible.
Tabs, collapsed sections and hidden docks preserve history state.
Navigation never submits changes. Local settings remains available from the menu;
Home retains local-project rename.

Project-editor SpeleoDB menus and the old settings dialog are replaced by this
workspace. Home retains sign-in, browsing, creation and local-project publishing.
Local projects expose the SpeleoDB tab with one centered conversion action and a
short explanation focused on version history and team collaboration. Conversion
opens the existing project creation form. Account forms remain in application
Settings; returning from sign-in restores the creation draft without submitting it.
Publishing shows a centered, app-blocking progress overlay through completion.
The overlay expires after 120 seconds with a timeout explanation and an enabled
Close action; an in-flight request cannot be submitted again. Account/project
changes and newer edits still invalidate stale results.
On confirmation, the project is saved and its first revision uploaded, then a fresh
server copy is downloaded and opened as a managed project. After successful
reopening, its original local Recent Projects reference is removed. The original
`.stalx` file is retained; unsuccessful conversions keep their original recent
reference. If removing the recent reference cannot be persisted, the reference
remains available for a later manual retry and the original file stays intact.
Editing is acquired once through the shared verified acquisition flow. Unconfirmed uploads preserve the
linked working copy for Save Changes; failed downloads preserve the remote identity
and offer Reload Project. Retrying recovery never creates a second server project.

## Save, switching and exit

Menu Save, Cmd/Ctrl+S and Save Changes share one dispatcher (`dispatchProjectSave`).
Local projects retain local Save. A linked project we are editing opens the upload
dialog. A linked project that cannot be uploaded right now is never left with
discard-only edits: the work is first saved to the local copy (the backend
`save_project` has no edit gate), then
- when the lock can be taken (signed in, write permission, nobody else holding
  it), the upload dialog opens and takes it, and the user uploads with a
  description as usual;
- otherwise a warning toast says the work is on this computer and why it was not
  uploaded (signed out, someone else editing, no write access), and Save and
  Close goes on to close.

The upload dialog's blocked panel (shown whenever upload is not allowed) offers
Save on this computer beside Connect in Settings / Resume editing. Connecting and Resume editing
recover access without uploading automatically; the user confirms the upload
afterward. A clean linked project reports No changes to upload, without creating
an empty revision.

Every exit, close or switch question about unsaved edits offers a non-destructive
answer as well as Discard: the unsaved-changes dialog (quit, close, open another
project, New Project) saves a SpeleoDB project's edits on this computer instead of
routing to the upload dialog; the open-from-SpeleoDB and account-change questions
(in-app, `askExitChoice`) offer Save on this computer whenever the project has a
file of its own. Saved edits keep the copy's un-uploaded flag, so the next open
offers to open the local copy.

Uploading requires a nonblank change description, saves locally first, and verifies
current permission and lock ownership against the server. Ordinary successful upload
retains the lock. The same centered progress surface blocks the app during explicit
uploads from Save Changes, menu Save and Cmd/Ctrl+S. It expires after 120 seconds,
allows closing the dialog, and preserves the description without resubmitting.
An upload whose dialog (Save Changes, Finish editing or the exit prompt) was closed
after that notice is still applied when it ends, for as long as the same project
and SpeleoDB session are open: a success updates the link and shows its usual
toast; a failure raises the push-failure banner and an error toast, and keeps the
description for the retry. Only the dialog's own follow-ups (closing it, Save and
Close) need it to still be open. While a dialog or prompt that shows the failure
inline is open (Finish editing, the revert and unwound-upload confirmations, the
exit prompt, the sign-out prompt), the banner stays hidden, so the failure is
never shown twice; it comes back if that surface closes with the failure
unresolved.
Offline, failed or uncertain uploads never report success; the
message and local work remain available for retry. Autosave and failed-autosave
retry write only to the local SQLite file.

The upload dialog (Save Changes, and Finish editing when it has changes to
upload) does not upload a survey with errors (validation issues of error
severity; warnings do not count) without a decision. It shows the number of
errors in red, and its upload button stays disabled until the user ticks "Upload
anyway, with the errors"; the tick holds for those errors only (by warning
identity, not by count). "Resolve now" uploads nothing and leaves nothing
pending: it closes the dialog (a Save and Close does not close), sets the
description aside for the next upload dialog, or the push-failure banner's Retry,
to start from (`setAsideUploadMessage`; unlike a pending upload it does not
reopen the dialog when Settings closes, is not an exit-prompt draft, and lapses
once nothing is left to upload), and opens the Details panel on the Warnings tab
(`openWarningsTab`, leaving Plan or Export for Survey). The notice is hidden while
the upload itself is blocked, and it counts the errors the Warnings tab lists: if
those cannot be computed (`refreshWarnings` fails) the dialog does not ask. The other
upload surfaces do not ask yet: the exit and sign-out prompts' upload step, and
the unwound-upload and revert confirmations when opened on their own (the
unwound one opened from this dialog comes after its question).

The change description is limited to 1000 characters as SpeleoDB stores it. The
server strips it and turns every run of line breaks (with the whitespace around
them) into ". ", then stores it in a 1024-character column only after committing
the upload; a longer description commits, fails to store (HTTP 500), and breaks
every later upload of the project. The upload dialogs count the stored length (a
line break counts as two characters), show the count under the field, and refuse
a longer description with an inline message. The backend refuses it too, both at
the `speleodb_done` / `speleodb_push` / `speleodb_publish` IPC boundary (before
any work) and in `client::upload_stalx`, so no caller can send one
(`speleodb/commit_message.rs`, mirrored by `src/utils/commitMessage.ts`).

Save and Close carries its close intent through recovery and closes only after a
confirmed upload and successful release, with no newer local changes. Cancellation,
conflict, uncertainty and release failure leave the project open. Plain Close,
including Hamburger Menu -> Close Project, project switching and Quit hand a
clean session's lock back without a question, through the holder check (only a
lock SpeleoDB says this account holds is released). When the lock is held over
work SpeleoDB has not seen, they ask the one in-app exit prompt (Upload / Keep the
lock / Discard / Cancel, `utils/speleoExit`); the backend never releases over
un-uploaded edits unless the close carries the confirmed discard. Without the
lock, unsaved survey work gets the unsaved-changes dialog with Save on this
computer / Discard / Cancel (see above). Saved unuploaded work remains in the
local file.

A lock is never stranded silently. A clean Close, project switch (file open, New
Project, open from SpeleoDB) or Quit the app drives (menu Quit, Ctrl+Q, the window
X, restart-to-update) leaves the project only once its lock release is settled.
The backend reports the outcome on the reply of the call that released
(`CommandResult.lock_release`: `released`, `not_held` or `failed`), so the
frontend decides without a second round trip: `close_project` for a close, and
`speleodb_settle_open_lock` - the same release, without closing - before a switch
or a quit. A release that failed or is uncertain (the holder could not be read, a
transport error, a 5xx, a lost reply, a reply that still names this account, no
usable token for the account holding the claim, an unreadable session) closes
nothing: the claim stays, unbound, so editing is already paused, and the in-app
dialog "Couldn't hand back the lock" (`utils/speleoReleaseFailure`) asks. Like every
other dialog, its body says what went wrong (with the reason of the last attempt)
and what each answer does, and its buttons are in the one footer row: Cancel /
Close and keep the lock / Retry (primary).

- Retry - the holder-checked release again. The dialog stays up, with the new
  reason, while it keeps failing; a lock SpeleoDB now says is someone else's (a
  lost reply after which another account took it) counts as settled, and nothing
  is sent for it.
- Close and keep the lock - leave on purpose (`close_project` with
  `release_lock: false`, or `speleodb_settle_open_lock` with `release_lock: false`
  before a switch or quit). The claim is marked kept, so neither the stranded-claim
  cleanup, the switch's own release nor the exit's hands it back, and the lock
  appears in the next sign-in's held-lock prompt.
- Cancel - stay in the project, read-only: the switch or quit is abandoned
  (with any update restart riding on it), and Resume editing takes the lock again.

A quit that first waited for a transfer (see below) is re-issued un-confirmed, so
it reaches the same question rather than exiting. Any other close failure keeps
the project open with the "Close Project failed" error dialog and pauses editing
until the link status is refreshed. A close after a sign-out (or a rejected token)
does not ask: the claim belongs to an account no longer signed in, the sign-out
flow already asked about its locks (Try again / Disconnect anyway), and the claim
stays for the stranded-claim cleanup after that account's next sign-in. The
bounded silent release at process exit (`RunEvent::ExitRequested`) remains only
for the exits the app cannot intercept (an OS-forced exit, Quit now over a
running transfer).

Form drafts are unsent SpeleoDB input: an upload description (in the upload dialog,
or kept while Settings is open for sign-in or an error replaced the dialog) and the
project creation form. A description with nothing left to upload, or one being
uploaded, is not a draft. Every way out that would drop one asks about it in-app,
ONE question per step: Close, project switching (file open, New Project, open from
SpeleoDB) and Quit ask first, in the shared in-app confirm, before the exit prompt
or anything that uploads, releases or closes, and Cancel stops the exit with nothing
done. Where the way out also asks about unsaved survey edits (the open from SpeleoDB
over a read-only or local project, an account change) the drafts are listed in that
question instead, and either answer that goes on (Save on this computer, or Discard)
drops them. Quit asks after the quit-after-work gate below, so a re-issued quit asks
it against the settled state; declining abandons the quit and any update restart or
forced quit riding on it. An account change drops only drafts written for another
account or server: a sign-out or the same account keeps them, and the question comes
after the sign-out guard, together with the unsaved-changes one. No guard or SpeleoDB
flow uses a native dialog. Backend release consent remains scoped to the project, authenticated
account and session generation. Another account's lock is never released by an
ordinary exit.

### Quitting while a transfer is in flight

Quitting never cuts a SpeleoDB upload, acquire, download or release that is still
running, so a slow connection does not lose work (lock#6, frontend#4). Menu Quit,
Ctrl+Q, the window X, an OS window close and restart-to-update all request a
window close, and the close listener's first gate (`utils/quitAfterWork`) runs
before any question. On macOS the native app-menu Quit / Cmd+Q may end the app
without a close request (#640, unverified); then only the backend exit hook below
holds the exit for a transfer, with no prompt. For the routes that request a close:

- With nothing in flight the quit goes on as before.
- With an exclusive SpeleoDB call (a project close and the sign-out's discard of a
  cached copy included), a hand-off verb, an exit-prompt answer or a
  "Couldn't hand back the lock" Retry in flight, the window stays. An info toast says the save or upload is in progress
  and that Stalagmight will close when it finishes, and the waiting prompt
  (`QuitWaitDialog`, layered over the upload progress) shows a spinner with Keep
  Stalagmight open (the default; also Escape) and Quit now. Once the work has
  drained and stayed drained for 300 ms, the quit is re-issued un-confirmed, so
  every guard sees the settled state: a successful upload quits once the clean
  project's lock is handed back (or "Couldn't hand back the lock" is answered); a
  failed one raises the exit prompt.
- Quit now is an explicit choice behind a warning (the transfer is cut off and may
  or may not reach SpeleoDB; the project can stay locked until it is opened
  again). It saves unsaved edits on this computer first (a failed save stops it),
  then quits without waiting and without the exit questions the wait would have
  re-asked; the unsent-input and the import / dive-plan questions still run, for
  as long as the user takes to answer them. The frontend arm is a one-shot, not a
  deadline: the quit consumes it, and declining either question, a refused window
  close or a guard taking the close over drops it, so a later quit waits again.
  Right before the window goes it calls `arm_forced_exit` so the backend may cut
  the operation. When the window stays after all (its destroy, or the update
  install, failed), `disarm_forced_exit` takes that arm back, so the app that is
  still running does not cut a transfer the user starts or quits over next.
- One quit attempt asks its questions at a time. A close request while the
  unsent-input or the import / dive-plan question is on screen (the user pressed
  the window X again) joins the attempt that is asking instead of starting
  another, whose own question would have answered the pending one with "no" and
  dropped the update restart or forced quit riding on it. Whatever was armed for
  the quit stays armed for the asking attempt, which still runs every guard (a
  pre-confirmation the joining request carried is dropped).

The backend exit hook (`commands/exit_wait.rs`, from `RunEvent::ExitRequested`) is
the backstop for a user-visible transfer still running when the exit arrives anyway.
Only the transfers the frontend gate waits on can hold it, and the two sets are the
same: every IPC command the gate counts as `exclusive` (`speleoNetworkCommands.ts`;
the SpeleoDB ones run through `run_transfer`), including `close_project` and the
cached-copy discard, whoever calls them. A test in `exit_wait.rs` scans every
command module and fails when a command holds the exit without an `exclusive` row,
or the reverse, so a quit can never destroy the window while the process stays
alive for a command the gate did not count. While one runs and
no forced exit is armed (10-second arm), the hook prevents the exit, waits for it on
a background thread, runs the bounded lock release (5 s) and then exits.
Backend-internal work the user never sees (the background lock re-verify, the
stranded-claim cleanup, a detached release from a project switch) never keeps a
process alive without a window: the exit keeps the bounded behaviour for it, the
release waiting at most 5 s behind it before the app exits. Tauri cannot prevent an
`app.restart()`, so the install command guards the update restart itself
(`install_app_update_and_restart`, async, off the main thread): while a user-visible
transfer is in flight and no forced exit is armed it refuses with a typed
`transfer_in_flight` error, checked before its bounded lock release and again right
before the install. Nothing is installed and the download is kept: the update banner
stays on Restart and a toast says a SpeleoDB transfer is still running. The frontend
gate normally waited for the transfer already; the backend does not rely on it.

### Signing out

Disconnect in Settings is the only way to sign out from the UI, and it never logs out
directly: it first raises the sign-out dialog ("Before you disconnect") over Settings.
Cancel there leaves the user connected, with Settings open and nothing staged.

Signing out first resolves what the account still holds (`utils/speleoSignOut`).
The dialog is laid out like every other one in the app: the body says what is going
on, and every button is in the one footer row, Cancel first and the primary last.

- The open project's un-uploaded work under our lock: the body says it has changes
  not on SpeleoDB yet, and the footer offers Cancel / Discard / Keep on this
  computer / Upload and disconnect (Upload and switch, Discard and switch for an account
  switch). Upload goes through the description step (Back / Upload and disconnect)
  and runs the normal upload, which releases. Discard goes through a "lost forever"
  step (Back / Discard and disconnect) that lists what is lost (unsaved changes,
  changes saved on this computer but not uploaded) and applies to the open project
  only. A discarding close that leaves something behind (the lock could not be
  handed back, or the copy could not be removed and stays reserved) shows its
  warning as a toast, as the exit prompt does, and the sign-out goes on. Upload is
  offered only while we hold the open project's lock.
- Other cached copies with un-uploaded work are always kept on this computer: the
  body names them (names only, no buttons) and says they can be opened and uploaded
  after connecting again. Their edits and un-uploaded flag survive, and a lock held
  on one is handed back with the others. The sign-out never discards another
  project's work.
- The remaining locks are listed as handed back. With nothing to answer for the open
  project, the footer is Cancel / Disconnect.

Every answer hands the locks back and then disconnects, with Try again / Disconnect
anyway (same footer row) when a release fails. There is no keep-the-lock answer.
The backend still guards `speleodb_discard_cached_copy` (which the sign-out no
longer calls): it needs the explicit `discard_unpushed` consent for a copy with
un-uploaded or unreadable work, and refuses a copy the signed-in account is not the
owner of whatever the flag says (owner recorded as another account or instance; an
owner-less copy from the signed-in instance counts as the account's, as on the home
screen). Keep on this computer is the non-destructive answer when the upload is not
possible: unsaved edits of the open project are saved first, the copy keeps its
edits, its un-uploaded mark and its recents row, and its lock is handed back
through the explicit per-project release (`speleodb_release_lock`). Confirming the
dialog, which says so, is the consent: the logout's own silent release still keeps
the lock over un-uploaded edits. After the same account's next sign-in the ordinary
open takes over (base equals tip: the local copy opens and resumes; a divergence
offers "Open my local copy"). Another account opening the project gets that prompt
instead.

Switch to Read Only Mode refuses unsaved or unuploaded work, including direct IPC
calls: "Save your changes using Save Changes before switching to read-only mode."
It never discards or uploads. The clean check and the switch to read-only happen
together, serialized against mutations, before the release request is sent; edits
attempted while it is in flight are refused as read-only rather than waiting. A
failed release returns the same session to editing. Consent is bound to the
project, account, session and edit version.
Only the signed-in account's lock can be released. The server has no atomic
compare-and-release operation; changes between the ownership probe and release
remain a protocol limitation. Offline release failures leave the project open.

Reload Project verifies live revision history without acquiring a lock. Pending
work requires Discard changes / Cancel, explaining that saving first preserves it.
Discard removes unsaved and unuploaded modifications. Historical checkouts and
explicit discard return to the server version even if BASE already equals the tip.
Equal clean revisions need no download. Reload preserves the existing editing mode;
a failed download or validation preserves the current tree and pending work.

Opening, Resume editing, held-lock resume and authentication recovery requesting
editing all use Start Editing. It acquires or confirms the lock, fetches fresh
revision history, resolves the tip from parent topology and reconciles BASE. Only
verified newer descendants are downloaded, by exact revision. Cached listing
metadata cannot prove freshness. Ambiguous history and unavailable verification
cannot open the editing gate. Status refresh may close editing on confirmed lock
or permission loss, but never enables editing. An already acquired session may
continue local editing through transient disconnection. Offline opening remains
read-only. In-place frontend reconciliation suppresses automatic acquisition.

An unreachable server or forcibly terminated process cannot guarantee server-side
release, and SpeleoDB never expires a lock on its own: a lock left held stays held
until the next sign-in's held-lock prompt or the SpeleoDB website releases it.
Close, Quit and Save-and-Close retain their separate lifecycle behavior.

## Edit gate and lock claims

The edit gate is not a stored flag. It is derived, on every read, from what is
open and from the lock claims this process holds
(`core/state/speleodb_edit_gate.rs`):

- A local project, or no project, is always editable.
- A SpeleoDB project is editable only while the signed-in account holds a claim
  on that project that is bound to the current session generation. Editing a
  SpeleoDB project without a held lock is therefore not a representable state.

A claim records which project this process holds the lock on, under which
account and deployment. It is authored when Start Editing's probe found the
mutex free (this process took it, which licenses the authorship recovery of a
landed push), and unauthored when the account already held the lock and Start
Editing inherited it.

- Opening the gate: only Start Editing's commit binds a claim to the session
  (Switch to Read Only re-binds the claim it took out when its release fails).
  An in-place reload (fast-forward, Reload, Restore) carries an existing binding
  into the generation it moves to and cannot create one. Status refresh never
  opens the gate and never drops a claim.
- Shutting the gate: releasing or dropping the claim, or unbinding it after a
  confirmed or unverifiable loss of the lock or permission. Every session
  boundary (close, open, create, logout, sign-in, a rejected token) moves the
  generation on, which leaves any binding stale, so the gate shuts without the
  boundary having to remember it.
- Take remote shuts the gate and releases the lock before it reloads the server
  version, so the project comes back read-only; the frontend reopens it without
  re-acquiring and offers Resume editing.
- A claim whose release failed is kept as an unbound pointer for a retry: the
  user's Retry in "Couldn't hand back the lock", or a later holder-checked
  release. It opens nothing.

Every release - automatic (close, project switch, quit, restart-to-update, logout,
account switch, unlink, Take remote, delete, Start Editing's unwind, the
stranded-claim cleanup) or explicit (Save Changes and release with nothing to
upload, the held-lock prompt's Release, Switch to Read Only) - first reads the lock
holder and sends the release only when the server names the signed-in account.
The automatic paths cap that read at 2 seconds so it fits inside the 5-second exit
and update-restart wait. Another account's lock is never released, even when the
signed-in account is an administrator, whom the server would let release anybody's.
When someone else or nobody holds the lock, the local claim ends, the gate shuts
and nothing is sent. A release reply that still names this account released
nothing and counts as a failure. When the holder cannot be read, nothing is sent
and the claim is kept: an explicit verb reports the failure (Save Changes stays in
editing), a close, switch or quit asks "Couldn't hand back the lock" (see Save,
switching and exit), the stranded-claim cleanup retries on a later status refresh
with no project open, and an exit the app cannot intercept leaves the lock held
for the next sign-in's held-lock prompt.
Automatic releases are decided from the claim, never from the gate, so a session
whose gate a lost lock or a sign-out already shut still ends its claim. The
un-uploaded-work rule above applies to all of them except the explicit discard
paths, and a claim deliberately kept (an earlier veto, Keep the lock, or the case
below) is left alone by every automatic release.

When Start Editing's fast-forward refuses to overwrite the managed cache copy
because it holds un-uploaded work the open copy does not show, the lock it just
took is kept, as before, and now also recorded as a kept claim, so it is no longer
held with nothing local pointing at it. Only an explicit Release, an abandon or a
new edit session ends it.

An acquire that fails with a transport error, a 5xx or an unreadable reply may
still have taken the lock. Start Editing then reads the holder again. If it is
the signed-in account, the acquire is adopted and editing opens normally. If it
is someone else, the contention dialog names them. If nobody holds it, the
original error is shown. If the holder still cannot be read, the error is shown
and a provisional, unbound claim is kept where the lock would otherwise have been
handed back, so the next close or the stranded-claim cleanup releases it after
reading the holder.

## Network activity and responsiveness

Tauri runs synchronous commands on the main thread, so a SpeleoDB network wait
inside one freezes the whole window. The SpeleoDB commands, the Settings save and
reload, and project deletion run their work off the main thread. Panel, column and gas settings writes never wait on SpeleoDB: only a
settings change that moves to another SpeleoDB instance joins the queue of lock
operations, and its lock release is sent after the settings lock is let go. While
an edit session is active, the lock check that runs at most every 20 minutes
happens in the background; if it finds the lock gone, the next edit is refused
and routed to the conflict prompt.

While any SpeleoDB network call is in flight, a small spinner beside the SpeleoDB
tab label says "Talking to SpeleoDB..."; it overlays the tab and never changes its
size. While a lock, upload, download or account call runs, the controls that would
start another one are disabled: the workspace's Save Changes, Resume editing /
Switch to Read Only Mode, Reload Project, Close Project and conversion, revision
View/Restore/Revert, opening from SpeleoDB, the held-lock
prompt's Resume and Release, publishing, removing a cached copy, "Purge all cached
files", and the SpeleoDB endpoint, connection and sign-in/sign-out controls in
Settings. Reads such as the status refresh show the
spinner but lock nothing. The map, panels, other settings and editing stay
available. Explicit uploads and publishing keep their app-blocking progress.

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

The Open from SpeleoDB browser cannot be dismissed (Escape, the X, a backdrop click
or Close) while an open it started is in flight, like the unpushed-local prompt. The
open's error is shown inline; a 401 signs out and its message stays on the login
prompt. If something else takes the dialog slot mid-open, the error is toasted
instead and a 401 still ends the session.
Cancelling read-only or locked-project warnings in the browser or managed-file prompt
restores retry and dismissal without an error. A replaced managed-file prompt retains
ownership of its own controls. A superseded discard-and-open in the unpushed-local
prompt similarly restores retry and dismissal without resetting a replacement prompt's
operation.

## Immediate SpeleoDB connection settings

Connect saves changed preferences and authenticates immediately, after the
account-change questions (see "Account sign-in" above). Success closes Settings and
refreshes the project listing. Disconnect runs the sign-out flow first (see "Signing
out"); once it goes ahead it takes effect immediately, clears the remote listing and
stays on the SpeleoDB Settings tab. Neither action
requires a subsequent Save. While connected, the server endpoint and both reset
controls are disabled; backend requests to change instances are also refused.
Disconnect first, including when the current server is temporarily unavailable.

The SpeleoDB tab has a Close button instead of Save/Cancel. Closing validates and
saves preference changes, including changes in other categories, without connecting.
Connection and persistence errors keep Settings open. The ordinary Settings
unsaved-changes prompt offers only Cancel (red) and Discard Changes (blue).

HTTPS is required except for HTTP to the exact host `localhost`, the exact IPv4
address `127.0.0.1`, or valid IPv4 addresses in `192.168.0.0/16` (see
Authentication and transport). Only a development build shows the "Allow insecure
HTTP" switch, next to the server and locked with it while connected. An endpoint
the policy refuses produces an error dialog when connecting, or when saving a new
server. Certificate verification and redirect restrictions remain.
