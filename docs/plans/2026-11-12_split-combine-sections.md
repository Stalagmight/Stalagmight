# Split Section Here (#335) and Combine Sections, with per-shot credit

Status: DRAFT. Questions are folded in one batch at a time.

## Summary

- **Split Section Here.** Right-click a station or shot in the middle of a section to split the
  section there. No shot is deleted, and the action is one undo step. The maintainer answers on
  #335 are the spec.
- **Combine Sections.** Right-click the station where two sections' ends meet, or select two
  sections in the Sections panel, to join them into one. One undo step. This is the inverse of
  Split.
- **Credit survives both.** Surveyors care about credit for the line they surveyed. Today
  surveyor and date live only on the section, so a combine would credit one section's metres to
  the other section's surveyor. Shots gain optional surveyor and date overrides. That is the
  smallest storage change that keeps credit exact.

## Why not copy Ariane

Ariane's `DATA TOOLS > SECTION -> MERGE` re-labels rows. In Ariane a section is a label on each
row, and connectivity comes from each row's `FromID`. Ariane keeps credit because explorer and
date are stored on every row. Ariane has no split command.

Stalagmight differs in two ways:
- A section owns an ordered shot chain and one surveyor/date/azimuth reference/quality
  (`crates/models/src/section.rs:55-58,122`).
- Station `i+1` is the end of shot `i` (`project.rs:1410-1419`).

So a combine has to stitch two chains and re-point every stored station reference, not only
re-label rows.

## Part 1: per-shot attribution

