# Performance

This document covers key algorithmic decisions and their performance
characteristics, for both the Rust backend and the WebGL frontend.

## Loop Closure Adjustment - Sparse PCG Solver

**File:** `src-tauri/crates/models/src/project/lsq.rs`, `adjust_least_squares()`

The loop closure adjustment solves a weighted least-squares system to distribute
survey errors across the network. The normal equations form a weighted graph
Laplacian matrix - each edge (shot) contributes a constraint between two station
groups, with weights derived from measurement uncertainties (sigma for length and
azimuth, scaled by section quality).

### Problem structure

The normal matrix A^T A is:
- **Symmetric positive-definite** (graph Laplacian)
- **Extremely sparse** - at most ~3 non-zeros per row (each station group
  connects to a small number of neighbors via shots)
- **Dimension n** = number of unknown station groups (groups connected via
  merges and aligned closures)

X and Y axes are solved independently since they have different per-shot weights
(the azimuth orientation affects directional uncertainty differently).

### Algorithm: Jacobi-preconditioned Conjugate Gradient

Instead of assembling a dense n×n matrix and using LU decomposition (O(n³)), the
solver uses a sparse representation and iterative Conjugate Gradient:

1. **Sparse assembly:** diagonal entries in `diag_x[i]` / `diag_y[i]`,
   off-diagonal entries in adjacency list `adj[i] = [(j, w_x, w_y), ...]`.
   A `macro_rules! add_sparse!` macro handles the fixed/unknown group indexing
   to avoid borrow issues with mutable arrays.

2. **Sparse matrix-vector product** (`spmv`): iterates diagonal + adjacency
   list - O(nnz) per call.

3. **Jacobi preconditioner:** M = diag(A), trivially inverted. Good enough for
   graph Laplacians where the diagonal dominance ensures fast convergence.

4. **Exact degree-≤2 reduction, then CG:** `solve_reduced_laplacian` peels
   degree-1 spurs and series-reduces degree-2 chains exactly in O(n), runs
   Jacobi-PCG on the surviving 2-core (≈ one node per eligible closure), then
   back-substitutes.

**Complexity:** O(n) peeling plus PCG over the tiny 2-core, vs O(n²) for the
former full-system PCG on a single long loop and O(n³) for dense LU.

**Tolerance:** 1e-12 relative residual norm, max 10,000 iterations.

### Why not a direct sparse solver?

A sparse Cholesky (LDL^T) factorization would also work and give exact results
in one pass. However:
- It requires a sparse matrix library (e.g., `sprs`, `cholmod` bindings) - an
  extra dependency.
- The graph Laplacian structure makes CG converge extremely fast (typically
  under 50 iterations for survey networks), so the iterative approach is both
  simpler and fast enough.
- The PCG implementation is ~50 lines of pure Rust with no dependencies beyond
  `std`.

### Profiling results

On a project with ~500 sections (~10,000 stations):
- **Before (dense LU):** `adjust_least_squares` consumed 96.78% of CPU for
  multiple minutes. The inner loop was a Gaussian elimination kernel
  (movsd/mulsd/subsd) operating on a dense n×n matrix.
- **After (sparse PCG):** `adjust_least_squares` dropped to 5.14% of CPU. The
  hotspots shifted to normal app activity (image resizing, tile rendering).

## Frozen Shot Detection - Tarjan's Bridge-Finding

**File:** `src-tauri/crates/models/src/project/distance.rs`, `compute_frozen_shots()`

A shot is "frozen" (its geometry cannot be edited) when removing it would not
disconnect any zero datum station from the rest of the network. Equivalently: a
shot is frozen when both its endpoints can reach a zero datum via alternative
paths (shots, merges, and aligned closures) that don't include the shot itself.

### Why this matters for performance

`compute_frozen_shots()` is called on every `get_project_state()` IPC call,
including after every undo/redo operation. It must be fast.

### Previous approach: per-shot BFS

