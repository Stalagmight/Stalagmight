# The station-graph survey model (`models::survey`)

The new survey model of the station-graph redesign
([plan](plans/2026-11-20_station-graph-model.md), sections 1-5). It lives in
`src-tauri/crates/models/src/survey.rs` and `src-tauri/crates/models/src/survey/`,
next to the legacy `models::project`, which it does not use or change (it only
reuses the crate's shared types and pure helpers). Streams M0 to M3 built it;
in the persistence crate, the `.stalx` v2 codec (`persistence::stalx_v2`,
[file format](file-format.md)) reads and writes it and the legacy migration
(`persistence::migrate`) produces it. Undo (U) and the app core (C) build on
it next.

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
| `survey/legacy_convert.rs` | Test only: converts a survey built with `models::project` for the cross-checks, plus a random legacy survey generator. |
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
| `edit/shots.rs` | `shot_insert_refusal`, `create_shot(line, data, ShotPlace::End \| After \| Before \| Start)` (End / Mid / Prepend naming, credit inheritance), `append_shot`, `prepend_shot`, `update_shot`, `set_shot_kind`, `delete_shot`, `preview_delete_shot` (`DeleteShotPreview`), `split_shot_at_distance`. A removed junction stays on its other lines; a station on no other line leaves with its comment, datums, hand moves and closures. |
| `edit/ranges.rs` | `delete_shot_range(line, first, last, RangeDeleteMode::Shift \| Split)` and `preview_delete_shot_range` (`DeleteRangePreview`). |
| `edit/reverse.rs` | `reverse_line(line, swap_lr)`: the chain reversed and every shot flipped; positions do not move. |
| `edit/split.rs` | `split_line(line, SplitAt, &SplitOptions)` and `preview_split_line`: the halves share the split station; the hand move is copied. |
| `edit/combine.rs`, `edit/combine/` | `combine_candidate_for_lines`, `combine_candidate_joining`, `combine_candidate(s)` and `combine_candidates_at_point` (station queries), `combine_refusal`, `preview_combine`, `combine_lines(kept, other, &CombineLinesOptions)` -> `CombineReport` (moving side, torn ties, closures removed or replaced, renames). The preview and the command return the same report. |

### Topology, closures, datums, move tools, Add (stream M2)

| File | Commands and queries |
|---|---|
| `ops.rs` | `OpError`, `OpOutcome`, `run_op`, `station_of_slot`, `station_text`. |
| `ops_merge.rs` | `merge_stations(a, b)` (plan 5.1/4.2: survivor = stationary side, retired id -> alias, comments joined, names re-uniquified on the moving lines, `via` hints kept), `merge_refusal`, `merge_direction`, `merge_datum_shift`, `merge_check` (today's `check_merge_connectivity`), `piece_lines`, `placement_datum_of`; the `merge_ids` core. |
| `ops_detach.rs` | `detach_line(station, line)`, `unmerge_station_pair(a, b)` (detaches `b`'s line), `unmerge_station(slot)` (no-op off a junction); the `detach_ids` core. |
| `ops_closures.rs` | `create_closure(a, a_via, b, b_via, strategy)` (reports `closure_metrics_via` of the picked slots), `create_closure_at(slot, slot, strategy)`, `remove_closure(id)`, `closure_refusal`, `closure_preview`, `closure_between`. |
| `sweep.rs` | `sweep_orphan_closures() -> SweepReport` (plan 5.5): one pass in list order; `a == b` removed; a sole bridge promoted by a merge (end `a`'s piece moves unless it is placed and `b`'s is not), kept between two placed pieces. |
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

## Naming and identity (`naming.rs`)

- `name_conflicts(station)`, `conflicts_with_name(station, name)` (a rename
  check), `names_around(station)`: every line the station is on.
- `uniquify_station_name(station)`: the letter-suffix rule
  (`project::station_naming::name_with_free_letter`), free on all its lines.
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
| `legacy_crosscheck_tests.rs` | Surveys built with `models::project` (merges, a merge at a line's last station, hand moves on merged members, datum pieces, a fix, free pieces, 40 seeded random surveys) lay out every slot within 1e-9 m of `compute_station_coordinates`. |
| `graph_tests.rs` | Penetration, distance fields, the ruler, roles, entrances and GPS gaps against the legacy model (30 random legacy surveys). |
| `adjust_tests.rs` | The Computed View per slot within 1e-6 m of `compute_adjusted_stations` (150 random legacy surveys), skip set, failures, alignment, new-closure verdicts, misclosures, path length, spurs, gauge, determinism, scale. |
| `gama_tests.rs` | The GNU Gama gold standard (`tests/fixtures/gama_golden.json`) on networks built directly on the new model. |
| `derived_tests.rs` | Frozen shots, rigid groups, anchored lines, immovable stations, stats against the legacy model; derived views bit-identical under shuffled lines. |
| `gas/tests.rs`, `gas/differential_tests.rs` | The 112 ported gas-engine tests and the old-vs-new differential run (36 surveys x 8 rigs x 5 argument sets). |
| `edit/shots_tests.rs`, `lines_tests.rs`, `ranges_tests.rs`, `reverse_tests.rs`, `split_tests.rs`, `combine_tests.rs` | Each line and shot operation: names, uuids and ids, which side stays, refusals in today's words, junctions kept, datums / comments / closures leaving with a station, promotions, #737 removals, previews. |
| `edit/combine_port_*_tests.rs` | The legacy combine tests ported (lines, closures, snaps, hand moves, residuals, realign, dedupe), each legacy test's fate in the module doc; two random-network sweeps (about 8,700 combines). |
| `edit/edit_fuzz_tests.rs` | 32 seeds x 80 random operations: valid after every step, refusals change nothing, previews equal results, seeds replay bit for bit. |
| `ops_merge_tests.rs`, `ops_detach_tests.rs`, `ops_closures_tests.rs`, `ops_datums_tests.rs`, `ops_move_tests.rs`, `append_tests.rs` | Each M2 command, with ports of the legacy datum, entrance and placement tests. |
| `ops_scenario_tests.rs`, `ops_property_tests.rs` | The model review's scenarios; 48 seeds x 70 random commands (valid after every step, refusals change nothing, no-move commands move nothing, a merge moves exactly the moving piece, a seed replays bit for bit). |
