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
    pub counters: Counters,               // next_line_id, next_station_id, next_closure_id, next_section_id

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
    pub gps: GpsCoordinates,
    pub accuracy_m: Option<f64>,
    pub entrance: Option<bool>,
    /// Legacy only: the horizontal displacement (local metres) between where an
    /// older build placed this datum's piece and where its GPS says. Zero/None
    /// for everything this build creates. Kept so an upgraded file opens with
    /// every station where it was.
    pub correction: Option<[f64; 2]>,
}

/// The anchor of a piece that holds no placement datum: `station` sits at
/// local (x, y). Exactly one per such piece.
pub struct Placement { pub station: StationId, pub x: f64, pub y: f64 }

pub struct LoopClosure {
    pub id: ClosureId,
    pub a: StationId,
    pub b: StationId,
    pub strategy: ClosureStrategy,        // Batch, Move (snap), legacy variants kept for reading
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
7. Closures: `a != b`; a closure never joins two stations of the same component *across exactly one shot* (refused at creation, as #737). A closure's ends may be in different components only if both components hold a datum (today's rule).
8. Station names: unique within each line, across all stations on that line (5.3).
9. Counters are above every id in use (monotonic, never reused).
10. Numbers finite.

`validate` returns typed violations; load refuses a file with any (section 6.6), and commands roll back if they would introduce one (as the integrity guard does today).

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

As today: absolute, never walked. `Z(S)` = mean of the depth readings at S over every line through S (`depth_out` of the incoming shot, `depth_in` of the outgoing shot), plus the readings of stations tied to S by an **aligned snap** (3.6), summed in sorted order (#469). Slot Z = `Z(S) + hand(L, S).z`.

### 3.5 Wire positions per slot

The wire carries one entry per slot (as today). Slot `(L, i)` with station S: XY = `P(S)` (all slots of a station agree), Z = slot Z. Merged-with / closures-with lists are derived from shared ids and closures.

### 3.6 Closures: alignment and metrics

- `is_aligned(c)` = `c.strategy == Move` AND horizontal gap between the ends < 1 cm AND the tie would not double-anchor. Recomputed every time (today it is stored and re-measured).
- An aligned snap is a rigid tie for: depth co-location (3.4), Move tools' co-located sets, rigid groups, frozen shots, immovable stations, LSQ ties. It does not move raw XY (a snap always closes a loop inside a component, or joins two placed pieces, so it never places anything).
- Metrics (`path_length`, `misclosure_distance`, `error_percentage`, `horizontal_misclosure`, `vertical_misclosure`) are recomputed from raw positions. `path_length` is the shortest path along shots between the ends (junctions are free, as one station; **closures are not traversable**). This replaces today's creation-time value that counted earlier closures as zero-length edges; values on old files can change and that is accepted.

### 3.7 Closures Computed View (least squares)

The current algorithm (`lsq.rs`) is kept, operating on stations instead of slots: one unknown per station (a junction is one unknown), shots as observations with today's weights, eligible closures as ties, snaps tied, hard fixes on GPS, soft fixes weighted, across-one-shot closures skipped and reported, spurs untouched, per-component fallback with a report.
- **Gauge** of a datum-less component = its `Placement` station, held at its raw position. This replaces `held_gauges` and gauge pairs, and is stable across every edit that does not delete that station.
- Unknowns numbered by sorted StationId.

### 3.8 Other derived values

Datum roles (first datum per component = placement, others GPS fixes), entrance defaults (placement datum; else override; else first or last station of a line it is on), GPS gaps and their warnings, frozen shots, rigid groups, immovable stations, anchored lines, stats, survey graph for Plan/gas (junctions are one node; closures zero-cost edges as today), merge pairs for the wire (3.5 / 9.2).

---

## 4. What stays still: placement rules on edit

The traversal makes "the anchored side stays" automatic: every station's position is derived from its component's anchor. Edits change anchors only where the rules below say so.

1. **Datum piece:** the placement datum's station stays at its GPS. Nothing else needed.
2. **Free piece, geometry edit to line L** (UpdateShot, insert, delete, prepend, split shot at distance): before the edit, record `pin = (L's first station, its position)`; after the edit, if that station still exists set the component's `Placement` to the pin; otherwise pin L's new first station at its pre-edit position. This reproduces today's "the edited line's station 0 stays" rule.
3. **Merge two pieces** (5.1): the piece of `station_a` moves onto `station_b` unless A's piece is placed and B's is not (then B's moves), or both are placed (the piece whose placement datum is later in list order moves; its datum becomes a GPS fix by derivation). The moving piece's `Placement` (if any) is dropped.
4. **A piece is cut off its anchor** (unmerge/detach, delete shot/range/line, split line): the piece that lost its anchor keeps every station where it was: it gets a `Placement` at its lowest-StationId station's pre-edit position. If it holds a datum, that datum becomes its placement datum and the piece moves onto its GPS (today's `settle_datum_layout` rule).
5. **Frame change** (the first datum removed or a new first datum set): datum pieces are re-derived from GPS against the new frame; every `Placement` is translated by the frame correction so free pieces keep their place relative to the frame station's old position (today's `renormalize_frame`). Legacy `correction`s are kept.
6. **Move Segment / Move Line:** translate the free piece's `Placement`; refused if the rigid group holds a datum (as today).
7. **Move Station:** rewrite every shot incident to the station, on every line through it (a junction has three or more), so the station lands on the target and its neighbours stay (today's shot rewrite, generalised); if the station is its free piece's `Placement`, move the placement too; refused on datum stations (as today); deletes any hand move of the station.
8. **Reverse line:** reverse the chain and flip each shot (today's `reversed_azimuths`, depth swap, pitch swap and negation, optional L/R swap). Positions do not change.

---

## 5. Topology operations

Each operation is expressed on the new model; previews (merge dialog, delete impact, combine plan) report the same user-facing facts as today, computed from the model before and after (a dry run on a clone is acceptable and preferred over parallel bookkeeping).

### 5.1 Merge stations (`merge_stations(a, b)`)

Refused when: same station; the two stations are already connected (shots, junctions or aligned snaps), so the user is routed to Create Loop Closure, as today; one station per line rule (a line may not pass through the merged station twice).

Effect: station `b'` (the moving side's station, see rule 4.3) is replaced by the surviving station `s` everywhere (`line.start`, `shot.to`, datums, closures, placements, hand-move keys). The survivor keeps its id, name and comment; the retired station's id and name go to `s.aliases`; its comment is appended to the survivor's if both are non-empty and different (separated by a blank line), else whichever exists. Datums of both are kept (a station may carry more than one datum; the first in list order is the one the datum dialog edits). Closures between the two become `a == b` and are removed (#738). Names are re-uniquified on every line through `s` (letter suffix rule).

**Survivor:** the station on the side that stays put (4.3). In the Merge dialog the user sees the same two stations as today.

### 5.2 Unmerge (detach)

`unmerge_station_pair(slotA, slotB)` detaches **slotB's line** from the shared station. `unmerge_stations(slot)` detaches that slot's line from every other line at that station. Detaching line L from station S: L's slot gets a new station S' (new id; name copied from S, then uniquified; no comment, datums, aliases or closures), placed exactly where S was; components split per 4.4; then the orphan-closure sweep (5.5) runs.

### 5.3 Station naming

The creation rules (End, Mid, Prepend, Combine prefix) are unchanged and apply to the line the station is created on. **Uniqueness:** a station's name must be unique among the stations on every line it is on. Renaming a junction checks all of its lines. Migration and merges uniquify with the existing letter-suffix rule.

### 5.4 Shots and lines

- **Create shot** (append, insert before/after, prepend, split shot at distance): a new station is minted for the new chain position (insert: the new shot ends at a new station and the following shot now starts there; prepend: new start station). Ids: `next_station_id`.
- **Delete shot** removes one station from the line exactly as today (station 0 for the first shot, else the shot's end station; the following shot re-attaches to the shot's start). If the removed station is a junction it **stays** for the other lines (their connection is untouched); if it is on no other line it is deleted with its comment, datums and hand moves, and its closures are dropped. Then the sweep (5.5) runs. This makes #776 impossible by construction: closures on a junction stay on the junction; there is no replacement or promotion planning per closure.
- **Delete shot range** (gap-close / split / prefix / suffix) and **delete line**: the same rule per removed station.
- **Split line here** (at station k): the second half becomes a new line whose start is **the same station k** (a junction). This replaces today's duplicate co-located unmerged station: the two halves stay connected. Datums and comments stay on the one station.
- **Combine lines:** append/prepend the other line's chain onto the kept line at their common station. If the join stations are distinct (side by side, or across a closure), they are first merged per 5.1 (the other line's piece moves) and any closure between them disappears. Torn ties: an edge that would close a loop becomes a `Batch` closure (today's tie-tearing rule, now much smaller).
- **Reverse:** 4.8.

### 5.5 Closures

Create (refusals as today: same station, already a closure for the pair, across one shot, unconnected unless both pieces are placed), remove by id. **Orphan-closure sweep** after topology edits (not on load): a closure whose ends are the same station is removed; a closure that is the sole connection between its two ends' components (no path over shots) is **promoted**: the two stations are merged (5.1, with the movable piece moving onto the other, refused if it would double-anchor), iterating in list order so a later closure sees an earlier promotion. Removed and promoted closures are reported as warnings, as today.

### 5.6 Hand moves (legacy)

Only migrated files carry them (no current command creates one). Stored per (line, station) because merged members could carry different values. Move Station on a station deletes its hand moves on every line (folding them into the rewritten shots, as today).

### 5.7 Datums

Set (refusals as today; setting a datum on a piece that already has a placement datum stores a GPS fix by derivation), remove, accuracy and entrance edits, frame rules (4.5), previews (`preview_zero_datum`, merge datum shift). A station may hold several datums (from migrated merges, or merging two placed pieces at stations that each had one).

### 5.8 Add `.stalx` / Merge projects

Copy stations (ids offset by the target's `next_station_id`), lines, datums, placements, closures (ids re-minted), Sections, as today; no cross-source junctions are created (unchanged behaviour). Source files may be SQLite (migrated first, 7) or v2 JSON.

---

## 6. File format: `.stalx` v2

### 6.1 Container and sniffing

- Extension stays `.stalx`. Content is UTF-8 JSON, no BOM.
- Sniff: a file starting with `SQLite format 3\0` is legacy (7); a file starting with `{"format":"stalx","version":` is v2; anything else is refused as not a Stalagmight project. Every opener sniffs (load, Add .stalx, Recents metadata, foreign-copy probe, survey_geometry, recovery/backups, SpeleoDB pull/install/revisions/unwind, stalx-reader).

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
- `requires` lists feature names this file uses that a reader must understand. This build understands its own list. An unknown name makes the project open **read-only** with a banner ("made with a newer Stalagmight"); saving is refused, Save As writes nothing either.
- Unknown top-level members are preserved verbatim on re-save, in their original order after the known members; unknown keys inside records are refused (written by a newer minor version that should have declared a required feature) - list in docs which ones are optional.
- `conventions` is fixed for this version and checked on read (any other value is refused).
- `summary` is a non-authoritative cache for Recents (name, counts, length, bounds, SpeleoDB id); a reader may use it without parsing the body; the writer always recomputes it; tests pin that it equals the recomputed value.

### 6.5 Save path

Serialize in memory; write `<file>.stalx.tmp`; `sync_all`; rename over the destination; fsync the parent directory. One write, one data fsync, one directory fsync. No in-place writes to the project file anywhere else: the per-edit SpeleoDB dirty bit, display toggles, warning ignores and Recents renames become in-memory state written by the next save; display toggles and ignores on an otherwise clean project trigger an immediate quiet save (same pipeline). Windows: no open handle on the destination.

### 6.6 Load and validation

Parse with size and count limits (port stalx-reader's `CountLimits`), check `format`/`version`/`conventions`/`requires`, build the model, run `validate` (section 2). Any violation of invariants 1-5 or 10 refuses the open with a precise message; invariant 6 (missing anchor) is a refusal too (the writer always writes one). Load never mutates the model (no heals); the in-memory project equals the file.

---

## 7. Migration from SQLite (legacy)

### 7.1 Frozen legacy reader

`src-tauri/crates/legacy` (crate `stalagmight-legacy`): a frozen copy of today's model, SQLite reader and load heal pipeline, taken at the cutover commit, with its own module paths. It is never changed except to fix a migration bug. It exposes:

- `legacy::open(path_or_bytes) -> LegacySurvey`: copies the file and any `-journal` to a private temp, runs the old migration chain and the full old load pipeline (exactly what the app did on open), and returns the settled legacy project plus raw positions and Closures Computed View positions per slot, and the gauge station of each datum-less component.
- For tests only (`feature = "writer"`): `legacy::write(project, shape) -> bytes`, the v18 writer (and the older shapes where cheap), so property tests can produce "what an old build wrote".

### 7.2 Conversion

1. **Stations:** union-find over the legacy merges. Self-merges, same-line merges and merges that would close a cycle (taken in the old cascade's BFS order) become `Batch` closures instead. Each group is one station. Survivor id = the member the map labelled (the label-representative rule: not a line's first station, then not its last, then line order, then index); others become aliases with their names. Names: survivor's, uniquified. Comments: distinct non-empty member comments joined in member order with a blank line. Datums: all kept, in list order.
2. **Lines:** shots in order with their uuids (the legacy load already made them project-unique; mint any still missing deterministically from line id and index), chain stations mapped.
3. **Hand moves:** each legacy `station_offsets` entry goes to `line(key.line).hand_moves[station]`.
4. **Closures:** ids, ends, strategy kept. Legacy aligned non-`Move` closures become `Move` (their alignment is then derived; reported).
5. **Anchors:** per component: if it holds a datum, the first one in list order places it; `correction = legacy_raw(datum station) - gps_to_local(gps, frame.gps)`, stored when its magnitude exceeds 1e-9 m. Otherwise a `Placement` at the legacy gauge station (held gauge, else the legacy default gauge) at its legacy raw position.
6. **Verify:** compute new raw and Closures Computed View positions and compare per slot with the legacy ones (raw within 1e-6 m, adjusted within 1e-6 m, Z within 1e-9 m). Mismatches are reported (log and MigrationReport); tests and the corpus run assert zero.
7. **History:** dropped, with the existing "history discarded" banner (as v7/v13/v15/v16/v17 and #808 did).
8. **Everything else** (settings, Sections, credits, ignores, import hashes, SpeleoDB link and per-machine state, display settings) carried over.

The result is held in memory; the original file is untouched until the first save writes v2 over it (silent upgrade), after which the stale `-journal` (if any) is removed.

### 7.3 MigrationReport

Every normalisation and every conversion rule that fired, with counts: merges collapsed, aliases created, comments joined, cycle merges demoted, aligned closures converted, corrections stored, placements created, hand moves carried, verification mismatches. Logged; returned to tests; never shown raw to users.

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
- Reverse Line on a long line must stay under the size gate: store it compactly (e.g. an op that records "reverse line L" when that transform is an exact involution, checked bitwise by a test; else field-level images).
- The 1% size gate (`undo_size_gate_tests.rs`) is ported: every entry at most 1% of the v2 file on the 5k and 20k synthetic surveys, except actions that construct the whole file; undo/redo bit-exact.

### 8.2 Kept as is (UX contract)

IPC (`undo`, `redo`, `undo_to`, `redo_to`, `can_*`, `get_undo_stack`, `get_redo_stack`), `UndoHistoryItem` (description, timestamp, commandType, preview), record-time labels for composite edits and live name resolution for single commands (names of deleted entities from before-images), previews, failure semantics, strictly increasing timestamps, SpeleoDB watermark and clear-on-push, byte budget, history limits, privacy (no paths), command lock and lock order, read-only gate.

### 8.3 Persistence

`"history":{"undo":[...],"redo":[...]}` in the file, one entry per line: `{"t":timestamp,"cmd":<command for label/preview>,"ops":[...]}`. Pushes strip redo always and undo unless Full mode, cut at the watermark (as today). Histories from older files are dropped (7.2.7).

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
| L | `crates/legacy`: frozen reader + load pipeline + positions; test-only writer | - |
| M0 | `models::survey` skeleton: types, ids, slots, components, invariants/validate, positions (3.3-3.5), naming, identity, wire projection helpers | - |
| M1 | shots/lines/ranges/reverse/split/combine on M0 | M0 |
| M2 | merge/detach/closures/sweep/datums/frame/placements/move tools on M0 | M0 |
| M3 | derived views: LSQ port, closure metrics, roles/entrances/gaps, frozen shots, stats, pathfinding/distance/gas engine port | M0 |
| F | v2 encoder/decoder, canonical rules, versioning, validation, `docs/file-format.md`, format property tests | M0 |
| G | migration legacy -> survey + MigrationReport + property tests + corpus runner | L, M0 (M1-M3 for full verification) |
| U | changeset undo engine + history persistence + size gate | M0 |
| C | switch core (`AppState`) to `models::survey`: execute pipeline, every command family, queries, imports funnel, add/merge projects, dive plan resolver, warnings | M0-M3, U |
| P | save/load/autosave/recents/recovery/backups on v2; SpeleoDB push/pull/cache/revisions | F, G, C |
| X | exporters; stalx-reader | M0-M3 |
| W | IPC handlers, wire DTOs, TS type generation, binary envelope | C |
| FE | frontend changes (9.3) and the IPC mock | W |
| T | fuzzers and harness ported; new property suites | all |
| D | docs: architecture, saving, closures, undo-redo, SPEC data model, SpeleoDB, testing; `docs/file-format.md`; `docs/survey-model.md` | all |

The old `models::project` module and its tests are deleted once C compiles against `models::survey`; tests that pin user-visible behaviour are ported to the new model first.

---

## Background (current-state findings this plan relies on)

- Merges already form a forest by construction (a merge between connected lines is refused; every loop is an explicit closure), so "shared station + forest" changes no topology any current code path creates. Legacy files can hold merge cycles; migration demotes the cycle-closing merge to a closure.
- `line_offsets` is derived for every line reached by the cascade, but the seeds (placement-datum lines and each datum-less component's lowest line) carry user decisions: where a free piece sits and where a datum piece sits relative to its GPS. These become `Placement` and `ZeroDatum.correction`.
- `station_offsets` (hand moves) are no longer created by any command; they survive only in old files.
- Closure metrics are stored at creation and never refreshed; `is_aligned` is re-measured only for `Move` closures.
- Merged stations carry independent names, Station IDs, comments and datums today; the map shows one label per merge group (the representative rule), which the migration reuses for the survivor.
- Schema "v18" spans several file shapes (side tables added without a version bump); the migration must not key on the version alone.
- No Rust test opens a file an old build wrote; `stalx-reader` already reproduces the app's open positions bit-exactly and is the basis of the legacy oracle.
