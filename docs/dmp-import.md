# DMP Import Wizard

The DMP import wizard (`DmpNewImportDialog`) is the primary UI for importing cave survey data from MNemo DMP files. It uses a connection-wizard pattern: the backend pre-computes placement suggestions, the user confirms or adjusts each one, and a batch import executes at the end.

Each MNemo section in the file (a `DmpSection`) becomes one Stalagmight **line**. This page says "DMP section" for the file's unit, "import line" for a DMP section on its way into the project (the code's `importLineIndex`, `ImportLineConfig`), and "line" for what the project holds.

## Reading the device over USB (#725)

**Import from MNemo...** (both hamburger menus, hidden on Android) reads a
USB-connected MNemo v2 instead of a file. It does not import anything itself: it
saves what it read as a `.dmp` and hands that path to `routeOpenedFile`, the same
route as a `.dmp` picked from disk. The read-only guard, the no-project path
(`new-project-for-import`), duplicate detection and this wizard therefore
apply unchanged; pulling the device again after each dive brings the earlier
sections back already unticked, because their content hashes match.

**The serial exchange** (`src-tauri/src/commands/mnemo/protocol.rs`, written from
the device's public serial CLI documentation):

| Step | Sent | Reply |
|------|------|-------|
| 1 | `getfirmwareversion\n` | text, then `MN2Over` |
| 2 | `getdata\n` | the survey memory (the bytes a `.dmp` holds as text), then `MN2Over` |

- **57600 baud, 8N1, raw, never 1200.** Opening an RP2040 board at 1200 baud
  reboots it into its bootloader. No IPC argument carries a baud rate or a
  command: the two commands are private constants.
- **`getdata` only after a good firmware reply.** If step 1 does not end with
  `MN2Over`, the pull stops (`not_mnemo`), so another USB serial device never
  receives `getdata`.
- **The end of a reply** is an `MN2Over` within the last 16 bytes, followed only
  by text-like bytes (line endings, a short prompt, NUL padding), and then 250 ms of
  quiet. It is re-derived after every read, so a chance `MN2Over` inside the survey
  bytes stops counting as soon as binary data follows it, even after a pause.
- **Stale bytes first:** before `getfirmwareversion`, whatever the device is still
  sending (the rest of a cancelled transfer) is read and discarded until 250 ms of
  quiet, for at most 3 s.
- **Limits:** 10 s to the first byte; 2 s (firmware) or 8 s (data, the device
  pauses while reading its memory) without a byte; 10 s or 10 min in all; 8 MiB of
  data. Cancel is checked between reads (200 ms read timeout).

**The port** (`port.rs`, the `serialport` crate with default features off):

- Only ports the OS lists as USB serial are offered, and `pull_mnemo` refuses any
  name not in that list at the moment it runs, so the IPC cannot open an arbitrary
  device or file. On macOS only `/dev/cu.*` (`/dev/tty.*` waits for carrier detect).
- Opened exclusively (the crate's Unix default; Windows never shares a COM port); a
  port already open elsewhere (often the MNemo desktop app) is `port_busy`. The
  crate reports both "in use" and "gone" as `NoDevice`, so the ports are listed
  again to tell `port_busy` from `port_not_found`.
- A port whose USB name mentions "mnemo", or that reports the MNemo v2's board,
  an Arduino Nano RP2040 Connect (vendor `0x2341`, product `0x005E`, seen on a real
  device), is marked `likely_mnemo`. The dialog preselects only when exactly one
  port is; anything else needs an explicit pick.
- **Linux:** without `libudev` the crate scans `/sys/class/tty` (and reports
  nothing when that is missing). Opening `/dev/ttyACM*` needs the `dialout` group
  (`uucp` on Arch); a refusal is `permission_denied` with that advice. ModemManager
  may briefly probe a newly plugged `ttyACM` device.
- **Windows:** the built-in CDC driver gives a `COMn` port.
- **macOS:** no entitlement is needed for a USB serial port under the hardened
  runtime without App Sandbox.

**The saved file:** `<app local data>/MNemo downloads/MNemo-YYYY-MM-DD_HHMMSS.dmp`.
The text is written and fsynced as `.<name>.part`, then hard-linked to the `.dmp`
name (a clash gets `-2`, `-3`, ...; a link never replaces an existing file), so a
crash mid-write leaves no truncated `.dmp`. Nothing deletes old downloads. It is saved before anything is parsed, and kept even when nothing
in it parses, so it can go in a bug report; the dialog then offers Show file but
not Import.

**What the parser skipped:** `parse_dmp_bytes` returns a `DmpReadLoss` with the
sections that failed to parse (for example a corrupted date) and the bytes after
the last complete section, ignoring zero padding and a `MN2Over` trailer. A non-zero
tail is usually a survey that was not closed on the device. The dialog shows both;
the wizard still drops them, as it does for a file. Recovering such sections is a
separate follow-up.

## Wizard Phases

| Phase | What happens |
|-------|-------------|
| `init` | Import the zero-datum line (if provided), then call `suggest_all_import_placements` for all remaining import lines |
| `suggesting` | Present suggestions one at a time; user confirms, edits, or skips each |
| `manual_placing` | Nothing was suggested for the remaining import lines, so they are offered one SEGMENT at a time (`computeUnreachableSegments`) with four choices: reuse a skipped suggestion, attach to an existing line, place on the map by hand, or discard. Entered with `autoStartManualPlacementPending` set, which hides the dialog (`DialogWrapper`'s `hidden`) until the map placement it auto-starts has begun |
| `placing` | Late import: build batch `ImportLineConfig[]` for anything manual placement added and call `import_lines` again (overlap treatment already ran on the way out of `suggesting`) |
| `done` | All lines placed and `onComplete` fires; if any remain unconnected the wizard re-enters `manual_placing` instead |

The phase union is declared inline at `DmpNewImportDialog.tsx`'s `phase` state and
mirrored, for the onboarding tour, as `DmpImportWizardPhase` in `src/store/uiStore.ts`.
The STORE copy only ever takes `suggesting`, `manual_placing` or `null`, so it is not
a phase oracle - the DOM markers (`wizard-suggestion`, `wizard-manual-placing`, ...)
are.

### The manual-placement round trip

Placing a segment on the map needs the map, so `handleStartSegmentPlacement`
CLOSES the wizard and reopens a fresh instance afterwards, handing the run's
state across in `SavedManualPlacingState` (`openDialog`'s `dialogData`). The
new instance is a new component: anything living in a ref of the old one is
gone unless it rides that struct. `importedReceipts` is one of those (#487) -
each `import_lines` call returns an `import_receipt`, the wizard banks them
all and aggregates them once at completion (`aggregateImportReceipts`), and an
empty list there aggregates to `undefined`, which App.tsx reads as "no receipt
to show". A new per-run accumulator has to be added to the struct in the same
change that introduces it.

Three live completion sites pass that aggregate to `onComplete`: early import,
late import, and the `done` phase. A fourth, the pending-segment import, is
dormant: it fires on `SavedManualPlacingState.pendingSegmentImport`, which
nothing writes any more (`buildSavedState` omits it, and it is the only place
a saved state is built). Pre-existing, and left in place - it still carries the
aggregate, so it stays correct if a caller ever sets that field again.

`importedLineIds` is the other per-run accumulator on that struct, and it
fails LOUDER when it is not carried: App.tsx gates `suggest_orphan_placements`
on `newLineIds?.length`, so an instance that completes with an empty list
skips orphan detection entirely - which is what the manual-placement route used
to do while the direct route always ran it.

### Leaving the wizard: Back and Cancel (#394)

Because the imports are eager, both exits have to decide what happens to
lines that are ALREADY in the project - `connections` does not answer that
question (it is a client-side record that has imported nothing), so both gates
read `importedLineIdsRef` as well.

* **Cancel** offers three actions once anything is imported: remove and close,
  keep and close, or continue editing. The copy names the count.
* **Back** offers no "keep". It returns to a step BEFORE any import, and the
  forward pass re-runs the whole pipeline from the mount effect, so a kept
  line would simply be imported a second time. Back therefore withdraws
  first, and App.tsx hands the zero-datum config the user entered back to
  `zero-datum-config` (`previousZeroDatumConfig` in `dialogData`, consumed as
  `ZeroDatumConfigDialog`'s `initialConfig` prop) so stepping back is an edit
  rather than a restart. The prefill covers the GPS, the metadata, the shot-table
  edits AND the station pick - the last two together, because `stationIndex` is
  numbered over the edited rows, so restoring one without the other silently
  re-anchors the datum.

There is no "carry a done-flag across Back" alternative: the mount effect that
imports the anchor is the same one that fires `suggest_all_import_placements`,
so suppressing it strands the reopened instance in `init` - and it would pin the
anchor to the first pass's GPS/name/shot edits, which is what the user pressed
Back to change.

The carry survives `ZeroDatumConfigDialog`'s own Back too (#534), and what
survives is the dialog's LIVE state - `onBack` is handed the current GPS
(text and, if it parsed, the coordinates), metadata, station pick and shot
edits (`buildCurrentDraft`), not just what the dialog opened with. App.tsx
relays that draft into `zero-datum-selection`'s `dialogData`
(`ZERO_DATUM_CARRY_KEY`, `src/utils/zeroDatumCarry.ts`), and
`onSelectLine` re-attaches it to the reopened `zero-datum-config` ONLY
when the re-picked line matches the draft's `lineIndex`
(`carryZeroDatumConfig`) - a different line starts blank, because the
name/GPS/shots answered a different question. It also starts blank on a
THIRD hop back to `dmp-selection`: `openDialog` replaces `dialogData`
wholesale (`uiStore.ts`), and neither hop back through it relays the draft
any further.

The withdrawal is ONE `execute_batch` of `delete_line` - one undo entry for
one user action. `Command::DeleteLine` is batch-allowed for exactly this
(`AppState::is_batch_allowed`). Four properties it has to keep, each pinned by a
test:

* **Deduplicated.** The id list is seeded from the saved state and appended to by
  live imports, so one id can appear twice across the map-placement handoff.
  `execute_batch` is all-or-nothing and a second `delete_line` for one id
  fails "not found", which would roll the whole withdrawal back.
* **Exclusive with imports.** Neither exit opens while an import is in flight (an
  import banks its ids only on resolution, so a withdrawal decided mid-flight
  would leave the late arrival untracked), and every button in both
  confirmations - Escape and the backdrop included - is shut while the delete is
  on the wire.
* **Project-stamped.** The ids travel with the project's `created_at` and the
  withdrawal refuses on a mismatch. Nothing clears `dialogData` when the open
  project changes, and line ids are small per-project integers, so a stale id
  would otherwise resolve happily against an unrelated survey.
* **Written through.** In the import-into-NEW-project flow the withdrawal saves,
  because the eager anchor import already resolved that project's deferred first
  save (`finalize_deferred_import`) and the file would otherwise keep the
  lines that were just removed. Importing into an EXISTING project it only
  marks dirty, exactly as `saveAfterImport` does, so unrelated pending edits are
  not committed behind the user's back. That save is NOT best-effort: a failed
  write keeps the wizard open with the reason, and the next Remove click retries
  the SAVE (there are no lines left to delete by then), because closing is
  what makes the removal final from the user's side.
* **Forgetful.** `Command::UntrackImport` rides the same batch. `import_lines`
  records the file hash and each line's content hash for duplicate detection,
  and deleting a line does not touch them - so without this the project still
  claims the file was imported while none of it remains, and `DmpSelectionDialog`
  auto-DESELECTS every DMP section it believes is a duplicate: the obvious retry
  offers nothing to import. In the batch, so one undo restores the lines and
  their tracking together. Only the hashes this run sent are forgotten; the file
  hash is per-file, so a file also imported in an earlier session loses its
  file-level duplicate flag (its per-line flags stand).

Two residues, both deliberate:

* **Remove and close empties the project; the file itself stays where you
  created it.** Nothing here unlinks a file the user asked for, and the Recent
  Projects entry stays with it.
* **Undoing the withdrawal from the main window is outside the wizard's
  knowledge.** The accumulators are cleared when it succeeds, so lines
  restored by a later undo are ones a re-entered wizard no longer tracks, and a
  second forward pass would import them again. Cheap guards were considered and
  rejected: the wizard is gone by then, and the undo entry is an ordinary one the
  user is entitled to.

## Virtual Connections

During the `suggesting` phase, confirmed placements are recorded as **virtual connections** - lightweight client-side records that describe how two lines should be joined. No backend write happens for them until the early import that runs on the transition out of `suggesting`.

```typescript
type VirtualConnection = {
  importLineIndex: number;        // DMP section being placed
  importStationIndex: number;     // Station in that section
  targetType: 'existing' | 'importing';
  existingLineId?: number;        // For existing targets
  existingStationIndex?: number;
  targetImportIndex?: number;     // For import-to-import targets
  targetImportStationIndex?: number;
};
```

**`existing`** connections attach a DMP section to an already-imported project line.
**`importing`** connections attach a DMP section to another DMP section that hasn't been imported yet.

## Suggestion Scoring

The backend command `suggest_all_import_placements` compares every import line against both existing project lines and other import lines. It produces a flat list of `CrossLineSuggestion` objects sorted by score, each carrying:

- `importLineIndex` / `existingStationIndex` - which stations to merge
- `matchedShots` / `direction` - how many shots overlapped and in which direction
- `confidence` - `high` / `medium` / `low` tier
- `targetType` - `existing` or `importing`
- `score` - aggregate match score (higher is better)

The wizard picks the top suggestion for each unplaced import line and shows alternatives for the same line.

## Overlap Handling

When a suggestion has `matchedShots > 0`, the wizard shows an overlap mode picker:

| Mode | Import side | Target side (import-to-import) | Target side (import-to-existing) |
|------|-------------|-------------------------------|----------------------------------|
| `interpolate` | Delete actionable overlap shots | Replace overlap shots with interpolated values | Send `ExistingShotUpdate` with interpolated values |
| `import_data` | Delete actionable overlap shots | Replace overlap shots with import values | Send `ExistingShotUpdate` with import values |
| `existing_data` | Delete actionable overlap shots | No change | No change |
| `keep_both` | Mark overlap shots as reference (no deletion) | No change | No change |

The overlap mode is persisted to the project config.

### Centralized Overlap Treatment

All overlap mutation is handled by a single function - `applyAllOverlapTreatments` - which runs once on the transition out of the `suggesting` phase (the early import), before `buildBatchImport`. The `handleConfirm` callback does **not** mutate overlap shots; it only records:

1. The virtual connection (placement info)
2. A `ConfirmedOverlapInfo` entry (which merge station the user chose)

```
handleConfirm → records VirtualConnection + ConfirmedOverlapInfo
              → saves raw user edits to lineEditsRef (NO overlap mutation)

applyAllOverlapTreatments → processes ALL pool suggestions in one pass
                          → mutates lineEditsRef (import-to-import targets)
                          → returns ExistingShotUpdateLocal[] (import-to-existing targets)

buildBatchImport → reads lineEditsRef → ImportLineConfig[]

import_lines IPC → lines + existingShotUpdates
```

`applyAllOverlapTreatments` walks the full suggestion pool. For each suggestion:

- **Confirmed suggestions** (matched by field comparison to `confirmedOverlapsRef`): use `computeActionableIds` with the user's merge station to determine which overlap shots to process.
- **Unconfirmed suggestions**: use the full `computeOverlapIds` result.

The function handles two target paths:

- **Import-to-import**: Modifies the target import line's edits in `lineEditsRef` directly (interpolation, import data replacement, or reference marking).
- **Import-to-existing**: Collects `ExistingShotUpdateLocal[]` entries that are sent to the backend alongside the batch import.

### Interpolation

`interpolateShots` averages each numeric field between the import shot and the target shot. Azimuths use circular mean (`atan2((sin(a)+sin(b))/2, (cos(a)+cos(b))/2)`) to handle wraparound correctly. When the suggestion direction is `reverse`, the target shot is direction-reversed (`reverseTargetShot`) before interpolation so that in/out values align.

### Merge Station Picker

When multiple overlap offsets are possible, a dropdown lets the user choose which station pair to merge at. This controls which shots are "actionable" (subject to the overlap mode) versus past-merge (deleted at import).

### Doubly-Anchored Guard

When the target existing line has positional constraints on **both sides** of the overlap shot range, `interpolate` and `import_data` are disabled:

- The overlap mode dropdown greys out those options (`isTargetDoublyAnchored` useMemo)
- The current mode auto-switches to `existing_data` if it was set to a blocked mode
- The backend independently skips `ExistingShotUpdate` entries for doubly-anchored lines as a safety net

**Anchor types** checked on each side of the modified range:
- Zero datum stations
- Station merge endpoints
- Aligned loop closure endpoints (`is_aligned === true`)

**Rationale**: Modifying shots between two positional anchors changes the geometry between them. The backend can compensate for a single anchor via line offset adjustment, but with two anchors on opposite sides of the modification, one constraint will inevitably break (e.g., shifting the offset to keep a zero datum fixed would misalign an aligned closure on the other side).

## Existing Shot Updates (Backend)

The `import_lines` backend command accepts an optional `existing_shot_updates: Vec<ExistingShotUpdate>` parameter. Each entry specifies a `(line_id, shot_id, ShotUpdates)` triple for modifying shots on existing project lines atomically within the same undo transaction as the batch import.

### Application Order

```
1. Snapshot project state (for undo)
2. Filter out doubly-anchored lines (backend safety net)
3. Record zero datum anchor positions BEFORE updates
4. Apply ExistingShotUpdates to existing line shots
5. Compensate line offsets to keep zero datums fixed
6. Create new lines + apply placements (FromExisting uses updated geometry)
7. Snapshot project state (for redo)
8. Push undo entry
```

### Zero Datum Stability

After applying shot updates, the backend checks whether any affected line has a zero datum. If so:

1. The datum station's absolute position was recorded before updates
2. The new position is computed after updates
3. The delta is applied (inverted) to the line offset - and to all lines in the same rigid group (`get_merged_line_ids`) - so the datum stays pinned

This ensures that modifying overlap shots at indices lower than the zero datum station doesn't shift the line's GPS-anchored position.

## Line Editing

Each import line can be edited before import:
- **Metadata**: name, surveyor, date, color, direction, quality
- **Shots**: add, delete, insert before/after, modify values

Edit state is stored in `lineEditsRef` (a `Map<number, LineEditState>`) and restored when the user revisits a line. The `effectiveShots` derived view applies overlap transformations without mutating the raw edit state.

## Preview & Segment Context

The `LinePreview` component shows a 2D plan-view rendering of the import line, the target line, and surrounding context. Context lines appear dimmed behind the main preview.

### Coordinate Frames

The preview has two coordinate frames:

- **Target frame**: The target line's station 0 is at origin. Target-side context is positioned in this frame directly.
- **Import frame**: The import line's station 0 is at origin. Import-side context uses this frame and gets an offset applied by `mergedContextLines` to align with the target.

### Context Gathering

A single `useEffect` computes `segmentContext: { importSide, targetSide }`:

1. **Import-side BFS** - `computeDmpBfsContext(importIdx, ...)` walks `importing`-type virtual connections from the current import line, collecting:
   - DMP neighbor geometry (positioned relative to the import line)
   - Existing anchors (positions where DMP sections connect to project lines)

2. **Target-side handling**:
   - `existing` target: Fetch `get_segment_geometry(existingLineId)` from backend. This returns all lines in that segment with shot-line geometry in a coordinate frame centered on the queried line.
   - `importing` target: Run a second BFS from the target DMP section to gather its neighbors and existing anchors.

3. **Existing anchor resolution** - For each `ExistingAnchor` discovered by BFS (either side), fetch `get_segment_geometry` and position the result relative to the anchor point.

4. **Bridge discovery** - `discoverBridgedDmpSections()` finds DMP sections that are connected to existing lines we already have geometry for, but weren't reached by either BFS. This handles the case where DMP sections A and B both connect to existing line X through separate `existing`-type connections but have no direct `importing`-type edge.

### Bridge Discovery Algorithm

```
For each existing-type connection where the DMP section isn't already shown:
  1. Look up the existing line's geometry in the geo map
  2. Find the merge station's position in that geometry
  3. Compute the DMP section's offset from the merge alignment
  4. Render the DMP section at that offset
  5. BFS from the bridged DMP section to find its importing-type neighbors
  6. Offset all BFS results into the correct frame
```

### Deduplication

A `coveredExistingLineIds` set tracks which existing line IDs have already been included. The direct target segment is processed first (most natural positioning), then import-side anchors, then bridged DMP sections.

## Batch Import

When all suggestions are exhausted (or skipped), the early import runs on the transition out of `suggesting`:

1. `applyAllOverlapTreatments` processes all overlap suggestions → mutates `lineEditsRef` + returns `ExistingShotUpdateLocal[]`
2. `buildBatchImport()` constructs the final `ImportLineConfig[]`:
   1. Build adjacency from `importing`-type connections
   2. Find anchored import lines (zero datum + `existing`-type connections)
   3. BFS from anchors to determine import order
   4. For disconnected components, search the full suggestion pool for fallback `existing`-type anchors
   5. Multi-pass emit: retry lines whose `from_importing` target hasn't been emitted yet
   6. Adjust station indices for deleted overlap shots
3. IPC call: `import_lines(lines, fileHash, existingShotUpdates)`

Lines unreachable after all passes remain in the wizard's manual segment-placement flow until the user places or discards them.

## Station Names

MNemo numbers a section's start station 0 and the end of its N-th shot N, and
divers key their notes to those numbers. The `.dmp` file carries no station
numbers, and every later edit - the shots the importer skips, an overlap trim,
a deleted or added row, any edit after import - shifts the positional station
indexes, so the import records each station's MNemo number as its **name**
(`Station.name`; names are unique within a line, see
[SPEC 4.2](SPEC.md#42-station-identification)).

- **Counting.** ASSUMED, not yet confirmed against a device: only the shots the
  importer keeps advance the number - those `isImportableDmpShot` passes
  (Standard, not 1 m). It matches the station numbers the wizard itself shows
  before any edits. `mnemoStationNumber` in `src/hooks/useImportState.ts` is the
  one place the rule lives (with the filter it runs after), so if a device
  counts other shots too, that is what changes. `convertToEditableShots` stores
  the result on each row as `mnemoStation` (the number of the row's END
  station).
- **Through the wizard.** Every edit spreads the row, so the number survives
  interpolation, data replacement and Reference marking. A row the user adds
  has none.
- **Into the config.** `stationNamesForEmittedShots`
  (`src/utils/importLineConfig.ts`) turns the rows actually sent into
  `ImportLineConfig.station_names`: the end of emitted row k is station
  k + 1; a start keeps the previous row's number (as an in-app delete keeps the
  surviving start), so a station after an added row has no MNemo number - the
  backend names it by the creation rules (`5`, `5a`, ...; SPEC 4.2); only the
  start of the first DMP row falls back to its number minus 1. A line whose
  first m shots an overlap trim removed therefore starts at m, and the trimmed
  numbers are gone with their stations - one name per station, no aliases.
  Indices are in the emitted, pre-reverse order: the backend mirrors them for a
  reversed import exactly like `station_comments`.
- **At a junction.** A line placed on an existing station starts ON that
  station, which keeps its own name; the line's own number for it is kept as
  an alias of the junction on the new line (under a retired Station ID, as
  Merge Stations keeps the station it retires). The new line knows the
  junction by that alias, so its own stations keep their numbers even where
  one is the junction's name (a line placed on station `12` keeps its own
  `12`, as 0.6.0 kept a name per slot and as a merge or the conversion of a
  0.6.0 file leaves it). Only when the line keeps no alias (the file gives
  the junction no number of its own) does a station whose number is the
  junction's name take a letter (`12a`), and the import summary says so,
  naming the line, the junction and the new name.
- **After import.** A name belongs to its station, so it follows the station
  through later inserts, deletes, splits and reverses; a merge keeps the
  surviving station's name and the other one's as an alias. On the map, the Display menu's **Station
  names** labels each station with its name in place of its Station ID (it
  and **Station IDs** are one choice), and the image export has the same
  **Station names** option.

## Key Types

| Type | Location | Purpose |
|------|----------|---------|
| `VirtualConnection` | `src/utils/segmentContext.ts` | Client-side connection record |
| `ConfirmedOverlapInfo` | `src/utils/batchImportHelpers.ts` | Merge station choice per confirmed suggestion |
| `ExistingShotUpdateLocal` | `src/utils/overlapTreatment.ts` | Frontend representation of existing shot modifications |
| `ExistingShotUpdate` | `src-tauri/crates/models/src/line.rs` | Backend type for atomic existing shot modifications |
| `ExistingAnchor` | `src/utils/segmentContext.ts` | Position where a DMP section meets an existing line |
| `ExistingLineGeoEntry` | `src/utils/segmentContext.ts` | Cached segment geometry with frame offset |
| `CrossLineSuggestion` | `src-tauri/crates/models/src/placement_suggest.rs` | Backend suggestion with score/confidence |
| `SegmentGeometry` | `src-tauri/crates/models/src/line.rs` | Backend segment shot-line data |
| `ContextLineGeometry` | `src/components/Dialogs/LinePreview.tsx` | Shot pairs for preview rendering |
| `LineEditState` | `src/utils/batchImportHelpers.ts` | Per-line editable metadata + shots |

## Key Functions

| Function | Location | Purpose |
|----------|----------|---------|
| `applyAllOverlapTreatments` | `src/utils/overlapTreatment.ts` | Single-pass overlap mutation for all suggestions |
| `buildBatchImport` | `src/components/Dialogs/DmpNewImportDialog.tsx` | Construct ordered `ImportLineConfig[]` for backend |
| `stationNamesForEmittedShots` | `src/utils/importLineConfig.ts` | A line's station names (MNemo numbers) from its emitted rows |
| `buildAdjacency` | `src/utils/segmentContext.ts` | Adjacency map from `importing`-type connections |
| `computeDmpBfsContext` | `src/utils/segmentContext.ts` | BFS collecting DMP neighbor geometry + existing anchors |
| `stationPositionFromShotLines` | `src/utils/segmentContext.ts` | Extract XY from backend `shot_lines` (station 0 = `[0][0]`, station N = `[N-1][1]`) |
| `discoverBridgedDmpSections` | `src/utils/segmentContext.ts` | Find DMP sections connected through shared existing lines |
| `computeStationsFromDmpShots` | `src/components/Dialogs/LinePreview.tsx` | XY station positions from DMP shot headings + lengths |
| `computeOverlapIds` | `src/utils/overlapHelpers.ts` | Set of import shot IDs in the overlap region |
| `computeActionableIds` | `src/utils/overlapHelpers.ts` | Subset of overlap IDs based on merge station choice |
| `computeTargetShotIndex` | `src/utils/overlapHelpers.ts` | Map import overlap ID → target line shot index |
| `interpolateShots` | `src/utils/overlapHelpers.ts` | Average two shots (circular mean for azimuths) |
| `reverseTargetShot` | `src/utils/overlapHelpers.ts` | Swap in/out and reverse azimuth by 180° |

## Backend Commands

| Command | Input | Output | Notes |
|---------|-------|--------|-------|
| `suggest_all_import_placements` | `ImportLineForSuggestion[]` | `CrossLineSuggestion[]` | Compares against existing + other imports |
| `get_segment_geometry` | `lineId` | `SegmentGeometry` | All connected lines' shot-lines, centered on queried line |
| `import_lines` | `lines, fileHash, existingShotUpdates` | `CommandResult` | Batch import with atomic existing shot updates and undo support |
| `list_mnemo_ports` | none | `MnemoPort[]` | USB serial ports now present, likely MNemos first |
| `pull_mnemo` | `port` | `MnemoPullResult` or `MnemoError` | Reads the device, saves the `.dmp`; emits `mnemo-pull-progress`; one at a time |
| `cancel_mnemo_pull` | none | none | Stops a running pull within one read |

## Backend Safety Guards

The `execute_import_lines` function in `src-tauri/crates/core/src/core/state/command_import.rs` applies two guards before modifying existing line shots:

1. **Doubly-anchored filter**: For each affected line, if the modified shot range is straddled by positional constraints (zero datums, station merges, aligned closures) on both sides, updates for that line are skipped and the skipped shot count is named on the import receipt.

2. **Zero datum offset compensation**: For lines with a zero datum above the modified shots, the line offset (and all lines in the same rigid group) is adjusted after updates to keep the datum station at its original absolute position.

## Tests

Unit tests for the pure context-geometry functions live in `src/utils/__tests__/segmentContext.test.ts`. Run with:

```bash
pnpm vitest run src/utils/__tests__/segmentContext.test.ts
```

Overlap helper tests: `src/utils/__tests__/overlapHelpers.test.ts`

Receipt round trip (both halves of the carry, plus the `done` phase):
`src/components/Dialogs/__tests__/DmpNewImportDialog.receiptCarry.test.tsx` -
a component test that renders the wizard against a synthetic DMP file and
drives `placementMode.onComplete` in place of a click on the map.

Back/Cancel transactionality:
`src/components/Dialogs/__tests__/DmpNewImportDialog.transactionality.test.tsx`,
whose IPC mock keeps a fake project that `import_lines` and `execute_batch`
really mutate, so "the second pass leaves one line" is measured rather than
inferred from call counts. That a batch of `delete_line` is ONE undo entry is
pinned in Rust: `batched_line_deletes_are_one_undo_entry`.

Backend integration test for overlap interpolation: `src-tauri/crates/core/src/core/state/line_commands_tests.rs` (`kb5_import_overlap_interpolation_integration`, currently `#[ignore]`).
