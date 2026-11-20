# Station-graph model and `.stalx` v2 (JSON)

Status: in progress on `feat/survey-graph` (base `release/0.6.0`).

This plan replaces the survey data model and the save format together:

1. Stations become first-class entities with stable ids. A shot is an edge from one station to the next. A merge is one shared station, so two merged stations can no longer drift apart.
2. The file stores only observations and user decisions. Everything derived (positions, line offsets implied by merges, closure misclosure figures, gauges, alignment flags) is recomputed on load by a deterministic, versioned solver.
3. (Not in scope: per-machine state and undo history stay in the project file.)
4. The file is canonical, line-oriented JSON: same model, same bytes; one record per line; one fsync per save; NaN and infinity refused.
5. Explicit versioning: a format version, a list of required features, and declared unit and angle conventions. A build that meets an unknown required feature opens the file read-only instead of rewriting it.
6. Identity rules are enforced at load: unique ids, every reference resolves, external (imported) ids are kept as plain attributes.

Product decisions already made:
- SpeleoDB pushes send the new JSON.
- An old SQLite `.stalx` is upgraded silently on its first save.
- One PR.
- Undo history of old files is dropped on upgrade (with the existing banner).
- The new history format need not read older histories.

The current-state maps that informed this plan are summarised in "Background" at the end.

---

## 1. The model

### 1.1 Identifiers