The original `is_shot_frozen()` ran two BFS traversals per shot - one from each
endpoint - to check if both sides can still reach a datum station when the shot
is excluded. With S shots and V stations:

**Complexity:** O(S × V) - for 500 sections with ~20 shots each, this means
~20,000 BFS operations, each visiting up to V stations.

### Current approach: Tarjan's bridge detection

The key insight is that frozen/non-frozen status is determined by the graph's
**bridge** structure:

- **Non-bridge shot** in a component with ≥1 datum: **frozen**. Removing a
  non-bridge edge doesn't disconnect the graph, so both endpoints can still
  reach the datum.
- **Bridge shot**: **frozen only if both sides have datums**. Removing a bridge
  splits the component into two parts; both parts must independently contain a
  datum for both endpoints to be anchored.
- **Any shot** in a component with 0 datums: **not frozen**.

The algorithm:

1. **Index stations** into compact integers for array-based access.
2. **Build adjacency list** with edge metadata (is_shot, section_id, shot_index)
   from shots, merges, and aligned closures.
3. **Iterative Tarjan's DFS** (avoids stack overflow on large graphs):
   - Computes discovery times (`disc`) and low-link values (`low`)
   - An edge parent→child is a bridge iff `low[child] > disc[parent]`
   - Simultaneously counts datums in each DFS subtree (`subtree_datums`)
   - Tracks total datums per connected component (`component_datums`)
4. **Classify each shot:**
   - Look up whether the shot's edge is a bridge
   - Non-bridge + component has datums → frozen
   - Bridge + both sides have datums → frozen
   - Otherwise → not frozen

**Complexity:** O(V + E) - a single DFS pass over the entire graph.

### Multigraph note

The DFS uses a `parent_edge_skipped` flag to correctly handle the first back-
edge to the DFS parent (skip it as the tree edge) vs subsequent edges (treat as
back edges that prevent bridging). However, in practice the survey graph is a
**simple graph**: shots connect consecutive intra-section stations, merges
connect inter-section stations, and closures connect non-adjacent stations.
No two edge types can create parallel edges between the same station pair, so
the multigraph handling is purely defensive.

### Profiling results

On a project with ~500 sections, undo operations that previously stalled due to
`compute_frozen_shots()` dominating CPU no longer show the function in profiling
hotspots.

## Per-Command Geometry Reconcile

Every command whose `Command::affects_geometry()` returns true runs a
`reconcile_geometry()` pass in the `execute()` epilogue
(`remeasure_closure_alignment` + `realign_merges` +
`remeasure_closure_alignment`, with `realign_merges` reaching
`check_merge_alignment` and the offsets re-solve internally). That pass is
O(total shots + total merges) and allocates two whole-project maps
(`section_local_coords` and the resolved station offsets) - it is **not** cached
between calls.

`UpdateShot` is in this group (a length/azimuth/depth edit moves stations, and
the bespoke `realign_after_shot_edit` no-ops on a merge group with no zero
datum, so without the reconcile a shot edit upstream of a datum-less merge would
leave the partners drifted). The cost is one O(N) pass **per discrete edit** -
acceptable because the Edit Shot dialog commits a single `UpdateShot` on save,
not one per keystroke, and it matches the per-command cost every other geometry
command (`MergeStations`, `MoveStation`, `SetZeroDatum`, …) already pays.