| | |
|---|---|
| Storage | A new `shot_credits` table: `(shot_uuid PRIMARY KEY, surveyor, date)`, one row per shot that overrides its section. No schema version bump and no change to the `shots` table, like `station_names` (#697): SpeleoDB reads a `.stalx` through OpenSpeleo's Python reader, which accepts exactly v18 and only its known tables. Keyed by shot uuid, not position, so an older build that rewrites `shots` in place cannot shift credit onto another shot. See `persistence/sqlite/shot_credits.rs`. |
| Meaning | No row: the shot inherits both values. In a row, per column: `NULL` inherits it, `''` is explicitly unknown, any other value overrides the section. |
| Effective value | The shot's override, falling back to the section's value. |
| Existing files | No table, so every shot inherits and every file means exactly what it meant before. No backfill. An unreadable row is skipped, never fatal. Older builds drop the overrides when they save. |
| Exports | Ariane TML writes explorer and date per row (`export/tml.rs:791-792`), so it writes the effective value. JSON export and the `stalx-reader` crate gain optional per-shot fields. CSV does not export surveyor today. |
| UI | The shot Detail panel shows the shot's credit where it differs from its section's, with the section's beside it. Table columns wait for the credit view, because saved table views would surface them unasked. |

The `""` sentinel is needed for one case. Combining an unattributed section into an attributed one
must not credit the unattributed shots to the attributed surveyor.

## Part 2: Split Section Here (#335)

These are the maintainer answers on #335:
- **Entry point:** wherever "Insert Station..." appears (`src/components/common/ContextMenu.tsx:489`).
  Survey mode only, and disabled in the read-only closure view.
- **Station target:** splits there. A section's first or last station offers no split.
- **Shot target:** inserts a station at the click point (`split_shot_at_distance`,
  `crates/models/src/project/shots.rs:434`), then splits. A click near either end of the shot snaps
  to that station.
- **Shared station:** duplicated into two independent co-located stations. Merges at that station
  are copied to both copies, as are its name and manual offset. **Loop closures stay on the first
  half only** (a deliberate departure from the #335 answer): a copied closure would count the same
  physical loop twice in the least-squares adjustment, and when it is the second half's only tie
  the orphan-closure rule (#8) promotes it to a merge, which moves the second half by the loop's
  misclosure.
- **Depth:** the halves are independent, so the first half's end takes the incoming shot's
  `depth_out` and the second half's start the outgoing shot's `depth_in`, instead of their
  average. Every other station keeps its position exactly.
- **Gauge:** the least-squares gauge pair moves to the half holding both of its shots, and is
  cleared where it would not resolve. The second half keeps the section's reverse parity.
- **Comment and datum:** the user picks which half keeps the comment (A, B or both) and the zero
  datum (A or B).
- **Metadata:**
  - The first half keeps the original section id.
  - Names default to `<name>-A` / `<name>-B`.
  - Colours start as the original's, with a picker for each half.
  - Surveyor and date are copied.
- **Stage drops:** a plan anchor whose shots moved into the second half is found again by shot
  uuid. Selection is cleared.
- **Dialog summary:** "Splitting X at station Y" / "Splitting X 30 cm (10%) along shot Y", in
  display units and display IDs.
- **Delete-and-split:** keeps its own dialog and gains colour pickers.

**Credit addition:** shots keep their overrides. Then each half is *folded*: if every shot in a
half has the same effective surveyor (or date), that value becomes the half's section value and
the overrides are cleared.

## Part 3: Combine Sections

**Eligible** when an end of section A and an end of section B are the same point: either merged
together, or co-located within 1 mm in measured (not closure-adjusted) positions, which is the
state a fresh split leaves. A join at a mid-section station is a branch and is not offered.

**Blocked, with the reason shown in the menu:**

| Condition | Why |
|---|---|
| Different azimuth reference | B's bearings would change meaning |
| Different quality | Quality weights the least-squares adjustment (`project/lsq.rs:450`) |
| A and B also connect at a second place, by a merge or an aligned closure | The single section would contain a rigid loop. An unaligned closure does not block: it stays as a loop to adjust inside the combined section |
| Result over 99,999 shots | Per-section shot-id budget (`shots.rs:140`) |
| Read-only (closure view, Plan, Export) | Same as every mutation |

**Behaviour:**
- **Kept section:** the lower id by default, with a swap control in the dialog. It keeps its
  direction, and the other section is reversed first if needed, swapping its left and right
  wall distances so the walls stay on the correct sides. All four end pairings are supported.
- **Credit:** every shot of the other section whose effective surveyor or date differs from the
  kept section's gets an override. The dialog shows the result per surveyor and date, e.g.
  "Surveyor A 212 m (2026-03-02), Surveyor B 96 m (2026-04-11)".
- **Direction:** the kept section's value wins (direction is metadata only), and the dialog says
  so.
- **Name and colour:** name defaults to the kept section's (`X-A` + `X-B` defaults to `X`). A
  colour picker defaults to the kept section's colour.
- **Joined station:**
  - Comments: pick A, B or both.
  - Station names: pick one if they differ.
  - The zero datum is kept.
  - Manual station offsets: when both ends carry a different one, the dialog asks which to keep.
  - Merges to other stations are combined and de-duplicated; the A-B join merge is dropped.
  - A zero-length closure between the two ends is dropped.
- **After:** the result is selected. One undo step.

## Visual preview

Both dialogs preview the result on the map before confirming. They reuse existing renderer
primitives:
- Highlight the involved sections (`setSectionSelect`, which accepts an array).
- Ring the station being split or joined (`setHighlightedStations`).
- Dim everything else (`setDimming`).
- Draw the resulting section(s) as a ghost in the chosen colour (`GeometryPreview` /
  `setGhostSection`), updating live as the colour picker changes.

## Implementation notes

- **Backend:** both operations are direct `AppState` methods modelled on
  `execute_delete_shot_and_split` (`crates/core/src/core/state/command_delete_split.rs`):
  - command lock, then `guard_mutation`, then before-snapshot
  - mutate
  - `reconcile_geometry`
  - `push_snapshot_undo` (one `ProjectSnapshotPair`), then `mark_remote_forward_edit`
- **Station references:** re-pointed by resolving to positional refs and re-keying with
  `key_for_ref`. One shared helper relocates the keys of a section's index range to another
  section, used by both operations. Collisions at the shared station are resolved explicitly per
  the dialog choices, never by a `HashMap` `collect()` that silently keeps one entry.
- **Combine eligibility:** a read-only `combine_candidate` query reports the eligibility, the
  pairing, the default kept section, the credit summary and the blocking reason. The context menu
  and the dialog use it, as the Merge tool uses its connectivity check.
- **Registries and gates:**
  - `MUTATION_ENTRYPOINTS`
  - the SpeleoDB unwind file list
  - the telemetry registry and `UNDOABLE_COMMANDS`
  - dialog telemetry
  - the IPC mock and its contract test

## Verification

- **Unit tests:**
  - old files load with no overrides; save/reopen round-trip; TML per-row explorer
  - split at a station and on a shot, the end snap, a single-shot section, credit folding
  - combine in all four pairings; every reference re-pointed; join collisions; each refusal;
    credit overrides; exact undo/redo
- **Metamorphic:** compare geometry, station keys, comments, merges and effective credit per shot.
  - `combine(split(p, s)) == p`
  - splitting a combined section at the join restores both inputs
  - Both operations join the undo fuzzer.
- **Playwright:** split a mid station and see two sections. Combine at the joined end and see one
  section with both surveyors credited. Undo restores each.

## Delivery

1. Per-shot attribution (storage, model, IPC, exports, Detail panel).
2. Split Section Here (#335).
3. Combine Sections.

## Open questions

1. A split leaves the two halves unmerged (per #335). Routes and gas planning are disconnected
   across the split until the user merges them, and the split station's depth is no longer
   averaged across the cut. Confirm this is intended. Combine works either way.
2. Loop closures on the split station stay on the first half instead of being copied to both
   (see Part 2). Confirm.
3. The Ariane importer cuts sections where the explorer changes (`docs/ariane-import.md`, trip
   cut). With per-shot credit it could keep the mainline in one section. Follow-up?

## Follow-ups (not in scope)

- A per-surveyor / per-trip credit view.
- Editing per-shot credit in the shots table.
- Markers at section ends on the map.
