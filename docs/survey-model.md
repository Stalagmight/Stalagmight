# The station-graph survey model (`models::survey`)

The new survey model of the station-graph redesign
([plan](plans/2026-11-20_station-graph-model.md), sections 1-5). It lives in
`src-tauri/crates/models/src/survey.rs` and `src-tauri/crates/models/src/survey/`,
next to the legacy `models::project`, which it does not use or change (it only
reuses the crate's shared types and pure helpers). Nothing outside the module
uses it yet; streams M1, M2, M3, F, G, U and C build on it.

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
| `survey/types.rs` | `Station`, `StationAlias`, `Line`, `LineMeta`, `Shot`, `ZeroDatum`, `Placement`, `LoopClosure`, `ClosureStrategy` (shared variants plus `Tie`), `ProjectSettings`, `Counters`, `ClosureId`, `MAX_SHOT_UUID_CHARS`. |
| `survey/topology.rs` | `Topology` (chains, slots, components) and `ShotUuidIndex`. |
| `survey/build.rs` | Construction API, id minting, slot lookups, `drop_stale_attachments`, `BuildError`, `StartAt`, `StaleAttachments`. |
| `survey/validate.rs` | `Project::validate` and `Violation` (plan section 2). |
| `survey/positions.rs` | Solver v1 raw positions and depth: `RawPositions`, `Anchor`, `SlotPosition`, `PositionIssue`, `closure_is_aligned`. |
| `survey/anchors.rs` | `AnchorSnapshot`, `EditContext`, `settle_anchors`, `correction_to_hold`, `AnchorReport` (plan 4.1, 4.3). |
| `survey/naming.rs` | Station-name uniqueness on every line of a station, the letter-suffix rule, shot-uuid helpers. |
| `survey/closures.rs` | Closure alignment per closure, the across-one-shot knot rule (skip set, `CLOSURE_ACROSS_ONE_SHOT` data), tie groups. |
| `survey/metrics.rs` | Closure metrics from raw positions (`closure_metrics`, `closures_metrics`), path length along shots. |
| `survey/lsq.rs`, `survey/adjust.rs` | The Closures Computed View (`adjusted_positions`, `AdjustedPositions`, failures, `adjustment_report`). |
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
  deserializing). `ShotId` is skipped by serde.
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
ids (station keys, id ranges, alias collisions, duplicate line / closure /
Section ids, empty, overlong or repeated shot uuids), references (line starts,
shot targets, datum / placement / closure stations, `via` hints, hand-move
keys, Sections), simple chains, orphan stations, the forest (the shot that
closes a cycle), one anchor per component, closures (`a != b`; across pieces
only between two datum-held pieces), station names on every line, counters
above every id in use (aliases included), finite numbers (shots, quality,
azimuth corrections, hand moves, datums, placements, elevation, validation
thresholds). Every `Violation` has a precise `Display` message.

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
| Roles, entrances, gaps | `datum_roles`, `datum_entrances`, `entrance_stations`, `datum_gap`, `gps_fix_gaps`, `far_gps_fixes`, `gps_gap_warnings` | Entrance default: first or last station of the `via` line, else of the station's first line. Survey distance along shots only. |
| Frozen shots | `frozen_shots(&aligned)`, `is_shot_frozen(line, index)` | Tarjan bridges over shots and aligned closures; positional output. |
| Rigid groups | `rigid_groups(&aligned)`, `rigid_lines_of`, `anchored_lines`, `immovable_stations(&ties)`, `merge_pairs()` | Fixes count as anchors for Move Segment and Move Station, as today. |
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