Watch this if a future bulk/Excel-style data-entry surface (see roadmap #61)
fires an `UpdateShot` per cell: that would multiply the reconcile by the edit
count and should batch the edits (the `Batch` command already finalizes once)
or debounce the reconcile.

## The Refresh After an Edit

After every successful edit the frontend re-reads the project: the state, the
warnings list and the integrity check (`projectStore.executeCommand`). Three things
keep that off the critical path (#576 C2, A9):

- **Off the main thread.** Every command runs on the blocking pool
  (docs/architecture.md, "Where commands run"), so the window keeps painting while
  the backend works; before, each of these reads ran inline on the main thread.
- **No copies.** The project is held copy-on-write (`core/state/project_slot.rs`);
  a read takes an O(1) snapshot instead of deep-cloning the survey. Each of the three
  reads used to clone it.
- **Binary state.** The refresh fetches `get_project_state_binary`, the load's
  msgpack + typed-array envelope (`core/load_binary.rs` `encode_refresh`, decoded by
  `utils/loadProjectBinary.ts`), instead of the JSON `get_project_state`, which stays
  for the agent harness and tests.

Release probes (`src-tauri/src/commands/refresh_probe_tests.rs`, `#[ignore]`d; run
with `--release -- --ignored --nocapture --test-threads=1`), synthetic 75,000 shots
in 750 sections, medians of 7 on a loaded machine (2026-11):

| Step | Before | After |
|---|---|---|
| whole-project copies per edit cycle | 3 (about 7 ms each) | 0 |
| warnings list | 16.4 ms | 4.6 ms |
| integrity check | 25.9 ms | 13.3 ms |
| state refresh payload | JSON: 180.6 ms, 36.5 MB, then `JSON.parse` | binary: 74.0 ms, 7.2 MB |

The rest of the state refresh is the measured coordinate walk, penetration and the
frozen-shot pass, which both forms pay.

## Placement Suggestions - the Crossing Check

`suggest_placements` / `suggest_all_placements` (models `placement_suggest.rs`,
behind the import wizards and `suggest_orphan_placements`) score every way an
imported section could continue an existing one, then reject a candidate whose
placed trace would cross the existing trace too closely. That crossing check is
exact: since #576 A2 a uniform grid (`CrossingPrefilter`) hands each placed
import segment only the existing segments within a proven rounding margin, which
took two straight 2,000-shot sections from 370 s to well under a second.

The worst case that left was a scene where almost every candidate survives the
scoring: long, dead-straight, constant-depth sections, where every shot matches
every other. Each surviving start index then needed one crossing check over the
whole placed import - candidates x import length - and a profile of the 5x2000
scene below put 58% in the per-segment grid walk, 15% in `floor`, 10% in
`cell_range` and 9% in the segment test itself: the walk, once per placed
segment per candidate, with no single hot spot.

### The collinear speedup (#576 NF-W4U-3)

The check now skips what it can PROVE the exact segment test passes, so every
verdict - and with it every suggestion, rejection and score - is unchanged.
`CrossingIndex`, built once per section pair (and only on the first candidate
that reaches the check, which most pairs never produce), applies three shortcuts:

1. **Straight runs proven parallel.** The import trace is cut into straight runs
   (16+ consecutive segments whose direction vectors agree to 1e-10 m / leg).
   For a run and one existing segment, `run_parallel_to` bounds the
   denominator the segment test computes, for every translation the check can
   apply - including the rounding of placing the import - and when that bound
   is under half of `PARALLEL_DENOM_EPS` the test returns "parallel, no
   crossing" for every pair, whatever the candidate. A run proven parallel to EVERY existing segment is
   not walked at all; on the collinear scenes that is the whole import.
2. **Sparse lanes.** A run proven parallel to only some existing segments walks
   a grid built over the others (same margin, same cell rule, same entry test).
3. **Chunk skips.** Placed segments are walked in chunks of 32. When a chunk's
   placed box, grown by the margin, misses its lane's grid, the chunk is skipped
   in one test: rounding is monotone, so that box contains every member
   segment's own query box, and each of those would have missed too.

The crossing check now lives in `placement_suggest/crossing.rs`. The bound and
its derivation are in the `run_parallel_to` doc comment, the other two
arguments in `CrossingIndex`'s. The proof needs the placed coordinates times
the leg length to stay small (about 1e6 m^2: 5 m legs over roughly 70 km of
section); past that it proves nothing and the walk runs, still exact.

**The gate.** `placement_suggest/differential_tests.rs` keeps the pipeline's
crossing half as it was before this change (`reference`, verbatim) and runs
seeded scenes through both: random walks, collinear, skewed and micro-noise
sections around the parallel cutoff, bends, crossings near the 4 m clearance,
constant depth, zigzags, long legs, degenerate shots, import-to-import. The
suggestions must match field for field with scores bit for bit, and the crossing
checks the two pipelines make must match one for one with the same verdicts; each
section pair also gets extra direct crossing checks. A failure prints its seed
(`PLACEMENT_DIFF_SEED=<seed>` replays it). The default run is 660 scenes in
debug; this change passed 50,000 seeds (sections up to 160 shots) plus 3,000
(up to 600) and 11 worst-case-size scenes in release, with 0 mismatches, and the
gate caught two deliberately wrong variants (a 100x looser parallel tolerance,
seed 0x13; a chunk box one point short, seed 0x4c). A NaN reading used to make
the score sorts panic (35 of those 53,000 scenes): they took NaN as equal to
every score. They order by `score_cmp` now (#576 NF-W9PERF-1) - `partial_cmp` on
any two numbers, NaN below all of them - so no finite result moved (20,000
seeded scenes compared against the old sorts: every all-finite scene
bit-identical) and the gate fails on any panic (20,000 seeds in release: 0
panics, 0 mismatches).

Release probe (`placement_suggest_timing_probe`, 2026-11-03, before = b83b12de5).
The machine was shared (load average 13-17 on 22 cores for this table, 8-17
over the session) and wall-clock runs vary by +/-25%, so the base and optimised
binaries ran alternately, 5 times each, and the table gives the medians. The
instructions column is the exact, load-independent ratio of user-space
instructions for the pipeline alone (`perf stat -e instructions:u`, pinned to
one core, the verbatim reference vs the optimised pipeline in one binary):

| Scene | Before | After | Instructions after / before | Suggestions |
|---|---|---|---|---|
| spec-scale random: existing 200x50, import 20x50 | 91 ms | 55 ms | 0.70 | 3 |
| spec-scale straight: existing 200x50, import 20x50 | 619 ms | 729 ms (*) | 0.95 | 400 |
| long straight: existing 1x2000, import 1x2000 | 519 ms | 13 ms | 0.018 | 10 |
| long straight: existing 5x2000, import 2x2000 | 8,366 ms | 165 ms | 0.017 | 40 |

(*) Noise: that scene's samples overlapped completely (483-799 ms before,
486-778 ms after), its medians went both ways over the six interleaved runs
made during this work (-6% to +18%), and it runs 5% fewer instructions.

What is left:

- **spec-scale straight** is essentially unchanged: its candidates pair sections
  1.7 degrees apart, which no parallel argument covers. Its time splits between
  those candidates' grid walks (about a third), the matcher's scoring (about a
  quarter) and allocation, hashing and sorting.
- **Long, straight, nearly-but-not-exactly parallel sections** (between ~1e-9
  degrees and the matcher's 25 degrees apart) still walk the grid per
  candidate. Axis-aligned ones mostly leave the grid's box and are chunk-skipped
  (2x2000 at 90 / 91 degrees: 1.5 s before, 0.1 s after), diagonal ones do not
  (37 / 38 degrees: 2.2 s before, 0.9 s after). Cutting those needs a different
  exact argument - a separating line with its own rounding bound - not more of
  this one. That 0.9 s for two 2,000-shot diagonal sections 1 degree apart is
  accepted as it stands (#576 NF-W9PERF-2).

Real surveys rarely look like any of these scenes: a random 2,000-shot pair
takes about a millisecond.

## Profiling Guide

### Build for profiling

The project has a `[profile.profiling]` in `Cargo.toml` that inherits from
release with debug symbols preserved:

```toml
[profile.profiling]
inherits = "release"
strip = false
debug = 1
```

Build with:
```bash
cargo build --profile profiling --manifest-path src-tauri/Cargo.toml
```

The binary is at `src-tauri/target/profiling/stalagmight` (setting `CARGO_TARGET_DIR`
moves it to `$CARGO_TARGET_DIR/profiling/`).

### Recording with perf

```bash
# Start the app
WEBKIT_DISABLE_DMABUF_RENDERER=1 /path/to/profiling/stalagmight &

# Record (replace PID and duration)
perf record -g -p <PID> -o /tmp/perf.data -- sleep 15

# Or with DWARF call graphs for better stack traces
perf record -g --call-graph dwarf -p <PID> -o /tmp/perf.data -- sleep 120
```

### Analyzing

```bash
# Top functions by overhead
perf report -i /tmp/perf.data --no-children --percent-limit 1

# Source annotation for a specific function
perf annotate -i /tmp/perf.data --symbol=<function_name>
```

### Debug build caveat

Never profile a debug build for performance analysis. `nalgebra` and other
numerical code runs 20-50x slower without optimizations due to bounds checks
and lack of inlining/SIMD. Always use the `profiling` profile.

---

## Frontend - Survey Rendering Performance

The survey is drawn by custom WebGL2 layers (MapLibre `CustomLayerInterface`)
under `src/components/Map/surveyLayer/customRender/`, not by MapLibre style
layers. There is no stock-layer fallback: if a custom layer cannot start or
self-disables mid-render, it is removed and the failure latches for the session.

[`map-renderer.md`](map-renderer.md) owns the internals - tessellation, shaders,
the anchor-relative coordinate scheme, colors, labels, hit-testing, failure
paths. This section is only about where the time goes and how to measure it.

### Per frame vs per data change

**Per frame** the geometry layer (`customRender/surveyCustomLayer.ts`,
`render()`) composes the anchor matrix in float64, sets blend state, and issues
**two `drawElements` calls** - one for every shot, one for every station -
regardless of survey size. No CPU-side geometry work, no per-feature iteration,
no expression evaluation. Labels, the measure-path tint, and the drag ghost are
separate layers with their own draws.

Consequences worth exploiting:

- Show/hide toggles never rebuild: they cost a flag flip plus `triggerRepaint`.
  Shots and stations skip their draw call outright (`if (this.drawShots)`,
  `if (this.drawStations)`); reference legs and jumps stay in the buffers and are
  discarded in the shader (`u_hide_reference`, `u_hide_jump`).
- Selection, hover, dimming, and drag-hide are per-vertex float flags with their
  own re-upload paths, so they do not disturb colors and colors do not disturb
  them.
- Uniform and attribute locations are resolved once at link time into a
  program bundle (`customRender/glProgram.ts`), rebuilt by `onAdd` on context
  restore, so `render()` never calls `getUniformLocation` /
  `getAttribLocation` (#339) - those are per-name string lookups in the
  driver, and before the cache the custom layers issued ~55 of them per frame.
- The one extra per-frame call is `gl.getError()` after the draws, and only for
  the first 5 frames (`MAX_CHECKED_FRAMES`) - a `getError` is a synchronous
  pipeline stall, so the layer stops asking once it has drawn cleanly.

**Per data change** `SurveyLayer.flushUpdate` runs synchronously in the same
frame: no worker, no tile cut. It rebuilds the hit index (a fresh
`SurveyGeometryIndex`, never mutated in place), syncs the geometry and label
controllers, and re-publishes the small stock overlays. `SurveyCustomController
.sync` then picks the cheapest of three tiers - incremental, color-only, full
build - with the gates in `customRender/incremental.ts`.

### What is expensive

**A full rebuild** reprojects every station, tessellates every shot, resolves
every color, packs, and uploads. The two fast paths exist to avoid it, so the
question that matters is what makes them bail. From `incremental.ts`:

| Trigger | Why the fast paths cannot bound it |
|---|---|
| Structural edit (insert / delete / merge / split / reorder, and their undos) | vertex slots move, so a per-section re-emit lands in the wrong range |
| An edit that moves the survey bbox extent | the mercator anchor shifts, so every anchor-relative position changes |
| GPS-origin change | same, via reprojection |
| Issue-severity coloring active | severity arrays are recomputed per flush and severity is non-local |
| Move-comment tint active | per-feature comment ownership is not tracked by the diff |
| More than half the sections dirty | many `bufferSubData` calls lose to one rebuild (`incremental.ts:197`) |

Everything else takes a fast path: an edit that leaves the SoA layout byte-stable
re-emits only the dirty sections' vertex ranges, and a pure global recolor
(analytical overlay enter/exit or recompute, theme swap, depth toggle)
re-resolves and re-uploads only the four color buffers, skipping reprojection and
tessellation entirely.

**Reprojection with a GPS origin** goes through `metersToMapCoords`
(`src/utils/coordinates.ts`), once per station in every full rebuild, label
build and hit-index rebuild. Since #576 D36 it inverts the backend's WGS84
projection instead of the old 6371 km sphere, and since #576 GEO-D1 it does so
with the backend's exact solve (`local_to_gps` in geo.rs): one quadratic for
where the origin's vertical meets the ellipsoid, then the closed-form latitude
of a surface point - 2 `atan2`, 1 `sqrt` and 1 `hypot` per point, the origin's
trig memoised. The D36 inverse it replaced was an approximation (two Bowring
ECEF-to-geodetic inversions and one forward projection per point: 6 `atan2`,
8 `sin`/`cos`, 3 `sqrt`). Measured under Node 26 / V8 (vitest, one worker,
origin (20.5, -87.3), 400 000 calls over a 20 km square, medians of 5 runs over
3 rounds, both versions in the same run): 0.080-0.084 us per call for the exact
solve, against 0.185-0.200 us for the D36 approximation and 0.05-0.06 us for
the pre-D36 sphere (measured earlier) - about +2 ms per full pass on an
89 000-station project over the sphere, likely more on WebKit. The
transcendental calls are the cost: an earlier scalar rewrite of the D36 code
with no intermediate objects measured the same as the original, as V8 already
removes the allocations and hoists the memoised origin frame. If a large
GPS-referenced project shows this pass in a profile, measure it in the app
first; the remaining lever changes the call shape (one batch inverse per data
build).

**Label builds** (`customRender/labelController.ts`) do glyph shaping plus a
zoom-ladder greedy declutter, which is the heaviest CPU pass in the renderer.
They are scoped to a padded viewport and emit a slack zoom window, and they
rebuild on camera settle (`moveend` / `zoomend`, coalesced and deferred off the
render stack) only when that padded viewport is exited or the slack runs out. A
modest zoom-in inside the slack window needs no rebuild.

**Hit-testing** runs per pointer move through
`SurveyLayer.hitCandidateSections`, since `queryRenderedFeatures` returns nothing
from a custom layer. `surveyLayer/geometryIndex.ts` caches projected section
AABBs against a camera snapshot, so a static hover reprojects once per camera
rather than once per probe. Every camera change invalidates that cache.

**React re-renders** of `MapCanvas` are still the reason cursor-tracking feedback
does not go through the store. Update the layer directly, keep a ref
(`snappedStationRef`) for the hot-path comparison, and defer the Zustand write
with `startTransition` so click handlers still see the right state.

### The `setData` / `setFeatureState` trap

MapLibre's `setData()` drops **all** `setFeatureState` on that source. When the
survey rode vector-tile sources, the distance overlay re-applied its per-feature
state from the per-hover visual-state update; that is a measured frame-rate
collapse, fixed in `88ec311d` ("perf(map): stop reapplying distance feature-state
on hover", #34).

The current design removes the hazard rather than managing it. There is **no
`setFeatureState` call left anywhere in `src/`**: survey geometry carries hover,
selection, and dimming as per-vertex attributes, and the few surviving stock
GeoJSON overlays (the interact overlay, the gas route, loop closures) carry their
gradient value as a feature *property* - `['get', 'dist']` / `['get', 'gasReach']`
through the same ramp expressions the bulk layers used
(`utils/distanceGradient.ts`, `utils/gasReach.ts`), so a `setData` cannot wipe
it.

The rule that survives, and that any new overlay must follow:

- **Re-push per-item overlay state in the post-rebuild tail of `flushUpdate`.**
  Today that tail invalidates `lastInteractKey`, refreshes the interact overlay,
  re-applies its paint inputs, and re-pushes the stored gas-route legs; a new
  overlay joins that list.
- **Never recompute it per hover.** The per-hover path (`updateVisualState` ->
  `refreshInteractOverlay`) is deliberately bounded: a handful of features, and
  short-circuited when the composed state key is unchanged.
- If you reintroduce `setFeatureState` on any source, both halves of that rule
  apply again verbatim.

Two stock-layer patterns also still hold. Paint transitions must be zeroed on
interactive layers (`'circle-opacity-transition': { duration: 0, delay: 0 }`) -
MapLibre's 300 ms default transition reads as lag on hover feedback. And pointer
moves are rAF-batched, so only the latest event is processed per frame.

### How to profile the frontend

**Dev/Map/Zoom Stats overlays** (`src/components/Map/useMapDevStats.ts` plus
`DevStatsPanel.tsx` / `MapStatsPanel.tsx` / `ZoomStatsPanel.tsx`, toggled from
the app menu's Debug submenu - `display-toggle-showDevStats` /
`-showMapStats` / `-showZoomStats` in `useProjectMenuItems.tsx`, not the
Display menu) are the primary tool and the only view inside a custom layer,
since MapLibre's own instrumentation cannot see in. `useMapDevStats` wraps
`drawArrays` / `drawElements` on the context to count draw calls and
triangles per frame; Dev Stats shows FPS, frame time, and that render profile
(draw calls, triangles, layer/source counts), Zoom Stats shows the current
zoom level and last-zoom average FPS/worst frame, and Map Stats shows WebGL
renderer and limits and tile-cache stats. Its per-source feature counts call
`querySourceFeatures`, which is expensive, so they are collected on `idle`
with a 500 ms throttle and never on `render`.

**Counting per-frame GL lookups.** The same context-wrap trick verifies the
link-time location cache (#339): in the harness (or devtools), wrap
`getUniformLocation` / `getAttribLocation` on
`window.__stalagmight_map__.getCanvas().getContext('webgl2')` with counters,
run the `map-storm` scenario, and divide by frames (`map.on('render')`).
Measured on the fractal fixture (32 sections / 116 stations, 300 jumps):
~54 lookups per frame before the cache, ~0.4 after - the residue is MapLibre's
own lazy program setup on the shared context. The survey layers contribute
zero: the only `get*Location` call sites left under `src/` are inside
`linkProgramBundle`, and `surveyCustomLayerGl.test.ts` pins the geometry
layer's render loop at zero lookups. Frame times were vsync-bound
(~16.7 ms mean) on the measurement machine both before and after; the win is
removing driver string lookups from the hot loop, not a measured FPS change.

**Dev-only build timings.** `controller.ts` logs `[customRender] build` with
elapsed ms, shot-segment and station counts, and buffer megabytes;
`labelController.ts` logs `[customRender] label build` with the same plus
per-phase splits. Both go through `utils/debugLog.ts`, which compiles to a no-op
unless `import.meta.env.DEV` - so they appear under `./scripts/dev.sh` and not in
a packaged build. Use these rather than a stopwatch: they attribute a slow edit to
a specific tier and give the buffer size that drove it.

**Ungated lifecycle logs.** `[customRender] enabled`, `first frame drawn OK`, and
the self-disable warnings are plain `console.info` / `console.warn` on purpose,
so a production build stays diagnosable.

**Other gates.** Hit-detection tracing is config-gated
(`debug.map.log_hit_detection`); the macOS pan/zoom toolkit is gated on
`VITE_DEBUG_MACOS_LOCKUP`. Both are documented in
[`debug-gates.md`](debug-gates.md).

**There is no FPS monitor hook.** The unused `useFpsMonitor` was deleted
(#576 D27-32(e)); the live FPS readout is `useMapDevStats`' own loop.
