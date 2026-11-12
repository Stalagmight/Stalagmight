# DMP Import Wizard

The DMP import wizard (`DmpNewImportDialog`) is the primary UI for importing cave survey data from mNemo DMP files. It uses a connection-wizard pattern: the backend pre-computes placement suggestions, the user confirms or adjusts each one, and a batch import executes at the end.

## Wizard Phases

| Phase | What happens |
|-------|-------------|
| `init` | Import the zero-datum section (if provided), then call `suggest_all_import_placements` for all remaining sections |
| `suggesting` | Present suggestions one at a time; user confirms, edits, or skips each |
| `manual_placing` | Nothing was suggested for the remaining sections, so they are offered one SEGMENT at a time (`computeUnreachableSegments`) with four choices: reuse a skipped suggestion, attach to an existing section, place on the map by hand, or discard. Entered with `autoStartManualPlacementPending` set, which hides the dialog (`DialogWrapper`'s `hidden`) until the map placement it auto-starts has begun |
| `placing` | Late import: build batch `ImportSectionConfig[]` for anything manual placement added and call `import_sections` again (overlap treatment already ran on the way out of `suggesting`) |
| `done` | All sections placed and `onComplete` fires; if any remain unconnected the wizard re-enters `manual_placing` instead |

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
each `import_sections` call returns an `import_receipt`, the wizard banks them
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

`importedSectionIds` is the other per-run accumulator on that struct, and it
fails LOUDER when it is not carried: App.tsx gates `suggest_orphan_placements`
on `newSectionIds?.length`, so an instance that completes with an empty list
skips orphan detection entirely - which is what the manual-placement route used
to do while the direct route always ran it.

### Leaving the wizard: Back and Cancel (#394)

Because the imports are eager, both exits have to decide what happens to
sections that are ALREADY in the project - `connections` does not answer that
question (it is a client-side record that has imported nothing), so both gates
read `importedSectionIdsRef` as well.

* **Cancel** offers three actions once anything is imported: remove and close,
  keep and close, or continue editing. The copy names the count.
* **Back** offers no "keep". It returns to a step BEFORE any import, and the
  forward pass re-runs the whole pipeline from the mount effect, so a kept
  section would simply be imported a second time. Back therefore withdraws
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
`onSelectSection` re-attaches it to the reopened `zero-datum-config` ONLY
when the re-picked section matches the draft's `sectionIndex`
(`carryZeroDatumConfig`) - a different section starts blank, because the
name/GPS/shots answered a different question. It also starts blank on a
THIRD hop back to `dmp-selection`: `openDialog` replaces `dialogData`
wholesale (`uiStore.ts`), and neither hop back through it relays the draft
any further.

The withdrawal is ONE `execute_batch` of `delete_section` - one undo entry for
one user action. `Command::DeleteSection` is batch-allowed for exactly this
(`AppState::is_batch_allowed`). Four properties it has to keep, each pinned by a
test:

* **Deduplicated.** The id list is seeded from the saved state and appended to by
  live imports, so one id can appear twice across the map-placement handoff.
  `execute_batch` is all-or-nothing and a second `delete_section` for one id
  fails "not found", which would roll the whole withdrawal back.
* **Exclusive with imports.** Neither exit opens while an import is in flight (an
  import banks its ids only on resolution, so a withdrawal decided mid-flight
  would leave the late arrival untracked), and every button in both
  confirmations - Escape and the backdrop included - is shut while the delete is
  on the wire.
* **Project-stamped.** The ids travel with the project's `created_at` and the
  withdrawal refuses on a mismatch. Nothing clears `dialogData` when the open
  project changes, and section ids are small per-project integers, so a stale id
  would otherwise resolve happily against an unrelated survey.
* **Written through.** In the import-into-NEW-project flow the withdrawal saves,
  because the eager anchor import already resolved that project's deferred first
  save (`finalize_deferred_import`) and the file would otherwise keep the
  sections that were just removed. Importing into an EXISTING project it only
  marks dirty, exactly as `saveAfterImport` does, so unrelated pending edits are
  not committed behind the user's back. That save is NOT best-effort: a failed
  write keeps the wizard open with the reason, and the next Remove click retries
  the SAVE (there are no sections left to delete by then), because closing is
  what makes the removal final from the user's side.
* **Forgetful.** `Command::UntrackImport` rides the same batch. `import_sections`
  records the file hash and each section's content hash for duplicate detection,
  and deleting a section does not touch them - so without this the project still
  claims the file was imported while none of it remains, and `DmpSelectionDialog`
  auto-DESELECTS every section it believes is a duplicate: the obvious retry
  offers nothing to import. In the batch, so one undo restores the sections and
  their tracking together. Only the hashes this run sent are forgotten; the file
  hash is per-file, so a file also imported in an earlier session loses its
  file-level duplicate flag (its per-section flags stand).

Two residues, both deliberate:

* **Remove and close empties the project; the file itself stays where you
  created it.** Nothing here unlinks a file the user asked for, and the Recent
  Projects entry stays with it.
* **Undoing the withdrawal from the main window is outside the wizard's
  knowledge.** The accumulators are cleared when it succeeds, so sections
  restored by a later undo are ones a re-entered wizard no longer tracks, and a
  second forward pass would import them again. Cheap guards were considered and
  rejected: the wizard is gone by then, and the undo entry is an ordinary one the
  user is entitled to.

## Virtual Connections

During the `suggesting` phase, confirmed placements are recorded as **virtual connections** - lightweight client-side records that describe how two sections should be joined. No backend write happens for them until the early import that runs on the transition out of `suggesting`.

```typescript
type VirtualConnection = {
  importSectionIndex: number;     // DMP section being placed
  importStationIndex: number;     // Station in that section
  targetType: 'existing' | 'importing';
  existingSectionId?: number;     // For existing targets
  existingStationIndex?: number;
  targetImportIndex?: number;     // For import-to-import targets
  targetImportStationIndex?: number;
};
```

**`existing`** connections attach a DMP section to an already-imported project section.
**`importing`** connections attach a DMP section to another DMP section that hasn't been imported yet.

## Suggestion Scoring

The backend command `suggest_all_import_placements` compares every import section against both existing project sections and other import sections. It produces a flat list of `CrossSectionSuggestion` objects sorted by score, each carrying:

- `importSectionIndex` / `existingStationIndex` - which stations to merge
- `matchedShots` / `direction` - how many shots overlapped and in which direction
- `confidence` - `high` / `medium` / `low` tier
- `targetType` - `existing` or `importing`
- `score` - aggregate match score (higher is better)

The wizard picks the top suggestion for each unplaced section and shows alternatives for the same section.

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
              → saves raw user edits to sectionEditsRef (NO overlap mutation)

applyAllOverlapTreatments → processes ALL pool suggestions in one pass
                          → mutates sectionEditsRef (import-to-import targets)
                          → returns ExistingShotUpdateLocal[] (import-to-existing targets)

buildBatchImport → reads sectionEditsRef → ImportSectionConfig[]

import_sections IPC → sections + existingShotUpdates
```

`applyAllOverlapTreatments` walks the full suggestion pool. For each suggestion:

- **Confirmed suggestions** (matched by field comparison to `confirmedOverlapsRef`): use `computeActionableIds` with the user's merge station to determine which overlap shots to process.
- **Unconfirmed suggestions**: use the full `computeOverlapIds` result.

The function handles two target paths:

- **Import-to-import**: Modifies the target import section's edits in `sectionEditsRef` directly (interpolation, import data replacement, or reference marking).
- **Import-to-existing**: Collects `ExistingShotUpdateLocal[]` entries that are sent to the backend alongside the batch import.

### Interpolation

`interpolateShots` averages each numeric field between the import shot and the target shot. Azimuths use circular mean (`atan2((sin(a)+sin(b))/2, (cos(a)+cos(b))/2)`) to handle wraparound correctly. When the suggestion direction is `reverse`, the target shot is direction-reversed (`reverseTargetShot`) before interpolation so that in/out values align.

### Merge Station Picker

When multiple overlap offsets are possible, a dropdown lets the user choose which station pair to merge at. This controls which shots are "actionable" (subject to the overlap mode) versus past-merge, which are imported unchanged as ordinary shots - drawn on top of the passage they repeat and counted again. The picker therefore opens on the last option, the end of the repeat, so every repeated shot is actionable (`defaultMergeOffset`, #685); the matcher's own `preferredMergeOffset`, usually a station inside the repeat, only feeds the ranking.

### Doubly-Anchored Guard

When the target existing section has positional constraints on **both sides** of the overlap shot range, `interpolate` and `import_data` are disabled:

- The overlap mode dropdown greys out those options (`isTargetDoublyAnchored` useMemo)
- The current mode auto-switches to `existing_data` if it was set to a blocked mode
- The backend independently skips `ExistingShotUpdate` entries for doubly-anchored sections as a safety net

**Anchor types** checked on each side of the modified range:
- Zero datum stations
- Station merge endpoints
- Aligned loop closure endpoints (`is_aligned === true`)

**Rationale**: Modifying shots between two positional anchors changes the geometry between them. The backend can compensate for a single anchor via section offset adjustment, but with two anchors on opposite sides of the modification, one constraint will inevitably break (e.g., shifting the offset to keep a zero datum fixed would misalign an aligned closure on the other side).

## Existing Shot Updates (Backend)

The `import_sections` backend command accepts an optional `existing_shot_updates: Vec<ExistingShotUpdate>` parameter. Each entry specifies a `(section_id, shot_id, ShotUpdates)` triple for modifying shots on existing project sections atomically within the same undo transaction as the batch import.

### Application Order

```
1. Snapshot project state (for undo)
2. Filter out doubly-anchored sections (backend safety net)
3. Record zero datum anchor positions BEFORE updates
4. Apply ExistingShotUpdates to existing section shots
5. Compensate section offsets to keep zero datums fixed
6. Create new sections + apply placements (FromExisting uses updated geometry)
7. Snapshot project state (for redo)
8. Push undo entry
```

### Zero Datum Stability

After applying shot updates, the backend checks whether any affected section has a zero datum. If so:

1. The datum station's absolute position was recorded before updates
2. The new position is computed after updates
3. The delta is applied (inverted) to the section offset - and to all sections in the same rigid group (`get_merged_section_ids`) - so the datum stays pinned

This ensures that modifying overlap shots at indices lower than the zero datum station doesn't shift the section's GPS-anchored position.

## Section Editing

Each DMP section can be edited before import:
- **Metadata**: name, surveyor, date, color, direction, quality
- **Shots**: add, delete, insert before/after, modify values

Edit state is stored in `sectionEditsRef` (a `Map<number, SectionEditState>`) and restored when the user revisits a section. The `effectiveShots` derived view applies overlap transformations without mutating the raw edit state.

## Preview & Segment Context

The `SectionPreview` component shows a 2D plan-view rendering of the import section, the target section, and surrounding context. Context sections appear dimmed behind the main preview.

### Coordinate Frames

The preview has two coordinate frames:

- **Target frame**: The target section's station 0 is at origin. Target-side context is positioned in this frame directly.
- **Import frame**: The import section's station 0 is at origin. Import-side context uses this frame and gets an offset applied by `mergedContextSections` to align with the target.

### Context Gathering

A single `useEffect` computes `segmentContext: { importSide, targetSide }`:

1. **Import-side BFS** - `computeDmpBfsContext(importIdx, ...)` walks `importing`-type virtual connections from the current import section, collecting:
   - DMP neighbor geometry (positioned relative to the import section)
   - Existing anchors (positions where DMP sections connect to project sections)

2. **Target-side handling**:
   - `existing` target: Fetch `get_segment_geometry(existingSectionId)` from backend. This returns all sections in that segment with shot-line geometry in a coordinate frame centered on the queried section.
   - `importing` target: Run a second BFS from the target DMP section to gather its neighbors and existing anchors.

3. **Existing anchor resolution** - For each `ExistingAnchor` discovered by BFS (either side), fetch `get_segment_geometry` and position the result relative to the anchor point.

4. **Bridge discovery** - `discoverBridgedDmpSections()` finds DMP sections that are connected to existing sections we already have geometry for, but weren't reached by either BFS. This handles the case where DMP sections A and B both connect to existing section X through separate `existing`-type connections but have no direct `importing`-type edge.

### Bridge Discovery Algorithm

```
For each existing-type connection where the DMP section isn't already shown:
  1. Look up the existing section's geometry in the geo map
  2. Find the merge station's position in that geometry
  3. Compute the DMP section's offset from the merge alignment
  4. Render the DMP section at that offset
  5. BFS from the bridged section to find its importing-type neighbors
  6. Offset all BFS results into the correct frame
```

### Deduplication

A `coveredExistingSectionIds` set tracks which existing section IDs have already been included. The direct target segment is processed first (most natural positioning), then import-side anchors, then bridged sections.

## Batch Import

When all suggestions are exhausted (or skipped), the early import runs on the transition out of `suggesting`:

1. `applyAllOverlapTreatments` processes all overlap suggestions → mutates `sectionEditsRef` + returns `ExistingShotUpdateLocal[]`
2. `buildBatchImport()` constructs the final `ImportSectionConfig[]`:
   1. Build adjacency from `importing`-type connections
   2. Find anchored sections (zero datum + `existing`-type connections)
   3. BFS from anchors to determine import order
   4. For disconnected components, search the full suggestion pool for fallback `existing`-type anchors
   5. Multi-pass emit: retry sections whose `from_importing` target hasn't been emitted yet
   6. Adjust station indices for deleted overlap shots
3. IPC call: `import_sections(sections, fileHash, existingShotUpdates)`

Sections unreachable after all passes remain in the wizard's manual segment-placement flow until the user places or discards them.

## Station Names

mNemo numbers a section's start station 0 and the end of its N-th shot N, and
divers key their notes to those numbers. The `.dmp` file carries no station
numbers, and every later edit - the shots the importer skips, an overlap trim,
a deleted or added row, any edit after import - shifts the positional
`S:I` station IDs, so the import records each station's mNemo number as its
**name** (`Project.station_names`).

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
  (`src/utils/importSectionConfig.ts`) turns the rows actually sent into
  `ImportSectionConfig.station_names`: the end of emitted row k is station
  k + 1; a start keeps the previous row's number (as an in-app delete keeps the
  surviving start), so a station after an added row stays unnamed; only the
  start of the first DMP row falls back to its number minus 1. A section whose
  first m shots an overlap trim removed therefore starts at m, and the trimmed
  numbers are gone with their stations - one name per station, no aliases.
  Indices are in the emitted, pre-reverse order: the backend mirrors them for a
  reversed import exactly like `station_comments`.
- **After import.** Names are keyed by `StationKey`, so they follow their
  station through later inserts, deletes, splits and reverses; a merge keeps
  both stations' own numbers. The stations grid has a read-only Name column
  (hidden by default; the eye menu shows it) and the detail panel shows
  `S:I · name` on station links. On the map, the Display menu's **Station
  names** labels each named station with its number in place of its ID (it
  and **Station IDs** are one choice), and the image export has the same
  **Station names** option.

## Key Types

| Type | Location | Purpose |
|------|----------|---------|
| `VirtualConnection` | `src/utils/segmentContext.ts` | Client-side connection record |
| `ConfirmedOverlapInfo` | `src/utils/batchImportHelpers.ts` | Merge station choice per confirmed suggestion |
| `ExistingShotUpdateLocal` | `src/utils/overlapTreatment.ts` | Frontend representation of existing shot modifications |
| `ExistingShotUpdate` | `src-tauri/crates/models/src/section.rs` | Backend type for atomic existing shot modifications |
| `ExistingAnchor` | `src/utils/segmentContext.ts` | Position where a DMP section meets an existing section |
| `ExistingSectionGeoEntry` | `src/utils/segmentContext.ts` | Cached segment geometry with frame offset |
| `CrossSectionSuggestion` | `src-tauri/crates/models/src/placement_suggest.rs` | Backend suggestion with score/confidence |
| `SegmentGeometry` | `src-tauri/crates/models/src/section.rs` | Backend segment shot-line data |
| `ContextSectionGeometry` | `src/components/Dialogs/SectionPreview.tsx` | Shot pairs for preview rendering |
| `SectionEditState` | `src/utils/batchImportHelpers.ts` | Per-section editable metadata + shots |

## Key Functions

| Function | Location | Purpose |
|----------|----------|---------|
| `applyAllOverlapTreatments` | `src/utils/overlapTreatment.ts` | Single-pass overlap mutation for all suggestions |
| `buildBatchImport` | `src/components/Dialogs/DmpNewImportDialog.tsx` | Construct ordered `ImportSectionConfig[]` for backend |
| `stationNamesForEmittedShots` | `src/utils/importSectionConfig.ts` | A section's station names (mNemo numbers) from its emitted rows |
| `buildAdjacency` | `src/utils/segmentContext.ts` | Adjacency map from `importing`-type connections |
| `computeDmpBfsContext` | `src/utils/segmentContext.ts` | BFS collecting DMP neighbor geometry + existing anchors |
| `stationPositionFromShotLines` | `src/utils/segmentContext.ts` | Extract XY from backend `shot_lines` (station 0 = `[0][0]`, station N = `[N-1][1]`) |
| `discoverBridgedDmpSections` | `src/utils/segmentContext.ts` | Find DMP sections connected through shared existing sections |
| `computeStationsFromDmpShots` | `src/components/Dialogs/SectionPreview.tsx` | XY station positions from DMP shot headings + lengths |
| `computeOverlapIds` | `src/utils/overlapHelpers.ts` | Set of import shot IDs in the overlap region |
| `computeActionableIds` | `src/utils/overlapHelpers.ts` | Subset of overlap IDs based on merge station choice |
| `computeTargetShotIndex` | `src/utils/overlapHelpers.ts` | Map import overlap ID → target section shot index |
| `interpolateShots` | `src/utils/overlapHelpers.ts` | Average two shots (circular mean for azimuths) |
| `reverseTargetShot` | `src/utils/overlapHelpers.ts` | Swap in/out and reverse azimuth by 180° |

## Backend Commands

| Command | Input | Output | Notes |
|---------|-------|--------|-------|
| `suggest_all_import_placements` | `ImportSectionForSuggestion[]` | `CrossSectionSuggestion[]` | Compares against existing + other imports |
| `get_segment_geometry` | `sectionId` | `SegmentGeometry` | All connected sections' shot-lines, centered on queried section |
| `import_sections` | `sections, fileHash, existingShotUpdates` | `CommandResult` | Batch import with atomic existing shot updates and undo support |

## Backend Safety Guards

The `execute_import_sections` function in `src-tauri/crates/core/src/core/state/command_import.rs` applies two guards before modifying existing section shots:

1. **Doubly-anchored filter**: For each affected section, if the modified shot range is straddled by positional constraints (zero datums, station merges, aligned closures) on both sides, updates for that section are skipped and the skipped shot count is named on the import receipt.

2. **Zero datum offset compensation**: For sections with a zero datum above the modified shots, the section offset (and all sections in the same rigid group) is adjusted after updates to keep the datum station at its original absolute position.

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
whose IPC mock keeps a fake project that `import_sections` and `execute_batch`
really mutate, so "the second pass leaves one section" is measured rather than
inferred from call counts. That a batch of `delete_section` is ONE undo entry is
pinned in Rust: `batched_section_deletes_are_one_undo_entry`.

Backend integration test for overlap interpolation: `src-tauri/crates/core/src/core/state/tests.rs` (`kb5_import_overlap_interpolation_integration`, currently `#[ignore]`).