| Id | Type | Meaning | Persisted |
|---|---|---|---|
| `StationId` | `u32` | One physical survey station. Equals the user-visible **Station ID** (the existing project-unique, never-reused number). A merged junction has ONE id. | yes |
| `LineId` | `u32` | A line (survey run), as today. | yes |
| shot uuid | `String` (<= 128 chars, opaque) | The persistent identity of a shot, as today. Required on every shot; minted v4 at creation; imported verbatim. Project-unique. | yes |
| `ShotId` | `u32` | In-session handle (`line_id * FEATURE_ID_STRIDE + seq`), exactly as today. Never persisted, never in history. | no |
| `ClosureId` | `u64` | Loop closure id, never reused. | yes |
| `SectionId` | `u32` | Cave Section (#327), as today. | yes |

`StationRef { line_id, station_index }` survives only as a **view address** (a slot: "the i-th station along line L"). It is derived from the line chain, used on the IPC wire and by the frontend, and resolved to a `StationId` at the command boundary. `StationKey`, `StationAnchor`, `AfterShot`, `LineStart` and every re-keying routine disappear from the live model. They live on only in the frozen legacy reader and in the small resolver that maps `.plnx` dive-plan keys (section 10.3).

### 1.2 Entities

```rust
/// models::survey
pub struct Project {
    // identity and metadata (as today)
    pub name: String,
    pub location: Option<String>,
    pub created_at: DateTime<Utc>,   // nanosecond instant must round-trip (.plnx ownership)
    pub modified_at: DateTime<Utc>,

    pub stations: BTreeMap<StationId, Station>,
    pub lines: Vec<Line>,                 // display order = list order (as today)
    pub datums: Vec<ZeroDatum>,           // list order is semantic (frame, roles)
    pub placements: Vec<Placement>,       // user decisions: where each datum-less piece sits
    pub closures: Vec<LoopClosure>,       // list order kept (sweeps, across-one-shot skip set)
    pub sections: Vec<Section>,           // cave Sections (#327), unchanged

    pub settings: ProjectSettings,        // validation, surface_elevation, default_azimuth_reference
    pub imported_file_hashes: Vec<String>,
    pub imported_line_hashes: Vec<String>,
    pub counters: Counters,               // next_station_id, next_closure_id, next_section_id (next_line_id is derived: max + 1, as today, so MAX_LINE_ID never becomes a lifetime budget)

    // session-only
    #[serde(skip)] pub file_path: Option<PathBuf>,
    #[serde(skip)] caches: Caches,         // slot index, component index; rebuilt, never persisted
}

pub struct Station {
    pub id: StationId,
    pub name: String,                     // see 5.3 for uniqueness
    pub comment: Option<String>,
    pub aliases: Vec<StationAlias>,       // ids (and names) merged into this station
    pub external: Option<String>,         // imported identity (Compass/Walls name, Ariane ID), attribute only
}

pub struct StationAlias { pub id: StationId, pub name: Option<String> }

pub struct Line {
    pub id: LineId,
    pub name: Option<String>,
    pub color: String,
    pub direction: SurveyDirection,
    pub quality: f64,
    pub azimuth_reference: Option<AzimuthReference>,
    pub section_id: Option<SectionId>,
    pub legacy_surveyor: Option<String>,  // as today (credit fallback)
    pub legacy_date: Option<NaiveDate>,
    pub start: StationId,                 // station 0
    pub shots: Vec<Shot>,                 // shot i runs stations[i] -> stations[i+1]
    pub hand_moves: BTreeMap<StationId, [f64; 3]>, // legacy per-slot hand moves (5.6); never created by new commands
    #[serde(skip)] pub next_shot_id: ShotId,
}

pub struct Shot {
    #[serde(skip)] pub id: ShotId,       // session handle
    pub uuid: String,
    pub to: StationId,                    // from = previous shot's `to`, or `line.start`
    // measurements, exactly the current fields
    pub azimuth_in: f64, pub azimuth_out: f64, pub length: f64,
    pub depth_in: f64, pub depth_out: f64,
    pub pitch_in: Option<f64>, pub pitch_out: Option<f64>,
    pub left: Option<f64>, pub right: Option<f64>, pub up: Option<f64>, pub down: Option<f64>,
    pub comment: Option<String>, pub kind: ShotKind,
    pub marker_index: Option<u32>, pub temperature: Option<f64>,
    pub credit: Option<Arc<SurveyCredit>>,
}

pub struct ZeroDatum {
    pub station: StationId,
    pub via: Option<LineId>,              // attachment hint: the line whose slot the datum was set on (5.2, 3.8)
    pub gps: GpsCoordinates,
    pub accuracy_m: Option<f64>,
    pub entrance: Option<bool>,
    /// Horizontal displacement (local metres) between where this placement
    /// datum's piece sits and where its GPS says, relative to the current
    /// frame. A maintained field ("measure, do not re-derive", 4.5): set by
    /// frame changes and Add .stalx so nothing moves; cleared by a GPS edit of
    /// this datum and when it is demoted to a GPS fix. None for a datum that
    /// sits exactly on its GPS.
    pub correction: Option<[f64; 2]>,
}

/// The anchor of a piece that holds no placement datum: `station` sits at
/// local (x, y). Exactly one per such piece.
pub struct Placement { pub station: StationId, pub x: f64, pub y: f64 }

pub struct LoopClosure {
    pub id: ClosureId,
    pub a: StationId,
    pub a_via: Option<LineId>,            // attachment hints (5.2)
    pub b: StationId,
    pub b_via: Option<LineId>,
    pub strategy: ClosureStrategy,        // Batch, Move (snap), Tie (legacy rigid tie, 3.6), legacy variants kept for reading
}
```

Notes:
- A line is a **simple chain**: its station list `[start, shots[0].to, shots[1].to, ...]` holds no StationId twice. That keeps `(line, index)` well defined and every positional consumer (renderer, data table, delete ranges, split, combine, `shotLabelAt`) valid.
- The station list of a line is derived (cached) and never stored separately.
- A station belongs to every line whose chain contains it. A station on two or more lines is a **junction**: that is what a merge was.
- Nothing above is derived. Positions, offsets, closure metrics, alignment flags, datum roles, gauges, stats, merge pairs and frozen shots are all computed (section 3).

### 1.3 What goes away

`line_offsets`, `station_offsets` (became per-line `hand_moves`, legacy only), `station_merges`/`MergeTopology`, `StationKey`/anchors, `station_names`/`station_ids`/`station_comments` side maps (now on `Station`), `held_gauges`, line gauge pairs, `counter_parity_shots`, closure metrics and `is_aligned` as stored fields, the offset cascade, `realign_merges` and its fixpoint, `realign_after_shot_edit`, insert/delete/reverse offset compensation, replacement merges after deletes, merge-alignment integrity checks, `demote_double_anchoring_merges`, all re-keying (prepend fix-up, delete re-anchor, reverse mirror, split/combine rekey, uuid remaps of keys), `loaded_created_at`, `loaded_legacy_surveyor`, save-time heals.

---

## 2. Invariants (checked by `Project::validate`, asserted after every fuzz move)

1. Ids unique: StationId, LineId, shot uuid (project-wide), ClosureId, SectionId.
2. Every reference resolves: `line.start`, `shot.to`, datum/placement/closure stations, `hand_moves` keys (must be on that line), line `section_id`.
3. Every line is a simple chain (no StationId twice in one line's chain).
4. Every station is on at least one line, or is a **lone station** (a line with zero shots has `start` only; stations on no line are not allowed).
5. **Forest:** the graph whose nodes are stations and whose edges are shots is a forest per connected component. No cycle without an explicit loop closure. (Equivalently: lines + shared stations never close a loop. This is today's merge-forest invariant, now structural.)
6. Placement: every connected component has exactly one anchor: its first placement datum in list order, else exactly one `Placement` whose station is in it. No `Placement` in a component that holds a datum.
7. Closures: `a != b`. A closure's ends may be in different components only if both components hold a datum (today's rule). "Across one shot" is NOT an invariant: it is refused at creation (#737) and otherwise derived as a skip set (today's knot rule over closures in list order, transitive through other closures and snaps) plus the `CLOSURE_ACROSS_ONE_SHOT` warning, exactly as today (old files, Combine, promotions and Add can produce such closures).
8. Station names: unique within each line, across all stations on that line (5.3).
9. Stored counters are above every id in use (monotonic, never reused). `next_line_id` is derived on load (max + 1).
10. Numbers finite.

`validate` returns typed violations. The writer runs it before every save and refuses to write an invalid model (the in-memory state is kept). Load refuses a file with any violation unless opened through salvage (6.6). Commands roll back if they would introduce one (as the integrity guard does today). `validate` may only become stricter together with a format feature (`requires`) and a migration.

---

## 3. Derived state: solver v1

All derived values are pure functions of the model, computed with sorted iteration (BTreeMap or sorted vectors; no HashMap iteration influences a result). The file records `"solver": 1`. A future build that changes any formula bumps it.

### 3.1 Components and slots

- A union-find over line chains gives components (stations joined by shots, junctions joined because they are one station).
- **Slots:** for each line in list order, for each chain position, `(line_id, index) -> StationId`, plus the inverse `StationId -> [slots]` sorted by (line order, index).

### 3.2 Shot vector

Exactly today's formula: horizontal length `sqrt(length^2 - (depth_out - depth_in)^2)` clamped at 0; azimuth = circular mean of `azimuth_in/out` plus the line's azimuth reference; vector `h * (sin az, cos az)` from `from` to `to`. Reuse the existing functions, do not re-derive them.

### 3.3 Raw (measured) horizontal positions

For each component, find the anchor:
- **Frame** = the first resolvable datum in list order across the whole project. Local (x, y) is ENU on the tangent plane at the frame's GPS, as today.
- If the component holds a datum, the first one in list order is its **placement datum**: its station sits at `gps_to_local(gps, frame.gps) + correction`. The frame station is at `(0, 0) + correction`.
- Otherwise the component's `Placement` gives `(station, x, y)`.

Traversal from the anchor station:
1. Seed: anchor station position `P`.
2. Visit lines through a positioned station in a deterministic order: a BFS queue of `(station)`, and for each station its lines in list order; a line is walked once, the first time it is reached.
3. Walking line L reached at station S: `chain(S) = P(S) - hand(L, S)`; for every other station k on L, `chain(k) = chain(S) ± Σ shot vectors` walking outwards from S along L (forwards add, backwards subtract, in chain order); `shown(L, k) = chain(k) + hand(L, k)`. Each station not yet positioned gets `P(k) = shown(L, k)` and joins the queue.
4. Because of invariant 5 every station is reached exactly once per component; nothing is torn by construction.

Hand moves (5.6) are legacy-only and absent from almost every file; without them `shown = chain` and the walk is the plain sum of shot vectors.

### 3.4 Depth (Z)

As today: absolute, never walked. `Z(S)` = mean of the depth readings at S over every line through S (`depth_out` of the incoming shot, `depth_in` of the outgoing shot), plus the readings of every station tied to S, **transitively**, by aligned snaps and `Tie` closures (3.6), summed in sorted order (#469). Slot Z = `Z(S) + hand(L, S).z`.

### 3.5 Wire positions per slot

The wire carries one entry per slot (as today). Slot `(L, i)` with station S: XY = `P(S)` (all slots of a station agree), Z = slot Z. Merged-with / closures-with lists are derived from shared ids and closures.

### 3.6 Closures: alignment and metrics

- `is_aligned(c)`: for `Move`, horizontal gap between the ends < 1 cm AND the tie would not double-anchor (recomputed every time; today it is stored and re-measured). For `Tie` (legacy rigid tie): always true, never derived. Other strategies: false.
- `Tie` exists only in migrated files: merges the migration could not collapse (cycle-closing, same-line, self) and legacy aligned non-`Move` closures (whose stored `is_aligned` today is trusted, never re-measured). New commands never create it. It keeps today's behaviour for those ties: depth co-location, rigidity, LSQ one-point tie.
- An aligned closure (snap or tie) is a rigid tie for: depth co-location (3.4), Move tools' co-located sets, rigid groups, frozen shots, immovable stations, LSQ ties. It does not move raw XY (a snap always closes a loop inside a component, or joins two placed pieces, so it never places anything).
- Metrics (`path_length`, `misclosure_distance`, `error_percentage`, `horizontal_misclosure`, `vertical_misclosure`) are recomputed from raw positions. `path_length` is the shortest path along shots between the ends (junctions are free, as one station; **closures are not traversable**). This replaces today's creation-time value that counted earlier closures as zero-length edges; values on old files can change and that is accepted.

### 3.7 Closures Computed View (least squares)

The current algorithm (`lsq.rs`) is kept, operating on stations instead of slots: one unknown per station (a junction is one unknown), shots as observations with today's weights, eligible closures as ties, snaps tied, hard fixes on GPS, soft fixes weighted, across-one-shot closures skipped and reported, spurs untouched, per-component fallback with a report.
- **Gauge** of a datum-less component = its `Placement` station, held at its raw position. This replaces `held_gauges` and gauge pairs, and is stable across every edit that does not delete that station.
- Unknowns numbered by sorted StationId.

### 3.8 Other derived values

Datum roles (first datum per component = placement, others GPS fixes), entrance defaults (placement datum; else override; else first or last station of its `via` line, or of its station's first line when `via` is unset; migration writes explicit overrides wherever this default would differ from today's), GPS gaps and their warnings, frozen shots, rigid groups, immovable stations, anchored lines, stats, survey graph for Plan/gas (junctions are one node; closures zero-cost edges as today), merge pairs for the wire (3.5 / 9.2).

---

## 4. What stays still: placement rules on edit

The traversal makes "the anchored side stays" automatic: every station's position is derived from its component's anchor. Edits change anchors only through the single post-edit algorithm in 4.1.

### 4.1 One post-edit anchor algorithm

Every command (and every bypass recipe) runs this after its mutation and before validation:

1. Before the mutation, record `P_before(s)` for every station (raw positions; cheap, cached) and the set of components with their anchors.
2. After the mutation, for each component of the new model:
   - **holds a datum:** its first datum in list order places it (with its `correction`). If that datum was a GPS fix before (promotion because the old placement datum left the component), the piece moves onto its GPS (today's `settle_datum_layout`), correction None.
   - **holds no datum:** choose the *still point* `s*`, the first that applies: (a) the command is a geometry edit of line L (UpdateShot, insert, delete, prepend, split shot at distance, range delete) and the component contains L's pre-edit first station: that station; (b) the component contains its old `Placement` station: that station; (c) the first surviving station of the edited line in it; (d) its lowest StationId. Lay the component out with a temporary anchor `s*` at `P_before(s*)`.
   - Choose its `Placement` station `g`: its old `Placement` station if it survives in it (so the least-squares gauge never moves, 3.7); else, for a component formed by a merge, the stationary piece's `Placement` station; else its lowest StationId. Store `Placement = (g, laid-out position of g)`.
3. A free piece that gains a datum (Set datum) drops its `Placement`. A piece that loses its last datum (Remove datum, detach, delete) gets a `Placement` by step 2 at its current position (nothing moves), unless it still holds a datum (then that one places it, step 2 first bullet).

This reproduces today's rules: datum pieces stay on their datum; in a free piece the edited line's first station stays; merges move the piece of `station_a` onto `station_b` unless that would move a placed piece (4.2); cut-off pieces stay where they were unless a promoted GPS fix places them.

### 4.2 Merge direction

The piece of `station_a` moves onto `station_b`, unless A's piece is placed and B's is not (then B's moves), or both are placed (the piece whose placement datum is later in list order moves; its datum becomes a GPS fix by derivation and its `correction` is cleared). The moving piece's `Placement` (if any) is dropped.

### 4.3 Frame change ("measure, do not re-derive")

When the frame (first resolvable datum) changes, nothing moves relative to anything else; the survey is translated rigidly so the new frame station is at local (0, 0), exactly as today's `renormalize_frame`:
- `T = -P_before(new frame station)`; every `Placement` is translated by `T`;
- every placement datum `i` gets `correction_i := P_before(i) + T - gps_to_local(G_i, G_newframe)` (so the new frame's correction is 0 and every other piece keeps its place despite tangent-plane non-additivity);
- corrections below 1e-9 m are stored as None.
A GPS edit of a placement datum clears its correction and places its piece on the new GPS (the frame stays at (0, 0)); re-setting the frame's own GPS moves the other pieces by `current + new - old`, as today's `place_pieces_on_frame`. A datum demoted to a GPS fix has its correction cleared.

### 4.4 Other rules

- **Move Segment / Move Line:** translate the free piece's `Placement`; refused if the rigid group holds a datum (as today).
- **Move Station:** rewrite every shot incident to the station, on every line through it (a junction has three or more), so the station lands on the target and its neighbours stay (today's shot rewrite, generalised); if the station is its free piece's `Placement` station, move the placement too; refused on datum stations (as today); deletes the station's hand moves on every line.
- **Reverse line:** reverse the chain and flip each shot (today's `reversed_azimuths`, depth swap, pitch swap and negation, optional L/R swap). Positions do not change (up to the flip's documented ulp rounding).
- **Add .stalx:** the copied pieces move rigidly by the same vector today's `place_by_datum` uses (the source's first datum station onto `gps_to_local(G_source, frame)` or the origin): copied `Placement`s are translated by that vector and copied corrections recomputed with the frame-change formula, so free pieces land next to their cave.

## 5. Topology operations

Each operation is expressed on the new model; previews (merge dialog, delete impact, combine plan) report the same user-facing facts as today, computed from the model before and after (a dry run on a clone is acceptable and preferred over parallel bookkeeping).

### 5.1 Merge stations (`merge_stations(a, b)`)

Refused when: same station; the two stations are already connected (shots, junctions or aligned snaps), so the user is routed to Create Loop Closure, as today; one station per line rule (a line may not pass through the merged station twice).

Effect: station `b'` (the moving side's station, see rule 4.2) is replaced by the surviving station `s` everywhere (`line.start`, `shot.to`, datums, closures, placements, hand-move keys). The survivor keeps its id, name and comment; the retired station's id and name go to `s.aliases`; its comment is appended to the survivor's if both are non-empty and different (separated by a blank line), else whichever exists. Datums of both are kept (a station may carry more than one datum; the first in list order is the one the datum dialog edits). Closures between the two become `a == b` and are removed (#738). Names are re-uniquified on every line through `s` (letter suffix rule).

**Survivor:** the station on the side that stays put (4.2). In the Merge dialog the user sees the same two stations as today. Datums and closure ends keep their `via` line hints, so a later detach can give them back to the line they came from. The orphan-closure sweep runs after a merge (as today).

### 5.2 Unmerge (detach)

`unmerge_station_pair(slotA, slotB)` detaches **slotB's line** from the shared station. `unmerge_stations(slot)` detaches that slot's line from every other line at that station. Detaching line L from station S: L's slot gets a new station S' (new id; name copied from S, then uniquified; no comment or aliases), placed exactly where S was. Datums and closure ends whose `via` is L move to S' (so today's unmerge results are reproduced: a fix that belonged to L's member promotes and places L's piece on its GPS; a closure that belonged to L's member becomes an orphan and is promoted by the sweep). L's hand move at S moves to S'. Components and anchors per 4.1; then the orphan-closure sweep (5.5) runs.

Declared behaviour change: "Unmerge station" on a hub of three or more lines detaches the chosen slot's line only (today it removed every direct merge pair of that member, which could split a hub into three pieces). The user can detach further lines one at a time.

### 5.3 Station naming

The creation rules (End, Mid, Prepend, Combine prefix) are unchanged and apply to the line the station is created on. **Uniqueness:** a station's name must be unique among the stations on every line it is on. Renaming a junction checks all of its lines. Migration and merges uniquify with the existing letter-suffix rule.

### 5.4 Shots and lines

- **Create shot** (append, insert before/after, prepend, split shot at distance): a new station is minted for the new chain position (insert: the new shot ends at a new station and the following shot now starts there; prepend: new start station). Ids: `next_station_id`.
- **Delete shot** removes one station from the line exactly as today (station 0 for the first shot, else the shot's end station; the following shot re-attaches to the shot's start). If the removed station is a junction it **stays** for the other lines (their connection is untouched); if it is on no other line it is deleted with its comment, datums and hand moves, and its closures are dropped. Then the sweep (5.5) runs. This makes #776 impossible by construction: closures on a junction stay on the junction; there is no replacement or promotion planning per closure.
- **Delete shot range** (gap-close / split / prefix / suffix) and **delete line**: the same rule per removed station.
- **Split line here** (at station k): the second half becomes a new line whose start is **the same station k** (a junction). This replaces today's duplicate co-located unmerged station: the two halves stay connected. Datums and comments stay on the one station; the hand move at k is copied to the new line (`hand(L2, k) = hand(L1, k)`). Declared consequences: edits to one half now move the other, Move Segment on the second half is refused when the first half holds a datum, and merging the halves' far ends routes to the closure dialog.
- **Combine lines** must reproduce today's results (`combine_plan.rs`, `combine_lines.rs`; the existing `combine_*_tests.rs` are ported first and are the oracle). Algorithm on the station graph:
  1. Choose the moving side M by today's rule: the other line's side moves, unless its side is datum-held and the kept side is not, or (no datum deciding) its side holds the component's `Placement` station (the adjusted-view gauge) and the kept side does not.
  2. If the two join stations are already connected (across a closure, or side by side within one component), detach the junction on the path between them nearest M (today's "the one nearest M, so the least moves"), giving the M-side line a new station, and record a `Batch` closure between the two stations of that torn junction.
  3. If both sides are placed, detach the junctions from M towards the datum sides into `Batch` closures instead of merging two placed pieces; refuse only when both lines carry a datum on their own stations (as today).
  4. Merge the other line's join station into the kept line's (5.1), then concatenate the chains (reversing the other line first if needed).
  5. Snaps torn by the move become `Batch` (as today). The kept line's hand move wins at the join; the other line's hand moves move to the kept line.
  6. A tie that would make the combined line pass a station twice is torn into a `Batch` closure (invariant 3).
- **Delete datum / set datum, merge, detach, delete:** anchors per 4.1; the orphan-closure sweep runs after Merge, Unmerge/Detach, RemoveZeroDatum and every delete (as today).
- **Reverse:** 4.8.

### 5.5 Closures

Create (refusals as today: same station, already a closure for the pair, across one shot, unconnected unless both pieces are placed), remove by id. **Orphan-closure sweep** after topology edits (not on load): a closure whose ends are the same station is removed; a closure that is the sole connection between its two ends' components (no path over shots) is **promoted**: the two stations are merged (5.1, with the movable piece moving onto the other, refused if it would double-anchor), iterating in list order so a later closure sees an earlier promotion. Removed and promoted closures are reported as warnings, as today. Promotions never fire on load.

### 5.6 Hand moves (legacy)

Only migrated files carry them (no current command creates one). Stored per (line, station) because merged members could carry different values. Move Station on a station deletes its hand moves on every line (folding them into the rewritten shots, as today). Split copies the split station's hand move to the new line; Detach moves it to the new station; Combine keeps the kept line's at the join and moves the other line's entries to the kept line.

### 5.7 Datums

Set (refusals as today; setting a datum on a piece that already has a placement datum stores a GPS fix by derivation), remove, accuracy and entrance edits, frame rules (4.5), previews (`preview_zero_datum`, merge datum shift). A station may hold several datums (from migrated merges, or merging two placed pieces at stations that each had one).

### 5.8 Add `.stalx` / Merge projects

Copy stations (ids offset by the target's `next_station_id`), lines, datums, placements, closures (ids re-minted), Sections, as today; no cross-source junctions are created (unchanged behaviour). The copy moves rigidly per 4.4 (placements translated, corrections recomputed). Source files may be SQLite (migrated first, 7) or v2 JSON.

---

## 6. File format: `.stalx` v2

### 6.1 Container and sniffing

- Extension stays `.stalx`. Content is UTF-8 JSON, no BOM.
- Sniff: a file starting with `SQLite format 3\0` is legacy (7); a file starting with `{"format":"stalx","version":` is v2; anything else is refused as not a Stalagmight project.
- One `stalx::probe(path) -> Header { format, summary, speleodb, local }` API reads the cheap header of either format; every link reader and opener uses it or the full open: load, Add .stalx, Recents metadata, the foreign-copy probe, logout's forget-recents (`speleodb_logout.rs`), the cached-copies / local-state listing (`speleodb_local_state.rs`), the access probe (`speleodb_admin.rs`), `survey_geometry`, recovery/backups, create project and Merge Projects (no SQLite temp anymore), SpeleoDB pull/install/revisions/revision swap/unwind, stalx-reader.

### 6.2 Layout

One JSON object. Each record on its own physical line; members in this fixed order; empty optional members omitted.

```
{"format":"stalx","version":2,"solver":1,"requires":[],
"conventions":{"length":"m","angles":"deg","azimuth":"cw-from-north","inclination":"+down","depth":"+down","local":"enu-m"},
"generator":"Stalagmight 0.6.0",
"summary":{...},
"project":{"name":...,"location":...,"created":...,"modified":...},
"settings":{...},
"counters":{"line":N,"station":N,"closure":N,"section":N},
"speleodb":{...shared link identity...},
"local":{...per-machine state: display settings, sync state, undo chain/watermark...},
"sections":[
{...one cave Section per line...}
],
"stations":[
[id,"name",{optional: comment, aliases, external}],
...one station per line, ascending id...
],
"lines":[
{"id":..,"name":..,"color":..,...,"start":id,"hand":{...}?},
[uuid,to,length,az_in,az_out,depth_in,depth_out,{extras}?],   <- one shot per line, under its line header
...
],
"datums":[...],
"placements":[...],
"closures":[...],
"ignored_warnings":[...],
"imports":{...},
"history":{"undo":[...one entry per line...],"redo":[...]},
"end":true}
```

The exact field lists and the shot-row column layout (including how LRUD and optional fields are written, extras object keys, and the `lines` header/row interleaving) are defined in `docs/file-format.md`, written with the encoder, and pinned by tests. The prototype on `bench/stalx-json-encoder` is the starting point for the writer, the float/time helpers and the row visitor.

### 6.3 Canonical rules

- Same model, same bytes (byte fixpoint `save(open(save(M))) == save(M)`).
- Key order fixed; ids ascending where the order carries no meaning (stations, sections); list order kept where it does (lines, datums, closures, placements, history).
- Floats: finite only (NaN and infinity refuse the save with an error naming the field); the shortest representation that reads back to the same bits, including `-0.0`.
- Times: RFC 3339 UTC with full nanoseconds, after #810's normalisation.
- Strings: JSON-escaped minimally and deterministically.
- `"end":true` is the last member, so a truncated file never parses as a project.

### 6.4 Versioning and required features

- `version` is the major format version (2). A higher major is refused with the existing "newer version" prompt, file untouched.
- `requires` lists feature names this file uses that a reader must understand. This build understands its own list. An unknown name makes the project open **read-only** with a banner ("made with a newer Stalagmight"); saving the body is refused (local-only saves, 6.5, still work).
- Unknown content, three tiers:
  1. Structural records (stations, lines, shots, datums, placements, closures, sections): an unknown key opens the project read-only with the same banner. Never a refusal.
  2. `project`, `settings`, `speleodb`, `summary` and unknown top-level members: preserved verbatim on re-save (top-level members after the known ones, in their original order).
  3. `local` and `history`: lenient. Unknown keys in `local` are dropped with a log line; an unreadable `history` is dropped whole with the existing banner, never making the project unopenable.
  A newer writer must declare a `requires` feature for anything that changes meaning or references ids.
- `conventions` is fixed for this version and checked on read (any other value is refused).
- `summary` is a non-authoritative cache for Recents (name, counts, length, bounds, SpeleoDB id); a reader may use it without parsing the body; the writer always recomputes it; tests pin that it equals the recomputed value.

### 6.5 Save path and modes

Pipeline: `validate`; serialize in memory; write `<file>.stalx.tmp`; `sync_all`; check the external-modification guard; rename over the destination; fsync the parent directory. One data fsync and one directory fsync. Windows: no open handle on the destination. The app keeps no live database handle (stream AS).

Serializer modes:
1. **Normal** save.
2. **For push** (SpeleoDB): no `local`, no redo; undo only in Full mode, cut at the watermark and then by bytes so the whole file fits 5 MiB (newest entries kept).
3. **Detached copy** (Save a Copy of a managed project): no `speleodb`, no SpeleoDB keys in `local`, no history inherited from the link (today's `copy_only`).
4. **Local-only durable save:** rewrites `speleodb`, `local` and `history` while copying every other member of the last saved file verbatim. Used immediately for every SpeleoDB state change (push result, pull install, unwind chain, watermark, verify stamp, history cut, logout bookkeeping) and for display-setting toggles. Allowed on read-only projects and on files with an unknown `requires`. Never used on a legacy SQLite file that has not been upgraded yet (such changes stay in memory until the first real save).

Rules:
- A push first saves (normal), then serializes "for push" from that same in-memory snapshot.
- Warning ignores are survey content: they mark the project dirty and ride the next save (today they were written into the file immediately; declared change: "Don't save" now discards them).
- **Recents rename** of a closed file: v2: probe, replace `project.name` and `summary.name`, rewrite with every other member verbatim (local-only style). Legacy SQLite: upgrade (7) with the backup, then rename.
- **External-modification guard:** remember size, mtime and a hash of the bytes last read or written. Before the rename, if the file on disk differs, refuse the autosave and ask (reload, overwrite, save as).
- `-0.0` is written as `-0.0`; every number is parsed as `f64` (never through an integer path).

### 6.6 Load and validation

Parse with size and count limits (port stalx-reader's `CountLimits`), check `format`/`version`/`conventions`/`requires`, build the model, run `validate` (section 2). A violation refuses the normal open with a precise message and offers **salvage**: open read-only with a report, plus a best-effort repair into a copy (drop dangling references, give anchorless pieces a `Placement`, break a cycle into a `Batch` closure), mirroring today's CORRUPT/Repair route. Load never mutates the model (no heals); the in-memory project equals the file.

---

## 7. Migration from SQLite (legacy)

### 7.1 Frozen legacy reader

`src-tauri/crates/legacy` (crate `stalagmight-legacy`): a frozen copy of today's model, SQLite reader and load heal pipeline, taken at the cutover commit, with its own module paths. It is never changed except to fix a migration bug. It exposes:

- `legacy::open(path_or_bytes) -> LegacySurvey`: copies the file and any `-journal` to a private temp, runs the old migration chain and the full old load pipeline (exactly what the app did on open), and returns the settled legacy project plus raw positions and Closures Computed View positions per slot, and the gauge station of each datum-less component.
- For tests only (`feature = "writer"`): `legacy::write(project, shape) -> bytes`, the v18 writer (and the older shapes where cheap), so property tests can produce "what an old build wrote".

### 7.2 Conversion

1. **Stations:** union-find over the legacy merges. Self-merges, same-line merges and merges that would close a cycle (taken in the old cascade's BFS order) become `Tie` closures instead, placed at the FRONT of the closure list (today merges are united before any closure, so later closures keep their skip verdicts). Each group is one station. Survivor id = the member the map labelled (the label-representative rule: not a line's first station, then not its last, then line order, then index); others become aliases with their names. Names: survivor's, uniquified. Comments: distinct non-empty member comments joined in member order with a blank line. Datums: all kept, in list order.
2. **Lines:** shots in order with their uuids (the legacy load already made them project-unique; mint any still missing deterministically from line id and index), chain stations mapped.
3. **Hand moves:** each legacy `station_offsets` entry goes to `line(key.line).hand_moves[station]`.
4. **Closures:** ids, ends, strategy kept. Legacy aligned non-`Move` closures become `Tie` (their stored alignment is trusted today and never re-measured; `Tie` keeps that).
5. **Anchors:** per component: if it holds a datum, the first one in list order places it; `correction = legacy_raw(datum station) - gps_to_local(gps, frame.gps)`, stored when its magnitude exceeds 1e-9 m. Otherwise a `Placement` at the legacy gauge station (held gauge, else the legacy default gauge) at its legacy raw position.
6. **Entrances:** a datum whose new default entrance (3.8) differs from today's gets an explicit `entrance` override. `via` hints are filled from the legacy member that held each datum and closure end.
7. **Verify:** compute new raw and Closures Computed View positions and compare per slot with the legacy ones. Tolerance: raw XY within `1e-6 m x (merge-tree depth of the slot's line + 1)` (the old cascade leaves up to 1e-6 m dust per hop), adjusted within 1e-6 m, Z within 1e-9 m. Expected-difference classes (drifted or torn legacy merges collapsed into one station) are reported separately with their maximum displacement; anything else is a mismatch. Mismatches are logged and in the MigrationReport; tests and the corpus run assert zero unexplained mismatches.
8. **History:** dropped, with the existing "history discarded" banner (as v7/v13/v15/v16/v17 and #808 did).
9. **Everything else** (settings, Sections, credits, import hashes, SpeleoDB link and per-machine state, display settings) carried over. Ignored warnings are re-keyed through the legacy load's shot-uuid backfill (pre-v8 rows keyed by session shot id included).
10. **Backup:** on the first v2 write over a SQLite file, the original file and any `-journal`/`-wal` sidecar are moved into the app-data backups folder (silent; listed by `list_backups`, restorable) before the rename, so no old build can later roll a stale journal into the JSON file. When verification reported unexplained mismatches, the app shows one non-blocking notice ("Converted from the old file format; N stations moved by up to X m; the original is kept in Recovery").

The result is held in memory; the original file is untouched until the first save writes v2 over it (silent upgrade).

### 7.3 MigrationReport

Every normalisation and every conversion rule that fired, with counts: merges collapsed, aliases created, stations renamed, comments joined, cycle/same-line/self merges demoted to `Tie`, aligned closures converted to `Tie`, corrections stored, placements created, entrance overrides written, hand moves carried, ignores re-keyed or dropped, expected-difference classes and their maximum displacement, verification mismatches. Logged; returned to tests; never shown raw to users.

---

## 8. Undo and redo

### 8.1 Engine: id-keyed changesets

One payload type replaces the 14 `UndoData` variants and the #808 text delta engine:

```rust
pub struct Changeset {
    pub ops: Vec<Op>,                // ordered
    pub counters: CounterChange,     // before/after, applied as max(live, recorded)
}
pub enum Op {
    Station { id: StationId, before: Option<StationImage>, after: Option<StationImage> },
    LineMeta { id: LineId, before: Option<LineMetaImage>, after: Option<LineMetaImage> },
    Shots { line: LineId, edit: ShotListEdit },   // keyed by uuid: inserts/deletes with full rows, field-level changes
    Datums { before: Vec<ZeroDatum>, after: Vec<ZeroDatum> },          // small, whole list
    Placements { before: Vec<Placement>, after: Vec<Placement> },
    Closure { id: ClosureId, position, before, after },
    Sections { ... }, Settings { ... }, ImportHashes { ... }, Name/Location { ... },
    LineOrder { before: Vec<LineId>, after: Vec<LineId> },
}
```

- Capture is generic: snapshot the model before the command, diff after the full epilogue (field-level images for touched entities only). No per-command capture code.
- Apply checks preconditions (the live value equals the recorded `after` before an undo, `before` before a redo); a mismatch refuses the step and keeps the entry (as today's refusal semantics). Then recompute derived state once (undo_to/redo_to recompute once at the end).
- Encoding must meet the existing gates on the (smaller) v2 file: `Shots` ops are per-line runs (index range plus one uuid anchor per run, verified on apply) with values as compact arrays; credit images are interned once per op; Reverse is `{line, swap_lr, exceptions: [(index, field, before_bits)]}` where the exceptions are computed at capture by re-applying the flip to the after-state and diffing (the flip is not an exact involution: #808's per-copy corrections are the precedent).
- Gates ported (`undo_size_gate_tests.rs`): every entry at most 1% of the v2 file on the 5k and 20k synthetic surveys (except actions that construct the whole file), the 40-bytes-per-changed-cell paste gate, the credit and rename-1,000-stations gates; undo/redo bit-exact.

### 8.2 Kept as is (UX contract)

IPC (`undo`, `redo`, `undo_to`, `redo_to`, `can_*`, `get_undo_stack`, `get_redo_stack`), `UndoHistoryItem` (description, timestamp, commandType, preview), record-time labels for composite edits and live name resolution for single commands (names of deleted entities from before-images), previews, failure semantics, strictly increasing timestamps, SpeleoDB watermark and clear-on-push, byte budget, history limits, privacy (no paths), command lock and lock order, read-only gate.

### 8.3 Persistence

`"history":{"undo":[...],"redo":[...]}` in the file, one entry per line: `{"t":timestamp,"k":<kind>,"label":<record-time resolved label>,"refs":{stations, lines, shots by uuid},"ops":[...]}`. The forward `Command` is never persisted (its payload holds session `ShotId`s and positional slots); labels and previews are resolved at record time or from ids and before-images. Pushes strip redo always and undo unless Full mode, cut at the watermark and then by bytes (6.5). The Full-mode in-file history budget is lowered from 32 MiB (sized for gzip in SQLite) to 4 MiB of JSON. Histories from older files are dropped (7.2.8).

---

## 9. Commands, IPC and the frontend

### 9.1 Commands

The `Command` enum stays the forward API with its current payloads (positional `StationRef` slots, `LineId`, session `ShotId`, `ClosureId`); the executor resolves slots to `StationId` at execute time. One execute pipeline: lock, gate, snapshot, mutate, validate (rollback on new violation), sweep (topology commands), recompute, diff into a changeset, push. The eight bypass recipes (split, combine, delete-split family, imports, repair, settings, elevation) call the same pipeline with a label.

A batch that moves several slots of one station (today's co-located `move_station` batch) moves that station once.

### 9.2 Wire compatibility layer

`get_project_state` returns `ProjectState { project: ProjectWire, stations, stats, frozen_shots }` where `ProjectWire` keeps the TS `Project` shape the frontend reads today: `lines` with `shots`, `station_merges` **derived** (for each junction, pairs from its first slot to each other slot), `zero_datums` and `loop_closures` with slot refs (first slot of the station) and **derived** metrics and `is_aligned`, `cave_sections`, counters. `line_offsets` is dropped from the wire (nothing reads it). `Station` per slot: `station_ref`, coordinates, `merged_with` (other slots of the same station), `closures_with`, and the shared `id`, `name`, `comment`. The binary load envelope keeps its layout where it can and bumps `EXPECTED_VERSION` if it changes.

### 9.3 Frontend changes (only where semantics changed)

- Junction slots share one Station ID, name and comment: labels group by id; rename validates across all the station's lines.
- Unmerge: "Detach <line> from this station" (pair unmerge detaches the chosen partner's line).
- Station details: "Merged with" lists the other lines through the station; "Connected" shows every incident shot (a junction has three or more), not just previous/next.
- Move tools: one move per station.
- Split line: the halves share the split station (copy in the dialog).
- Delete/combine impact panels: drop replacement-merge rows that can no longer happen.
- `get_line_relationships`: `has_line_offset` becomes "the line's piece has a non-trivial Placement" (or false) and `station_offset_count` counts hand moves; the Delete Line confirm keeps working.
- `plan_anchors_for_refs` / `plan_refs_for_anchors` are generated from the new model (StationId -> `AfterShot(uuid)` / `LineStart` keys).
- `e2e-playwright/reverse-line.spec.ts` and the agent harness (`fuzzOracleCore.ts`, `undoPersistenceFuzz.ts`, `fixtures.ts` via `node:sqlite`) read or inject `line_offsets` or SQLite: updated with the wire change.
- Add a shape check for identity-bearing commands in the mock-contract test (it checks only names today).
- The e2e IPC mock: shared ids for merged slots, transitive `merged_with`, no fake auto-merge; the mock-contract test still gates command names.

---

## 10. Other consumers

### 10.1 SpeleoDB

- Push: the canonical v2 bytes in a "for push" mode (no `local`, no redo; undo only in Full mode, cut at the watermark), filename `project.stalx`, content type `application/json`. Fallback without history when over 5 MiB, as today.
- The server currently rejects non-SQLite uploads; until it accepts v2, a push fails with the existing error path (the project stays dirty and locked). This is called out in the PR.
- Pull/install/revisions/unwind: sniff; SQLite revisions are migrated in memory (7), v2 opened directly. `drop_inherited_undo`, display-setting restore and the dirty columns map to the v2 `history` and `local` members.

### 10.2 stalx-reader

Reads both formats (sniff); SQLite through the legacy crate; v2 directly. Its public `Survey` keeps its positional station ids and gains the shared `StationId` per station.

### 10.3 `.plnx` dive plans

Unchanged file. Stored keys resolve on the new model: `LineStart(line)` -> `line.start`, `AfterShot(uuid)` -> that shot's `to`; plan anchors by neighbour shot uuids as today. `created_at` keeps its nanosecond instant.

### 10.4 Recents, foreign-copy probe, recovery

Recents reads `summary` from the first bytes; the foreign-copy probe reads `speleodb`; `list_backups`/`restore_from_backup` accept and validate v2 files and temps.

### 10.5 Importers and exporters

Importers keep producing lines, placements and closures through the shared funnel. A placement into an existing or same-batch station makes the line's start (or the chosen chain station) **that station** instead of recording a merge. Compass/Walls/Ariane names and ids are stored in `Station.external` (item 6). Exporters read stations through the chain (CSV/JSON `from_id`/`to_id` become shared ids at junctions; Ariane export walks the station graph). Behaviour that changes is listed in the PR.

---

## 11. Tests and fuzzing

1. **Legacy oracle and migration** (`core/state/migration_property_tests.rs`, gated): generators build projects through the legacy API (`legacy` crate), write them with the legacy writer, migrate, and assert: total station map; merged members map to one station; every slot position within tolerance (7.2.6) raw and adjusted; measurements bit-identical; counts; closures preserved or reported; `save(migrate(x))` canonical and reopen-identical. Generators cover floating components, datum pieces with stored offsets, multi-datum components, hand moves, drifted merges, merge cycles, closures across one shot, orphan closures, aligned closures, reversed/split/combined lines, empty and single-shot lines, missing uuids, old value conventions. The two tracked fixtures (`e2e/fixtures/*.stalx`, v4 and v14) are migrated in a Rust test. An `#[ignore]` corpus runner (`STALAGMIGHT_MIGRATION_CORPUS=<dir>`) prints only counts and failure classes.
2. **Format properties** (`format_v2_property_tests.rs`): byte fixpoint, round trip, canonicalisation of shuffled/whitespace/escape/float-spelling variants, float bit round trip (subnormals, -0.0), diff locality per command (bounded changed lines), version/requires/conventions gating, unknown-member preservation, hostile input (depth bombs, huge counts, NaN literals, duplicate keys and ids, dangling refs, cycles, truncation, BOM), crash timing (torn temp refused, old-or-new).
3. **Model invariants** after every fuzz move: section 2 plus determinism (positions bitwise equal under shuffled input order), no derived field written (delete any field: refuse or identical positions).
4. **Fuzzers ported** to the new vocabulary (station ids, detach instead of pair unmerge): undo roundtrip (now bit-exact canonical JSON, only `modified_at` masked, allocators "may be higher"), metamorphic, adversarial, concurrency, multi-datum, crash, format. A `legacy_origin` mode starts runs from a migrated old-format project.
5. **TS harness and Playwright**: canonical snapshot, invariants, fractal generator, fixture handling, leftover-temp names, the IPC mock; journeys for opening the tracked old fixtures.
6. **Farm**: all campaigns on the branch build plus migration sweeps over generated legacy files and the private corpus (on the host only, reporting counts and mechanisms).

---

## 12. Work breakdown

| Stream | Scope | Depends on |
|---|---|---|
| L | `crates/legacy` (done): frozen reader + load pipeline + positions + test-only v18 writer; `crates/legacy-fidelity` proves bit-exactness against the live app | - |
| K | Characterisation goldens recorded from the CURRENT build before `models::project` is deleted: history labels and previews per command kind, delete/combine/merge previews, IPC DTO shapes, export outputs (CSV, JSON, TML, SVG) for the synthetic projects | - |
| M0 | `models::survey` skeleton: types, ids, slots, components, invariants/validate, positions (3.3-3.5), depth, anchors (4.1), naming, identity | - |
| M1 | shots/lines/ranges/reverse/split/combine on M0 | M0 |
| M2 | merge/detach/closures/sweep/datums/frame/placements/move tools/Add on M0 | M0 |
| M3 | derived views: LSQ port, closure metrics and alignment, skip set, roles/entrances/gaps, frozen shots, stats, pathfinding/distance/gas engine port | M0 |
| F | v2 encoder/decoder, canonical rules, tiers, versioning, validation, salvage, probe, serializer modes, `docs/file-format.md`, format property tests | M0 |
| G | migration legacy -> survey + MigrationReport + property tests + corpus runner | L, M0-M3 |
| U | changeset undo engine + history persistence + size gates | M0, F, then C for the AppState-level gates |
| AS | AppState without a live database: every `self.database` use replaced (warnings, display settings, link, undo policy, revisions...), local-only durable saves, external-modification guard | F |
| C | switch core (`AppState`) to `models::survey`: execute pipeline, every command family, queries, imports funnel, add/merge projects, dive plan resolver, warnings | M0-M3, U |
| P | save/load/autosave/recents/recovery/backups on v2 (incl. upgrade backup); SpeleoDB push/pull/cache/revisions/swap/unwind | F, G, C, AS |
| X | exporters; stalx-reader | M0-M3 |
| W | IPC handlers, wire DTOs, TS type generation, binary envelope | C |
| FE | frontend changes (9.3) and the IPC mock | W |
| T | fuzzers and harness ported alongside each stream; new property suites | each stream |
| D | docs: architecture, saving, closures, undo-redo, SPEC data model, SpeleoDB, testing; `docs/file-format.md`; `docs/survey-model.md` | all |

The old `models::project` module and its tests are deleted once C compiles against `models::survey`; tests that pin user-visible behaviour are ported to the new model first, and K's goldens are compared after the switch (differences must be in the declared-change list).

### 12.1 Rollout prerequisites outside this repository (called out in the PR)

- The SpeleoDB server accepts v2 uploads (today it refuses non-SQLite), its Python reader reads v2, and ideally a minimum-client gate for projects whose HEAD is v2.
- A small patch to the 0.5.x line that sniffs `{"format":"stalx"` and shows "made with a newer Stalagmight" instead of the corrupt-file route.

### 12.2 Declared behaviour changes (to list in the PR)

Junctions share one Station ID, name and comment (migration keeps the others as aliases); Split Line Here makes a junction; Unmerge detaches one line; closure metrics are recomputed (path length counts shots only); warning ignores ride the save instead of being written at once; CSV/JSON `from_id`/`to_id` at junctions; Ariane export station ids; anything else K's goldens show.

## Background (current-state findings this plan relies on)

- Merges already form a forest by construction (a merge between connected lines is refused; every loop is an explicit closure), so "shared station + forest" changes no topology any current code path creates. Legacy files can hold merge cycles; migration demotes the cycle-closing merge to a closure.
- `line_offsets` is derived for every line reached by the cascade, but the seeds (placement-datum lines and each datum-less component's lowest line) carry user decisions: where a free piece sits and where a datum piece sits relative to its GPS. These become `Placement` and `ZeroDatum.correction`.
- `station_offsets` (hand moves) are no longer created by any command; they survive only in old files.
- Closure metrics are stored at creation and never refreshed; `is_aligned` is re-measured only for `Move` closures.
- Merged stations carry independent names, Station IDs, comments and datums today; the map shows one label per merge group (the representative rule), which the migration reuses for the survivor.
- Schema "v18" spans several file shapes (side tables added without a version bump); the migration must not key on the version alone.
- No Rust test opens a file an old build wrote; `stalx-reader` already reproduces the app's open positions bit-exactly and is the basis of the legacy oracle.

## Deviations (stream M0, `models::survey`)

What the built model does differently from the text above, and why. The module guide is `docs/survey-model.md`.

- **D1. `ClosureStrategy` with `Tie` is its own enum for now.** `survey::ClosureStrategy` has the shared variants with the same serde spellings and legacy aliases, plus `Tie`; `From<crate::ClosureStrategy>` and `to_shared()` convert. Adding `Tie` to the shared enum would change the generated TypeScript union and break the frontend's exhaustive `Record<ClosureStrategy, boolean>` (`src/utils/closures.ts`) before W/FE switch. W unifies the wire enum.
- **D2. Traversal order (3.3 step 2).** A station's lines are visited in line-id order, not list order. In a valid model the values are the same (each station has one tree path from its anchor), but the float summation route no longer depends on display order: reordering the line list changes no bit (tested).
- **D3. `via` hints are checked by invariant 2.** A datum's `via` and a closure end's `a_via`/`b_via` must name a line through that station. A stale hint is not harmless: line ids are re-derived as `max + 1`, so a deleted line's id can come back and a stale hint would then misdirect a detach. `Project::drop_stale_attachments()` clears stale hints and drops hand moves keyed on a station the line no longer passes (already required by invariant 2); the epilogue of delete shot / range / line, split and combine runs it before `validate`.
- **D4. Rule (0) in 4.1: commands declare their own anchor decisions.** A placement the command set or moved itself (Move Segment / Line, an import's placement, Add .stalx) is listed in `EditContext::placed` and is authoritative; a datum correction the command computed (Add .stalx) is listed in `EditContext::corrected` and kept. Without this, 4.1 re-lays a free piece out from its pre-edit position (undoing a Move Segment) and clears Add's corrections. Any other placement-list entry is treated as pre-edit state only, so a placement re-keyed by a merge's station replacement cannot drag the stationary piece.
- **D5. Datum identity across an edit.** Datums have no ids. `settle_anchors` recognises a post-edit datum as a pre-edit one by its station (or a pair in `EditContext::renamed`: a merge's retired station and survivor, a detach's station and its new one) and exact GPS bits (the previous GPS for `EditContext::gps_edit`). Two datums with identical GPS on one station can swap identity; harmless unless one of them is removed in the same edit that changes the frame.
- **D6. A new component with no pre-edit station** (a brand-new free piece) needs a declared placement; without one it is laid out from its lowest station at the local origin.
- **D7. Stricter `validate` at birth** (no format cost now, costly later): ids within their spaces (Station and Section ids below 2^31, line ids at most `MAX_LINE_ID`), alias ids distinct from live ids and from each other, counters above alias ids too, shot uuids non-empty and at most 128 characters, validation thresholds finite.
- **D8. Depth summation order** (3.4 "sorted order") is `(line id, index)` over the slots of the tie group, which makes a junction's depth bit-identical to today's merge-group depth.
- **D9. The letter-suffix rule is reused, not copied**, from `models::project::station_naming` (pure functions). That file must move under `survey/` or the crate root when C deletes `models::project`.

## Deviations (stream M1, shots / lines / ranges / reverse / split / combine)

Declared by the plan and implemented as written: Split Line Here makes a junction (no duplicated station, no comment or datum side, the hand move copied); junctions share one id, name and comment (merges, promotions and combines add aliases); a delete leaves a junction on its other lines with its comment and datums whatever their `via` (today a datum on the removed member was deleted); no replacement merges or closure re-anchoring (a closure on a junction stays; what the line's own slot carried there follows S13). What follows from the station graph and the plan does not say:

- **S1. Combine detaches the moving line's slot at a torn junction** (plan 5.4 step 2): the moving line gets a new station (new id, copied name); the old id stays on the fixed side's lines. Closure ends and ids differ from today's merge-to-closure; positions and misclosures match.
- **S2. `TornTieReason::PassesTwice`.** A station both lines pass besides the join is reported with this new reason (step 6), where today a tie reaching a non-join station was reported as `Loop`. Wire mapping: `PassesTwice -> Loop`, `Moved -> JoinedStationMoves` (or `Loop`), an unaligned torn snap `-> FormerSnappedClosure`.
- **S3. A gauge on a station both lines pass** is torn by step 6 and stays on the line that does not move, so no side "holds" it and the plain rule applies (the other line moves); today the kept line moved (`AdjustmentAnchor`). The Computed View does not move either way.
- **S4. Hand move at the join: the fixed side's wins** (today's rule). The plan text says the kept line's; with it, branches at a fixed join station would move. When nothing moves, the kept line's wins.
- **S5. Third lines at a torn junction** stay with the fixed side (the moving line takes the new station); today it depended on which pairwise merges existed.
- **S6. A legacy merge loop at a hand-moved join** (migrated to a junction plus a `Tie`): the other line moves with the joined station and the `Tie` becomes `Batch`, where today it stayed and its merge at the join tore. Not representable otherwise (the line would hang by closures only); one ported test is `#[ignore]`d with this reason.
- **S7. `depth_difference_m`** reads each join station's depth from the solver, which averages over a junction's lines and its snaps; today it read one member's readings.
- **S8. Two datums on a merged join** (one station) are allowed (plan 5.7); on side-by-side join stations they are still refused, as today.
- **S9. Payload trims.** `SplitOptions` has no `comment_side` / `datum_side`; `CombineCandidate` drops `repeated_shot_ids` (uuids are project-unique) and the always-`None` `merged` / `station_name_a` / `station_name_b`.
- **S10 (integration). Combine's gauge rule reads the solve.** "Is the joined survey solved" and "which station is the gauge" are M3's `Project::solve_hold` (the least squares' own partition and held component). A component tied only by a `Tie` now counts as solved (D10), and with a datum in the component the gauge is the datum station, which no free side holds (today's `adjustment_anchor_of`). M1's copy (no `Tie`; the first `Placement` in list order, even beside a datum) is gone.
- **S11 (integration). Combine's join and torn junctions use M2's mutation cores** (`merge_ids` with the dialog's comment pick, `detach_ids`). Two visible differences from M1's own fold: the survivor lists the retired station's own aliases before the retired station, and keeps the retired station's `external` id when it has none (E7). Every ported combine test passes unchanged.
- **S12.** Frozen-shot refusals ("Cannot edit geometry of a frozen shot" and the delete and split variants) are not in the model operations: C checks `is_shot_frozen` before calling them.
- **S13. A delete at a junction follows 0.6.0's member rules; there is no "slide".** Characterised on `models::project` and the frozen 0.6.0 crate: deleting a line's first or last shot at a merged station removed the line's member there with its merges, and the line's new end was never merged to the junction; the line left it and stayed where it was, as on the station graph (a one-shot line stays shot-less on its far station). What differed was what the member carried (`edit/junction_leave.rs`, every delete position, range shift and split):
  - a `Tie` attached via the line (a 0.6.0 cycle or same-line merge the migration demoted) stayed on the junction and, left as the only bridge, was promoted, moving a piece by the 0.6.0 misclosure (wire equivalence seed 5: 18 m). 0.6.0 dropped that merge and re-stitched the remaining members only when co-located and not otherwise connected. The `Tie` now leaves with the slot unless its ends were co-located before the edit and the edit leaves them in different pieces; then the sweep promotes it, nothing moves, and its far end is retired so the junction keeps its id;
  - a closure attached via the line was re-anchored onto a remaining member in 0.6.0 and, when the delete broke its loop, promoted before the rest with the junction's side moving (unless held by a datum); the station graph's sweep took list order and moved end `a`'s piece (seeds 23, 215). The delete now names the junction's end (`sweep_orphan_closures_moving`), and those closures are judged first.

  Unavoidable differences: the junction keeps its datums and comment (declared above), so a datum on the line's member stays and can change which piece is held; a legacy aligned non-`Move` closure the migration made a `Tie` cannot be told from a demoted merge, so when not co-located it leaves like one (0.6.0 re-anchored it); 0.6.0 could promote a second closure on the member into a merge cycle (the first promotion had joined its ends), which stays a loop closure here; 0.6.0's preview listed only the member's promotions, the station graph's lists the sweep's others too. `delete_line` is unchanged (0.6.0's `preserve_connections_for_line_deletion` is not compared). The wire equivalence compares the promotions in 0.6.0's preview scope, and seeds 5, 23 and 215 run the delete on both models and compare every slot.

## Deviations (stream M2, topology / closures / datums / move tools / Add)

- **E1. The datum a slot has.** A station can hold several datums; Set and Remove on a slot act on the first datum on the station attached `via` the slot's line, else the first datum on the station (`datum_index_for`). Consequence (declared): Set on a junction slot whose line has no datum there edits the station's existing datum instead of adding a second one, where today each merge member could get its own (a fix beside a placement). A second datum on one station now only comes from a merge of two placed pieces or a migration.
- **E2. The sweep reports closures as it found them** (after the command's own renames, e.g. a merge puts both ends on the survivor), with their positions and ids; the command layer names them from its pre-command snapshot by id. Promotions report the survivor and the retired station.
- **E3. Model commands do not touch `modified_at`** and do not log; the command layer (C) does both, so every model operation is a pure function of its input (the property test replays seeds bit for bit).
- **E4. Accuracy-only datum edits** (same latitude and longitude bits) skip the anchor algorithm entirely (nothing can move); a changed depth reference rides along, as today's in-place store did.
- **E5. Add .stalx refuses on Station-ID exhaustion** (`base + source counter >= 2^31`) instead of renumbering every station as today's overflow path did; repeated line names are made unique (` (k)`) by the Add itself rather than by the normalisation that followed it (same result).
- **E6. Detach station form off a junction is a no-op** (as today's `UnmergeStations`); `detach_line` and the pair form refuse with "Stations are not directly merged". Any two slots of one station are a valid pair (today a pair had to be a direct merge; the UI fell back to the whole-station unmerge otherwise).
- **E7. A merge keeps the survivor's `external` id, else takes the retired station's** (plan 5.1 does not say).
- **E8. Closure metrics, the across-one-shot knot rule, roles, GPS gaps and entrance defaults were stubbed in `survey/m3_stubs.rs`** until M3 landed. Resolved at integration: the stubs are deleted and the commands call M3's views. A created closure reports `closure_metrics_via` on its picked slots (slot depth on the `via` line, the figures its wire entry shows); the closure preview and the Merge check use `closure_metrics` (each station's first slot).
- **E9 (integration). One copy of each derived set.** Rigid groups, anchored lines, co-located and immovable stations are M3's (`rigid.rs`, `TieGroups`; D15). M2's `rigid_group_*`, `rigidly_tied`, `colocated_stations` and its own `anchored_lines` / `immovable_stations` are gone; Move Segment and the Merge refusal read `rigid_groups` (`RigidGroups::tied`).

## Deviations (stream M3, derived views)

- **D10. `Tie` gives its component something to solve**, like a legacy aligned `Batch` closure today (it is "eligible"). A migrated cycle-closing merge with no other closure is therefore solved, so a torn tie is pulled together in the Closures Computed View, where today that component was shown measured. The migration verifies and reports this as `TieSolved` (G2).
- **D11. Every closure that ties in the solve, `Tie` included, goes through the across-one-shot check in list order.** Today merges were united before the check. A demoted same-line adjacent merge (or a cycle-closing merge spanning one shot) is skipped and reported (`CLOSURE_ACROSS_ONE_SHOT`) instead of failing its component; the "inconsistent line translation" failure cannot occur on the station graph. The migration verifies and reports this as `TieNoLongerFails` (G2).
- **D12. The Computed View keeps today's depth co-location of adjusted closure ends.** Today's solve stamps the closures it adjusted as aligned on its working copy, so the view's depth averages each such closure's two ends (over the tie group). Positions are horizontal only; depth follows that rule.
- **D13. Solve numbering.** A solve component is the shot components joined by any closure (today's `get_connected_line_ids`). Unknowns are constraint groups numbered by lowest StationId, shots are assembled in line-id order, the warm start is the group's lowest member. Results match today within 1e-6 m (tested), not bit for bit.
- **D14. `path_length` between two pieces.** A closure between two datum-held pieces has no shot path: `path_length` and `error_percentage` are 0 (today: the reachable line lengths).
- **D15. Immovable stations follow the documented rule** (datum stations and their tie groups over aligned closures). Today's frontend walk also follows chain-transitive `closures_with` entries, which carry the far closure's id, so an unaligned closure followed by an aligned one counted as rigid.
- **D16. Stats** carry `station_count` (distinct stations) and `slot_count`; `to_project_stats` keeps today's meaning (slots) for the wire.
- **D17. Graph builders** emit shots in line-id order and closures in id order (as D2), so reordering lines or closures changes no tie-break and no bit.
- **D18. Gas engine.** The route has no merge hops (route indices shift; legs are station to station); the turn frontier lists each station once (its first slot on the wire); a route node has one depth (only legacy hand-move z on a junction differs); a drop override on another slot of the default drop's junction is a no-op.
- **D19. The ruler's wire `path_stations`** are the two ends of each traversed shot on that shot's line (consecutive repeats removed).

## Deviations (stream F, `.stalx` v2 format)

The full format is `docs/file-format.md`.

- **F1. No `counters.line`.** Plan 6.2 shows one; plan 1.2 derives the next line id (`max + 1`).
- **F2. `conventions` adds `gps: "wgs84-deg"` and `temperature: "degC"`**, for third-party readers.
- **F3. `sections` keep list (panel) order**, not ascending id: the model's `Vec` is panel order, and SQLite stored a `position` column.
- **F4. `undo_mode` is in `speleodb` (shared), not `local`**: it rides pushes deliberately today (`speleo_link.rs`).
- **F5. The sniff tolerates JSON whitespace between the prefix tokens** (a pretty-printed file is accepted); the canonical form is the exact prefix, and the tokens may not be escaped.
- **F6. Unknown top-level members are written after `imports` and before `history`**; `history` and `end` stay last.
- **F7. Unknown enum values are tier 1:** the file opens read-only with a fail-closed fallback (kind `reference`, strategy `batch`).
- **F8. The read-only reason lives in `Extras.read_only`**, so every encode mode refuses while it is set; `Decoded::read_only()` is an accessor.
- **F9. History container contract.** Each list is a stack (its last element is the next step). The format reads only an entry's `"t"`, for the watermark cut (today's SQLite rules); stream U defines the rest.
- **F10. Dates are clamped into 0001-9999**, like times.
- **F11. Bounds the format needed and the model lacked:** at most 99,999 shots per line (session shot ids), alias ids below 2^31, station and Section counters at most 2^31. Resolved at integration: they are invariants (`validate`: `TooManyShots`, `AliasOutOfRange`, `CounterOutOfRange`; `MAX_SHOTS_PER_LINE` is the model's), the decoder and the encoder refuse through `validate` (`DecodeError::Invalid`, `EncodeError::Invalid`; `Limit::ShotsPerLine` and `EncodeError::OutOfRange` are gone), and salvage drops an out-of-range alias and lowers an out-of-range counter to its bound (`Repair::LoweredCounter`); a line over the cap stays refused.
- **F12. `rename_project` is implemented here** (plan 6.5 Recents rename).
- **F13. `probe` on a legacy file returns only `Format::Legacy`**; the legacy header (metadata) comes from the legacy reader.

## Deviations (stream G, migration)

- **G1. Self-merges and repeated merges are dropped**, not made `Tie`s (7.2.1 says `Tie`): a `Tie` needs two stations (invariant 7). Counted (`merges_dropped_self`, `merges_dropped_redundant`).
- **G2. The Closures Computed View is verified, with two declared expected-difference classes** (7.2.7). The migrated view is solved with the least-squares config the legacy open used (`migrate` takes it) and compared per slot with 0.6.0's (1e-6 m plus the slot's raw dust bound; depth 1e-9 m); the components each solve falls back on must match. Two differences are expected and reported per class with their slot count and maximum displacement, never hidden:
  - `TieSolved` (D10): 0.6.0 showed a component measured, without a failure, because nothing in it was eligible; a migrated `Tie` (a cycle-closing or same-line merge, or a legacy aligned closure of a non-eligible strategy such as `ProportionalByLength`) is eligible, so the view now solves it and pulls the torn tie together.
  - `TieNoLongerFails` (D11): 0.6.0 failed a component ("inconsistent line translation", shown measured) because it united merges before the across-one-shot check; the demoted merge (same-line, or a cycle-closing merge spanning one shot) is a `Tie` the new solve ties as one point or skips and reports, and the component is adjusted by its other closures and GPS fixes. The legacy failure is explained, not a fallback mismatch.

  Measured on the generator (3000 seeds, every one clean): `TieSolved` in 285 seeds (3136 slots, at most 47.99 m), `TieNoLongerFails` in 34 seeds (913 slots, at most 2292.8 m: the legacy view showed those components raw while the synthetic GPS fixes, placed far away, now pull them). The tracked fixtures have neither (`naharon.stalx` within 9.1e-10 m, `fractal_plant_iter3.stalx` exact). The corpus runner prints both views' classes with their maxima. Stream P decides whether the conversion notice (7.2.10) mentions Computed View changes; today it counts raw moves only (`Verification::moved`).
- **G3. Comment join order** ("member order", 7.2.1) is the survivor first, then the other members by label rank.
- **G4. `TornMergeCollapsed` is a real move.** A torn merge across two datum-less pieces cannot be kept as a `Tie` (invariant 7), so it is collapsed and its pieces move by up to the gap; counted per merge (`merges_collapsed_torn`, `..._max_m`) and in `moved()`. Recommendation for stream P: show the 7.2.10 notice whenever `verification.moved().slots > 0`, not only for unexplained mismatches.
- **G5. Cascade replay, phase 2,** reads closure `is_aligned` after the legacy load re-measured it (0.6.0 used the file's flags). It only orders merges in pieces merges alone cannot reach, which needs dangling references.
- **G6. `migrate_path` computes the legacy Computed View** (about twice the open time) for G2; `OpenOptions::with_adjusted = false` skips it, and the comparison then reports `NoLegacyPositions`.
- **G7. Publishing.** `stalagmight-persistence` depends on the unpublished `stalagmight-legacy` (path only); `cargo publish` of persistence (for stalx-reader) fails until legacy is published or the dependency is gated.
- **G8. Dates are clamped at the migration too** (F10, wave B C24): a legacy credit or line date outside 0001-9999 becomes the date the v2 file would hold, so the migrated session equals its first save. Counted (`MigrationReport::dates_clamped`); the wire comparison declares the class.
- **G9. Open migration findings (wave B C24, not fixed).** The hostile generator still makes the migration report unexplained Computed View slots in two shapes, and either can raise the conversion notice on a real file: F2, a shot under about 20 cm within 2 degrees of a grid axis gives a degenerate least-squares weight on that axis, and the 0.6.0 and the ported solves each go wrong their own way (candidate fix: a per-axis sigma floor); F3, shots or depth changes of 10 km and more make 0.6.0's Computed View fail its own 1e-6 m line-translation check (candidate: a declared class or a relative tolerance). Pinned by the ignored `migrate_fuzz_tests::short_shots_on_a_grid_axis_keep_the_computed_view` and `km_scale_shots_keep_the_computed_view`.

## Deviations (stream U, undo engine)

The engine is `models::survey::changeset` (guide: `docs/survey-model.md`, "Undo engine").

- **U1. A struct of parts, not an ordered `Vec<Op>`.** A changeset has fixed optional parts (project fields, stations, line order, lines, the six lists, counters, the credit table). A step checks every precondition and builds every new part before it changes anything, then commits all of it, so the order of ops carries no meaning; the persisted form is an object with members in a fixed order.
- **U2. Every rebuilt line carries its content hash.** Each side of a line whose shots change records the shot count and a 53-bit FNV-1a hash of the whole line (metadata, start, hand moves, every shot, session ids aside). The live line must hash to the side a step reads before it, and the line built must hash to the side it writes after it, so a step is exact even where a run records no values. The uuid anchor (8.1) is recorded only on runs out of another line; a run within its own line is covered by the hash. The hash layout is part of the history format.
- **U3. Reverse is not an op of its own.** Every copy run has a mode: as is, reversed, reversed and flipped (Reverse Line's per-shot step), or flipped with L/R swapped; capture picks the mode that leaves the fewest exceptions (every mode for a one-shot run). A reverse is then one run per side plus the flip's ulp exceptions, as 8.1 asks, and the same mechanism covers Combine's reversed other line and any other reversed stretch.
- **U4. Stations are columns.** Created and deleted stations are whole images; changed ones are one column per field (`name`, `comment`, `aliases`, `external`) as `[id runs, [before...], [after...]]`, so renaming 1,000 stations costs two names per station.
- **U5. Line metadata is field-level on lines on both sides**; a created or deleted line carries its whole image in the side that holds it. A merge's re-keying of a shot's `to` is an ordinary field edit, of a line's `start` a metadata edit.
- **U6. Every list is hunks**, datums included (8.1 had datums and placements as whole lists and closures per id with a position): `{n: [before, after], h: [[at, before, after], ...]}` from a Myers diff capped at 512 edit steps. A list is checked by its length and the stretches its hunks replace.
- **U7. `modified_at` and `file_path` are not recorded**, and a step never touches them; the command layer stamps the edit time (E3). Settings are recorded whole; `created_at` is recorded if it changes (no command changes it).
- **U8. Session shot ids ride in memory only.** An entry captured in the session gives every shot its id back; a decoded one mints ids for re-created shots. Never persisted (1.1).
- **U9. The entry has a version and a recorded preview.** `{"v":1,"t","k","label","refs","preview","ops"}`: `v` lets a later build drop old histories with the banner; `preview` carries rows worded at record time that no changeset holds (a batch's sub-commands, an offset); `refs` also lists closures and Sections.
- **U10. A step does not validate.** The preconditions make an in-session step exact; a persisted entry that passes them yet would leave an invalid model can only come from a crafted file. The core validates after a step when it wants to and rolls back with the inverse step (`redo` of what `undo` applied is exact).
- **U11. The size gate's denominator is the survey's v2 file with an empty history** (R2 H6 asked for the choice to be explicit). The 1% cap is kept as 8.1 says; every gated action passes with room (largest: Delete Line on the 5k survey at 0.80%).

## Deviations (stream X, exporters and stalx-reader)

The exporters on `models::survey` are `export::survey` (CSV, JSON, Ariane, image, Levels histogram, dive profile), beside the legacy ones and sharing their writers; the reader reads both formats.

- **X1. Ariane ids are Station IDs** (12.2 "Ariane export station ids", made concrete). A row's `ID` is the Station ID of the station it ends at and its `FromID` the one it starts from; a START's id is the station it stands on; CLOSURE rows and the GPS-fix STARTs are numbered from the highest Station ID + 1. Ids are therefore stable across exports (today every insert renumbers). Row order, blocks, names, depths, the repair and the GPS rules are today's: on the 18 characterisation scenarios every `.tml`/`.tmlu` equals the legacy one after renumbering ids, except one comment (X4).
- **X2. The walk is today's, read off the station graph.** Root: the frame datum's line; with no datum, the first line in list order that shares no station with a line of lower id (today: that is not the higher-id end of a merge pair). Lines are visited breadth first through their junctions, every pair of lines at a junction adjacent (today: direct merge pairs only). Each line is walked once, outwards from the station it is reached at. There is no alias pass and no unresolved parent link.
- **X3. Every closure is a CLOSURE row**, `Tie` included: a migrated cycle-closing merge is exported as a loop (today such a merge was not a tree link and was dropped silently).
- **X4. A station comment is folded once** (into the row ending at the station). Merged twins that carried the same comment (Split Line Here duplicated it) were folded twice; the migration joins distinct comments, so a junction has one.
- **X5. Station names and external ids are not written to Ariane.** `<Name>` stays empty on shot rows: the importer ignores it and Ariane's use of it is unverified, so writing it would be a further behaviour change.
- **X6. JSON export time is an argument** (`write_json(project, exported_at, out)`), so one project and time give one byte string; `export_json` passes the wall clock as before.
- **X7. Image: one comment and one star per station.** The comment sits on the station's first drawn slot, the star on the datum's slot (its `via` line's, else the first drawn one); the Levels depth histogram counts each station once (today: once per slot, so twins twice), over the extent the render normalises across (every slot's depth and every shot reading). Colocated slots are every junction's drawn slots, then the ends of the closures aligned in this state (`closure_alignment` on the raw layout). The renderer itself is shared: it draws a model-neutral sketch (`image_sketch.rs`).
- **X8. Dive profile:** the survey renderer takes a `GasReach` and renders its wire field; the route has no merge hops (D18), so no zero-length point at a junction.
- **X9. stalx-reader.** Sniffs the copy; SQLite goes through the reader's own pre-load checks, the frozen 0.6.0 open and `migrate`, JSON through `stalx_v2::decode` with the reader's caps mapped onto the decoder's. `Survey` keeps positional station ids and gains `format`, `format_version`, `newer_content`, `Section::station_ids`, `StationPosition::station_id` and `LoopClosure::tie`; `station_merges` are the derived junction pairs, a junction's name is listed at every slot and its comment once (first slot in reader order). New `ReadError::NewerFormat` and six JSON-only `CountLimits`. Anything that is neither format is `InvalidKind::NotSqlite` (an empty input too, which used to be `NoProjectData`). Publishing now needs `stalagmight-legacy` published or gated (G7).

## Deviations (stream W, wire DTOs)

The wire API is `docs/survey-model.md`, "Wire API".

- **W1. Datum and closure ends are shown on their `via` slot** (`datum_slot`, `closure_end_slot`), else the station's first slot. Plan 9.2 says "first slot of the station"; with the `via` hint a migrated reference comes back on the slot it had.
- **W2. A datum flags every slot of its station** (`is_zero_datum`, `gps_coordinates`), with the datum that slot's dialog edits (E1). Today only the merged member it was set on showed it.
- **W3. `Tie` is an aligned `Batch` on the wire** (D1 resolved without a TS change): the frontend's exhaustive `Record<ClosureStrategy, boolean>` and its eligibility rule stay as they are, and a legacy aligned closure looked exactly like that.
- **W4. Derived merge pairs carry the project's `created_at` as `merged_at`** (no edit changes it; nothing in the app reads it).
- **W5. `Line.reverse_parity`, `gauge_shot_uuid`, `gauge_neighbour_uuid` stay on the wire as `false` / `null`** so the TS `Line` does not change; no frontend code reads them.
- **W6. Previews report what cannot happen as empty**: no replacement merges or rewired closures, no rewire-skipped flags, no split half that moves. A delete's `removed_merges` is the removed slot leaving its junction; `closure_to_merge` is the sweep's promotions; `removed_closures` includes the #737 and swept removals.
- **W7. `get_line_relationships` counts what deleting the line removes**: merge pairs on the line, closures and datums on stations only that line passes, its hand moves, and `has_line_offset` = the line alone holds its piece's non-origin `Placement` station.
- **W8. The merge check's closure preview reads each end's depth on the picked slot's line** (`closure_metrics_via`), as today's slot-based preview did; M2's `merge_check` uses first slots.
- **W9. The binary load envelope drops `line_offsets`: version 8 -> 9.** Every other field keeps its name and element type.
- **W10. `file_path` is sent lossily as UTF-8**; today a non-UTF-8 path failed the serialization.

## Deviations (stream C0, the core switch foundation)

`AppState` holds `models::survey::Project`; every unported executor, query and subsystem answers with a typed "not yet ported" failure (`core::stub::not_ported`); the remaining work is the C0 work breakdown.

- **C1. How an undo entry is held.** The stack keeps the decoded `HistoryEntry` (its changeset carries the session shot ids of an in-session capture, so in-session steps give every shot its id back) AND its canonical text (`encode_entry`, computed once at push or read from the file). The byte budget counts the text, which is exactly what a save writes and a push carries, and a save copies it instead of re-encoding the history. A decoded entry (no session ids) is validated after its step and rolled back with the inverse step on a violation (U10).
- **C2. No live database: `FileState`.** Everything the file holds besides the survey and the history (link identity, `local` sync state, unwind chain, watermark, display settings, ignored warnings, preserved members, `requires`, the read-only reason) lives in memory beside the guard fingerprint, the origin (new / v2 / legacy) and a dirty flag. The local-only durable save (6.5 mode 4) rewrites `speleodb`, `local` and `history` from it; while the survey has unsaved changes it writes the history the file last held, never the in-memory stacks (they would describe a body the file does not have). On a legacy or never-saved project it only marks `local_pending`; the first real save writes it.
- **C3. The upgrade backup** is a COPY of the SQLite original at `<app data>/backups/<stem>.<UTC stamp>.pre-v2.stalx`, with any `-journal` / `-wal` / `-shm` sidecar MOVED beside it before the rename (R2 L19) and moved back if the write fails (superseded by H1: a hot journal moved away from the unsettled original left it as another survey). `list_backups` does not list that folder yet (recovery package). Resolved in wave A: `list_project_backups` lists the originals beside the project's own backups (N12).
- **C4. Conversion notice** whenever `verification.moved().slots > 0` or anything is unexplained (G4); `AppState::take_conversion_notice()` returns it once. The IPC and the banner are the frontend's.
- **C5. Undo and redo stamp `modified_at`, bump the revision and mark the file dirty**; on a linked project they set the SpeleoDB dirty bit (not a forward edit: the Unwind position stays, as in 0.6.0).
- **C6. Save a Copy** of a managed project is `Mode::DetachedCopy` when the explicit destination is not the open file (`unlinked_copy_path`); an explicit destination that IS the open file is a normal save (0.6.0 refused it).
- **C7. The guard.** A save to the open file expects the fingerprint last read or written; the first write of a project created in this session expects nothing there (`Guard::Absent`); its first implicit save (a deferred project's reserved path) moves to the next free name when a file appeared there, and writes over an empty placeholder, as 0.6.0 did (wave B); an explicit Save As destination is overwritten (the dialog confirmed). A refusal is `EXTERNALLY_MODIFIED: ...`; `AppState::save_project_overwriting` is the Overwrite answer. A read-only file refuses edits, undo, redo and saves with `READ_ONLY_FILE: ...`.
- **C8. One `apply` per command family** (`command_line.rs`, `command_shot.rs`, `command_station.rs`, ...), dispatched by `commands::apply_command`, so the families are ported in parallel. The mutation receives the `Record` (kind, label, refs, preview rows) to add what only the result can say. `run_recorded` is the pipeline every bypass recipe uses.
- **C9. `cfg(sg_unported)`** (never set; declared in the workspace `check-cfg`) parks the test modules and integration suites that do not compile against the new model; a port flips its modules back to `cfg(test)`. Surviving tests that exercise an unported path are `#[ignore]`d with a reason.
- **C10. Derived views are memoised per revision** (`AppState::revision`, bumped by every change of the project slot): the raw layout and, per closure-adjustment config, the Computed View. The adjustment report and the ruler read the adjusted view, the distance field the raw one (as 0.6.0).
- **C11. Ignored warnings** are kept in memory, mark the project dirty and ride the save (declared change, 6.5); `shot_uuid_is_persisted` is "the shot exists" (every uuid is written with the ignore). The datum GPS-gap code pinning and the warnings computation itself are not ported yet (no warnings in C0). Resolved in wave A (N9).
- **C12. History previews** are generic in C0 (the digest's fields, lines, datums, closures, Sections, and recorded rows); the 0.6.0 wording per command kind is the history-labels package, with the K goldens as the oracle. Labels are worded at record time with 0.6.0's `describe` rules. Resolved in wave A (N1).
- **C13. `generator`** is `Stalagmight <app version>`, set by the app at startup (`core::state::set_generator`).
- **C14. The legacy fidelity suite** (`crates/legacy-fidelity`) compares the frozen reader with the live 0.6.0 app, which the switch replaced; it is parked (it ran green before the cutover). Resolved in C22: the crate is deleted (Q6).

## Deviations (stream C, the command families and subsystems)

What the packages that ported the app core onto the model (wave A, C1-C14 and
C21) decided where the plan was silent or where 0.6.0 could not be reproduced.
Each package report has the details and the tests. Labels `N1`... (the C0
decisions above already use `C1`...).

- **N1 (history labels). History text is worded at record time, with the names before the edit; batch rows too** (0.6.0 resolved single-command labels and every batch row when the history was listed). So a metadata edit that renames a line is labelled by its old name, batch rows name lines and stations as they were before the batch, and an entry never changes after later edits. Update Shot previews list only the cells that changed; the station comment preview shows the previous comment; `previewData` is never null (an empty changeset records nothing). Labels, refs and the rows the changeset cannot say come from `state/labels.rs`; the rest is read off the digest when listed (`undo/history_item.rs`).
- **N2 (shots, lines). No-op edits keep the redo branch** (an unchanged credit, metadata, Section move or colour, a comment set to its value, a zero-offset Move Line or Segment, MoveComment onto its own station): C0's empty-changeset rule; 0.6.0 cleared redo for some of them. Create Line names and uniquifies only the new line (0.6.0 normalised every line name after every identity change). Section-id exhaustion refuses instead of renumbering (as E5). A range gap-close reports the closures its sweep removed (#738); 0.6.0's range path dropped them silently. A merge loop can no longer block a range gap-close (shots form a forest).
- **N3 (split, combine). A datum "on both halves" of a split is no longer refused** (it is on the one shared station); the split preview always reports nothing moved, merged or removed. Combine warnings name a torn junction by its two slots and its one name.
- **N4 (topology). A slot that names no station answers 0.6.0's words** (Merge / Create Loop Closure: "One or both stations do not exist"; Set Zero Datum: "Station does not exist"; Remove Zero Datum: "Station is not a zero datum"; Unmerge Station Pair: "Stations are not directly merged"; Unmerge Stations: a no-op), not the model's. Unmerge off a junction keeps the redo branch. In a Batch, swept closures are named before each sub-command. Remove Zero Datum runs no heal. `get_segment_geometry` lists lines ascending by id (0.6.0's order was a `HashSet`'s).
- **N5 (names, comments, moves). MoveComment onto the same station is a no-op** (0.6.0's set-then-clear deleted the comment). A repeated MoveStation in a batch moves the station once only when the offsets are equal. A name clash on a junction names the slot's own line when the name is taken there, else the station's first other line holding it. The no-op rename check runs inside batches too.
- **N6 (imports). External ids are not stored yet** (`Station.external` stays empty: `ImportLineConfig` has no per-station external key). Resolved in C22 (Q1). A reversed `Origin` / `Manual` line places the file's first station exactly. `track_imported_file` / `track_line_hashes` record an undo entry and are gated (0.6.0: unrecorded, ungated): the hash lists are changeset-captured content, and an unrecorded change would make the next undo of an import refuse. A stand-alone `UntrackImport` is undoable.
- **N7 (Add .stalx, Merge Projects).** The receipt warning that a source's ignored warnings could not be read cannot occur. A source this build would open read-only is refused with its own message, and unreadable sources get the open path's message. The size cap is checked from metadata, sidecars included; a version 2 source over the decoder's 256 MiB cap is refused by it. Only the read window of the mid-command swap remains. A refused add leaves everything as it was, allocators and `modified_at` included. A legacy source's pending Jump/Reference classification is finished by the frozen reader. Merge still carries neither the sources' ignored warnings nor their import hashes.
- **N8 (dive plans). `.plnx` anchors gain an optional `station` (Station ID)**, trusted when the anchor's own shots vouch for it; 0.6.0 anchors read the migrated session's record parity, else literally; `upgrade_dive_plan_anchors` rewrites them on the upgrading save (after every successful non-detached save; a no-op without parity); `load_dive_plan` hands out fresh anchors; `AfterShot` keys are found project-wide.
- **N9 (warnings).** The load result reads the opened file's ignores, never the previous file's. An ignore change is refused on a file opened read-only (`READ_ONLY_FILE`). Two GPS fixes shown on one slot share their row's (shot, code), the first naming the ignore. There are no position-keyed rows, no re-pinning after reverse, split or combine, and no renamed-uuid aliases: ignores are keyed by uuid, which travels with its shot.
- **N10 (SpeleoDB sync). "Uploaded without undo history" is reported only when every entry was cut** (the push keeps the newest entries that fit, R2 H8; a partial cut is logged). No watermark stamp after a push (0.6.0 cleared the chain there). A stale download is refused as `SessionMoved` before the local-edits guard. A download with an unknown `requires` installs and opens read-only. Legacy revisions are migrated, so the cache only ever holds version 2 written by this build (no conversion notice for downloads). The push serializes the in-memory state under one guard right after the save. Every SpeleoDB state change is a local-only save at once, clear-on-lock included (0.6.0 left it to the next save). The pushed file has no `local`, so every install restamps BASE. Times read back are spelled `+00:00`.
- **N11 (SpeleoDB local state).** A legacy foreign copy is upgraded to version 2 when its link is stripped, with the original backed up (0.6.0 deleted the SQLite row in place). The access probe answers `Ok(None)` for any unreadable file. An empty file in a link reader's path reads as unlinked; any other non-project file is unreadable. Logout keeps the open project's Recents row while its in-memory link is dirty. Publish refuses an invalid survey or a read-only file before the create POST.
- **N12 (recents, recovery). Attempt Repair writes in place behind a numbered `.backup`** (6.6 says "repair into a copy"): the damaged original IS kept as a copy, and the repaired file keeps the project's path, so the dialog's Open, Recents and a SpeleoDB link keep pointing at it. A read-only salvage open is not implemented (the load refuses an invalid file). The raw-data view is kept, over the saved v2 file's members. An open project's rename by path is durable at once and not undoable (0.6.0 parity), and writes only the name. Recents counts a version 2 file's stations as entities (a junction once), a legacy file's as slots. The survey preview positions are the workspace's (local metres from the frame datum's GPS, correction included).
- **N13 (frontend).** The save prompt (`SaveConflictDialog`) answers `EXTERNALLY_MODIFIED` with Keep my version, Save as a new file, or Use the file's version (a SpeleoDB copy gets only the first); an autosave refusal goes to the autosave banner and pauses autosave. A read-only file gets a banner and a third `useEditingBlocked` term. The conversion notice is a banner until dismissed. After an Ignore the frontend asks `has_unsaved_changes`. Unmerge is "Detach Line from Station"; Station details list other lines' shots at a junction; one `move_station` per Station ID; the Split dialog has no side choices.
- **N14 (app commands). With no project open, undo, redo and the jumps answer "No project open"** (0.6.0: "Nothing to undo" / "Invalid undo index"; the buttons are disabled then). UpdateShot / SetShotKind on a line that is gone say "That line no longer exists." (0.6.0: "That shot no longer exists.").

## Deviations (stream C22, deleting `models::project`)

`models::project`, the old-model code of the models root files, persistence's
live SQLite layer, `core::stub` and `cfg(sg_unported)` are gone; old files are
read only by the frozen crate and the migration. Labels `Q1`...

- **Q1. `Station.external` is filled on import** (10.5, resolving N6).
  `ImportLineConfig::station_external` (station index, id; file-order indices,
  mirrored on a reversed import like `station_names`) is filled by the backend
  importers: a Compass or Walls chain's station names (Walls' anonymous wall
  endpoints none), every Ariane row's station ID (none for the cave start,
  `FromID` -1). A station that already holds one, a junction the line joins,
  keeps it (E7's rule). Not in the TS shape (`#[ts(skip)]`): the frontend's
  importers (DMP, GPX) fill none, and Compass, Walls and Ariane imports run in
  the backend. Station names themselves are unchanged (Compass, Walls and
  Ariane names are still seeded by index, as in 0.6.0).
- **Q2. A hostile legacy schema loads as `UNSAFE_FILE` again.** The frozen
  crate's open reports it as `LegacyError::RefusedSchema` (a typed variant in
  its own error type, not in the frozen code), which the load maps to
  `LoadError::unsafe_file`; wave A reported `CORRUPT`.
- **Q3. A legacy file's unreadable shot kinds reach the frontend again** (#27):
  the migration carries the frozen open's report with the migrated shot ids and
  the full load stores them under its loading id; wave A dropped them.
- **Q4. Export Salvageable Data on a legacy file** reads each table as 0.6.0
  did, but its full project comes from the frozen open, which runs 0.6.0's
  integrity and foreign-key checks: on a file it refuses the error is listed
  instead of a best-effort project (0.6.0 read the tables without the checks).
  Attempt Repair is 0.6.0's (`persistence::recovery`).
- **Q5. A closure from a junction to a station a closure already ties it to is
  that closure.** 0.6.0 created a redundant second closure from the merged
  twin; on the station graph the twin is the junction, so the merge check and
  the command refuse the pair as a duplicate ("A loop closure already exists
  between these stations.").
- **Q6. Cross-checks against the frozen model.** The M3 cross-checks build
  their surveys with the frozen 0.6.0 model and compare what it computes (raw
  layout, Computed View, skip set, misclosures, rigid groups, immovable
  stations, roles, entrances, GPS gaps, far fixes). The comparisons the frozen
  crate cannot answer are retired: frozen shots, stats, penetration, distance
  field, ruler and the gas differential (the gas golden tests stay, their
  fixtures built with the frozen model). `crates/legacy-fidelity` is deleted:
  its oracle, the live 0.6.0 app, no longer exists.

## Deviations (wave C, D1: the legacy upgrade and Recovery)

Wave B left one data-safety bug open (C17 bug 3): the upgrade moved an old
build's hot journal away from the original before the version 2 rename, so a
kill in that window left the original alone at the project's path, which opened
as another survey, and Recovery refused the backup pair. Labels `H1`...

- **H1. The upgrade settles the original before anything else** (replaces C3
  and the "moved into the backups folder" of 7.2.10). Every write over a legacy
  file (the first save, a Recents rename, a SpeleoDB link strip) goes through
  `core::state::legacy_upgrade::upgrade`: check the guard; if a hot journal or
  a WAL with frames is beside the original, fold them into a private copy
  (`persistence::legacy_settle`, the frozen reader's own rule), stage it beside
  the original, sync, check the guard again and rename it over the original;
  remove every sidecar; keep the original in `backups/` through a hidden
  synced file renamed to `<stem>.<UTC stamp>.pre-v2.stalx`; write version 2
  guarded by what the original holds now. A kill between any two steps leaves
  the survey the user had or the complete version 2 file at the project's path,
  and never a JSON file beside an armed journal or a WAL. The kept original is
  one self-contained SQLite file (its bytes may differ from the file on disk:
  a journal rolled back, a WAL checkpointed; its survey does not). The original
  at the project's path is rewritten by the settle before the write: when the
  write then fails, the session's guard follows the settled bytes.
- **H2. A failed upgrade keeps no backup while the original is still whole**,
  so a save retried on a read-only folder does not fill the backups folder;
  the next upgrade of a file removes what a killed one left (the hidden
  settled copy, a partial backup).
- **H3. Recovery restores a SQLite backup with its unfinished sidecars** (a
  hot journal rolled back, a WAL read through, folded into the staged copy; the
  backup and its sidecars are only read). #720 refused such a backup. A project
  or a `.pre-restore` copy with unfinished sidecars is still refused: replacing
  it would lose them.
- **H4. Permissions.** Settling replaces the original by a rename, so it needs
  only a writable folder, as the version 2 write does: a write-protected legacy
  file in a writable folder upgrades and keeps its permission bits. On
  write-protected media the upgrade is refused with `PERMISSION_DENIED` and
  changes nothing.
