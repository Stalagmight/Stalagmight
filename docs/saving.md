# Saving

## Overview

Stalagmight uses an atomic save strategy with a SQLite `.stalx` database file. Saves are triggered manually (Ctrl+S), automatically via a frontend debounced timer, or by the first import into a new project whose initial save was deferred (`finalize_deferred_import` in `src-tauri/src/commands/import.rs`). A failed deferred-import save degrades to a successful import carrying a warning, never a hard failure, so the imported data stays in memory under the close-guard. The save pipeline is optimized to return quickly by deferring non-critical work to background threads and avoiding unnecessary recomputation.

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

The floor is schema 2 (`OLDEST_SUPPORTED_SCHEMA_VERSION`), the schema the first
public release (v0.0.3) shipped and stamped. A file whose project row claims a
schema below 2, with no `PRAGMA user_version` of 2 or more to contradict it (no
schema-1 build ever wrote the stamp), is refused before anything is written to it,
with the `PRE_RELEASE_FILE` load error and a message that says to create a new
project and import the survey again (#576 B1-D1). A file with no project row at all
is not refused: that is an empty file about to become a new project (Android's
document picker leaves a 0-byte placeholder at the chosen path). There
used to be a frozen "rebuild with foreign keys" step for these files; it had fallen
behind the schema and silently dropped columns and values, so it was removed
rather than kept in step with every future column.

`migrate_schema_from_the_oldest_supported_file_matches_a_fresh_file` (persistence
tests) guards the chain from that floor: it builds the v0.0.3 schema-2 tables,
opens them, and asserts every table's columns equal a freshly created file's, then
saves and reloads the migrated project. A column added to `init_schema` without the
matching `migrate_schema` step fails it.

### Files saved by an older version: upgraded on open, one copy kept

Opening a file stamped below `SCHEMA_VERSION` (and at or above the floor) upgrades
it in place, and the upgrade is one-way: several steps drop the persisted undo
history, and v12 rewrote every pitch. So before the first change,
`ensure_schema_version` keeps **one copy of the file as it was**, beside it, as
`<name>.stalx.pre-v<old>.backup` (#576 B6-D2). It is never overwritten: a later
upgrade from the same version keeps the first copy. The Restore list of the
recovery dialog offers it with the other backups. No copy is taken of a file that
is already current, of an empty file becoming a new project, or of an unstamped
file whose project row claims the current version (nothing is stamped there, so it
is not an upgrade). Nor by `Database::open_existing_untrusted`, the open the
`stalx-reader` crate uses: its file is already a private copy that is thrown away
after the read, and a second one beside it doubled the temp space a read needs. If the copy cannot be written, the file is not upgraded and the
open fails with a message saying so. A file inside the app's own folders or a
system folder is not upgraded at all: the open is refused before the chain starts
(load error `UPGRADE_REFUSED`), the file is left as it was, and the message says to
copy the project somewhere writable and open the copy (#576 RV4-R-M3-OPEN; see
docs/architecture.md, "IPC trust boundary").

This copy, the restore's staging and check copies and the repair's numbered
backup are each written as a new file, never through a link left at their name
(#576 RVF-b-1, `copy_to_new_file`). On macOS that makes them full byte copies
where APFS used to clone the file for free, so each costs the project's size in
disk space and time, and on a nearly full disk a large project's upgrade, repair
or restore can be refused for space it did not need before (#576 RVF-d-11).

The whole chain - `migrate_schema`, every version arm and the stamp - runs in ONE
`BEGIN IMMEDIATE` transaction (#576 B6), so an upgrade lands completely or not at
all: a step that fails part-way, or a crash, leaves the file as unmigrated as it
was, and the next open runs the chain again. The copy is taken inside that
transaction before its first write, so nothing else can change the file in
between. The v9 to v10 `speleo_link.last_edit_at` backfill also runs from its
version arm, not only when the column is missing, so a file left between that
ALTER and its backfill by a crash in an older build still gets it.

### Files saved by a newer version: open read-only

A file whose `PRAGMA user_version` is ABOVE this build's `SCHEMA_VERSION` was saved
by a newer Stalagmight. This build cannot know what that version stores, so it
opens the project **read-only** and never writes the file (#576 B6-D1):

- **The open writes nothing.** `ensure_schema_version` runs no migration and no
  undo purge on it, the load path skips its load-time backfills, and the session
  holds the file through a read-only SQLite connection
  (`Database::into_read_only`), so not even the deliberately ungated display-setting
  writes reach it. The file's undo history stays in the file for the build that
  wrote it; none of it is restored into this build's undo stack, whose payloads no
  migration arm here has vetted.
- **Editing is blocked.** The load commits `AppState::newer_file_schema`, and
  `guard_mutation` refuses every edit with the code `NEWER_FILE_EDIT_DISABLED`
  ahead of any SpeleoDB lock state. The frontend gets the same verdict on the load
  result (`newerSchemaVersion`) and composes it into the canonical edit gate
  (`useEditingBlocked`, see [app-modes.md](app-modes.md)), and a banner says why:
  "This project was saved by a newer version of Stalagmight. Update Stalagmight to edit it."
- **Every save is refused** - in place, Save As, the pre-push flush - with the
  `NEWER_FILE_READ_ONLY` code, which `surfaceSaveFailure` turns into a warning
  toast. Auto-save never attempts one. Writing the file at this build's older
  schema is exactly the silent downgrade this exists to prevent, and a Save As is
  the same downgrade under another name.
- **No SpeleoDB push either.** Start editing, force edit and Publish are refused
  before any network call, and the upload itself refuses as a backstop.
- **Nor a merge that reads it.** Merge Projects writes its output at this build's
  schema, so a newer file among the sources is refused (whether or not it is open)
  before any output is written or any source archived.
- **A SpeleoDB download of one** opens this way only from a first-time Open in the
  SpeleoDB list: no linked copy is there, so the file is installed without a
  SpeleoDB link and opens read-only (#576 RV4-R-S7-D2). A re-open over a linked
  copy, a Start editing / Update / Take remote fast-forward and a revision View /
  Restore would replace that copy, so each refuses the download before the swap
  with `NEWER_FILE_READ_ONLY` and the linked copy stays as it was (RV4-R-S7-D1).
- **Viewing and export work** as for any read-only project, and closing it or
  opening another project ends the read-only state.

Which file counts as newer is decided by the stamp alone, the same one
`ensure_schema_version` keys on. An unstamped file whose project row merely claims
a future version is not one any build writes (#485) and keeps its existing
handling.

## Save Pipeline

### Backend (`save_project` in `project_io.rs`)

```
0. Under command_lock, refuse with PROJECT_CHANGED when the save was made for
   another project than the open one (its `expected_project`, #576 FZ-FZ-INT-4;
   docs/architecture.md, "Where commands run") - nothing is written. The saves a
   SpeleoDB verb runs itself (Unlink's Save As, the pre-push save, Publish's
   relocation) check the verb's own `expected_project` the same way (#576 RVF-c-C1)
1. Lock project, resolve save path, ensure .stalx extension, refuse early if the
   destination folder is not reachable (see Failure classification below)
2. Snapshot the side tables from the open file and resolve the save mode
   (see Side tables and save modes below) - a refusal here writes nothing
3. Write project data to temp file (file.stalx.tmp)
4. Write the side tables into the temp database, the undo/redo stacks last
5. Close temp database, atomically rename to final path
6. Reopen database at final path
7. Update project.file_path
8. Swap in the new database handle
9. Update recent projects list in config
10. Return a SaveOutcome with saved_path, plus a warning for anything the save
    went ahead without (a failure is a CommandError with its code)
```

Steps 1-5 are the critical path for data safety. Step 4 writes every side table, the undo/redo stacks included, while the file is still staged, so a partial rewrite is never renamed into place.

### Side tables and save modes

`Database::save_project` rebuilds exactly the tables in `SAVE_PROJECT_TABLES`
(`persistence/sqlite/write.rs`) from the `Project` model. Every other table lives
outside the model, and because the save writes a FRESH temp file, each one has to
be carried by hand: snapshotted from the open file in step 2, written into the temp
in step 4. A table nobody carries is emptied by every save - which shipped twice
(#191, the SpeleoDB link; #303, the Unwind chain pointer).

The `Sidecar` registry (`core/state/save_sidecars.rs`) is the one list of those
tables, each with what happens when it cannot be carried (#576 A3, policy "stop
unless cosmetic"):

| Side table | Cannot be read or written | Why |
|---|---|---|
| `resolved_warnings` (ignored warnings) | The save stops: `IGNORES_UNREADABLE` | The user's own decisions (#517) |
| `speleo_link` | The save stops: `SPELEODB_LINK_UNREADABLE` | The project's identity, not decoration |
| `speleo_undo_chain` + `speleo_undo_watermark` | The save stops: `UNDO_MARKERS_UNREADABLE` | They decide which undo rows are the user's own (#303) |
| `display_settings` | Saved without them, warning `DISPLAY_SETTINGS_NOT_SAVED` | Per-machine view state |
| `undo_stack` + `redo_stack` | Saved with an EMPTY history, warning `UNDO_HISTORY_DROPPED` | A partial history is never written |

A stopped save has written nothing: an unreadable table is refused before the temp
file exists, and a failed write discards the temp, so the destination is untouched.
Two exceptions keep a way out. A SpeleoDB link that cannot be read refuses only a
save that would rewrite the open file; a Save As to another file goes ahead, as an
unlinked project without the undo-chain markers (#576 B35(b), `UnlinkedSaveAs`).
And the undo history stops a save only when even emptying it fails.

The coverage test (`save_sidecar_tests`) pins that every table `init_schema`
creates is either in `SAVE_PROJECT_TABLES` or registered - so a new table has to
be given a policy before any save can drop it.

The save MODE is resolved once, in step 2, instead of being inferred at each step
(inferring a first save from a missing database handle is what sent autosaves to
`<stem>_1.stalx`, B35(c)):

| `SaveMode` | When | The session |
|---|---|---|
| `FirstSave` | A deferred new project's first write; an implicit one picks a free name | Moves onto the file |
| `InPlace` | Ctrl+S, autosave, the pre-push flush, or a path naming the open file | Stays |
| `SaveAs` | An explicit destination elsewhere | Moves onto the file |
| `UnlinkedSaveAs` | An explicit destination elsewhere, from a file whose SpeleoDB link could not be read | Moves onto the file |
| `DetachedCopy` | An explicit destination for a SpeleoDB-managed project | Stays on the managed file (see Save a Copy) |

A detached copy and an unlinked Save As write a project without a link, so both
shed the link and both undo-chain markers: an unlinked file that kept them would,
once linked again, have its first push cut the user's history as "inherited"
(#303). Every other mode carries every side table.

### Known limit: an ignored warning after a reverse that is not saved (pre-v6 files)

An ignored warning is written to the open file at once, not at the next save, and
a section reverse moves the file's position-keyed `resolved_warnings` rows to their
shots' new positions (`pin_position_keyed_warnings_after_reverse` in
`core/state/warnings.rs`, through `Database::rekey_position_resolved_warnings`).
A shot in a file from before schema v6 has no stored UUID - one is minted in
memory on every open - so its position is the only name the file has for it.
Reverse such a section and close without saving, and the reopened file has the
shots back in their old order while the ignore stays at the mirrored position: it
now hides that shot's warning of the same kind, and the shot it was made for shows
its warning again. Saving after the reverse is not affected, and neither is a file
from v6 on, whose ignores are keyed by the shot's UUID. Left as a known limit
(decision Q-W56-1-D1); re-ignore the warning after reopening.

### Stored project settings that could not be read

The validation thresholds, the entrance elevation and the default north reference
live in `project_settings`, which the core save writes from the `Project` model. A
value that exists but cannot be READ on open (an I/O error, a structurally broken
table) is therefore not "not set": the load fills in the default, and a save would
write that default over the stored value (#576 B15). The open path keeps the two
apart (`Database::load_project_with_report`):

- **The project opens normally**, with defaults in place of the unread values. A
  missing key, or a file older than the table, is still simply "not set", and an
  unparseable value keeps its warn-and-default.
- **The user is told which settings.** The load commits the list as
  `AppState::unreadable_project_settings` and hands it to the frontend on the binary
  load envelope (`unreadable_project_settings`); `App.tsx` shows a banner naming them.
- **Every save is refused** - in place and Save As - with
  `PROJECT_SETTINGS_UNREADABLE`, before the save-time heals touch the project, so the
  file keeps its stored values. `surfaceSaveFailure` shows it as a warning toast, and
  auto-save does not try. Export still works; closing or reopening the project ends
  the state.
- **Setting a value again ends it for that value** (#576 B15 follow-up). Once the
  user sets the validation thresholds, the default north reference or the entrance
  elevation in Project Settings, the value in memory is theirs, not a default
  standing in, so `update_project_settings` (thresholds and north reference, #576
  NF-W3-2) / `set_surface_elevation` take that key off the list
  (`AppState::lift_unreadable_project_setting`) - also when the value set equals the
  one shown: while a key is unread, saving the dialog sends it unchanged (except on a
  project whose metadata cannot change at all, where the backend would refuse it).
  The north reference and the elevation are survey edits: in Plan and Export they
  are not set, so they stay unread until a Survey save (#576 NF-W3-2-PLAN).
  When the list is empty, saves (and auto-save) work again; the banner and the
  refusal both say how.

### Creation times are written back as stored

The project's `created_at` and each section's (`sections.created_at`) are kept
as the RFC 3339 text the file holds (`CreationStamp`, #576 RV14-2, NF-W18-1) and
written back byte for byte, whatever form another tool stored them in. A section
with no readable stamp - new, or its column empty or unreadable - is stamped by
the save on the model (`stamp_section_creation_times`, #576 NF-W14BE-1), so every
later save writes that same value.

What crosses to the webview: the project payload's `created_at` is still the UTC
moment in `DateTime<Utc>` serde form (`...Z`). Each section's `created_at` in
the JSON project payload (`#[ts(skip)]`, absent from the TS type) and the text
inside persisted undo payloads, for the project and its sections, are now the
stored text (`...+00:00` for every file this app wrote) rather than the `...Z`
form. Nothing in the frontend reads a section's `created_at`, and older builds
parse every RFC 3339 form, so this has no functional effect (#576 RV15-3).

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
below), the pipeline diverges after step 4: the copy is written and renamed into
place, recorded in Recent Projects, and the function returns with
`SaveOutcome.unlinked_copy_path` set - steps 6, 7 and 8 are skipped on
purpose, because the session stays on the managed file.

### Frontend (`saveProject` in `projectStore.ts`)

```
1. invoke('save_project', { path })
2. If unlinked_copy_path in result: toast the copy's location and stop - the
   open project was NOT saved (a detached copy was written elsewhere), so
   file_path and isDirty are left untouched
3. If saved_path in result, update project.file_path in store
4. refreshUndoState() - lightweight, just fetches can_undo/can_redo booleans
5. Set isDirty: false
6. surfaceSaveWarnings(result.warnings) - a warning toast for each thing the save
   went ahead without (also on the copy path in step 2)
```

A successful save's `SaveOutcome.warnings` is what it could not keep:
`UNDO_HISTORY_DROPPED`, `DISPLAY_SETTINGS_NOT_SAVED`, `DATABASE_NOT_REOPENED` and
`SIDE_TABLES_NOT_CARRIED` (#576 RV2-R-7: a Save As to another file away from an open
file whose database could not be reopened - the way out - goes ahead without the
ignored warnings, the SpeleoDB link and the undo-history markers only that file
holds, and says so).
Each becomes one warning toast (`saveFeedback.ts`, key `save-warning:<code>`): the
save succeeded with a caveat, nothing needs deciding, and the next save starts
clean - so not a dialog and not a banner (docs/notifications.md). The SpeleoDB
pre-push flush is the same save: its warnings ride the Done / Push answer
(`SpeleoDoneReport` / `SpeleoPushReport.save_warnings`, #576 NF-W2SAVE-1) and
`useSpeleoActions` shows them the same way.

The frontend calls `refreshProjectState()` after a save ONLY when the project entered the save with integrity issues (the backend's save-time heals mutate the live project). A clean save skips it: it changes no computed data (stations, stats, warnings), so recomputing everything would be wasted work. The only state that changes is `file_path` (on first save or save-as), which the backend returns via `SaveOutcome.saved_path`. When the save produced a detached copy instead (see Save a Copy below), the path comes back in `unlinked_copy_path` and `saved_path` is `null`.

### Asking before a file is replaced

Save As (`saveProjectAs`, `src/utils/projectActions.ts`) and every save or export
whose name the app rewrote after the OS dialog answered (`resolveSavePath`,
`src/utils/overwriteGuard.ts`: `map.svg` typed for a PNG export is written as
`map.png`) ask before replacing a file. Whether one is there is the backend's
answer, `save_target_exists` (`commands/save_target.rs`, #576 HFI-2): it runs the
write-path policy of the write it precedes (docs/architecture.md, "IPC trust
boundary") and reports anything at the name - a file, a folder, a dangling link -
as there. A probe that fails (a folder the OS will not let it look into) is an
error, and the prompt then asks with its "could not check whether ... already
exists" wording rather than saving over a file nobody looked for (#576 E10-D1). A
probe the policy refuses (an app or system folder; on Windows also a device,
volume, drive-relative, stream, device-name or loopback-share path) carries `WRITE_PATH_REFUSED`:
the write after it refuses the same path with the reason, so the caller goes
straight to it and the user sees one dialog, the refusal (#576 HFI-2-D2). The webview's fs plugin is not asked: its scope holds no user path, so its
`exists()` failed for every path in the real app and every Save As said "could not
check".

## Atomic Save (Temp File + Rename)

All saves use a temp file with atomic rename to prevent data loss on crash or power failure:

1. Write to `file.stalx.tmp` (new SQLite database)
2. On success: `rename(file.stalx.tmp, file.stalx)` - atomic on POSIX, nearly atomic on Windows
3. On failure: delete `file.stalx.tmp`, original file untouched

The rename replaces the file at the path, so a load of that path still reading the
file it replaced must not commit a session on it (its handle is on the deleted
file): the load compares the file it opened with the file at the path at its commit
and is refused with the retryable `FILE_REPLACED_DURING_LOAD`, which the frontend
answers by loading once more (#576 FZ-FZ-INT-R2, `core/state/load_file_stamp.rs`).
Every save rename moves the stamp: the path's dev + inode on unix, the session's
save epoch elsewhere.

If a leftover `.stalx.tmp` file exists from a previous failed save, it is removed before starting. Its rollback journal (`.stalx.tmp-journal`) is not, so the next save's fresh temp database can meet an orphaned hot journal - a state the crash-timing gate injects deliberately.

That gate (`src-tauri/crates/core/src/core/state/crash_property_tests.rs`, see [Testing Recipes](testing-recipes.md)) is what holds this promise up: it constructs the exact files a kill at each pipeline boundary would leave and asserts the destination comes back as EXACTLY the old project or EXACTLY the new one, never a blend, and that the next save recovers cleanly.

## Failure classification

Every failing save goes through `save_failure` (`project_io.rs`), which logs the
whole `anyhow` chain at WARN and gives the failure the `CommandErrorCode` that tells
the frontend which remediation to show (#576 C4: the code travels on the
`CommandError`, not as a prefix of its message, which is display-only).
`src/utils/saveErrors.ts` classifies the codes; `saveFeedback.ts` turns them into
copy. (The newer-version refusal is coded at the top of `save_project` itself,
before anything is attempted.)

| Code | Means | Surface |
|---|---|---|
| `FS_PERMISSION_DENIED` | The OS refuses a destination that IS there | `SavePermissionDialog` (folder-access guidance) |
| `FS_PATH_NOT_FOUND` | The folder is gone, or its drive is not connected | Error dialog, "the folder no longer exists or the drive is not connected" |
| `FS_PATH_TOO_LONG` | The path is past what the OS will open (Windows `MAX_PATH`) | Error dialog, "save to a shorter path" |
| `NEWER_FILE_READ_ONLY` | The project was saved by a newer Stalagmight and is open read-only (see above) | Warning toast, never Review & Repair |
| `IGNORES_UNREADABLE` | The ignored-warnings table could not be read (#517) | Error dialog / autosave banner with the backend's sentence, never Review & Repair |
| `SPELEODB_LINK_UNREADABLE` | The SpeleoDB link could not be read and the save would rewrite the open file | As `IGNORES_UNREADABLE` |
| `UNDO_MARKERS_UNREADABLE` | The SpeleoDB undo-history markers could not be read | As `IGNORES_UNREADABLE` |
| `PROJECT_SETTINGS_UNREADABLE` | Stored project settings could not be read when the project was opened (see above) | Warning toast; the banner says which |
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
  stacks, but NOT the `speleo_link` row or the two undo-chain markers - it is a
  plain unmanaged project.
- The session stays on the managed file: `project.file_path`, the database
  handle and the dirty state are untouched. The backend reports the copy via
  `SaveOutcome.unlinked_copy_path` and deliberately leaves `saved_path`
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

A `speleo_link` row that cannot be READ is not "no link" (#576 B35(b)). The
file opens as a local project, but every save that would rewrite it is refused -
the rebuilt file would lose the row - while a Save As to another file goes
ahead, unlinked and without the undo-chain markers, and moves the session there. The load reports it
(`speleo_link_unreadable` on the binary envelope), and a banner says so when the
project opens, so the first refused save is not the first the user hears of it.

## Auto-Save

Auto-save is handled entirely in the frontend via the `useAutoSave` hook (`src/hooks/useAutoSave.ts`). There is no per-command auto-save in the backend.

### How It Works

1. The hook subscribes to `isDirty` changes in the project store
2. When `isDirty` becomes true (any modifying command), a debounce timer starts
3. Each subsequent change resets the timer
4. When the timer fires, `saveProject()` is called
5. On success, `isDirty` is set to false, which cancels the timer and briefly shows "Saved"
6. While autosave is held (see the last guard below) a pending timer is dropped
   and nothing arms or fires; when the hold ends, a project that is still dirty
   waits a full interval again (#576 D34)

### Guards

Auto-save only fires when all of these are true:
- `config.general.auto_save` is enabled
- The project has a `file_path` (has been saved at least once)
- `isDirty` is true
- No save is already in progress
- The project was not saved by a newer version of Stalagmight (`selectNewerFileReadOnly`;
  every save of such a project is refused, so an attempt could only fail on every debounce)
- None of the project's stored settings went unread on open (`unreadableProjectSettings`;
  every save is refused while one did, for the same reason)
- Autosave is not held (`src/utils/autosaveHold.ts`): no "leave this project"
  prompt is open and no discard is switching projects - the user is deciding
  what happens to these edits, so autosave must not decide first (#576 D34)
- The file's SpeleoDB link was readable on open (`speleoLinkUnreadable`; a save over the
  file is refused while it was not, and a Save As to another file clears the flag)

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
3. A failed write is logged via `tracing::warn`, both stacks are cleared, and the save result carries an `UNDO_HISTORY_DROPPED` warning, which the frontend shows as a warning toast; if clearing also fails, the save is aborted with `save_failure` and the temp file is removed

### Details

- Only the last N entries are persisted (configurable via `general.undo_persist_limit`, default 20) - EXCEPT for a Full-mode SpeleoDB project, which persists its whole history since the last commit under a byte budget instead (#303)
- Each entry is stored as `(command_json, undo_data_json, timestamp_iso8601)`
- Serialization uses gzip compression
- A save rewrites the WHOLE persisted stack, every time - it has to, because the pipeline above builds a fresh temp file and renames it over the target, so anything a save skips is gone (#303). The byte budget is therefore a latency bound as much as a storage one; see [Undo/Redo](undo-redo.md)
- On project open, persisted stacks are restored from the database
- See [Undo/Redo](undo-redo.md) for the full undo system documentation, including the hybrid SpeleoDB mode and when the stack is cleared

## SaveOutcome.saved_path

The `save_project` backend command answers with a `SaveOutcome` (#576 A12) whose `saved_path` is the save path as a string. This allows the frontend to update `project.file_path` without a full state refresh. A Save a Copy on a managed project returns `unlinked_copy_path` instead and leaves `saved_path` `null`, precisely so this update cannot fire.

- Set on successful saves (regular save and save-as)
- `null` on a Save a Copy; both keys are always on the wire (the outcome types omit nothing, docs/ipc-typegen.md)

## Key Files

| File | Purpose |
|------|---------|
| `src-tauri/crates/core/src/core/state/project_io.rs` | Backend `save_project`, atomic save logic (`open_project` lives in `project_load.rs`) |
| `src-tauri/crates/core/src/core/state/save_sidecars.rs` | The side-table registry, its failure policies and `SaveMode` |
| `src-tauri/crates/core/src/core/outcome.rs` | `SaveOutcome` (`saved_path`, `unlinked_copy_path`, `warnings`) |
| `src/store/projectStore.ts` | Frontend `saveProject()` action |
| `src/hooks/useAutoSave.ts` | Debounced auto-save hook |
| `src-tauri/crates/persistence/src/persistence/sqlite.rs` | SQLite database operations |
| `docs/undo-redo.md` | Undo/redo stack persistence details |
| `docs/configuration.md` | Auto-save config options |
