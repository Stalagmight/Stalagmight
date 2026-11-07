# Undo/Redo System

## Overview

Stalagmight uses a command-pattern undo/redo system. Every state-changing operation is a `Command` enum variant. When executed, the system captures enough data to reverse it (`UndoData`), then pushes both onto an undo stack. Undo pops from the undo stack and pushes to the redo stack; redo does the inverse. Any new command clears the redo stack (standard diverging-timeline semantics).

## Core Types

### UndoEntry

```rust
pub struct UndoEntry {
    pub command: Command,                    // The original command
    pub undo_data: UndoData,               // Data needed to reverse it
    pub timestamp: Option<DateTime<Utc>>,  // Set at push time
    pub command_bytes: Option<usize>,      // [#303] serialized command length, measured at push
}
```

Entries are **moved** between stacks, never recreated. The same `UndoData` serves both undo and redo.

### UndoStack

Two stacks of entries (undo + redo; each slot also caches its compressed persisted rows) behind a `Mutex`, with a configurable `history_limit` (default 100, FIFO-trimmed on push) - and, for that count window, at most `COUNT_WINDOW_MEMORY_BYTES` (256 MiB) of estimated size per stack, oldest evicted first and the newest always kept (#576 B20: 100 whole-project snapshot pairs held several GB on a large survey). A separate `command_lock: Mutex<()>` on `AppState` serializes all execute/undo/redo calls to prevent interleaving.

## Two-Phase Undo Data Capture

Commands go through a two-phase capture during execution:

1. **`generate_undo_data()` - before execution**: Captures "before" state. For `CreateSection` this returns `UndoData::None` since the ID doesn't exist yet; `CreateShot` returns a `CreatedShotSnapshot` STUB instead, because its `previous_offsets` can only be read before the insert. For updates it snapshots previous field values. For deletes it snapshots the entity.

2. **`finalize_undo_data()` - after execution**: Fills in data that only exists post-execution. For `CreateSection`/`CreateShot`, this captures the assigned ID and full snapshot. For `CreateLoopClosure`, this grabs the closure object from the project.

## UndoData Variants

| Variant | Used By | Undo | Redo |
|---------|---------|------|------|
| `InverseCommand` | SetStationComment | Execute the inverse command | Execute the original command |
| `PreviousValues` | UpdateShot, SetShotKind, UpdateSectionMetadata | Parse JSON, apply old values, then put the recorded `section_offsets` back (see below) | Execute the original command |
| `CreatedSectionSnapshot` | CreateSection | Delete the section | Restore from snapshot (preserves the original ID **and** the recorded identity - see below) |
| `CreatedShotSnapshot` | CreateShot | Remove the created shot - the exact inverse of the insert, guard-free - then put the recorded `section_offsets` back (see below) | Restore from snapshot (preserves original ID) |
| `ShotSnapshot` | legacy persisted DeleteShot entries (a live DeleteShot records `ProjectSnapshotPair`) | Restore shot at exact position | Re-execute the delete |
| `MergeInfo` | legacy persisted MergeStations entries (a live MergeStations records `ProjectSnapshotPair`) | Restore previous offsets + unmerge | Execute the original command |
| `UnmergeInfo` | legacy persisted UnmergeStations entries (a live UnmergeStations records `ProjectSnapshotPair`) | Re-merge + restore removed closures | Re-execute the unmerge |
| `ClosureInfo` | Create/RemoveLoopClosure | Remove the closure, or put it back at its recorded index | Push closure back (preserves ID) |
| `ZeroDatumInfo` | SetZeroDatum (and legacy persisted RemoveZeroDatum entries) | Restore GPS + offsets directly, at the datum's recorded index | Execute the original command |
| `SegmentMoveInfo` | MoveSegment | Restore section offsets directly | Reapply offset to affected sections |
| `ProjectSnapshotPair` | Batch, DeleteSection, DeleteShot, ReverseSection, MergeStations, UnmergeStations, UnmergeStationPair, MoveStation, MoveComment, RemoveZeroDatum, CreateLoopClosure (distributing strategies only), and the label-only RepairIntegrity / UpdateProjectSettings / SetSurfaceElevation (see below) | Re-record "after" from the live project, then restore "before" JSON (see below) | Re-record "before" from the live project, then restore "after" JSON |
| `None` | Non-undoable commands | N/A | N/A |

### Undo data addresses a shot by uuid, never by its in-session id (#413)

`Shot::id` is regenerated on every load - the reader renumbers each shot to
`section_id * FEATURE_ID_STRIDE + sequence_order`
(`persistence/sqlite/read.rs`) - while undo entries are persisted and reloaded
as opaque JSON with no remapping, and `Section::take_shot_id` hands out ids from
that same space. So a persisted id does not miss after a reopen, it **collides**
with whatever shot now occupies the slot (issue #413, findings 8 and 12: the
undo deleted or edited the WRONG shot), or runs past the renumbered range and
misses everything, failing forever because `undo_inner`'s epilogue pushes a
failed entry back (finding 13).

Every id-bearing undo payload therefore also records the target's `Shot::uuid`,
which is the identity that survives a save (it is what `StationKey::AfterShot`
and the Ariane export already resolve by):

| field | on | addresses |
|---|---|---|
| `uuid` | `CreatedShotSnapshot` | the shot the `CreateShot` created |
| `after_shot_uuid` / `before_shot_uuid` | `CreatedShotSnapshot` | the anchor the redo re-inserts against |
| `shot_uuid` | `PreviousValues` (shot form) | the shot whose values are restored |

The rules the restore paths follow:

- **Resolve by uuid first.** The recorded integer id is only a fallback for a
  LEGACY entry that carries no uuid (all the fields are `#[serde(default)]`, so
  old persisted JSON still loads - no schema bump).
- **A recorded uuid that no longer resolves is a loud failure**, never a
  fall-back to the id: the id is exactly the thing that cannot be trusted.
- **A redo of `CreatedShotSnapshot` refuses to insert a uuid the section already
  holds** - two shots with one uuid is a corrupt project, and the next save's
  `ensure_unique_shot_uuids` would silently re-mint one of them (finding 16).
  The recorded in-session id is likewise only forced onto the recreated shot
  when it is still free.
- **Load drops what it cannot address.** `UndoStack::drop_entries_addressing_a_missing_shot`
  runs after the restore and drops (with a warning) entries whose shot target
  does not resolve, so a stale entry costs one undo step instead of wedging
  every step beneath it. It scans from the top and STOPS at the first entry
  whose undo restores recorded state wholesale (`ProjectSnapshot`,
  `ProjectSnapshotPair`, `SectionSnapshot`, `ShotSnapshot`): such an entry puts
  a whole project or section back in the id space it was recorded in, so
  anything below it may become addressable again once the walk reaches it.

`SetShotKind` records `PreviousValues` rather than the
`InverseCommand(SetShotKind)` it used to, for the same reason: `InverseCommand`
is a newtype around `Command`, so there is nowhere in it to record a uuid
without breaking every legacy payload of that variant. Persisted
`InverseCommand(SetShotKind)` entries still restore through their own arm, by
id, as legacy entries.

### A restore puts identity and ORDER back, not just values (#413, findings 1, 4, 17)

Three entries used to rebuild rather than restore, and lost something each time.
All three fixes are additive `#[serde(default)]` fields, so old persisted JSON
still loads and falls back to the previous behaviour - no schema bump.

- **`CreatedSectionSnapshot` carries a `SectionIdentity`** next to its
  `SectionData`: the shots' uuids, the section's `reverse_parity` and the two
  least-squares gauge anchors. `SectionData` is the CREATE shape and holds none
  of them, so a redo used to run `Section::new` and mint a fresh uuid per shot -
  dangling every `StationKey::AfterShot` anchor onto the section, exactly what
  `CreatedShotSnapshot`'s `uuid` field exists to prevent. `Section::restore_with_identity`
  applies the recorded identity; a legacy entry with no `identity` mints as
  before. The undo REFRESHES that identity from the live section immediately
  before it deletes it, because a save can heal the section (see the next
  section) after the entry was pushed.
- **`RemoveZeroDatum` records a whole `ProjectSnapshotPair`**, not the
  `ZeroDatumInfo` its sibling `SetZeroDatum` still records. Removing a GPS anchor
  can CONVERT a loop closure to a station merge: a sole-bridge closure between
  two independently anchored groups is left alone by
  `convert_orphan_closures_to_merges` (a merge there would put two anchors in one
  rigid component), and dropping one anchor lifts that refusal, so the command is
  classified `can_orphan_closures` and `execute()`'s epilogue converts - in
  session, exactly as the loader would on the next open. `ZeroDatumInfo` can put
  back a datum and a map of offsets; it cannot put back a closure that has become
  a merge. The pair restores both directions verbatim and is self-refreshing, so
  the conversion lands in the `after` half by construction. It is the expensive
  capture, and affordable here because removing an anchor is a rare deliberate
  act rather than a per-keystroke edit. Entries persisted before this change
  still carry `ZeroDatumInfo` and still restore through its arm.
- **`ClosureInfo` and `ZeroDatumInfo` carry the `position`** the forward
  `Vec::remove` took the entry out of, and the undo re-inserts THERE rather than
  appending. `Project::loop_closures` and `Project::zero_datums` are persisted in
  order, indexed positionally by the frontend, and walked in order by
  `resolve_offsets` when it builds adjacency and picks the datum that anchors a
  connected group - so a permutation is not cosmetic. `None` (a legacy entry, a
  create, or a datum that did not exist before) still appends.

### A restored snapshot is re-healed to the standard a save leaves (#413, finding 11)

`save_project` mutates the live project with **no undo entry of its own** - it
mints missing shot uuids, re-mints within-section duplicates, captures the
least-squares gauge pair, stamps a never-saved section's creation time, drops
orphan `section_offsets` and sanitizes non-finite values (`apply_save_heals`,
`core/state/project_io.rs`). A `ProjectSnapshotPair`
captured BEFORE that save holds the pre-heal project in both halves, so restoring
one used to revert the heal as a side effect of an unrelated undo - which made
the project at a given undo depth depend on the ROUTE taken to reach it.

`restore_snapshot_into` - the single place a restored project enters the live
slot - therefore ends in `reapply_heals_after_restore`, which re-applies the
heals that are pure fill-in (missing uuids, and a carry-forward of the gauge
anchors and section creation times the live project already holds). The heals that DROP or REWRITE state
(the duplicate-uuid re-mint, the orphan-offset retain, the non-finite sanitize)
are deliberately not re-applied: an undo has to be an exact inverse, and undoing
a Repair has to restore the very state that Repair healed. The per-heal argument
is on the function.

### A snapshot pair is a SELF-REFRESHING wall (#413, finding 11's re-creation residue)

The carry-forward above repairs the half being **restored**, and it can only do so
where the restored project and the live one hold **the same section** - it copies
anchors from a live section with the restored one's id. A pair whose two halves
disagree about whether a section EXISTS has no such counterpart: a `Batch`
containing `CreateSection`, `execute_delete_shot_range_split`'s re-created tail,
any delete-and-recreate. Undo restores a `before` in which the section is absent,
and the redo then restores an `after` recorded before the save that minted the
section's gauge pair - so the state at that depth still depended on the route.

So `undo_inner` and `redo_inner` both call
`AppState::refresh_snapshot_pair_counterpart` (`core/state/undo.rs`) before
stepping through a pair. A pair is a wall - nothing above it survives the step -
so at that moment the live project **is** the side being left behind: `after` when
undoing, `before` when redoing. Re-recording that side from the live project makes
undo and redo of a pair exact inverses of each other **by construction**, whatever
undo-less change happened since the entry was pushed. `refresh_created_section_identity`
does the same thing one variant over, for `CreatedSectionSnapshot`.

The two are complementary and both are kept: the refresh fixes the half being
LEFT, the carry-forward fixes the half being RESTORED (and is also on the path of
every single `ProjectSnapshot`, rollback and load-time restore, where there is no
pair to refresh). The cost is one project serialization per snapshot-pair history
move - not a new cost class, since recording a pair already serializes the project
twice and restoring one already deserializes it.

### A dust-zero `section_offsets` entry is unrepresentable (#413, finding 27)

`section_offsets` holds a key only for a section that is **actually displaced**,
and "not displaced" has exactly one spelling: the key is absent. A value within
`Project::OFFSET_DUST_M` (a micrometre) of the origin in both components is not a
position, so it is never stored - in memory or on disk, by any writer, computed or
restored. The invariant is stated on `Project::write_section_offset`, which is the
one write point everything routes through, and enforced on maps this app did not
write by `Project::normalize_section_offsets`.

It has to be an invariant rather than a per-writer rule because no rule local to a
write can satisfy both identities the app owes. `MoveSection(+d)` followed by
`MoveSection(-d)` must leave the map it found, and a writer sees only its own
arguments:

- from an **absent** key, `(0 + d) - d` lands a femtometre off zero and the key
  must end absent - the route that did nothing has none either (#413 finding 2's
  dust half);
- from a **present all-zero** key, the same arithmetic must end present.

Both were real. The second is what the metamorphic pair
`move-out-and-back-is-identity` reported once a project carrying a legacy all-zero
row reached it, and it is the same contradiction finding 27 hit from the undo side.
Removing the state removes the contradiction: there is no present all-zero key to
preserve, so the first rule is the only one left, and presence can no longer differ
between two states that agree geometrically.

Two boundaries enforce it on input this app did not produce:

- **the loader**, in `Project::redrive_offsets_after_load` after the zero-trust
  re-solve. The re-solve does not reach such a row (the cascade never seed-adjusts
  an anchor or a free-floating component), so it is dropped explicitly. A legacy
  `.stalx` therefore opens without it, and settles on the next save.
- **the snapshot restore**, in `AppState::restore_snapshot_into`. Undo history is
  persisted and reloaded across sessions, so an entry written by a build predating
  the invariant can carry one in its payload; a restore may not install a state the
  app cannot otherwise reach.

Nothing observable changes when a row is dropped: every consumer reads the map
through `Project::section_offset` or an inline `.get(..).unwrap_or((0.0, 0.0))`, so
an absent key and an all-zero one are the same geometry, the same coordinates and
the same drawn position. The **file-format gate is unaffected** for the same reason
it always was: the app never writes a dust-zero row, so a save-load-save cycle on a
file this app wrote is a fixpoint. A legacy file sees a one-time settle - the load
drops the row, the next save omits it - and is a fixpoint from then on.

##### Why the bound is a micrometre and not a picometre (#469)

The gate is absolute; the residue it swallows is **relative** to the coordinates
being summed, because it is a few ULPs of them. It was 1e-12 m, chosen against a
corpus measured when coordinates were metre-scale - and #469's frame
renormalisation (`Project::renormalize_frame`) legitimately moved them onto the
inter-datum distance, so the residue moved with them and started landing on both
sides of the gate. Two cascades over the same geometry, seeded differently, then
disagreed about whether a section HAS a key, which is structural rather than
numeric: `CLOSURE_SHAPES=1` seed 18 in `fuzz_undo_tests.rs`.

Re-measured with every gate site instrumented, the residue is one to five ULPs of
the coordinate magnitude: 1.96e-11 m at the ~8e4 m a real georeference produces,
and 4.66e-9 m at the 1.8e7 m the horizon corpus reaches with datums scattered
worldwide. 1e-6 m clears that by two decades and stays four decades below
`Project::MERGE_ALIGNMENT_INVARIANT_M`, which is the ordering the whole chain
rests on. The full table is on `Project::OFFSET_DUST_M`;
`the_dust_gate_clears_the_residue_at_the_georeference_ceiling` and
`the_dust_gate_is_decades_clear_of_both_ends` (models/project/tests/merge_and_offset_tests.rs) re-run the
measurement and pin the clearances. The gate stays a **pure function of the value
written** - never scale-aware, which would let two routes disagree by design - and
a residue that ever approaches it means re-measuring the constant, not widening an
oracle.

#### An inverse still RESTORES the offsets it re-derives

Two undo arms re-derive the offsets their inverse recomputes, and both carry the
pre-command map of the edited section's rigid connected set
(`Project::capture_rigid_connected_section_offsets`) and hand it back verbatim
afterwards:

- `UndoData::PreviousValues { previous_offsets }` - the undo re-executes
  `update_shot`, whose `realign_after_shot_edit` re-derives the group's offsets.
  Captured only for a geometry edit, which is the only one that realigns.
- `UndoData::CreatedShotSnapshot { previous_offsets }` - the undo removes the shot,
  and a mid-section removal pays the section's offset back by the segment it took
  out.

The invariant closes the **presence** question, not the **value** one. The realign
recomputes an offset from section-local coordinates and is self-correcting, but the
removal above genuinely accumulates: it computes `(v + d) - d`, which is not `v` in
IEEE. One cycle of that drift is below every oracle's tolerance, which is exactly
why no fuzz survey reaches it, and it is the shape the horizon mode watches
accumulate (finding 19). The captures make the undo bit-exact instead. They are
bounded - the rigid connected component only - and elided entirely when empty, so a
metadata-only shot edit carries none.

Only the undo direction needs them. A redo re-executes the forward command from a
state the undo restored exactly, and the forward path is deterministic, so it
reproduces the recorded post-command map.

#### `ZeroDatumInfo` captures the WHOLE project, not a connected set (#469, item 1)

`UndoData::ZeroDatumInfo { previous_offsets }` is the one capture whose scope is
not a connectivity question, and it takes `Project::capture_all_section_offsets`
rather than the merge group its two siblings above take.

Local `(0, 0)` MEANS the FRAME datum's GPS - the first entry of `zero_datums` that
still resolves (`Project::frame_datum`): `set_zero_datum` lands that datum
on the origin and puts every later one at `gps_to_local(its gps, frame.gps)`, and
the map, the data table's GPS column, the Ariane export's `START` lat/long and the
GlobalView pin all read local metres against it. So changing WHICH datum is first
re-defines every stored coordinate, and `Project::renormalize_frame` answers that
by translating the whole survey - free-floating, datum-less components included -
so the promoted datum's station lands back on the origin. Nothing bounded by
merges or aligned closures can hand that back.

ADDING the survey's first datum is the same frame change seen from the other end:
before it there is no frame, after it every coordinate is read against this one, so
the add re-frames the whole survey too (it does NOT translate just the datum's own
merge group, which would move that group relative to every other component). And
undoing that add REMOVES the frame again, so this is the arm that has to hold the
whole map.

The add performs its renormalisation INSIDE `Project::set_zero_datum`, not in the
epilogue: the subsequent-datum arithmetic reads "the frame datum's station sits at
(0, 0)" on every call, so the invariant has to hold when the model call returns,
and the three routes with no epilogue (a batch's second datum, the TML/DMP import
placement loop, redo's re-execute) would otherwise place a second datum against an
origin the first one does not sit on. It is idempotent (the delta is MEASURED), so
the gate below is a no-op on top of it.

Otherwise the renormalisation runs only in the FORWARD epilogue,
`AppState::forward_epilogue` (`core/state/command_epilogue.rs`), which every forward
entry point runs - `execute()`, the batch, the four delete/split paths, Insert
station, both imports, the project settings, the comment move (#576 A4; pinned by
`forward_epilogue_tests.rs`) - and inline in `repair_open_project_integrity`, which
settles its own derived state in an order the shared steps cannot express. Never in
`reconcile_geometry` - which
`undo()`/`redo()` share - and never in the loader. Every command that can reach it
records either a `ProjectSnapshotPair` or this widened map, so no inverse
re-derives a frame; and `resolve_offsets`' cascade never re-derives a seed's
offset, which makes a whole-project translation a provable no-op for the loader, so
save/reopen converges without teaching it anything.

The other three offset captures stay merge/rigid-scoped: no per-keystroke shot edit
can change the frame, and widening those would put an N-entry map on every one.
Entries persisted with a narrow map still restore through the same arm - what they
recorded, and no more.

#### The one derived field an inverse RE-DERIVES: `is_aligned` (#469, item 2)

`reconcile_geometry` runs on every history move as well as every forward command,
and it now re-derives two things rather than one: the section offsets
(`realign_merges`) and every snap-to-merge closure's alignment claim
(`Project::remeasure_closure_alignment`). The claim is swept on BOTH sides of the
realign - before, because an aligned closure is an EDGE the offset cascade
traverses, so the flags are an INPUT to the solve and must be a function of the
geometry rather than of the route taken to it; after, because the realign can move
a section, which changes the very gaps the claim is measured from. No undo arm
captures `is_aligned` at all, and none needs to.

That looks like the opposite of the rule above, and it is not, because the claim is
a **total function of the rest of the state**: `Move` strategy, a measured end gap
under `MERGE_ALIGNMENT_INVARIANT_M`, and no double-anchoring, none of which reads
any closure flag (see `Project::closure_alignment_stamp`). Every state the history
records therefore satisfies `flag == stamp(state)`, so an inverse that restores the
rest of the state reproduces the flag it recorded. The frame renormalisation has no
such property - it is a translation, not a function of what it translates - which is
why that one is forward-only and this one is not.

The direction matters. A sweep that could only DEMOTE would break the roundtrip
outright: `UpdateShot`'s `PreviousValues` carries no closure flag, so tearing an
aligned closure and undoing it would hand the geometry back with the claim stuck
false - a state the history never held. The sweep sets the flag BOTH ways.

Repair carries the same sweep itself (it never goes through `reconcile_geometry`),
and so does `redrive_offsets_after_load`, each after its own re-solve. A boundary
that left a stale claim behind would have the next command's sweep correct it, i.e.
produce a difference against a snapshot recorded one command earlier.

### An inverse consults no user-intent guard (#413, findings 3 and 6)

**If a forward command was permitted, its inverse must be too.** Undo of
`CreateShot` used to run the user-facing `delete_shot`, guards and all. Those
guards say what a USER may do - do not tear a loop apart, do not touch
collaborator-locked geometry - and both of them refuse an undo in practice: the
frozen-shot check, and the shift-blocked loop/datum pair behind it. A refused
undo is not a no-op: `undo_inner`'s epilogue pushes the entry back, so it fails
identically forever and every entry beneath it is unreachable. That is a WEDGED
history, and through the history dropdown a multi-step `undo_to` wedges partway,
leaving the earlier steps applied.

`AppState::remove_created_shot` (`core/state/command_shot.rs`) is the undo path.
It keeps the structural rewire (`Project::remove_shot_with_rewire`) and consults
neither guard. `Command::DeleteShot` still runs `delete_shot` unchanged, so a
USER delete is refused exactly as before.

It also picks the right inverse per insert shape, because `remove_shot_with_rewire`
is not the inverse of one of them:

- **Append into an EMPTY section** (both recorded anchors absent, position 0, and
  still the section's sole shot) -> `Project::remove_appended_first_shot`
  (`models/project/shots.rs`). `remove_shot_with_rewire`'s `pos == 0` branch
  answers the user's delete-the-first-shot question - station 0 goes away,
  station 1 becomes the new section start, refs keyed to station 0 are dropped
  and `adjust_section_offset` slides the section so the survivors keep their
  world positions. With exactly one shot there is no survivor: the section slid
  by the shot vector while the offset-neutral `Project::add_shot` gave nothing
  back, and the redo re-appended offset-neutrally, so every cycle slid it again
  (the RATCHET). The station the append CREATED is station 1, `AfterShot(uuid)`;
  that is what the inverse removes, leaving `section_offsets` and station 0's
  merges, datums and comments untouched.
- **Everything else** -> `remove_shot_with_rewire`, which already mirrors the
  insert exactly: an append into a non-empty section leaves no station beyond the
  deletion point, so `anchored_sides_of_edit` sees no anchored tail and
  compensates nothing; a mid-chain insert and a prepend are undone by the same
  offset arithmetic that made them.

### Why Some Undos Restore Directly Instead of Re-Executing

`ZeroDatumInfo` and `SegmentMoveInfo` store exact previous offsets and restore them directly rather than re-computing. This is critical: re-execution could produce different results if the connectivity graph changed between the original execution and the undo.

## Batch Commands

Batch operations (`Command::Batch { commands }`) are **atomic**:

- A pre-snapshot is taken before any sub-command runs
- Sub-commands execute via `execute_without_undo()` (no individual undo entries)
- A post-snapshot is taken after all succeed
- If any sub-command fails, the entire batch rolls back via the pre-snapshot
- The single `UndoEntry` uses `ProjectSnapshotPair` - undo restores "before", redo restores "after"
- The UI shows batch as one history item; the hover preview lists each sub-command

## Non-Undoable Commands

These are never pushed to the undo stack: `OpenProject`, `SaveProject`, `CloseProject`, `CreateProject`, `MergeProjects`.

## Shot ID Display Positions

Shot IDs are internal and opaque. After database reload they follow the formula `section_id * 100_000 + sequence_order` (0-indexed). Before first save they come from the section's counter, which `Section::new` (`models/section.rs`) starts at `section_id * 100_000` and which counts up as shots are created, so only section 0's are plain 0, 1, 2.

The `shot_display_position(shot_id, section_id)` helper in `models/shot.rs` converts these to 1-indexed display positions for both formats. All user-facing text (command descriptions, preview data) uses this helper so "Update shot 13:5" means section 13, 5th shot - not the raw internal ID.

## Persistence

Undo and redo stacks are serialized to the SQLite database synchronously on project save and restored on project open.

- Each entry is stored as `(command_json, undo_data_json, timestamp_iso8601)`
- Only the last N entries are persisted (configurable limit), except in the hybrid mode below
- Persistence is synchronous to avoid race conditions with concurrent readers
- Serialization failures are logged but don't block the save
- Deserialization failures skip invalid entries and load whatever is valid
- On project close, stacks are cleared from memory (DB retains persisted entries)
- Files written before schema v7 get their persisted stacks dropped on open: pre-v7 builds serialized untouched nullable `ShotUpdates` fields as explicit `null`s, which the v7 `double_option` semantics would replay as "clear the field" (see `UNDO_FORMAT_SAFE_VERSION` / `drop_legacy_undo_history` in `persistence/sqlite.rs`)

### What bounds a history LOAD (#469, #473)

A `.stalx` is a file the user opens and nothing in the schema bounds `undo_stack` /
`redo_stack`, so both tables are attacker-controlled. Three separate bounds apply to a
load, and the reason there are three is that the first two multiply rather than compose:

| Bound | Constant | What it bounds |
|---|---|---|
| Row count | `MAX_PERSISTED_HISTORY_ROWS` (65536) | how many rows one load reads |
| Per blob | `MAX_DECOMPRESSED_BLOB_BYTES` (256 MiB) | how far ONE blob may inflate - gzip or legacy |
| Per open | `MAX_HISTORY_LOAD_BYTES` (576 MiB) | the total across BOTH stacks of one open |

The third is not redundant. With only the first two, the default 100-row count window
allows 100 x 2 x 256 MiB = 50 GiB, and Full mode's row ceiling makes it 32 TiB nominal.
That was measured, not theorised: six rows whose blobs each declared 250 MiB, every one
inside the per-blob ceiling, came out of a 14 MB file as 3.1 GB of `String` and the load
returned `Ok`.

**"Per open", not "per stack"** - an open reads undo, then redo, and holds both results at
once, so a budget scoped to one stack bounds the pair at 2 GiB. `Database::load_history_budget`
exists so the open creates one `InflationBudget` and both loads share it; the
`*_within` loader variants are the ones that take it.

Mechanically, and in the order the checks actually fire:

1. **The declared-size pre-scan is what stops a hostile file.** Every blob states its
   inflated size in gzip's ISIZE trailer, so the whole window is priced by walking rows
   sqlite already read - refusing at zero allocation, naming the real problem ("this file
   claims 50 GiB of history"). It is checked against what the budget has LEFT, so the
   second stack only gets what the first did not spend.
2. **The declared size is attacker-controlled, so it is trusted only in the refusing
   direction.** What makes `actual <= declared` hold for every row - and therefore
   `actual total <= the sum the pre-scan approved` - is NOT the same mechanism in both
   branches, and the difference matters to anyone editing either:
   - **gzip**: the read is capped at exactly what the blob claimed, so one that
     understates and then inflates past its own claim trips that cap.
   - **legacy plain-UTF-8**: there is no read cap. The bound holds because the declared
     size IS the arithmetic worst case for `from_utf8_lossy` (3 bytes of U+FFFD per input
     byte), so no input can exceed it. Change how legacy blobs are decoded and this bound
     goes with it.

   The per-blob ceiling binds both branches - it is checked before they diverge - so a
   legacy blob is bound by it at its REAL `from_utf8_lossy` output - the row's own length
   when it is valid UTF-8, and up to 3x only when it is not.
3. **The running `InflationBudget` is defense in depth, not the mechanism.** Given the
   pre-scan it is unreachable by construction - `sum(declared) <= remaining` is
   established before any row reserves its own `declared`. It is kept because it is what
   still bounds a future caller that reaches `decompress_json_under` without a pre-scan,
   and it is what carries the spend from the first stack into the second. Do not describe
   it as the thing that stops the attack.

The old 1 GiB was derived from what a save could legitimately produce - and note WHICH knob bounds
that, because there are two and swapping them changes the answer by 5x. What a
count-window (non-SpeleoDB) project WRITES is bounded by `undo_persist_limit`, **default
20** (read in `project_io.rs::rows_to_persist`). `undo_history_limit` (100) is the
in-memory/read window and never reaches disk. Before #576 B20 the honest worst case was 20
`ProjectSnapshotPair` entries on a 20,000-shot survey at ~17 MB each = ~340 MB per stack,
~680 MB for the pair - under the ceiling, which is the point.

Since #576 B20 the writer is bounded in bytes as well: of those newest
`undo_persist_limit` entries, a save keeps only the newest whose estimated sizes fit in
`UNDO_BYTE_BUDGET_BYTES` (32 MiB) per stack - the newest always kept, even alone over it
(`UndoStack::persist_window`). So a count-window save now writes at most ~32 MiB of JSON
per stack, or one larger newest entry - whose biggest blob the per-blob ceiling bounds.
The per-open ceiling is derived from that (#576 NF-W4U-2): `2 x (MAX_DECOMPRESSED_BLOB_BYTES
+ UNDO_BYTE_BUDGET_BYTES)` = 576 MiB, down from 1 GiB, room for one stack's window or one
maximal newest entry (plus the small command blobs) on each side. A file an older build
wrote with more history than that (the ~680 MB pair above) opens without it, behind the
discarded-history banner, as one over 1 GiB always did; its next save writes the capped
window.

The number bounds RESERVATIONS, not process RSS. Two terms sit on top of it, and the
second is not small: `read_to_end` doubles its buffer, so one blob transiently costs
~1.5-2x what it reserved; and the whole window's COMPRESSED rows are materialised by
`newest_history_rows` before the pre-scan can price them, bounded only by the row cap
times sqlite's own `SQLITE_MAX_LENGTH`. That second term is linear in file size rather
than amplified - no bomb - but on a multi-gigabyte `.stalx` it dominates, and it is
#469's residual rather than something this ceiling addresses.

A guard for that second term was written and then **reverted**, which is worth recording
so the next attempt starts further along. It summed `length(command_json) +
length(undo_data_json)` over `(SELECT ... ORDER BY sequence DESC LIMIT ?1)`. sqlite's
header-only `length()` optimisation needs a DIRECT table-column reference; against a
subquery output it cannot flatten, so the co-routine materialised every blob and the
whole history was read TWICE per open - a measured **46,616 us against 11 us** for the
equivalent `WHERE id IN (SELECT id ... LIMIT ?1)` on a 400 MB history, for identical
sums. Any future version must use the id-set shape, charge the shared `InflationBudget`
rather than the bare constant (otherwise it is per-stack and an open reaches 2x),
propagate its error instead of failing open, use `octet_length` (plain `length` counts
CHARACTERS on a TEXT-typed row), and carry a test. Streaming the sum inside the existing
row loop avoids the second read altogether.

### What the banner does NOT cover

It fires when a stack could not be READ at all. It does not fire when the row cap
(`history_load_row_cap`) silently drops the oldest entries of a very deep history: that
is a bounded, by-design trim of rows the restore's own `enforce_depth_limit` would have
evicted anyway, not a failure. The asymmetry is deliberate but easy to misread, because
both outcomes end with the user holding less history than the file contains.

Tripping any of them fails the WHOLE load, and the project then opens with empty history
rather than not opening. That is deliberate, and since #473 it is no longer silent: a
discarded history raises a persistent banner (`history-discarded-banner`), because losing
every undo AND redo entry behind a log line is the silent state change
[notifications.md](notifications.md) forbids.

`AppState::history_discarded` is what the banner reads. It is written under the same
`project` mutex acquisition that commits the project it describes, so it cannot outlive
or mis-attribute one - and **every** project-slot commit must write it, loads and creates
alike. A create that forgets leaves the previous project's verdict standing and puts a
data-loss banner over a brand-new empty project; that regression is pinned by
`creating_a_project_clears_a_previous_projects_discarded_history_flag`.

### What the round trip does NOT preserve

Two things survive the write and die on the read. Both are open on
[issue 413](https://github.com/Stalagmight/Stalagmight/issues/413) and both are
found by the entry-content oracle in `fuzz_undo_tests.rs`, which compares the
`UndoHistoryItem` rows before a save against the ones after the reload:

- **A cleared section field (finding 5, command side).** `SectionUpdates` has
  the `double_option` treatment `ShotUpdates` got in v7 on neither `name`,
  `surveyor` nor `date`, so `Some(None)` ("clear it") and `None` ("leave it
  alone") both serialize to `null` and both read back as `None`. The persisted
  bytes are identical; the meaning is not. A reloaded entry stops listing the
  field it cleared, and - because `redo_inner` re-executes `entry.command` for a
  `PreviousValues` entry - its REDO silently leaves the field alone.
- **The last ULP of an `f64` (finding 23).** `serde_json` writes floats through
  ryu's shortest round-tripping form and reads them back with a parser that is
  not correctly rounded past 16 significant digits, so it cannot always read its
  own output: `13.565000000000001` comes back as `13.565`, one ULP low. Enough
  to move a two-decimal preview value across a rounding boundary. The
  `float_roundtrip` feature on `serde_json` is the switch that closes it.

### Every save is a full rewrite (#303)

`UndoStack::rows_to_persist` serializes every entry in the persisted window and
`Database::replace_history_stacks_with_timestamps` writes the lot for both stacks in one
transaction, `DELETE`-all then `INSERT` with `sequence` renumbered 0..n. There is no incremental path and there cannot be one:
`save_project` builds a fresh `.stalx.tmp` and renames it over the target, so every save
writes into a file that has no undo tables at all. Anything a save skips is gone.

This was tried the other way. An incremental writer (upsert only the ids at or after the
first changed one, delete what falls outside the surviving range) SILENTLY DROPPED the
history: a save with no undo change since the last one - an idle autosave, or the push's
own flush right after another save - wrote zero rows into a file that had none, so the
commit it produced carried no history at all. The unit test that blessed it ran the
database helper against a database that persisted between calls, which is exactly the
assumption the save path breaks. The whole apparatus (`PersistPlan`, `StackSync`, the
stable ids) has been removed rather than left standing as scaffolding.

The cost is real and is what `UNDO_BYTE_BUDGET_BYTES` bounds - for BOTH kinds of window
since #576 B20. A 20-entry window was not "fine": a delete-heavy session's whole-project
snapshots are megabytes each, and a synthetic 20,000-shot project with 20 whole-project
steps took 8.7 s per save, every autosave included (release build); capped, 1.0 s.

A save does not re-COMPRESS the window, though (#576 B20-D2). Each stack position keeps
its entry's gzipped rows once a save has computed them (`UndoStack::stored_rows_to_persist`,
written by `Database::replace_history_stacks_stored`, the same bytes the JSON path stores),
so a save compresses only the entries that are new since the last one: on that project an
idle autosave went from 0.78 s to 0.21 s and a save after one more step from 0.92 s to
0.67 s (release build, same machine, back to back). The cache cannot describe a different entry: it moves with the
entry, an entry popped for undo or redo leaves it behind (entries are only ever changed off
the stack), and any mutable access to a stacked entry drops it.

## Hybrid mode for SpeleoDB projects (#303)

A SpeleoDB-linked project is a binary `.stalx` committed to a git repo, so what its undo
stack holds at push time is what every commit stores forever. Two policies, resolved by
`AppState::undo_policy` from `speleo_link.undo_mode` (per project, rides to the server) then
`SpeleoDbConfig.undo_mode` (the application default):

| | `Full` (default) | `Disabled` | Local project |
|---|---|---|---|
| In-memory depth | byte budget | `undo_history_limit`, at most 256 MiB | `undo_history_limit`, at most 256 MiB |
| Persisted depth | byte budget | `undo_persist_limit`, at most the byte budget | `undo_persist_limit`, at most the byte budget |
| Rides in the push | yes (undo only) | no | n/a |

**Both stacks get the budget, so a Full-mode save can write up to 2x
`UNDO_BYTE_BUDGET_BYTES`.** That is an accepted ceiling rather than growth: each stack is
bounded, and a confirmed push clears the undo history. Dropping redo from the Full-mode
save was tried and reversed - it bought a smaller save on exactly the projects whose saves
are largest, and cost redo-after-reopen on all of them. No redo reaches the server in any
mode (`redo_stack` is unconditionally in `PUSH_SNAPSHOT_LOCAL_TABLES`), so both the cost
and the benefit are local.

**The byte budget** (`UNDO_BYTE_BUDGET_BYTES`, 32 MiB of serialized JSON ~= 2.5 MB per
commit) replaces the entry-count window rather than joining it. Count is the wrong unit
here: `Delete shot`, `Delete section`, `Reverse section` and every `Batch` store a
`ProjectSnapshotPair` - two whole-project JSON snapshots, about 4.2 MB on a 5,000-shot
project - so a fixed 100 entries is anywhere from kilobytes to half a gigabyte. The budget
buys roughly 32,000 ordinary entries, or ~8 whole-project snapshot pairs on a 5,000-shot
project, or ~2 on a 20,000-shot one. The newest entry is always kept even if it alone
exceeds the budget. Trims log once with a count.

Because every save rewrites the persisted stack, the budget also bounds the worst-case save
(~0.3 s of gzip at the measured throughput, which a 30 s autosave absorbs). Raising it
deepens snapshot-heavy history and slows every save in proportion. For scale: the old fixed
window persisted 20 entries with NO size cap, which on a 20,000-shot project is ~336 MB of
JSON per save - so this is much cheaper in the worst case and vastly deeper in the common
one, at the price of shallower history for repeated whole-survey operations on very large
projects. (The count window of a local or Disabled-mode project is now capped by the same
budget when it is saved - #576 B20 - so it has the same trade: on a 20,000-shot project
about two whole-project steps survive a reopen instead of twenty.)

**Redo is never uploaded**, in any mode. A redo branch replays FORWARD onto a state the
server has now committed past, so it is the stale-replay hazard with none of the undo
side's value.

### When the stack is cleared

| Event | Effect | Why |
|---|---|---|
| Confirmed push | entries with `timestamp <= push_start` dropped; redo cleared | they are in the commit now, so keeping them would duplicate them in the next one. Bounded by push-start because an edit recorded after the snapshot keeps its dirty bit, so its history has to survive with it. A redo keeps the entry's original timestamp, so an entry a redo put back while the upload ran (newer than anything the snapshot held) is kept too: it carries a transient `replayed_at`, and the cut (`drop_pushed_entries`) keeps entries replayed after the cutoff unless the snapshot carried them (#576 A18(b)) |
| Any server `.stalx` becoming the local cache file | undo/redo tables emptied in the downloaded FILE | `InheritedUndo::Drop` at the `replace_cache_stalx*` chokepoint - pull, fast-forward and revision swap all funnel there |
| Lock acquired over a copy that equals git (`UpToDate` + clean) | in-memory stacks cleared | the history describes already-committed operations. The clean check is what lets lock, edit, unlock-without-pushing, re-lock keep the user's own history |
| Force-edit | in-memory stacks cleared | no lock and no reconcile, so the equals-git fence cannot be evaluated; that session already carries the unmergeable warning |

Clearing on adopt happens in the FILE, not in memory: the load path reads those tables into
the live stack, so a memory-only clear would leave the history on disk for the next open.

The reason it is cleared at all is #194 b3 - an adopting collaborator pressing Ctrl-Z would
replay the pusher's command, with its prior-state payload, against a tree they have not
looked at. Shipping the history relaxes the strip that used to prevent that, so the guard
moves to these rules plus the one explicit route below.

### Unwind: undoing past a commit

`sync::unwind_target` walks BASE first, then first-parents:

1. After a push the working copy IS the new commit and its history was just cleared
   locally, so the FIRST Unwind loads that same commit. Content does not move; the undo
   list refills with the operations the user just pushed.
2. Undo through those and the copy sits at the parent's state, so the next Unwind loads the
   parent, and so on back to the root.

Merges take `parent_ids[0]`. The chain ends at the root and at SpeleoDB's own creation
commit, where the affordance is hidden rather than offered as a step that cannot be taken.

The position lives in `speleo_undo_chain` - a single-row, local-only table, stripped from
every push snapshot. BASE cannot double as it: a restore stamps BASE at the server TIP so
a later push lands forward, which is what we want and also what stops BASE describing where
the walk is. It is on disk so an Unwind interrupted by a restart resumes, and is cleared by
a confirmed push, a lock acquisition, a fast-forward, and a plain View / Restore.

Mechanically it is `speleodb_restore_revision` with `keep_undo_history: true`, so it reuses
the whole swap path. What the flavour adds is `speleodb_guard_unwind`, enforced in the
backend: the in-memory undo stack must be EMPTY (that is what makes "the copy equals a
commit" true), and any dirt must be a previous Unwind's own doing, proven by the chain
pointer - which is what makes the `discard_local` waiver safe from the second step onwards.

Dirt with **no** pointer is real un-pushed work, and it is not refused outright - it is
**warned about**. That state is ordinary, not exotic: the entry is offered exactly when the
undo stack is empty, which is also what undoing everything produces, and that leaves the
copy dirty with the undone steps on the redo stack. A flat refusal there meant the app
offered a button whose only outcome was an error. So `unwind_discards_unpushed` (one
predicate, two readers) both fills `SpeleoUnwindTarget.discards_unpushed` and decides the
guard: the confirmation raises a red warning on the flag when it OPENS, naming the survey
changes as well as the history (a copy can be dirty for reasons no undo explains - a
byte-budget trim, a legacy file - and there content really is replaced), and confirming
sends `discard_unpushed` back. Without that acknowledgement the guard refuses exactly as
before, so no other `keep_undo_history` caller can discard un-pushed work by accident.

The frontend offers it as one entry pinned below the undo history list, and clicking it
opens a confirmation rather than acting. Undo / Ctrl+Z on an empty stack still do nothing.

### ...and forward again

The walk is not one-way. **Redo into next SpeleoDB version** is the mirror: where the
Unwind takes the first parent, `sync::rewind_target` finds the child on the path back to
BASE and loads ITS history, so a user can step across commit boundaries in both
directions. It is the same `speleodb_restore_revision` with the same
`keep_undo_history: true`; only two things differ by direction.

**Which commit.** The child is found by walking BASE's own first-parent chain down to the
current position, never by scanning for "any commit whose parent is here" - an abandoned
line can share a parent, and `authored_date` cannot break that tie. BASE is the end of the
line: past it lies another session's push, and adopting that is an Update, not a step
through our own history. Landing on BASE is reported as `is_latest` so the confirmation can
say "return to the latest version" rather than name a sha.

**Which stack must be empty.** Backward it is the undo stack (everything undone, so the
tree sits at the parent's state). Forward it is the **redo** stack: every operation of the
commit being sat on has been re-applied, so the tree really is that commit's and the
child's history stacks onto it cleanly. `speleodb_guard_unwind` takes the direction and
picks accordingly; checking the wrong one would let a half-redone copy jump a commit and
silently drop the operations still parked on the other stack.

**An edit ends the forward direction, with no code of its own.** A forward step is only
defined relative to the chain pointer, and a forward command drops it (above), so after an
edit `speleodb_rewind_target` returns `None` and the entry disappears rather than offering
to discard that work. The backward direction stays available - it just re-warns first,
because the pointer no longer excuses the dirty bit.

`SpeleoRewindTarget` deliberately carries no `discards_unpushed`: a forward step requires a
pointer, and the predicate is false whenever one exists, so the field could only ever be a
constant `false` - a dead warning that would read as though the case had been considered
and could fire.

### Loaded history does not ride into the next commit

A chain step loads a past commit's history so the user can keep stepping. That is
scaffolding, not this session's work - so an edit on top and a push used to upload the lot,
recording every loaded operation a second time in the new commit. An ordinary session never
does that, because taking the lock clears the stack.

The rule: **keep it locally, cut it from the upload.** The loaded entries stay in the stack
so the user can still undo further at any point before the push - they are deliberately NOT
dropped on the first edit - and `sanitize_push_snapshot` deletes them from the snapshot
COPY on the way out, leaving the live stack untouched.

What separates them is `speleo_undo_watermark`: the instant, recorded at load time, through
which everything was inherited. **A timestamp, not a stack position** - entries are written
back with `sequence` = position, so after undoing into the loaded history and editing, the
user's new row occupies the id a loaded row had, and cutting by id would delete their work.
A timestamp belongs to the entry rather than its slot, so it survives undo, redo and
truncation. It is stamped as `max(now, newest loaded entry)`, because a pusher with a skewed
clock can stamp entries in the future and those must still be cut; rows with no readable
timestamp are cut too, the same reading `drop_entries_up_to` gives them.

It lives in its own table rather than beside the chain pointer because the lifetimes differ:
a forward edit clears the pointer but must **not** clear the watermark - outliving the edit
is the whole point. It is cleared where the stack legitimately becomes all-this-session
again (confirmed push, lock acquisition, fast-forward, any `InheritedUndo::Drop`), is
stripped from push snapshots like its sibling, and is preserved across `save_project` - an
autosave that dropped it would silently reintroduce the duplication.

One consequence: unwinding to an ancestor and pushing **without editing** yields a commit
carrying no undo history at all, since everything in the stack is inherited. That follows
from the rule, and those operations still live in the commits they came from.

## Lock Ordering

`command_lock` is always outermost. It serializes every command, undo and redo, and
(#576 A1) every PROJECT-SLOT COMMIT too: both loads, both creates, close, the
delete's check-and-teardown and the SpeleoDB cache close all write the slot through
`commit_open_session` (`core/state/session_commit.rs`), which demands a
`CommandsHeld` proof, so a commit can never land between a command's mutation and
its undo push and leave the old project's entry on the new project's stack. A load
takes it only after its file read, and `execute` dispatches Open/Close/Create
BEFORE taking it (they take it themselves). Done's local halves - the pre-push
flush, render and snapshot, and the link and history writes after the upload
concluded - hold it as well, never across a network call (#576 B35(d)), and so does
every other write of the open file's SpeleoDB link outside `execute` (#576 NF-W4C-1/2):
Publish's relocation save and link stamp (one span), the Ariane adoption's create,
import, save, promote, reopen and link stamp (one span, #576 NF-W4C-3), Start
Editing's commit, the metadata PATCH's name fold, Publish's BASE recovery and
unlink's link delete. An
autosave copies the link at its start and renames a fresh file over the live one, so a
link write that took only `database` could land in the replaced file. Below it,
the mutation paths avoid holding two
of `undo_stack` / `project` / `database` at once at all - `undo()`/`redo()` pop the
entry and drop the `undo_stack` guard before touching the project, and
`get_undo_stack()`/`get_redo_stack()` release the project lock before acquiring
`undo_stack`.

Where more than one IS held, `project` is the outermost of the three and nothing in
the workspace holds an `undo_stack` or `database` guard across a `project.lock()`. Two
places rely on that: `save_project` (holds its `project` guard while it writes the
DB and persists both stacks) and the close teardown
`close_open_project_locked`, whose caller `delete_project` has to decide *whether*
to close and then close under one `project` acquisition - splitting them let a
concurrent load install a different project between the check and the teardown.

Reads take it too, briefly (#576 C2). Commands run concurrently on the blocking
pool, and a command settles the project (reconcile, undo push) after its mutation
under the same `command_lock` span, so an IPC read that took only `project` could
see an edit without its undo entry. `AppState::settled_snapshot()` takes
`command_lock` for the O(1) snapshot of the copy-on-write project and releases it
before the read does any work; `AppState::settled_read` runs a short query under it.
Neither is reentrant: never call them while holding `command_lock`, i.e. from inside
`execute`, undo/redo, an import, a load or a save. The guarantees are pinned in
`core/state/command_ordering_tests.rs`: concurrent edits land once each with one
undo entry each, a settled read waits for the command it would otherwise have caught
in the middle, a handed-out snapshot never changes, and `cancel_loading` never waits
on `command_lock`.

One lock sits OUTSIDE even that: `speleodb_lock_ops`
(`core/state/speleodb_session.rs`), which serializes every SpeleoDB **lock
operation** against every other and is deliberately held across network calls.
Start Editing takes it before its first read (generation, link, token, holder)
and holds it through the commit; so do the reclaim-checked releases, the status
poll's inherited-lock gate reopen and its lost-lock close, Refresh's fast-forward
(from before it reads the gate through the restore), and logout - each of which can invalidate a
fact another one of them is mid-way through acting on. Nothing holding `project` / `database` / `undo_stack` may
block on it - its users decide under `project`, drop that guard, and only then
take it. The SpeleoDB session generation (`speleodb_session_generation`) is
likewise only ever bumped while holding `project`; see the module doc in
`speleodb_session.rs` for the rules.

## History UI

### Dropdown (`UndoHistoryDropdown.tsx`)

- Opens on ~300ms hover over the undo/redo toolbar button
- Shows up to 5 items with icon, truncated description (40 chars), and relative timestamp
- **Click item**: jumps to that stack position (`undo_to`/`redo_to`) and closes.
  A jump lands exactly where the same number of single steps lands: `undo_to` and
  `redo_to` loop the `*_inner` variants and run the SAME success epilogue
  `undo()`/`redo()` run - `reconcile_geometry()` after EVERY step, plus
  `mark_remote_dirty()` once after the loop. The reconcile is per step, not once
  at the end, because an intermediate state is not one the user ever sees but it
  IS one the steps above it read (the arms that re-derive rather than restore -
  `PreviousValues` via `update_shot`, `CreatedShotSnapshot` via
  `remove_created_shot` - read the live offsets at the moment they run); skipping
  it altogether was #413's finding 7, where a jump kept the drift
  `reconcile_geometry`'s `realign_merges` heals and left sections metres from
  where Ctrl+Z the same number of times leaves them. A FAILED step gets the same
  epilogue (#576 A18(a)): once a jump - or a single `undo()`/`redo()` - is past its
  index / empty-stack check, the failure arm reconciles geometry and marks the
  remote copy un-pushed too, because the steps before it landed and the failing
  arm itself may have mutated before reporting. Only a failure AT the check (an
  invalid index, nothing to undo) leaves the dirty bit alone. Such a failure
  carries the code `HISTORY_STEP_FAILED` with `history_step` (step N of M, a
  single step reporting 1 of 1) and the `reversed_sections` that landed (#576 C4);
  the frontend marks the project dirty on that code, and takes the SpeleoDB bits
  from the answer's `remote_link` (#576 D22) instead of mirroring them. A single
  undo or redo whose body crashed (`INTERNAL_ERROR`) is treated as having changed
  the project too - the step may have half-run - and also marks it dirty (#576 C4);
  a crashed jump (`undo_to` / `redo_to`) reports no failed step and keeps the
  jump's rule. A crashed mutation still carries the link bits as the crash left
  them (#576 RV7a-2): a single undo stamps the link before its geometry reconcile.
- **Click button while open**: performs single undo/redo, refreshes the list, stays open
- **Keyboard**: Arrow keys navigate, Enter selects, Escape closes
- Actions are queued and processed sequentially (`actionQueueRef`)

### Detail Preview Popup

Appears on item hover, positioned to the right of the dropdown (flips left if near viewport edge).

**Rendering patterns:**
- **Both old + new value**: `Field: old → new` (e.g., `Length: 10.00 → 15.50`)
- **New value only** (info display): `Field: value` (e.g., `Offset: 5.3m NNW`)
- **Old value only** (removed): `Field: ~~value~~` (strikethrough)
- **Batch sub-commands**: Description as gray header, details on indented line below

### Section Name Resolution

Section names in preview data are resolved at **display time**, not execution time. When you hover a "Merge stations" entry, it shows current section names, not the names at the time of the merge. This is intentional - the preview answers "what is this referring to now?" For historical names, only `ProjectSnapshotPair` data retains the state at execution time.

### Preview Data Generation (`core/undo/history_view.rs`)

`derive_preview_data()` matches `(Command, UndoData)` pairs to extract meaningful field changes. Every command type produces preview data - the fallback arm returns the command description as an "Action" field so hover is never empty.

For `UpdateShot`, all changed fields are shown (up to 5): Length, Azimuth In/Out, Depth In/Out, Pitch In/Out, LRUD, Comment, Kind, Temperature. The previous values come from `PreviousValues.values_json`.

For batch commands, `derive_command_details()` extracts a compact detail string from each sub-command (without access to undo data, since the batch uses a single `ProjectSnapshotPair`).

## Key Files

| File | Contents |
|------|----------|
| `src-tauri/crates/core/src/core/undo.rs` | `UndoStack`, `UndoEntry`, `UndoData` types, `DepthLimit` + the byte budget, the persist plan |
| `src-tauri/crates/core/src/core/state/undo.rs` | `undo()`, `redo()`, `undo_to()`, `redo_to()` implementation |
| `src-tauri/crates/core/src/core/state/undo_policy.rs` | `undo_policy()` (the Full/Disabled resolver), `speleodb_set_undo_mode` |
| `src-tauri/crates/models/src/undo_mode.rs` | `UndoMode`, `UndoPolicy` |
| `src-tauri/crates/speleodb/src/speleodb/sync.rs` | `unwind_target` - the BASE-then-first-parents chain |
| `src-tauri/crates/core/src/core/state/speleodb_cache.rs` | `InheritedUndo` and the drop-on-adopt chokepoint |
| `src/components/Dialogs/SpeleoUnwindDialog.tsx` | The Unwind confirmation |
| `src/components/Dialogs/settings/SpeleoUndoModeControl.tsx` | The per-project Full/Disabled control |
| `src-tauri/crates/core/src/core/state/commands.rs` | `execute()`, `execute_batch()` |
| `src-tauri/crates/core/src/core/state/undo_capture.rs` | `generate_undo_data()`, `finalize_undo_data()`, `is_undoable()` |
| `src-tauri/crates/core/src/core/command.rs` | `Command` enum and `description()` method |
| `src-tauri/src/commands/history.rs` | Tauri commands for stacks |
| `src-tauri/crates/core/src/core/undo/history_view.rs` | `UndoHistoryItem` DTO, `derive_preview_data()`, `entry_to_history_item()` |
| `src-tauri/crates/core/src/core/state/project_io.rs` | Stack persistence (save/load to SQLite) |
| `src/components/SingleSystemView/UndoHistoryDropdown.tsx` | Frontend dropdown + preview popup |
| `src-tauri/crates/models/src/shot.rs` | `shot_display_position()` helper |
