# The station-graph survey model (`models::survey`)

The survey model of the app, from the station-graph redesign
([plan](plans/2026-11-20_station-graph-model.md), sections 1-5). It lives in
`src-tauri/crates/models/src/survey.rs` and `src-tauri/crates/models/src/survey/`
and reuses the crate's shared types and pure helpers. Streams M0 to M3 built
it; in the persistence crate, the `.stalx` v2 codec (`persistence::stalx_v2`,
[file format](file-format.md)) reads and writes it and the legacy migration
(`persistence::migrate`) produces it from an old SQLite file through the frozen
0.6.0 reader (`crates/legacy`). Its undo engine (stream U) is
`survey::changeset` ([Undo engine](#undo-engine-changeset)). The app core
(stream C, `stalagmight-core`) runs every command, query and subsystem on it:
see [The app core on the model](#the-app-core-on-the-model-stream-c).

Run its tests with:

```bash
CARGO_TARGET_DIR=... TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-models --lib survey::
```

## In one paragraph

Stations are entities with stable ids (`StationId`, the user-visible Station
ID). A line is a simple chain: `start`, then each shot's `to`. A station on two
or more lines is a junction (what a merge was). The model stores observations
and decisions only: shots, stations, datums (with a maintained `correction`),
placements of datum-less pieces, closures, hand moves. Positions, depths,
alignment of snaps and datum roles are derived by solver v1
(`Project::raw_positions`). Every command snapshots the anchors, mutates, then
runs one post-edit anchor algorithm (`Project::settle_anchors`) and
`Project::validate`.

## Files

| File | Contents |
|------|----------|
| `survey.rs` | Module root: `Project` (fields as plan 1.2), the private `lines` field and the cache (`topology`, `shot_uuids`), re-exports. |
| `survey/types.rs` | `Station`, `StationAlias`, `Line`, `LineMeta`, `Shot`, `ZeroDatum`, `Placement`, `LoopClosure`, `ClosureStrategy` (shared variants plus `Tie`), `ProjectSettings`, `Counters`, `ClosureId`, `MAX_SHOT_UUID_CHARS`, `MAX_SHOTS_PER_LINE`. |
| `survey/topology.rs` | `Topology` (chains, slots, components) and `ShotUuidIndex`. |
| `survey/build.rs` | Construction API, id minting, slot lookups, `drop_stale_attachments`, `BuildError`, `StartAt`, `StaleAttachments`. |
| `survey/validate.rs` | `Project::validate` and `Violation` (plan section 2). |
| `survey/positions.rs` | Solver v1 raw positions and depth: `RawPositions`, `Anchor`, `SlotPosition`, `PositionIssue`, `closure_is_aligned`. |
| `survey/anchors.rs` | `AnchorSnapshot`, `EditContext`, `settle_anchors`, `correction_to_hold`, `AnchorReport` (plan 4.1, 4.3). |
| `survey/naming.rs` | Station-name uniqueness on every line of a station, the letter-suffix rule, shot-uuid helpers. |
| `survey/edit.rs`, `survey/edit/` | Line and shot operations (stream M1): create, update, delete and split shots, lines, ranges, reverse, Split Line Here, Combine Lines (`edit/combine/`), and their previews. |
| `survey/ops.rs`, `survey/ops_*.rs` | Topology, closure, datum and move commands (stream M2): merge, detach, closures, datums, Move Segment / Line / Station. |
| `survey/sweep.rs` | The orphan-closure sweep (plan 5.5). |
| `survey/append.rs` | Add .stalx / Merge Projects. |
| `survey/closures.rs` | Closure alignment per closure, the across-one-shot knot rule (skip set, `CLOSURE_ACROSS_ONE_SHOT` data), tie groups. |
| `survey/metrics.rs` | Closure metrics from raw positions (`closure_metrics`, `closures_metrics`), path length along shots. |
| `survey/lsq.rs`, `survey/adjust.rs` | The Closures Computed View (`adjusted_positions`, `AdjustedPositions`, failures, `adjustment_report`, `solve_hold`). |
| `survey/roles.rs` | Datum roles, the entrance rule with `via`, GPS-fix gaps, far fixes, the GPS-gap warning data. |
| `survey/frozen.rs`, `survey/rigid.rs`, `survey/stats.rs` | Frozen shots, rigid groups / anchored lines / immovable stations / merge pairs, survey stats. |
| `survey/graph.rs` | The survey graph (stations, shots, zero-cost closures), Dijkstra, penetration, the distance field, the ruler. |
| `survey/gas.rs`, `survey/gas/` | The Plan-mode gas engine on the station graph (`gas_reach`, `GasReach`, `gas_reach_field_wire`). |
| `survey/changeset.rs`, `survey/changeset/` | The undo engine (stream U): `Changeset`, `capture`, `undo` / `redo`, history entries, the preview digest, the shared test kit. See [Undo engine](#undo-engine-changeset). |
| `survey/wire.rs`, `survey/wire/` | The wire projection (plan 9.2): `ProjectWire`, `LineWire`, `ProjectStateWire`, the per-slot `Station` list, preview and query DTOs, slot resolution, the load envelope input. See "Wire API" below. |
| `survey/legacy_convert.rs` | Test only: converts a survey built with the frozen 0.6.0 model (`stalagmight-legacy`) for the cross-checks, plus a random legacy survey generator. |
| `survey/*_tests.rs`, `survey/test_helpers.rs` | Tests (see the end of this page). |

## Data and ids

- `Project.stations: BTreeMap<StationId, Station>`; `Station { id, name, comment, aliases, external }`.
- `Project.lines` is private: read it with `lines()`, `line(id)`, `line_position(id)`;
  change it with `lines_mut()` / `line_mut(id)`, which drop the cached
  topology and shot-uuid index. Every other field is public and feeds no cache.
- `Line { id, name, color, direction, quality, azimuth_reference, section_id, legacy_surveyor, legacy_date, start, shots, hand_moves, next_shot_id }`.
  `hand_moves: BTreeMap<StationId, [f64; 3]>` is per line (legacy only).
  `line.stations()` iterates the chain without the cache; `line.hand(s)`
  defaults to zero.
- `Shot { id (session), uuid: String, to, measurement fields as today, credit }`.
  `Shot::from_data(id, ShotData, to)` goes through the crate's one identity
  point (`crate::Shot::with_uuid`): the data's uuid, else a fresh v4.
  `Shot::vector(reference)` is today's walk vector bit for bit (tested against
  the legacy `Shot`).
- `ZeroDatum { station, via, gps, accuracy_m, entrance, correction }`;
  `Placement { station, x, y }`; `LoopClosure { id, a, a_via, b, b_via, strategy }`.
- `Counters { next_station_id, next_closure_id (starts at 1), next_section_id }`.
  `next_line_id()` is derived (`max + 1`).
- Session shot ids are `line_id * FEATURE_ID_STRIDE + index`
  (`Line::assign_session_shot_ids`, `Project::assign_session_shot_ids` after
  deserializing), so a line holds at most `MAX_SHOTS_PER_LINE` (99,999)
  shots. `ShotId` is skipped by serde.
- Serde derives exist for tests and undo images; they are deterministic
  (BTreeMaps, list order). `Project` equality ignores the caches.

## Topology (`project.topology()`)

Built lazily from the lines and cached until the next `&mut` borrow of the
lines. Total on invalid models.

- `chain(line)`, `chain_at(position)`, `station_at(slot)`; `slot` is a
  `StationRef { line_id, station_index }`, the view address.
- `slots_of(station)`: every slot, sorted by (line list order, index);
  `first_slot`, `slot_on(station, line)`, `lines_of`, `is_junction`.
- `components()`: union-find over chains, ordered by lowest station id;
  `component_of(station)`, `connected(a, b)` (closures do not connect).
- `stations()`: every chained station ascending; `dense(station)` is its index.

`project.shot_uuids()` maps uuid to every `ShotLocation { line, index }`.

## Construction API (`build.rs`)

Low level, not command-level: no refusals beyond structural ones, no anchor
logic. Callers build, then `validate()` (and commands also `settle_anchors`).

- `Project::new(name, location)`, `Project::new_at(name, location, created_at)`, `touch()`.
- Ids: `mint_station_id`, `mint_station(name)`, `insert_station(station)`
  (keeps its id, raises the counter past it and its aliases),
  `observe_station_id`, `is_alias_id`, `mint_closure_id`, `mint_section_id`,
  `next_line_id`.
- Lines: `add_line(meta, StartAt::New | StartAt::Station(id), shots)` mints a
  station per shot named by the seed rule (chain index, letter-suffixed when
  taken on that line); `add_line_on_stations(meta, chain, shots)` for any
  existing stations (junctions anywhere); `insert_line(line)` for decoders and
  the migration (only the id is checked). A uuid already in use is refused.
- Anchors and closures: `push_datum`, `set_placement(station, x, y)` (replaces
  the component's placement in place), `push_closure(a, a_via, b, b_via, strategy)`.
- Lookups: `station`, `station_mut`, `station_at`, `slots_of`, `line_stations`,
  `azimuth_reference_for(line)`.
- `drop_stale_attachments()`: clears `via` hints and drops hand moves of a line
  that no longer passes the station. Commands that remove a station from a
  line (delete shot/range/line, split, combine) run it before `validate`.

## Invariants (`validate.rs`)

`project.validate() -> Vec<Violation>` checks plan section 2 in a fixed order:
ids (station keys, id ranges, alias collisions and alias ids below 2^31,
duplicate line / closure / Section ids, at most `MAX_SHOTS_PER_LINE` shots per
line, empty, overlong or repeated shot uuids), references (line starts,
shot targets, datum / placement / closure stations, `via` hints, hand-move
keys, Sections), simple chains, orphan stations, the forest (the shot that
closes a cycle), one anchor per component, closures (`a != b`; across pieces
only between two datum-held pieces), station names on every line, counters
above every id in use (aliases included) and the station and Section
counters at most 2^31, finite numbers (shots, quality,
azimuth corrections, hand moves, datums, placements, elevation, validation
thresholds). Every `Violation` has a precise `Display` message. The `.stalx`
v2 encoder and decoder refuse through it; no bound is checked twice.

## Solver v1 (`positions.rs`)

- `frame_datum()`: the first datum whose station is on a line.
- `placement_datums()`: the first datum of each component (indexed like
  `topology().components()`); later datums are GPS fixes.
- `datum_anchor_position(d)`: `gps_to_local(gps, frame gps) + correction`, the
  frame at `(0, 0) + correction`.
- `component_anchors()`: `Anchor::Datum | Placement | Unanchored` per component.
- `raw_positions() -> RawPositions`: BFS from each anchor, a station's lines in
  line-id order, each line walked once outwards from the station it was
  reached at; per line `chain = shown - hand`. Depth: the mean of the readings
  at a station (incoming `depth_out`, outgoing `depth_in`) over its slots and
  the slots of every station tied to it transitively by `Tie` closures and
  aligned snaps, summed in `(line id, index)` order (bit-identical to today's
  merge-group depth).
- `RawPositions`: `xy(s)`, `z(s)` (no hand z), `iter()`, `slot(project, slot)`
  (adds the line's hand z), `slots(project)` (the wire list, one entry per slot
  in slot order), `frame()`, `anchors()`, `issues()`.
- `closure_is_aligned(&positions, &closure)`: `Tie` always; `Move` when the
  ends are less than `ALIGNMENT_TOLERANCE_M` (1 cm) apart and the tie would
  not join two datum-held components; others never.

No hash order reaches a result; reordering lines, closures, or datums without
changing roles changes no bit (tested).

## The anchor algorithm (`anchors.rs`)

```rust
let before = project.anchor_snapshot();      // positions, datums, roles, placements, line starts
/* mutate */
project.drop_stale_attachments();            // when the edit removed stations from lines
let report = project.settle_anchors(&before, &ctx);
assert!(project.validate().is_empty());      // or roll back
```

`EditContext` says what the edit was:

| Field / builder | Use |
|---|---|
| `EditContext::geometry(line)` | UpdateShot, insert, delete, prepend, split at distance, range delete: rule (a), the line's pre-edit first station stays. |
| `EditContext::line(line)` | Detach, split, combine: rule (c) prefers that line's stations. |
| `EditContext::merge(stationary, moving)` | A merge (pre-edit station ids): the stationary piece's placement station is kept. |
| `.with_renamed(old, new)` | Datums now on another station id (merge survivor, detach's new station). |
| `.with_gps_edit(datum, previous)` | A GPS edit (post-edit index, GPS before). |
| `.with_placed(station)` | A placement the command set or moved itself (Move Segment, import placement, Add): authoritative, post-edit coordinates. |
| `.with_corrected(datum)` | A correction the command computed itself (Add .stalx): kept. |

Rules: a component with a datum is placed by its first datum; it keeps its
correction if that datum was a placement datum before, else it goes onto its
GPS (promotion, new datum, GPS edit). Fixes carry no correction (demotion
clears it). A datum-less component keeps its still point (declared placement,
then the geometry line's first station, then a surviving placement station
preferring the merge's stationary piece, then the edited line's first
station, then its lowest station) at its pre-edit position, and stores its
placement on the surviving placement station (else the lowest station), so
the least-squares gauge only changes when its station goes. A frame change
settles promoted datums against the old frame GPS, then translates everything
by `T = -(new frame station)`: placements by `T`, every placement datum's
correction recomputed (`correction_to_hold`), so nothing moves relative to
anything else.

`correction_to_hold(datum, at)` is the frame-change formula on its own (for
Add .stalx). `AnchorReport` lists the frame change, the translation, datums
placed on their GPS, demoted datums and dropped placement stations.

## Commands

Two pipelines run one protocol: snapshot the anchors, mutate,
`drop_stale_attachments`, `settle_anchors`, the orphan-closure sweep for
topology edits, `validate`; a refusal or a violation leaves the project
exactly as it was (a clone is taken first). Commands never touch
`modified_at` and never log: the command layer (C) stamps and logs, so every
model operation is a pure function of its input (the property tests replay
seeds bit for bit). A preview is the same operation run on a clone.

- **Line and shot operations** (`edit.rs`, `edit/`): `Project::apply_edit` /
  `run_edit`. Refusals are `EditError::Refused(text)` with today's wording,
  violations `EditError::Invalid`. Results are `Edited { value, report }`,
  `EditReport { anchors, stale, sweep, removed_across_one_shot }`. Delete
  shot and the shift forms of range delete (and Combine) also remove the
  closures the edit newly left across one shot (#737).
- **Topology, closures, datums, move tools, Add** (`ops*.rs`): `Project::run_op`
  (and `run_plain` for edits that cannot move anything). Refusals are typed
  (`OpError`, `Display` = today's wording). Results are
  `OpOutcome { value, anchors, sweep }`. `EditError` converts from `OpError`.

Shared cores (one copy each):

| Core | Used by |
|---|---|
| `merge_ids(survivor, retired, comment)` (`ops_merge.rs`): one station replaced by another everywhere, aliases, the comment rule (`Join`, or Combine's pick), `external`, names re-uniquified | Merge Stations, the sweep's promotions, Combine's join |
| `detach_ids(station, line)` (`ops_detach.rs`): a line's slot gets a new station; `via` datums, closure ends and the hand move follow | Detach / Unmerge, Combine's torn junctions |
| `sweep_orphan_closures()` (`sweep.rs`) | Merge, detach, datum removal, every delete |
| The across-one-shot skip set (`closures.rs`) | The solve; the #737 removals (`skipped_closure_ids`, `remove_closures_newly_across_one_shot`); the creation refusal (`closure_spans_one_shot`) |
| `solve_hold(station)` (`adjust.rs`) | The Computed View's own partition; Combine's gauge rule |
| `rigid_groups`, `tie_groups` (`rigid.rs`, `closures.rs`) | Move Segment, the Merge refusal, the move tools' sets |
| `line_label_of(line)` (`edit.rs`), `station_text(station, line)` (`ops.rs`) | Every refusal that names a line or a station |

### Line and shot operations (stream M1)

| File | Operations and queries |
|---|---|
| `edit/lines.rs` | `create_line(meta, StartAt, shots, placement)` (`StartAt::Station` makes a junction), `delete_line`, `update_line_meta`, `validate_line_name`, `set_line_azimuth_reference` (a geometry edit), `reorder_lines`. |
| `edit/shots.rs` | `shot_insert_refusal`, `create_shot(line, data, ShotPlace::End \| After \| Before \| Start)` (End / Mid / Prepend naming, credit inheritance), `append_shot`, `prepend_shot`, `update_shot`, `set_shot_kind`, `delete_shot`, `preview_delete_shot` (`DeleteShotPreview`), `split_shot_at_distance`. A removed junction stays on its other lines and the line simply leaves it, first, middle or last shot alike (as 0.6.0: the line's new end is not merged to the junction); a station on no other line leaves with its comment, datums, hand moves and closures. |
| `edit/ranges.rs` | `delete_shot_range(line, first, last, RangeDeleteMode::Shift \| Split)` and `preview_delete_shot_range` (`DeleteRangePreview`). The shift's "Both sides are anchored to a zero datum." walks shots, junctions and closures but never enters a station the gap-close deletes (0.6.0's range walk). |
| `edit/junction_leave.rs` | What every delete (shot, range shift or split) does with the closure ends its line's slot carried at a junction it leaves (0.6.0's member rules, plan S13): a `Tie` (a demoted 0.6.0 merge) leaves with the slot unless its ends were co-located and the edit leaves them in different pieces (0.6.0's replacement merge: the sweep promotes it, nothing moves, the junction keeps its id); any other closure stays, and if the sweep promotes it the junction's side moves (unless held by a datum), these closures judged first. |
| `edit/reverse.rs` | `reverse_line(line, swap_lr)`: the chain reversed and every shot flipped; positions do not move. |
| `edit/split.rs` | `split_line(line, SplitAt, &SplitOptions)` and `preview_split_line`: the halves share the split station; the hand move is copied. |
| `edit/combine.rs`, `edit/combine/` | `combine_candidate_for_lines`, `combine_candidate_joining`, `combine_candidate(s)` and `combine_candidates_at_point` (station queries), `combine_refusal`, `preview_combine`, `combine_lines(kept, other, &CombineLinesOptions)` -> `CombineReport` (moving side, torn ties, closures removed or replaced, renames). The preview and the command return the same report. A closure join is a loop closure between the two ends' `Tie` groups (each station and the stations `Tie`s join it to: 0.6.0's merge group), so a shot-less line whose one station reaches the closure through a demoted merge is offered the closure join, as in 0.6.0. |

### Topology, closures, datums, move tools, Add (stream M2)

| File | Commands and queries |
|---|---|
| `ops.rs` | `OpError`, `OpOutcome`, `run_op`, `station_of_slot`, `station_text`. |
| `ops_merge.rs` | `merge_stations(a, b)` (plan 5.1/4.2: survivor = stationary side, retired id -> alias, comments joined, names re-uniquified on the moving lines, `via` hints kept), `merge_refusal`, `merge_direction`, `merge_datum_shift`, `merge_check` (today's `check_merge_connectivity`), `piece_lines`, `placement_datum_of`; the `merge_ids` core. |
| `ops_detach.rs` | `detach_line(station, line)`, `unmerge_station_pair(a, b)` (detaches `b`'s line), `unmerge_station(slot)` (no-op off a junction); the `detach_ids` core. |
| `ops_closures.rs` | `create_closure(a, a_via, b, b_via, strategy)` (reports `closure_metrics_via` of the picked slots), `create_closure_at(slot, slot, strategy)`, `remove_closure(id)`, `closure_refusal`, `closure_preview`, `closure_between`. |
| `sweep.rs` | `sweep_orphan_closures() -> SweepReport` (plan 5.5): one pass in list order; `a == b` removed; a sole bridge promoted by a merge (end `a`'s piece moves unless it is placed and `b`'s is not), kept between two placed pieces. `sweep_orphan_closures_moving(ends)` (the deletes): the named closures first, the named end's piece moving. |
| `ops_datums.rs` | `set_datum(station, via, gps, accuracy)` / `set_datum_at(slot, ...)` (append, accuracy-only edit, or GPS edit through the anchor algorithm), `remove_datum(index)` / `remove_datum_at(slot)` (sweeps), `set_datum_entrance`, `datum_entrance`, `datum_index_for(station, via)` (the datum a slot has), `preview_zero_datum`. |
| `ops_move.rs` | `move_segment(line, offset)` / `move_line` (translates every placement of the rigid group; refused with a datum in it), `move_station(station, offset)` (rewrites every incident shot on every line, folds the station's hand moves, moves its placement). The sets the move tools read are derived views (`rigid_groups`, `tie_groups`, `anchored_lines`, `immovable_stations`). |
| `append.rs` | `append_project(source) -> AppendSummary` (Add .stalx / Merge Projects: ids offset or re-minted, Sections and line names made unique, colliding uuids re-minted, the copy moved by `place_by_datum`'s vector with placements translated and corrections recomputed). |

## Derived views (stream M3)

Every view is a pure function of the model; per station, with a per-slot
form for the wire. Most take the alignment of this state,
`let aligned = p.closure_alignment(&p.raw_positions())`.

| View | API | Notes |
|---|---|---|
| Alignment | `closure_alignment(&raw) -> Vec<bool>` | `Tie` always; `Move` within 1 cm and not double-anchoring. |
| Skip set | `closures_across_one_shot()`, `..._with(&aligned)`, `closure_spans_one_shot(a, b)` | Today's knot rule over the closures that tie in the solve (eligible `Batch`/`LeastSquaresWeighted`/`Tie`, aligned snaps), list order, transitive. `ClosureAcrossOneShot::to_wire` gives today's `SkippedClosure`. |
| Tie groups | `tie_groups(&aligned) -> TieGroups` | Stations joined by aligned closures (depth, Move tools). |
| Metrics | `closure_metrics(a, b)`, `closure_metrics_via(..)`, `closures_metrics(&raw)` | Misclosures from raw positions (slot z on the `via` line); `path_length` along shots only (0 between two pieces). |
| Computed View | `adjusted_positions(&config) -> AdjustedPositions`; `adjustment_report(Some(&adj))` | One unknown per constraint group, gauge = the `Placement` station, hard/soft GPS fixes, spurs, failures rolled back to raw. |
| Solve hold | `solve_hold(station) -> SolveHold { anchor, datum, solved }` | The solve component's held station (first placement datum, else its lowest piece's `Placement` station) and whether the view solves it; edits that must not shift the view (Combine) read it. |
| Roles, entrances, gaps | `datum_roles`, `datum_entrances`, `entrance_stations`, `datum_gap`, `gps_fix_gaps`, `far_gps_fixes`, `gps_gap_warnings` | Entrance default: first or last station of the `via` line, else of the station's first line. Survey distance along shots only. |
| Frozen shots | `frozen_shots(&aligned)`, `is_shot_frozen(line, index)` | Tarjan bridges over shots and aligned closures; positional output. |
| Rigid groups | `rigid_groups(&aligned)` (`group_of`, `lines`, `has_datum`, `tied`), `rigid_lines_of`, `anchored_lines`, `immovable_stations(&ties)`, `merge_pairs()`; co-located stations are `tie_groups(&aligned).group(s)` | Fixes count as anchors for Move Segment and Move Station, as today (D15). |
| Stats | `survey_stats(&raw, include_reference) -> SurveyStats` | `station_count` (distinct) and `slot_count`; `to_project_stats` keeps today's slot count. |
| Survey graph | `survey_distances_from`, `penetration_distances`, `penetration`, `distance_field`, `along_line_between` | Junction = one node, closures zero-cost, reference ties infinite unless opted in; shots in line-id order. |
| Gas | `gas_reach(..) -> GasReach`, `GasReach::to_wire`, `gas_reach_field_wire(..)` | Today's engine on stations; the route has no merge hops. |

## Undo engine (`changeset`)

The undo/redo engine of plan 8: one payload type, the id-keyed
[`Changeset`], captured generically and applied either way with
preconditions. It depends on nothing but the model: no `AppState`, no
`Command`. The core stream wires it into the history stacks.

### API

```rust
use stalagmight_models::survey::changeset::{
    capture, undo, redo, step, Changeset, Direction, Refusal,
    HistoryEntry, HistoryRefs, RecordedPreview, PreviewRow,
    encode_entry, decode_entry, EntryLimits, EntryError, HISTORY_FORMAT_VERSION,
    Digest, ShotField, Val,
};

pub fn capture(before: &Project, after: &Project) -> Changeset;
pub fn undo(project: &mut Project, changes: &Changeset) -> Result<(), Refusal>;
pub fn redo(project: &mut Project, changes: &Changeset) -> Result<(), Refusal>;
pub fn step(project: &mut Project, changes: &Changeset, direction: Direction) -> Result<(), Refusal>;
impl Changeset {
    pub fn is_empty(&self) -> bool;          // capture of two equal states
    pub fn digest(&self) -> Digest;          // what a preview shows
    pub fn has_session_ids(&self) -> bool;   // captured in this session
    pub fn forget_session_ids(&mut self);    // as a persisted round trip does
}
pub fn encode_entry(entry: &HistoryEntry) -> Result<String, EntryError>;
pub fn decode_entry(text: &str, limits: &EntryLimits) -> Result<HistoryEntry, EntryError>;
```

The pipeline the core runs around a command:

```rust
let before = project.clone();                  // after lock and gate
/* the model operation(s); refusal or violation: restore `before` */
let changes = capture(&before, &project);      // after the whole epilogue
if !changes.is_empty() { push(HistoryEntry { timestamp, kind, label, refs, preview, changes }) }

// undo (redo is the mirror):
undo(&mut project, &entry.changes)?;           // Err: refused, project untouched
// if the core also validates and finds a violation, `redo` puts it back exactly
```

- **Capture** is generic (no per-command code): project fields (name,
  location, creation time, settings), stations (merge walk over the two
  maps), the line order, every line (bitwise compare first, only the lines
  that differ are diffed shot by shot), datums, placements, closures,
  Sections, import hashes, counters. `modified_at` and `file_path` are not
  recorded: the command layer stamps the edit time, and a step never
  touches it. Cost on the 20,000-shot gate survey: about 2 ms in a release
  build (undo and redo 0.01-0.4 ms), measured by the size gate.
- **Preconditions.** A step refuses (`Refusal`, with what differs) and
  leaves the project untouched unless the live project is the recorded
  source state on everything the changeset touches: every recorded field
  value, every station image, the length and hunk contents of every list,
  the line order, and the full content hash of every line whose shots it
  rebuilds (source side before the step, target side after it). A refused
  step needs no clone: `prepare` reads the project and builds the new
  parts, `commit` moves them in and cannot fail.
- **Counters** are applied as `max(live, recorded)`: an undo leaves them
  standing, a redo reinstates exactly the ids it removed.
- **Session shot ids.** A changeset captured in the session carries each
  touched line's session ids (never persisted, plan 1.1), so undo and redo
  give every shot its id back. A decoded entry has none: a copied shot keeps
  its live id, the others are minted from the line's allocator (which never
  goes down); the ids stay unique and inside the line's window.
- **Exactness.** Undo and redo land on the recorded state to the bit (the
  canonical model text and the v2 bytes; counters may stand higher), in the
  session and through the persisted entry. Rolling back: a successful
  `undo` is exactly reversed by `redo` of the same changeset, and the other
  way round.

### What a changeset records

| Part | Form |
|---|---|
| Project fields | `[before, after]` per changed field (settings whole). |
| Stations | whole images of the stations only one side has (`add`, `del`), and per field (`name`, `comment`, `aliases`, `external`) one column `[id runs, [before...], [after...]]`. |
| Line order | list hunks over the line ids (create, delete, reorder). |
| Lines | per touched line, ascending id: field-level metadata `[before, after]` pairs; and when its shots changed (or it exists on one side only), a **side** per state: shot count, content hash, the line image for a side-only line, and **segments**. |
| Segments | a copy run `[line, at, n, mode, anchor, {edits}]` takes `n` shots of a touched line of the other state from `at` (forwards, or backwards for the reversed modes), transforms them (`0` as they are, `1` reversed, `2` reversed and flipped as Reverse Line flips a shot, `3` the same with left and right swapped), then writes the field-level exceptions; literal rows `{"u": [uuids], "f": {columns}}` carry shots the other state does not hold. A run out of another line records its first shot's uuid as an anchor, checked on apply. |
| Columns | per shot field (`to`, `len`, `ai`, `ao`, `di`, `do`, `pi`, `po`, `l`, `r`, `u`, `d`, `c`, `k`, `m`, `tp`, `cr`), spans `[at, [values]]` or `[at, n, value]` (a run of equal values). Credits are interned once per changeset (`credits`), cells name them by index. |
| Lists | datums, placements, closures, Sections, both import-hash lists: `{"n": [before len, after len], "h": [[at, [before...], [after...]], ...]}` (Myers, at most 512 edit steps, else one hunk). |
| Counters | `[[station, closure, section], [station, closure, section]]`. |

So a cell edit is a run and one value per side; a paste of one column a value
per cell (about 11 bytes per changed cell); a credit over a whole line one
repeat span; a reverse one flipped run per side plus the few ulps the flip
does not give back (computed at capture by re-applying the flip and diffing
the bits, the #808 precedent); a split or a combine a few runs; a delete the
deleted shots as literal rows.

### History entries (plan 8.3)

One compact, deterministic JSON value per entry, the element of the v2
file's `history.undo` / `history.redo` lists (already in the format's
canonical form, so `RawJson::parse` leaves it unchanged):

```text
{"v":1,"t":"2026-11-20T12:59:00.000000000Z","k":"update_shot","label":"...",
 "refs":{"lines":[..],"stations":[..],"shots":["uuid"],"closures":[..],"sections":[..]},
 "preview":{"rows":[["Length","3.50","4.00"]],"more":2},
 "ops":{...the changeset...}}
```

- `v` is `HISTORY_FORMAT_VERSION` (1). Any other version decodes to
  `EntryError::Version`: drop the history whole with the existing banner.
  Bump it with any change to the layout or to the line hash.
- `t` is RFC 3339 UTC with nanoseconds (the format reads it for the
  SpeleoDB watermark cut).
- `k`, `label`, `refs`, `preview` are the core's: the command kind (icon),
  the label resolved at record time, the stable ids it names (never session
  ids or slots; at most `MAX_LISTED_SHOTS`, 16, shot uuids, as many as a
  preview lists), and rows worded at record time for what the changeset
  cannot say (a batch's sub-commands, an offset). The forward `Command` is
  never persisted.
- `decode_entry` is for untrusted input: a byte limit, the version, a strict
  layout (unknown keys refused), limits on references, labels and preview
  rows, then the structural rules (`shape.rs`, run before every step too):
  ids inside their id spaces, every run fitting the side it reads, each
  source shot copied at most once per direction, columns in order and in
  range with values their field takes, list hunks in order and inside the
  list, the line order consistent with the lines created and deleted. All
  linear or `n log n`; a step's work is bounded by the live lines it reads
  plus the rows the entry materialises.

### Previews (`Digest`)

`changes.digest()` reads what a history preview shows off the changeset
alone (no project, no `Command`): project field changes (`ProjectField`:
name, location, creation time, surface elevation, default azimuth reference,
validation), stations created and deleted (whole images: a deleted
station's name), station field changes, lines created and deleted
(`LineSummary`: id, name, shot count, start), line metadata changes
(`LineField`), lines reversed end to end (for the frontend's pin
mirroring), shot cells changed (`ShotCellChange`: line and index in the
after state, field, old and new value; the old value is paired for runs
read forwards), shots created and deleted (the first 16 built, all
counted), and the elements each list gained and lost.

### Tests

| File | Covers |
|---|---|
| `changeset/changeset_tests.rs` | What a capture records per edit kind (cell edit, reverse of 1 to 40 shots with and without the L/R swap, split and combine as runs, deleted line, merge and detach, closures and datums, credits interned, `-0.0`), counters, session ids in the session and after a reload, refusals and their text, the list diff (random lists both ways, far-apart edits stay apart). |
| `changeset/property_tests.rs` | 48 seeds x 70 random survey operations (every M1/M2 command plus the direct edits: names, comments, `external` ids, credits, Sections, settings, location, azimuth references, legacy hand moves, import hashes, Add): undo all and redo all land on every state to the bit, in the session (session ids exact) and persisted; at every depth and both directions, a project changed behind the engine's back is refused and untouched; steps out of order refuse. |
| `changeset/entry_tests.rs` | Entries are canonical fixpoints; versions and unknown members refused; truncation at every byte and byte flips at every offset never panic (a flipped entry that decodes steps or refuses, never panics, never half-applies); garbage, depth bombs, oversized entries; the shape rules one by one; large hostile entries decode in linear time; non-finite numbers refused on write. |
| persistence `history_size_gate_tests.rs` | The size gates on the v2 file (see `docs/undo-redo.md`). |
| persistence `history_v2_property_tests.rs` | 32 seeds: identical v2 bytes at every depth; a history that rides the file's `history` member and undoes from the decoded project. |

`changeset/testkit.rs` (compiled for tests and with `test-support`) holds
the gate surveys, the random walk, the replay and the tamper helpers the
persistence crate's tests share.

## Wire API (`survey/wire.rs`, `survey/wire/`, stream W)

What the IPC sends the frontend, in the shapes it reads today (plan 9.2).
The frontend keeps the slot `StationRef { line_id, station_index }` as its
view address and its generated TypeScript; every function here is a pure
function of the project state it is called on, and a preview mapping takes
the state the preview ran on (before the edit), so every slot it names is
one the user sees.

### Payloads

| IPC (today) | Builder | Notes |
|---|---|---|
| `get_project_state` -> `ProjectState` | `project_state_wire(include_reference_legs)`, `project_state_wire_with(&raw, ..)` | Raw layout and closure alignment computed once. `include_reference_legs` is `gas.include_reference_legs` (penetration). |
| `Project` in `LoadProjectFullResult` / `CreateProjectFullResult` | `project_wire()`, `project_wire_with(&raw)` | |
| `load_project_full_binary` | `load_envelope_input(include_reference_legs)` -> `LoadEnvelopeInput { state, station_offsets, station_comments_map }` | See "Binary load envelope". |
| `get_adjusted_stations` -> `Station[]` | `adjusted_stations_wire(&adjusted)` | Same slots and relations as the raw list. |
| `get_adjustment_report` | `adjustment_report_wire(&config)` (or M3's `adjustment_report(Some(&adjusted))`) | |
| `check_merge_connectivity` | `check_merge_connectivity_wire(a, b)` | Today's refusal words; the preview reads each end's depth on the picked slot's line. `connectivity_wire(&MergeCheck)`, `merge_datum_shift_wire(..)` map M2's structs. |
| closure dialog figures | `closure_preview_at(a, b)` | |
| `preview_zero_datum` | `preview_zero_datum_at(slot, &gps)` | |
| `preview_delete_shot` -> `DeleteShotImpact` | `delete_shot_impact(&preview)` | From M1's `DeleteShotPreview`. |
| `preview_delete_shot_range` -> `DeleteShotRangeImpact` | `delete_range_impact(&preview)` | From M1's `DeleteRangePreview`. |
| `split_line_preview` -> `SplitPreview` | `split_preview_wire(&preview)` | Nothing moves and nothing becomes a merge: the halves share the split station. |
| `combine_candidate*` -> `CombineCandidate` | `combine_candidate_wire(&candidate)` | Both plans included. |
| a combine's `CombinePlan` (preview or result) | `combine_plan_wire(&report)` | Call it on the pre-combine state. |
| `get_line_relationships` | `line_relationships(line)` | Plan 9.3; see below. |
| `get_station_distances_from` -> `DistanceField` | `distance_field_wire(&positions, from, include_reference)` | A `from` naming no station leaves every slot unreachable, as today. |
| `get_along_line_distance` -> `AlongLineResult` | `along_line_wire(&positions, &a, &b, include_reference)` | No distance and no path when an endpoint names nothing, as today. |
| `compute_gas_reach_field` -> `GasReachField` | M3's `gas_reach_field_wire(..)` | Slot inputs, today's signature. |
| `ProjectWarning` datum and closure refs | built by the command layer | M3 already gives slots: `gps_gap_warnings()` (`slot`, `shot_index`, `code`), `ClosureAcrossOneShot` (`closure_id`, `collapsed_shot`), `AdjustmentFailure::anchor_slot`. |

### Slots

- Per slot `(L, i)` with station `S` (`stations_wire`): `S`'s coordinates
  (depth plus `L`'s hand-move z, exactly `RawPositions::slots`), `S`'s
  `id`, `name` and `comment` (shared by a junction's slots), `merged_with` =
  `S`'s other slots (so a junction's slots list each other, transitively,
  like today's merge groups), `is_zero_datum` / `gps_coordinates` = the datum
  the slot's datum dialog edits (`datum_index_for(S, Some(L))`, so every slot
  of a datum station shows it), `closures_with` by today's rule with a
  station's slots as its merge group (direct on the end's own slot,
  transitive on its other slots and to the far station's other slots, then
  the closure-chain entries).
- A datum is shown on `datum_slot` and a closure end on `closure_end_slot`:
  the slot on its `via` line, else the station's first slot. A migrated
  reference keeps the legacy member's line as `via`, so it comes back on the
  slot it had.
- `station_merges` are derived: for each junction (ascending station id) its
  first slot paired with each other slot (`merge_pairs`). A derived pair has
  no merge time; `merged_at` is the project's `created_at` (nothing in the
  app reads it).
- `loop_closures`: metrics from the raw layout (`closures_metrics`),
  `is_aligned` from `closure_alignment`. A `Tie` is sent as an aligned
  `Batch` (`wire_strategy`): the TS union does not change and the frontend's
  eligibility table already treats `Batch` like a `Tie`.
- `Line`: `reverse_parity` is `false` and `gauge_shot_uuid` /
  `gauge_neighbour_uuid` are `null` (legacy gauge bookkeeping; no frontend
  code reads them). Shots are the shared `Shot` (session id, uuid, the same
  measurement fields).
- Command boundary: `resolve_slot(slot) -> SlotTarget { station, line }`
  (`OpError::UnknownSlot` for an unknown line or an index past the line's
  last station) and `resolve_slot_pair(a, b)`. The reverse is
  `closure_end_slot` / `datum_slot` / `slots_of`.

### Previews

What the station graph makes impossible is not on the wire (the delete
previews' 0.6.0 `replacement_merges`, `replacement_closure_updates` and
`*_rewire_skipped` are gone) or reported empty, never invented (the split
preview's `moved_side` / `moved_m` / `removed_already_one_point`).
`removed_merges` is the removed slot leaving its junction (one pair per other
slot of the station); `removed_closures` also lists the `Tie`s the slot takes
with it at a junction (S13), the closures the delete leaves across one shot
(#737) and the ones the sweep removes; `closure_to_merge`
lists the sweep's promotions. `LineRelationships` counts what deleting the
line takes: the wire merge pairs on the line, closures and datums on
stations only that line passes (a junction's stay), its hand moves
(`station_offset_count`), and `has_line_offset` = the line alone holds its
piece's `Placement` station and the placement is not at the origin. Combine
kinds and reasons: a torn junction is a torn `Merge`, a torn snap a
`SnappedClosure` (`FormerSnappedClosure` when it was not aligned before),
`PassesTwice` -> `Loop`, `Moved` -> `JoinedStationMoves`; every closure is
named by its ends before the combine (looked up by id, or by its torn tie).

### TypeScript

Only three wire types are new (their legacy source is the old model
itself): `ProjectWire` (TS `Project`, without `line_offsets`), `LineWire`
(TS `Line`) and `ProjectStateWire` (TS `ProjectState`). Every other payload
is today's DTO type, so its TS cannot drift. The three derive `TS` with the
legacy names (`#[ts(rename)]`) and no `export`: no test writes them into
`bindings/`; `src-tauri/examples/gen_ipc_types.rs` renders them
(`export_wire`, ts-rs `export_to_string` plus the generator's import
flattening) into `src/types/ipc/`. `wire/ts_tests.rs` renders each one the
same way and compares it with the committed file.

**The switch** (done in C0 and C21), for the record:

1. In `src-tauri/examples/gen_ipc_types.rs` export
   `models::survey::wire::{ProjectWire, LineWire, ProjectStateWire}` in place
   of `Project`, `Line` and the app's `ProjectState` (delete the latter, and
   point `LoadProjectFullResult` / `CreateProjectFullResult` at
   `ProjectWire`; their TS does not change).
2. `pnpm gen:ipc-types && git diff --stat -- src/types/ipc`: the only change
   is `Project.ts` losing `line_offsets`. `wire/ts_tests.rs` still passes.
3. Fix the TS that names `line_offsets`: `src/utils/loadProjectBinary.ts`
   (see below), `e2e-playwright/large-dataset-operations.spec.ts`, the agent
   harness (`e2e/agent/lib/projectSnapshot.ts`, `fuzzOracleCore.ts`,
   `scenarios/undoPersistenceFuzz.ts`).
4. `cargo test --workspace` writes nothing new into `src-tauri/bindings/`
   (no wire type has `export`).

### Binary load envelope

`LoadEnvelopeInput` holds everything `crates/core/src/core/load_binary.rs`
packs, under the same names: `state.project` (header fields, `lines` with
`shots` of the shared `Shot` type, `station_merges`, `zero_datums`,
`loop_closures` already in wire form), `state.stations` (the per-slot list),
`state.stats`, `state.frozen_shots`, plus `station_offsets` (every hand move
on its slot) and `station_comments_map` (every slot of a commented station).
The packing loops stay as they are; the layout change is that
`ProjectHeaderBinary.line_offsets` is dropped, so `BINARY_FORMAT_VERSION`
goes 8 -> 9 (the decoder would otherwise iterate a missing key). The
frontend decoder drops `line_offsets` from `ProjectHeaderBinary`, the
reconstruction loop and the `Project` literal, and pins
`EXPECTED_VERSION = 9`; the e2e IPC mock's `load_project_full_binary` builder
does the same. Semantics the decoder sees change without a layout change:
`stations_ids`, names and comments are shared by a junction's slots.

### Equivalence with today

`persistence/src/persistence/migrate/wire_equivalence_tests.rs` opens legacy
files with the frozen 0.6.0 reader (the generator's 48 gate seeds and the two
tracked fixtures), migrates them, and compares the wire projection with
0.6.0's own payloads field by field: the per-slot stations (raw and Computed
View), the IPC `Project`, line relationships, delete and range-delete
previews, merge connectivity and datum shift, the adjustment report and the
combine candidates. Delete promotions are compared in today's preview's
scope (the closures on the removed member); a few seeds also run the delete
itself on both models and compare every slot (S13). Every difference is put
in a named class (a declared change) or fails the test; `STALAGMIGHT_RUST_WIRE_SEEDS=<n>` runs a sweep
(ignored) and prints the classes. The combine classes are characterised
only (M1's ported combine tests are its oracle).

## The app core on the model (stream C)

`stalagmight-core` holds the open survey as `models::survey::Project` in
`AppState::project` and has no database handle; [architecture.md](architecture.md#appstate-and-the-open-file)
has the state, the pipeline and the lock order, [saving.md](saving.md) the file
side, [undo-redo.md](undo-redo.md) the history. What the core adds on top of
the model:

- **Slots to stations.** Commands keep their 0.6.0 payloads (slots, line ids,
  session shot ids, closure ids). The family applies resolve slots with
  `resolve_slot` / `resolve_slot_pair` (an out-of-range slot refuses) and shots
  through the line's session ids, then call the model operation. A slot that
  names no station answers 0.6.0's words, not the model's.
- **Refusals the model leaves to the core**: frozen shots (S12: `is_shot_frozen`
  before every geometry edit, delete and split, in 0.6.0's words and order), the
  SpeleoDB edit gate, the read-only file, the batch allowlist.
- **The pipeline** (`core/state/commands.rs`): one `apply(project, record, cmd)`
  per family, the model's anchors, sweep and validate inside the operation, then
  the core's own `validate()`, `capture`, the record-time label, the
  `modified_at` stamp (E3: the model never touches it) and the logs. Every
  recipe runs the same pipeline (`run_recorded`).
- **Derived views memoised per revision**: `raw_positions_of` and
  `adjusted_positions` cache the model's pure functions; everything the IPC
  sends is a `survey::wire` builder over them (`get_project_state` is
  `project_state_wire_with(&raw, ..)`).
- **Warnings** (`core/state/warnings.rs`): the per-shot validator over the
  project's own thresholds, the model's GPS-gap and across-one-shot data, and
  the Computed View failures of the memoised solve of the current revision
  only; ignores matched by `(shot uuid, code)` from the file's ignored warnings.

| Family (file) | Commands | Model operations |
|---|---|---|
| `command_line.rs` | CreateLine, DeleteLine, UpdateLineMetadata, ReverseLine | `create_line`, `delete_line`, `update_line_meta`, `reverse_line` |
| `command_shot.rs` | CreateShot, UpdateShot, DeleteShot, SetShotKind | `create_shot`, `update_shot`, `delete_shot`, `set_shot_kind` |
| `command_station.rs` | MergeStations, UnmergeStations, UnmergeStationPair, CreateLoopClosure, RemoveLoopClosure, SetZeroDatum, RemoveZeroDatum | `merge_stations`, `unmerge_station`, `unmerge_station_pair`, `create_closure_at`, `remove_closure`, `set_datum_at` (+ `set_datum_entrance`), `remove_datum_at` |
| `command_station_name.rs`, `command_comment.rs` | SetStationName, SetStationComment, MoveComment | the station's one name and comment (`conflicts_with_name`) |
| `command_move.rs` | MoveLine, MoveSegment, MoveStation | `move_line`, `move_segment`, `move_station`; a batch moves each station once |
| `command_credit.rs`, `command_cave_section.rs` | EditShotCredit, the five Section commands | shot credits, `sections` |
| `import_tracking.rs` | UntrackImport (and the recorded hash tracking) | the import hash lists |
| recipes | Split Line Here, Combine Lines, delete-and-split, split shot at distance / Insert Station, imports and create-and-merge, Add .stalx, settings, elevation | `split_line`, `combine_lines`, `delete_shot_range`, `split_shot_at_distance`, the import funnel (`import_funnel.rs`: a joining placement makes the chosen station the new line's chain station, a junction), `append_project` |

Subsystems ported onto the model and the v2 file:

- **Open, save, create** (`project_load.rs`, `project_io.rs`, `project_create.rs`):
  sniff, decode or migrate, the guard, the legacy upgrade (`legacy_upgrade.rs`:
  the original settled, kept, then replaced), the local-only save
  (`open_file.rs`).
- **SpeleoDB**: pull, install and revision swaps decode or migrate the
  download in memory before anything is locked or written
  (`prepare_download`); a push serializes `Mode::ForPush` under the push guard;
  every sync-state change is a local-only save; one link probe reads either
  format (`speleodb_local_state::probe_link`, legacy through
  `persistence::legacy_link`).
- **Recents, recovery, raw data** (`recent_projects.rs`, `project_restore.rs`,
  `project_salvage.rs`, `raw_data.rs`): the `summary` header for Recents
  (`migrate::legacy_header` for a legacy file), backups of either format and
  the pre-upgrade originals, salvage into the file with a `.backup`, the raw
  table view over the saved file's members.
- **Dive plans** (`dive_plan.rs`): a `.plnx` pin is placed by its Station ID
  when its own neighbour shots vouch for it, else by the neighbours (0.6.0's
  rule), else by its pre-#24 key; the upgrading save rewrites a migrated
  project's 0.6.0 pins (`upgrade_dive_plan_anchors`).
- **Add .stalx / Merge Projects** (`project_add.rs`, `project_merge.rs`): sources of
  either format read the way an open reads them, then the model's
  `append_project`; a merge writes a new v2 file.
- **Exports** (`src/commands/export.rs`): `export::survey` on a snapshot of the
  open survey and its memoised views.

The deviations each package made from the plan are in the plan's "Deviations
(stream C, the command families and subsystems)" section.

## Naming and identity (`naming.rs`)

- `name_conflicts(station)`, `conflicts_with_name(station, name)` (a rename
  check), `names_around(station)`: every line the station is on.
- `uniquify_station_name(station)`: the letter-suffix rule
  (`crate::station_naming::name_with_free_letter`), free on all its lines.
- `uniquify_line_names(line)`: first holder keeps a name, later ones get a
  suffix free on all their lines.
- `is_shot_uuid_taken`, `shot_by_uuid`, `duplicate_shot_uuids`, `mint_shot_uuid`.

## Tests

| File | Covers |
|---|---|
| `validate_tests.rs` | Every violation kind, with its exact payload. |
| `positions_tests.rs` | Hand-built layouts (junction of three lines, free piece, datum corrections, frame at the origin, per-line hand moves, Tie and snap depth, double-anchor snaps), the shot vector against the legacy `Shot`, and determinism on a seeded random forest (same build, JSON round trip, reordered lines / closures / datums). |
| `anchors_tests.rs` | Shot edits, explicit and undeclared placement changes, cut-off pieces, merges (free, placed, re-keyed placement), detach with and without a `via` fix, first and later datums, frame removal (translation and promotion), last datum, GPS edits, idempotence, declared corrections. |
| `build_tests.rs` | Construction, refusals, ids, caches, naming, uuids, closure strategy spellings, stale attachments. |
| `legacy_crosscheck_tests.rs` | Surveys built with the frozen 0.6.0 model (merges, a merge at a line's last station, hand moves on merged members, datum pieces, a fix, free pieces, 40 seeded random surveys) lay out every slot within 1e-9 m of `compute_station_coordinates`. |
| `graph_tests.rs` | Roles, entrances, GPS gaps and far fixes against the legacy model (30 random legacy surveys). The penetration, distance-field and ruler comparisons were retired with `models::project`: the frozen crate has no such queries. |
| `adjust_tests.rs` | The Computed View per slot within 1e-6 m of `compute_adjusted_stations` (150 random legacy surveys), skip set, failures, alignment, new-closure verdicts, misclosures, path length, spurs, gauge, determinism, scale. |
| `adjust_floor_tests.rs` | The per-axis sigma floor (`ShotSigmas`, plan Deviations G9): its value, that no shot at least `sigma_len` long is floored (20,000 random shots keep 0.6.0's sigmas bit for bit), and that a zero-length connector on a grid axis keeps its two ends together in the Computed View. |
| `gama_tests.rs` | The GNU Gama gold standard (`src-tauri/tests/fixtures/gama_golden.json`) on networks built directly on the new model. |
| `derived_tests.rs` | Rigid groups, anchored lines and immovable stations against the legacy model; frozen shots on a hand-checked fixture; derived views bit-identical under shuffled lines. The frozen-shot and stats comparisons were retired with `models::project` (the frozen crate has no such queries); the K goldens dump both. |
| `gas/tests.rs`, `gas/differential_tests.rs` | The 116 ported gas-engine tests (expectations set by the 0.6.0 engine, fixtures built with the frozen model), and determinism: 12 random surveys with their lines and closures reordered give bit-identical results for every rig. The old-vs-new differential run was retired with `models::project` (the frozen crate has no gas engine). |
| `edit/shots_tests.rs`, `shots_closure_promotion_tests.rs`, `lines_tests.rs`, `ranges_tests.rs`, `reverse_tests.rs`, `split_tests.rs`, `combine_tests.rs` | Each line and shot operation: names, uuids and ids, which side stays, refusals in today's words, junctions kept, datums / comments / closures leaving with a station, promotions, #737 removals, previews. The junction member rules (S13), the range shift's anchored walk and the `Tie`-group closure join are also checked against the same survey built with the frozen 0.6.0 model (`stalagmight-legacy`). Delete Shot's closure decisions after an earlier promotion (#776) are `shots_closure_promotion_tests.rs`. |
| `edit/combine_port_*_tests.rs` | The legacy combine tests ported (lines, closures, snaps, hand moves, residuals, realign, dedupe), each legacy test's fate in the module doc; two random-network sweeps (about 8,700 combines). |
| `edit/edit_fuzz_tests.rs` | 32 seeds x 80 random operations: valid after every step, refusals change nothing, previews equal results, seeds replay bit for bit. |
| `wire/ts_tests.rs`, `wire/wire_tests.rs` | The wire types generate today's TS files (less `line_offsets`); JSON keys as today's payloads; junction identity, derived merge pairs, closure references and chains, datum flags, `Tie` as aligned `Batch`, slot resolution, delete impact at a junction, line relationships, the envelope input; an ignored timing run on 78k slots. |
| `ops_merge_tests.rs`, `ops_detach_tests.rs`, `ops_closures_tests.rs`, `ops_datums_tests.rs`, `ops_move_tests.rs`, `append_tests.rs` | Each M2 command, with ports of the legacy datum, entrance and placement tests. |
| `ops_scenario_tests.rs`, `ops_property_tests.rs` | The model review's scenarios; 48 seeds x 70 random commands (valid after every step, refusals change nothing, no-move commands move nothing, a merge moves exactly the moving piece, a seed replays bit for bit). |
