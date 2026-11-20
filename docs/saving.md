# Saving

## Overview

A project is one `.stalx` file in format version 2: canonical, line-oriented
JSON ([file-format.md](file-format.md)). The app keeps no file handle open:
the survey lives in memory (`AppState::project`) beside what else the file
holds (`FileState`, `core/state/open_file.rs`), and every write builds the
whole file in memory and replaces the old one atomically. Older files are
SQLite databases with the same extension ("legacy"); they are read through the
frozen 0.6.0 reader and converted in memory, and the first save writes
version 2 over them after keeping the original (see
[Upgrading a legacy file](#upgrading-a-legacy-file)).

Saves are triggered manually (Ctrl+S), automatically via a frontend debounced
timer, or by the first import into a new project whose initial save was
deferred (`finalize_deferred_import` in `src-tauri/src/commands/import.rs`). A
failed deferred-import save degrades to a successful import carrying a
warning, never a hard failure, so the imported data stays in memory under the
close-guard. Some state is saved at once without a save of the survey: the
SpeleoDB sync state and the display toggles (see
[Local-only durable saves](#local-only-durable-saves)).

Menu Save, Cmd/Ctrl+S and the SpeleoDB workspace's Save Changes action share
`dispatchProjectSave`. Local projects use the existing local save flow. Linked
projects collect a required change description, save locally, then upload only with
fresh permission and editing-lock verification. Missing locks show an explanation
and recovery controls; recovery never uploads automatically. A clean linked project
reports No changes to upload. Offline and uncertain uploads never report success.
Ordinary successful upload retains the editing lock. Save Changes, menu Save and
Cmd/Ctrl+S share a centered, app-blocking progress indicator during the upload,
after the required description and successful local save. The indicator expires
after 120 seconds and explains that the request may still finish. The dialog can
then be closed; the description is retained and duplicate submission stays disabled
while that request is pending. Failure removes the indicator and keeps retry details.

Autosave and failed-autosave retry are always local-only, including while an upload
dialog is open or cancelled. Save and Close releases and closes only after confirmed
success with no newer edits. Cancel, conflict, uncertainty and release failure leave
the project open. Plain Close, project switching and Quit retain automatic lock
release without upload and Discard changes / Cancel protection for unsaved work,
including unsent SpeleoDB form drafts (asked first, in-app). Saved unuploaded work
remains local. See
[SpeleoDB integration](SpeleoDB.md) for navigation, ownership and offline behavior.

Switch to Read Only Mode requires a clean local file and no unuploaded work. It
refuses pending changes with Save Changes guidance and performs no implicit upload
or discard. Reload Project offers Discard changes / Cancel for unsaved or unuploaded
work and explains that saving first preserves it. Reload checks fresh revision
history and returns historical checkouts to the server version without acquiring a
lock or changing editing mode. Failure preserves current work. Consent and release
are guarded against project/account changes and newer edits.

Sign-in and connection management open application Settings on SpeleoDB. The Save
description and Save-and-Close intent survive recovery for the originating project
and account. Returning from Settings reopens confirmation; it never saves or uploads
automatically. Local settings has no SpeleoDB metadata or account controls.

Converting a local project through its SpeleoDB tab opens the creation form. Once
confirmed, publishing saves the latest survey into its managed cache and uploads
it. Only a clean confirmed upload is downloaded again from SpeleoDB and reopened;
newer edits and unconfirmed uploads remain protected for Save Changes. A failed
fresh download retains the already-created managed project for Reload Project,
so recovery does not repeat the server creation. The original local file remains
a separate local copy. Account recovery returns to the form without publishing.

Every import-into-new-project flow creates its project deferred, so the
`.stalx` appears on the first import rather than at the moment the name is
typed (#394 F14) - a wizard abandoned before then leaves no empty file and no
Recent Projects entry. Because that write happens on the BACKEND, the frontend
has to be told: `noteImportResolvedDeferredSave` (`projectStore.ts`) flips
`databaseOpen` (the store's name for "the project has a file") after any
successful import command unless the result carries the deferred save's own
`SYSTEM` failure. Without it only the DMP route knew - it goes on to call
`saveAfterImport` - and the TML/Compass/Walls/GPX dialogs, which finish
through `completeImportToSingleView`, left the file-backed surfaces (warning
resolution, dive-plan persistence) disabled over a file that existed.

## Opening a project

`project_load.rs` reads the file once, remembers its fingerprint (size,
modification time and SHA-256: the save guard below), and sniffs it
(`stalx_v2::sniff`):

- **Version 2.** `stalx_v2::decode` with the reader's size and count limits,
  checking `format`, `version`, `conventions` and `requires`, then
  `Project::validate`.
  - A higher `version` gets the existing "made with a newer version" prompt
    (`NEWER_SCHEMA`); the file is not touched.
  - An unknown `requires` feature, or an unknown key in a survey record, opens
    the project **read-only** (see [Read-only files](#read-only-files)).
  - A survey that breaks an invariant is refused through the corrupt route
    ("cannot be opened normally"), which offers Attempt Repair (see
    [SPEC 14.4](SPEC.md#144-file-corruption-recovery)).
  - Each history entry is decoded; if any one fails, the whole history is
    dropped and the history-discarded banner says so (an entry of an earlier
    history version: "written by an earlier version"; anything else: "could
    not be read").
- **Legacy SQLite.** `migrate::migrate_path_for_app` opens a private copy
  through the frozen 0.6.0 reader (`crates/legacy`), with the user's
  least-squares config, converts it and verifies every slot's position
  against 0.6.0's. Validation thresholds the file never stored are seeded from
  the global config. The history is always dropped, with the
  "earlier version" banner. When the verification found stations that moved
  (a torn legacy merge collapsed into one station) or anything unexplained,
  a one-time conversion notice is kept: "Converted from the old file format; N
  stations moved by up to X m; the original is kept in Recovery."
  (`AppState::take_conversion_notice`, shown as a banner).

Load is pure: the project in memory is exactly what the file says. Nothing is
healed, re-solved or rewritten on open; a legacy file stays untouched until a
save.

## Save Pipeline

### Backend (`save_with` in `project_io.rs`)

```
1. Lock project; resolve the destination (the open file, or an explicit path,
   which always gets the .stalx extension); refuse early if the destination
   folder is not reachable (FS_PATH_NOT_FOUND, see Failure classification)
2. validate(): a survey that breaks an invariant is refused ("Can't save: ..."),
   the project in memory is kept
3. Refuse a read-only file (READ_ONLY_FILE)
4. Build the file in memory: stalx_v2::encode(project, extras, mode) with the
   generator "Stalagmight <version>" and the persisted window of the history;
   mode is DetachedCopy for an explicit destination of a SpeleoDB project
   (Save a Copy), Normal otherwise
5. Pick the guard: the open file must still hold what was last read or
   written; a project never written expects nothing at its path; an explicit
   Save As destination is overwritten (the dialog confirmed)
6. First save of a legacy file: keep the original in the backups folder
7. write_atomic: <file>.tmp, sync, guard check, rename, folder sync
8. Update file_path, the fingerprint, origin (now version 2), clear the dirty
   and local_pending flags, remember the history just written
9. Update recent projects in config
10. Rewrite a migrated project's 0.6.0 dive-plan pins (never fails the save)
11. Return CommandResult with saved_path (or unlinked_copy_path)
```

Nothing is written before step 7, and step 7 either replaces the file whole or
leaves it as it was.

The first implicit save of a project never written (a deferred new project's
reserved path) moves to the next free name (`<name>_1.stalx`, ...) when a file
appeared at that path meanwhile, and writes over an empty file there (Android's
document picker creates one before it returns the path).

### An unchanged project saves to the same bytes

The format is canonical: the same model gives the same bytes
([file-format.md section 8](file-format.md#8-canonical-form)), so saving,
opening and saving again with no edit writes the file byte for byte the same,
history included. SpeleoDB keeps every push in git, so a line that changes for
no reason is a line every push uploads and stores. A one-shot edit changes a
handful of lines (the shot, `summary`, `project.modified`, one history entry).
Times are RFC 3339 UTC with nanoseconds, kept in years 0001-9999 (a `.plnx`
matches its project by the `created_at` instant).

The format gates pin this: `persistence/stalx_v2/format_v2_canonical_tests.rs`
and `format_v2_roundtrip_tests.rs` on the codec, and the AppState-level
file-format gate (`core/state/format_property_tests.rs`, see
[Testing Recipes](testing-recipes.md#the-file-format-gate)) on save,
open and save again.

### Frontend (`saveProject` in `projectSaveActions.ts`)

```
1. invoke('save_project', { path })  (or 'save_project_overwriting', below)
2. If unlinked_copy_path in result: toast the copy's location and stop - the
   open project was NOT saved (a detached copy was written elsewhere), so
   file_path and isDirty are left untouched
3. If saved_path in result, update project.file_path in store
4. refreshUndoState() - lightweight, just fetches can_undo/can_redo booleans
5. Clear isDirty only if no newer edits arrived during the save
```

Save completion checks the original project identity and load generation before
updating paths or dirty state. A late result cannot repaint a replacement project.

A save never changes the survey (no save-time heals), so it needs no
`refreshProjectState()`; the store still refreshes after a save of a project
the integrity check had flagged, as a cheap safety net. The only state that
changes is `file_path` (on first save or Save As), which the backend returns
via `CommandResult.saved_path`. When the save produced a detached copy instead
(see Save a Copy below), the path comes back in `unlinked_copy_path` and
`saved_path` is absent.

## Atomic Save (Temp File + Rename)

Every write of a project file, the local-only ones included, goes through
`stalx_v2::write_atomic` (`persistence/stalx_v2/atomic.rs`):

1. A leftover `<file>.tmp` from an interrupted save is removed (the name is
   ours; a symbolic link there is removed, never followed)
2. Write `<file>.tmp` beside the destination (on Unix with the destination's
   permissions) and `sync_all` it: one data sync
3. Check the guard (below)
4. `rename(<file>.tmp, <file>)` - atomic on POSIX, nearly atomic on Windows; no
   handle on the destination is open across it
5. Sync the folder (Unix; a failure is logged, not returned: the file is
   already in place. The warning is logged once per run, later failures at
   debug level, so a network or removable volume does not repeat it on every
   save)
6. On any failure before the rename: delete the temp; the destination is
   untouched

A symbolic link at the project's path is replaced by the saved file, not
written through. A torn temp never reads as a project: the format's last
member is `"end":true`, so a truncated file is refused, and the recovery
dialog never offers one (it offers a complete temp, which restores as the
interrupted save).

The crash-timing gate (`src-tauri/crates/core/src/core/state/crash_property_tests.rs`,
see [Testing Recipes](testing-recipes.md)) holds this promise up: it builds the
exact files a kill at each step would leave, the legacy upgrade's steps
included, and asserts the destination comes back as EXACTLY the old project or
EXACTLY the new one, and that the next save succeeds.

## The external-modification guard

The open file's `FileState` remembers the fingerprint of the bytes last read or
written. Just before the rename, a destination whose length or SHA-256 differs
(another program, a sync client, a second Stalagmight wrote it, or it was moved
or deleted) refuses the save with `EXTERNALLY_MODIFIED: <path> was changed by
another program since it was opened`; nothing is written and the edits stay in
memory. A touch or a rewrite with the same bytes is no change. The
local-only saves are guarded the same way.

The frontend asks (`SaveConflictDialog`, a modal: data is at risk):

- **Keep my version**: `save_project_overwriting` writes over the changed file
  (`AppState::save_project_overwriting`, the guard skipped);
- **Save as a new file**: the Save As flow, which leaves the changed file alone
  and moves the session to the new one;
- **Use the file's version**: reopen the file as it is now, discarding the
  unsaved changes;
- **Decide later**: keep editing.

A SpeleoDB-managed copy gets only Keep my version: its reload belongs to the
sync. An autosave refused this way raises no modal: the autosave banner says
the file was changed by another program and autosave pauses until a save
lands; the banner's Save opens the prompt.

## Read-only files

A version 2 file this build cannot rewrite without losing something - an
unknown `requires` feature, or an unknown key in a survey record (stations,
lines, shots, datums, placements, closures, Sections) - opens read-only, with
the "made with a newer Stalagmight" banner. The backend refuses every edit,
undo, redo and save of it with `READ_ONLY_FILE: ...`; the frontend's
`useEditingBlocked` reports it as a project-level block, a refused save is
worded for it (never routed to Review & Repair), and autosave pauses. The
local-only durable saves still work on it and copy the survey members byte for
byte.

## Upgrading a legacy file

The first save of a migrated legacy file (its origin is `Legacy` in
`FileState`) writes version 2 over it. Before the write,
`UpgradeBackup::take` (`project_io.rs`):

1. copies the SQLite original to `<app data>/backups/<stem>.<UTC stamp>.pre-v2.stalx`;
2. MOVES any `-journal` / `-wal` / `-shm` sidecar beside that copy, so no old
   build can later roll a stale journal into the JSON file.

If the write fails, the sidecars are moved back beside the untouched original.
The Recovery dialog lists the kept originals with the project's own backups
(`list_project_backups`), and Restore puts one back. The save itself is
silent; the conversion notice (above) was shown at open when the conversion
moved anything. Until that first save nothing is written to a legacy file:
display toggles and SpeleoDB state changes wait in memory (`local_pending`)
and ride the upgrading save.

Other writes to a closed legacy file upgrade it the same way, with the same
backup: a Recents rename (`set_project_name_for_path`) and the removal of a
foreign copy's SpeleoDB link (`speleodb_remove_link_at`).

## Local-only durable saves

`AppState::persist_local_state` rewrites the open file's `speleodb`, `local`
and `history` members and copies every other member of the file on disk
verbatim (`stalx_v2::rewrite_local`), behind the guard. Used at once, so these
changes are durable without saving the survey:

- every SpeleoDB state change: a push result, the dirty bit and `last_edit_at`
  of a linked project's edit, a pull install, the Unwind chain and watermark,
  the verify stamp, the history cut after a push, logout bookkeeping;
- display-setting toggles (`display_settings.rs`).

A Recents rename of the open project is durable at once in the same way, but
rewrites only the name (`stalx_v2::rename_project` behind the guard,
`recent_projects.rs`); it is not an undo step, and unsaved survey edits stay
unsaved.

While the survey has unsaved changes, a local-only save writes the history the
file last held, never the in-memory stacks (they describe a body the file does
not have). It works on read-only files. On a legacy file or a project never
saved it only marks `local_pending`; the next real save writes it. A caller
that already holds the `project` guard uses `persist_local_state_at`
(`Mutex` is not reentrant).

## Unsaved changes

`FileState.dirty` is the backend's "changes no save has written":
`AppState::has_unsaved_changes()` (IPC `has_unsaved_changes`). It is set by
every recorded edit, by undo and redo, and by an Ignore or Unignore of a
warning (ignored warnings are survey content: they ride the next save, and
"Don't save" discards them). It is not set by display toggles or SpeleoDB
state changes, which are durable at once. A save clears it. The frontend keeps
its own `isDirty` for the indicator and the close guard, and asks the backend
after an Ignore.

## Failure classification

Every failing save goes through `save_failure` (`project_io.rs`), which logs the
whole `anyhow` chain at WARN and prefixes the `CommandResult.error` with the
marker that tells the frontend which remediation to show. `src/utils/saveErrors.ts`
mirrors the markers; `saveFeedback.ts` turns them into copy.

| Marker | Means | Surface |
|---|---|---|
| `FS_PERMISSION_DENIED` | The OS refuses a destination that IS there | `SavePermissionDialog` (folder-access guidance) |
| `FS_PATH_NOT_FOUND` | The folder is gone, or its drive is not connected | Error dialog, "the folder no longer exists or the drive is not connected" |
| `FS_PATH_TOO_LONG` | The path is past what the OS will open (Windows `MAX_PATH`) | Error dialog, "save to a shorter path" |
| `EXTERNALLY_MODIFIED` | The guard refused: the file changed on disk | `SaveConflictDialog` (manual save), the autosave banner (autosave) |
| `READ_ONLY_FILE` | The file was made with a newer Stalagmight | The read-only wording; never Review & Repair |
| (none) | Everything else - disk full, an invalid survey | Error dialog, backend message verbatim |

The version 2 writer reports typed I/O errors with the step that failed
(creating, writing or flushing the temp, checking the file on disk, moving
the new file into place), so a permission denial or a missing path classifies
from the error chain itself. The destination-folder probe (gone, refuses a
probe file, or accepts one) and the Windows `MAX_PATH` measure remain for an
error that names neither (#429).

## Save a Copy (SpeleoDB-managed projects)

A project carrying a SpeleoDB link lives at exactly one usable path: the
pull cache (`<cache>/speleodb/<project_id>/project.stalx` - see
`core/state/speleodb_foreign.rs` for the invariant). Save As on such a project
therefore does not relocate the session; it writes a detached copy
(`Mode::DetachedCopy`):

- The copy carries the survey, display settings, ignored warnings and the
  undo/redo stacks, but no `speleodb` member, no SpeleoDB keys in `local` and
  no undo entries inherited from the server - it is a plain unmanaged project.
- The session stays on the managed file: `project.file_path`, the guard and
  the dirty state are untouched. The backend reports the copy via
  `CommandResult.unlinked_copy_path` and deliberately leaves `saved_path`
  unset so the frontend cannot re-point itself at the copy.
- The copy is added to Recent Projects.
- An explicit destination that IS the managed file is an ordinary save of it.
- Implicit saves (Ctrl+S, auto-save, the pre-push flush) are unaffected: they
  carry the link forward as always.

Opening a linked `.stalx` from any non-canonical path is refused by
`do_load_project_full` with a structured `SPELEODB_MANAGED` error; the frontend
raises `SpeleoManagedFileDialog`, offering to open the managed copy from
SpeleoDB or to remove the file's link and open it as a local project.

## Auto-Save

Auto-save is handled entirely in the frontend via the `useAutoSave` hook (`src/hooks/useAutoSave.ts`). There is no per-command auto-save in the backend.

### How It Works

1. The hook subscribes to `isDirty` changes in the project store
2. When `isDirty` becomes true (any modifying command), a debounce timer starts
3. Each subsequent change resets the timer
4. When the timer fires, `saveProject()` is called
5. On success, `isDirty` is set to false, which cancels the timer and briefly shows "Saved"

### Guards

Auto-save only fires when all of these are true:
- `config.general.auto_save` is enabled
- The project has a `file_path` (has been saved at least once)
- `isDirty` is true
- No save is already in progress
- It is not paused: a save refused as `EXTERNALLY_MODIFIED` or `READ_ONLY_FILE`
  pauses it (retrying would fail the same way) until a save lands or the
  failure banner is dismissed; an integrity save-block pauses it until the
  block clears

### Configuration

```toml
[general]
auto_save = true                    # Enable/disable auto-save
auto_save_interval_seconds = 30     # Debounce delay in seconds
```

### Status Indicator

The `useAutoSave` hook exposes an `AutoSaveState` with:
- `status`: `'idle'` | `'pending'` | `'saving'` | `'saved'` | `'error'`
- `countdown`: seconds remaining until auto-save fires
- `cancelPending()`: cancel the current pending auto-save

A manual Ctrl+S while a timer is pending cancels the auto-save timer (since `isDirty` goes false).

## Undo/Redo Persistence

Both stacks ride the file's `history` member
([file-format.md 5.15](file-format.md#515-history)), one entry per line, each
the canonical text the stack already holds (`UndoEntry`, `core/undo.rs`), so a
save copies text and re-encodes nothing.

- **What a save keeps** (`AppState::history_to_persist`): the newest
  `general.undo_persist_limit` (default 20) entries of each stack on a local
  project or a SpeleoDB project in Disabled mode; both stacks whole in Full
  mode, where the in-memory depth is the byte budget instead
  (`UNDO_BYTE_BUDGET_BYTES`, 4 MiB of persisted JSON over both stacks, oldest
  first). In memory a local project keeps `general.undo_history_limit`
  (default 100) entries per stack.
- **On open** every entry is decoded as untrusted input (size, version, strict
  layout, structural rules); one that fails drops the whole history with the
  banner, because an entry is only meaningful on the exact state the one
  before it left. An entry from the file has no session shot ids; its steps
  mint them.
- **Pushes** strip redo always and send undo only in Full mode, cut at the
  inherited-history watermark and then by bytes so the file fits the 5 MiB
  upload limit (newest entries kept).
- **A legacy file's** history is dropped on open (plan 7.2.8); a detached copy
  carries no history inherited from the server.
- On project close, stacks are cleared from memory.
- See [Undo/Redo](undo-redo.md) for the engine, the hybrid SpeleoDB mode and
  when the stack is cleared.

## Persisted names

The code calls a continuous run of shots a **Line** and a named grouping of
lines a **Section** (#327). The version 2 file uses the same words: `lines`
are lines and `sections` are Sections. A legacy SQLite file used the
pre-rename names (its `sections` table holds the lines, `cave_sections` the
Sections); only the frozen reader reads them now. Two places keep a pre-rename
spelling on purpose:

- **Display settings.** The Line names toggle is `showLineNames` in the store,
  but its key in `local.display` stays `showSectionNames`
  (`src/store/displaySettingKeys.ts`; the backend reads and writes it under
  that name in `core/loading.rs` and `core/state/project_create.rs`), so a
  migrated file keeps its toggle.
- **The dive-plan sidecar.** The `.plnx` JSON goes through `models::disk_names`
  (`core/state/dive_plan.rs`), which renames `line_id` <-> `section_id` and the
  other listed keys and tags on the way in and out.

## Legacy files and pre-release files

A legacy file is read by `crates/legacy` exactly as the 0.6.0 app opened it:
its schema migration chain, its side tables (station names and Station IDs,
per-shot credit, Sections, GPS fixes and accuracies) and its load pipeline,
then converted by `persistence::migrate` ([plan section 7](plans/2026-11-20_station-graph-model.md#7-migration-from-sqlite-legacy)).
The app never writes SQLite.

**`.stalx` files written before the first release are test artifacts, and no
migration path is provided for them.** A pre-release file that the frozen
reader cannot read is expected to be re-imported from its source survey, not
repaired.

## CommandResult.saved_path

The `save_project` backend command returns the save path as a string in `CommandResult.saved_path`. This allows the frontend to update `project.file_path` without a full state refresh. A Save a Copy on a managed project returns `unlinked_copy_path` instead and leaves `saved_path` absent, precisely so this update cannot fire.

- Present on successful saves (regular save and save-as)
- Absent (`None`/`undefined`) on all other command results
- Serialized with `#[serde(skip_serializing_if = "Option::is_none")]` to keep other results compact

## Key Files

| File | Purpose |
|------|---------|
| `src-tauri/crates/core/src/core/state/project_io.rs` | Backend save, the guard choice, the legacy upgrade backup, close |
| `src-tauri/crates/core/src/core/state/project_load.rs` | Open: sniff, decode or migrate, commit |
| `src-tauri/crates/core/src/core/state/open_file.rs` | `FileState`, the local-only durable save, the conversion notice |
| `src-tauri/crates/persistence/src/persistence/stalx_v2/atomic.rs` | `write_atomic`, `Fingerprint`, `Guard` |
| `src-tauri/crates/persistence/src/persistence/stalx_v2/encode.rs` | The serializer and its modes (normal, for push, detached copy) |
| `src-tauri/crates/persistence/src/persistence/migrate/mod.rs` | Legacy open and conversion |
| `src-tauri/crates/core/src/core/command.rs` | `CommandResult` with `saved_path` field |
| `src/store/projectSaveActions.ts` | Frontend `saveProject()` action and completion ownership |
| `src/components/Dialogs/SaveConflictDialog.tsx` | The prompt for a file changed on disk |
| `src/hooks/useAutoSave.ts` | Debounced auto-save hook |
| `docs/file-format.md` | The file format |
| `docs/undo-redo.md` | Undo/redo stack persistence details |
| `docs/configuration.md` | Auto-save config options |

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

### Confirmed project close

Hamburger Menu -> Close Project and the SpeleoDB workspace Close action share the
same close command. A clean close releases the lock only when SpeleoDB names this
account as its holder; over un-uploaded work the exit prompt decides (Upload / Keep
the lock / Discard / Cancel) and the backend never releases without the confirmed
discard. A close the backend reports as failed keeps the project open with a visible
error and pauses editing until the link status is refreshed. A release that fails
or is uncertain closes nothing: the claim stays unbound (editing paused) and the
in-app "Couldn't hand back the lock" dialog asks - Retry (the holder-checked
release again), Close and keep the lock (the lock waits for the next sign-in's
held-lock prompt) or Cancel (stay, read-only). Project switches and quits ask the
same question (see [SpeleoDB](SpeleoDB.md#save-switching-and-exit)). Local files
and saved unpublished changes remain intact. The API has no atomic
compare-and-release, so a server-side ownership change between the holder check and
the POST remains a protocol limitation.
