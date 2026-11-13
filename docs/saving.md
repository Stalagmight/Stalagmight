# Saving

## Overview

Stalagmight uses an atomic save strategy with a SQLite `.stalx` database file. Saves are triggered manually (Ctrl+S), automatically via a frontend debounced timer, or by the first import into a new project whose initial save was deferred (`finalize_deferred_import` in `src-tauri/src/commands/import.rs`). A failed deferred-import save degrades to a successful import carrying a warning, never a hard failure, so the imported data stays in memory under the close-guard. The save pipeline is optimized to return quickly by deferring non-critical work to background threads and avoiding unnecessary recomputation.

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

Every import-into-new-project flow now creates its project deferred, so the
`.stalx` appears on the first import rather than at the moment the name is typed
(#394 F14) - a wizard abandoned before then leaves no empty file and no Recent
Projects entry. Because that write happens on the BACKEND, the frontend has to be
told: `noteImportResolvedDeferredSave` (`projectStore.ts`) flips `databaseOpen`
after any successful import command unless the result carries the deferred save's
own `SYSTEM` failure or `DATABASE_NOT_REOPENED`. Without it only the DMP route
knew - it goes on to call `saveAfterImport` - and the TML/Compass/Walls/GPX
dialogs, which finish through `completeImportToSingleView`, left the DB-backed
surfaces (warning resolution, dive-plan persistence) disabled over a file that
existed.

## Schema version and pre-release files

The `.stalx` schema version lives in `SCHEMA_VERSION` (`persistence/sqlite.rs`);
opening an older file runs the migration chain in `ensure_schema_version`.

**`.stalx` files written before the first release are test artifacts, and no
migration path is provided for them.** Migrations exist for the shapes that ship;
a pre-release file that a bump cannot read is expected to be re-imported from its
source survey, not repaired.

## Per-shot survey credit

Surveyors, explorers and the survey date are stored per shot, in two side
tables that carry no schema bump (SpeleoDB's reader accepts exactly v18 and
ignores tables it does not know): `credit_people (id, name, email)` and
`shot_credits (section_id, shot_uuid, date, surveyors, explorers)`, keyed by
`(section_id, shot_uuid)`, the people columns
comma-joined `credit_people` ids. The legacy `sections.surveyor` and
`sections.date` are still written, as the section aggregate: the earliest shot
date, and the distinct surveyor names joined with `", "` (`"; "` for exactly
two one-word names, which the free-text `Last, First` rule would otherwise
read as one person) - or, when anyone explored the section, the Ariane
`"explorers / surveyors"` form, both halves name lists joined with `", "`
(`"Ida Ice /"` when nobody surveyed, which survives the trim an older build's
Edit Section does on save), with a `" / "` inside a name written `" - "`. A
`,` or `;` inside one name cannot come back as one person after an older
build's save; this build's per-shot rows keep it exact. A stored text
this build would not write the same way (a list longer than a role keeps, a
name past the length clamp, another spelling) is written back verbatim while
nobody's credit in the section has changed since the file was opened. A shot
WITHOUT a row reads its credit from those two columns through the legacy
splitter, which is the migration for older files; the writer therefore writes
a row only where that fallback would read back something else.

A released build (0.5.3 and older) saves by writing a fresh file and renaming
it over the project, so it drops the three credit tables: after such a save
every shot reads the legacy columns. That is why the surveyor column carries
both roles (#718): each section still lists its explorers and surveyors. What
collapses: per-shot differences, later dates and emails go to the section
aggregate; same-named people become one name; names come back as the legacy
text spells them (a `/` next to a space as `-`, a name with `,` or `;` as
several people, a placeholder-only name such as `?` or `unknown` as nobody);
and a 0.5.3 Ariane
TML export and re-import collapses identical halves (`"Jo Diver / Jo Diver"`
becomes the surveyor `Jo Diver` alone). Older Ariane imports stored only the
two-halves form, and only with two distinct, non-empty halves. A third table, `credit_sections (section_id, surveyor, date)`,
records the legacy pair this build wrote; when the tables survive next to a
pair retyped in place (a hand edit, another tool), a section whose pair no
longer matches the record ignores its rows (the retyped text wins). A credit
table of another shape gives no rows, with a warning, never a failed open; SQLite
failing for a reason of its own (busy, I/O, corrupt) still fails the open, so a
save cannot then drop the rows for good (#717). All three
tables are project data and ride in SpeleoDB pushes. See
`persistence/sqlite/shot_credits.rs`.

## Save Pipeline

### Backend (`save_project` in `project_io.rs`)

```
1. Lock project, resolve save path, ensure .stalx extension, refuse early if the
   destination folder is not reachable (see Failure classification below)
2. Load display settings from current database or in-memory cache
3. Write project data to temp file (file.stalx.tmp)
4. Copy display settings to temp database
5. Persist undo/redo stacks synchronously into the temp database
6. Close temp database, atomically rename to final path
7. Reopen database at final path
8. Update project.file_path
9. Swap in the new database handle
10. Update recent projects list in config
11. Return CommandResult with saved_path
```

Steps 1-6 are the critical path for data safety. Step 5 writes the undo/redo stacks while the file is still staged, so a partial rewrite is never renamed into place.

### `section_offsets` rows are never all-zero

The `section_offsets` table records sections that are **actually displaced**. A
value within `Project::OFFSET_DUST_M` (a micrometre) of the origin is not a
position and is never written - the invariant is stated on
`Project::write_section_offset` and explained in
[Undo/Redo](undo-redo.md#a-dust-zero-section_offsets-entry-is-unrepresentable-413-finding-27).
The save writes the in-memory map as it stands, so the rule holds on disk because
it holds in memory.

A file written by a build that predates the invariant can still carry such a row.
It is dropped at the load boundary (`Project::redrive_offsets_after_load`), so the
project opens without it and the **next save omits it** - a one-time settle, after
which the file is stable. Nothing observable changes: every consumer reads the map
through `Project::section_offset` or an inline `.get(..).unwrap_or((0.0, 0.0))`, so
an absent row and an all-zero one are the same geometry.

This does not disturb the file-format fixpoint property the format gate checks: the
app never writes such a row, so save-load-save on a file this app wrote is a
fixpoint, and a legacy file is one from its first save onward.

For a SpeleoDB-managed project given an explicit destination (Save a Copy, see
below), the pipeline diverges after step 5: the copy is written and renamed into
place, recorded in Recent Projects, and the function returns with
`CommandResult.unlinked_copy_path` set - steps 7, 8 and 9 are skipped on
purpose, because the session stays on the managed file.

### Frontend (`saveProject` in `projectSaveActions.ts`)

```
1. invoke('save_project', { path })
2. If unlinked_copy_path in result: toast the copy's location and stop - the
   open project was NOT saved (a detached copy was written elsewhere), so
   file_path and isDirty are left untouched
3. If saved_path in result, update project.file_path in store
4. refreshUndoState() - lightweight, just fetches can_undo/can_redo booleans
5. Clear isDirty only if no newer edits arrived during the save
```

Save completion checks the original project identity and load generation before
updating paths or dirty state. A late result cannot repaint a replacement project.

The frontend calls `refreshProjectState()` after a save ONLY when the project entered the save with integrity issues (the backend's save-time heals mutate the live project). A clean save skips it: it changes no computed data (stations, stats, warnings), so recomputing everything would be wasted work. The only state that changes is `file_path` (on first save or save-as), which the backend returns via `CommandResult.saved_path`. When the save produced a detached copy instead (see Save a Copy below), the path comes back in `unlinked_copy_path` and `saved_path` is absent.

## Atomic Save (Temp File + Rename)

All saves use a temp file with atomic rename to prevent data loss on crash or power failure:

1. Write to `file.stalx.tmp` (new SQLite database)
2. On success: `rename(file.stalx.tmp, file.stalx)` - atomic on POSIX, nearly atomic on Windows
3. On failure: delete `file.stalx.tmp`, original file untouched

If a leftover `.stalx.tmp` file exists from a previous failed save, it is removed before starting. Its rollback journal (`.stalx.tmp-journal`) is not, so the next save's fresh temp database can meet an orphaned hot journal - a state the crash-timing gate injects deliberately.

That gate (`src-tauri/crates/core/src/core/state/crash_property_tests.rs`, see [Testing Recipes](testing-recipes.md)) is what holds this promise up: it constructs the exact files a kill at each pipeline boundary would leave and asserts the destination comes back as EXACTLY the old project or EXACTLY the new one, never a blend, and that the next save recovers cleanly.

## Failure classification

Every failing save goes through `save_failure` (`project_io.rs`), which logs the
whole `anyhow` chain at WARN and prefixes the `CommandResult.error` with the
marker that tells the frontend which remediation to show. `src/utils/saveErrors.ts`
mirrors the three markers; `saveFeedback.ts` turns them into copy.

| Marker | Means | Surface |
|---|---|---|
| `FS_PERMISSION_DENIED` | The OS refuses a destination that IS there | `SavePermissionDialog` (folder-access guidance) |
| `FS_PATH_NOT_FOUND` | The folder is gone, or its drive is not connected | Error dialog, "the folder no longer exists or the drive is not connected" |
| `FS_PATH_TOO_LONG` | The path is past what the OS will open (Windows `MAX_PATH`) | Error dialog, "save to a shorter path" |
| (none) | Everything else - disk full, corruption | Error dialog, backend message verbatim |

SQLite reports ALL of these as the same errno-free "unable to open database
file" (`SQLITE_CANTOPEN`), so that message alone classifies nothing (#429). It is
corroborated instead: by the destination folder's own answer (gone, refuses a
probe file, or accepts one) and, on Windows, by the destination's length -
`MAX_PATH` binds SQLite's `CreateFileW` while Rust's std calls go through `\\?\`
verbatim paths and stat the parent perfectly happily.

## Save a Copy (SpeleoDB-managed projects)

A project carrying a `speleo_link` row lives at exactly one usable path: the
pull cache (`<cache>/speleodb/<project_id>/project.stalx` - see
`core/state/speleodb_foreign.rs` for the invariant). Save As on such a project
therefore does not relocate the session; it writes a detached copy:

- The copy carries display settings, resolved warnings and the undo/redo
  stacks, but NOT the `speleo_link` row - it is a plain unmanaged project.
- The session stays on the managed file: `project.file_path`, the database
  handle and the dirty state are untouched. The backend reports the copy via
  `CommandResult.unlinked_copy_path` and deliberately leaves `saved_path`
  unset so the frontend cannot re-point itself at the copy.
- The copy is added to Recent Projects.
- Saving a copy onto the managed file itself (its own path, or an alias of the
  cache path) is refused - that would strip the managed project's link in
  place.
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

Undo and redo stacks are persisted to the SQLite database synchronously on each save, using the same database connection.

### Flow

1. While the undo stack lock is held, entries are collected and serialized
2. Entries are written to the database synchronously (no background thread)
3. A failed write is logged via `tracing::warn`, both stacks are cleared, and the save result carries an `UNDO_HISTORY_DROPPED` warning; if clearing also fails, the save is aborted with `save_failure` and the temp file is removed

### Details

- Only the last N entries are persisted (configurable via `general.undo_persist_limit`, default 20) - EXCEPT for a Full-mode SpeleoDB project, which persists its whole history since the last commit under a byte budget instead (#303)
- Each entry is stored as `(command_json, undo_data_json, timestamp_iso8601)`
- Serialization uses gzip compression
- A save rewrites the WHOLE persisted stack, every time - it has to, because the pipeline above builds a fresh temp file and renames it over the target, so anything a save skips is gone (#303). The byte budget is therefore a latency bound as much as a storage one; see [Undo/Redo](undo-redo.md)
- On project open, persisted stacks are restored from the database
- See [Undo/Redo](undo-redo.md) for the full undo system documentation, including the hybrid SpeleoDB mode and when the stack is cleared

## CommandResult.saved_path

The `save_project` backend command returns the save path as a string in `CommandResult.saved_path`. This allows the frontend to update `project.file_path` without a full state refresh. A Save a Copy on a managed project returns `unlinked_copy_path` instead and leaves `saved_path` absent, precisely so this update cannot fire.

- Present on successful saves (regular save and save-as)
- Absent (`None`/`undefined`) on all other command results
- Serialized with `#[serde(skip_serializing_if = "Option::is_none")]` to keep other results compact

## Key Files

| File | Purpose |
|------|---------|
| `src-tauri/crates/core/src/core/state/project_io.rs` | Backend `save_project`, atomic save logic (`open_project` lives in `project_load.rs`) |
| `src-tauri/crates/core/src/core/command.rs` | `CommandResult` with `saved_path` field |
| `src/store/projectSaveActions.ts` | Frontend `saveProject()` action and completion ownership |
| `src/hooks/useAutoSave.ts` | Debounced auto-save hook |
| `src-tauri/crates/persistence/src/persistence/sqlite.rs` | SQLite database operations |
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
