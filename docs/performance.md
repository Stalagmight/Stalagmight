# Performance

This document covers key algorithmic decisions and their performance
characteristics, for both the Rust backend and the WebGL frontend.

## Loop Closure Adjustment - Sparse Solver

**File:** `src-tauri/crates/models/src/survey/lsq.rs` (`Grounded`), driven by `survey/adjust.rs` (`Project::adjusted_positions`)

The loop closure adjustment solves a weighted least-squares system to distribute
survey errors across the network. The normal equations form a weighted graph
Laplacian matrix - each edge (shot) contributes a constraint between two station
groups, with weights derived from measurement uncertainties (sigma for length and
azimuth, scaled by line quality).

### Problem structure

The normal matrix A^T A is:
- **Symmetric positive-definite** (a graph Laplacian grounded at the held
  groups: datums, hard GPS fixes or the gauge)
- **Extremely sparse** - at most ~3 non-zeros per row (each station group
  connects to a small number of neighbors via shots)
- **Dimension n** = number of unknown station groups (groups connected via
  merges and aligned closures)

X and Y axes are solved independently since they have different per-shot weights
(the azimuth orientation affects directional uncertainty differently).

### What is solved: corrections, not coordinates

The unknowns are each free group's **correction** from its raw position (its
lowest member's), not its absolute coordinates. Per axis the system is
`sum_j w_ij (d_i - d_j) + g_i d_i = r_i`: positive edge weights between free
groups, ground weights `g` for shots to held groups and for soft GPS fixes, and
right-hand sides `r` that are each shot's weight times what the current
positions leave of its vector (and each soft fix's weight times its offset).
In exact arithmetic it is the same minimiser as the absolute form.

Why: weights can be 1e14 apart (a zero-length connector on a line of quality
0.25 weighs 3.4e8, a 100 km shot 5e-6 across). Solved for coordinates of 1e5 m,
the right-hand side carried `eps x weight x coordinate` of rounding and the
diagonal (a float sum, reduced by subtraction) leaked to ground; a part of the
survey held by one weak shot turned that into hundreds of metres (migration fuzz
farm seed 34022454: 809 m, where the exact solution moves 10.7 m). In the
correction form a satisfied shot contributes exactly nothing, the operator is
applied as differences (a rigid shift of a part is exactly no current), and the
elimination only adds positive terms.

### Algorithm

1. **Assembly:** per shot between groups an `Observation` (its two ends, free
   or held, its vector plus the hand-move difference, its two weights; an axis
   whose weight is not a positive number is not observed); edges in `adj[i] =
   [(j, w_x, w_y), ...]`, ground weights per axis.

2. **Exact elimination (GTH form):** every node of current degree up to 8 is
   eliminated, lowest degree first (ties by node), both axes in lockstep:
   the pivot `p_u = g_u + sum w_uv` is summed afresh, a neighbour's ground grows
   by `w_uv g_u / p_u`, the fill-in between two neighbours is
   `w_ua w_ub / p_u`. Degree-1 and degree-2 nodes (spurs and chains, the bulk
   of a cave survey) go first, as the former peeling of degree 1 and 2 did; the bound
   caps one step's fill-in at 28 edges. A zero pivot (an axis nothing holds)
   is left to the core.

3. **Core:** whatever survives (a dense web of loops) is solved with a
   Jacobi-preconditioned conjugate gradient applied in difference form, from
   zero corrections, to a 1e-12 relative residual, at most `min(n, 10,000)`
   iterations.

4. **Refinement:** the solve runs 3 passes on one factoring. Each pass
   recomputes the right-hand side from the positions the previous pass left,
   so the rounding of the first pass's large currents (a group's offset times a
   strong weight), which a weakly held part amplifies, is corrected by the
   next. A pass whose currents are all exactly zero stops the loop: a survey
   with nothing to close keeps its raw positions to the bit.

**Complexity:** O(n log n) elimination (a heap of degrees), each step bounded
by the degree cap, plus 3 back-substitutions and the conjugate gradient over
whatever core is left, vs O(n²) for the former full-system PCG on a single
long loop and O(n³) for dense LU.

**Precision:** on the migration fuzz farm's seeds the solve agrees with a
100-digit solve of the same least squares within 5e-9 m (the absolute-coordinate
solve was off by up to 807 m); `adjust_precision_tests` pins the weakly held
part.

**Against the absolute-coordinate solve** (2026-11-21): the whole Closures
Computed View (`Project::adjusted_positions_from` on a given raw layout) in the
profile the app ships with (`opt-level = "s"`, thin LTO), the minimum of three
interleaved rounds of 15 runs on a 16-core host under load from other builds,
so the ratios matter more than the milliseconds. "Before" is `lsq.rs` and
`adjust.rs` of 13523a421, everything else the same; `adjust_bench_tests.rs`
(ignored) builds the surveys and prints the "after" side:

| Survey | Shots | Before (ms) | After (ms) | After / before |
|---|---|---|---|---|
| size gate, 5k (its closures are snaps: no solve) | 5,031 | 0.30 | 0.29 | 0.97 |
| size gate, 20k (no solve) | 20,031 | 1.49 | 1.44 | 0.97 |
| size gate, 5k, closures soft | 5,031 | 2.48 | 2.45 | 0.99 |
| size gate, 20k, closures soft | 20,031 | 10.7 | 11.6 | 1.08 |
| maze 10 x 10, 28-shot passages (81 loops) | 5,040 | 2.49 | 3.09 | 1.24 |
| maze 20 x 20, 25-shot passages (361 loops) | 19,000 | 10.4 | 13.1 | 1.27 |
| maze 40 x 40, 6-shot passages (1,521 loops) | 18,720 | 14.7 | 18.4 | 1.26 |
| maze 70 x 70, 2-shot passages (4,761 loops) | 19,320 | 36.1 | 41.9 | 1.16 |
| 5k gate + 800 soft closures between random stations | 5,031 | 3.39 | 4.95 | 1.46 |
| 20k gate + 3,000 random soft closures | 20,031 | 14.8 | 22.1 | 1.50 |
| 20k gate + 10,000 random soft closures | 20,031 | 20.9 | 25.3 | 1.21 |
| maze 40 x 40 with unit mistakes (every 40th line's first shot x 1000) | 18,720 | 14.9 | 20.3 | 1.37 |
| maze 70 x 70 with unit mistakes | 19,320 | 36.6 | 41.6 | 1.14 |
| 20k gate + 3,000 random closures, unit mistakes | 20,031 | 17.9 | 21.3 | 1.19 |

At `opt-level = 3` the ratios were 0.89 to 1.23. A maze is a grid of
junctions joined by passages, each passage into a junction that already exists
closed onto it by a soft closure. The two solves agree within 6.5e-8 m on every
one of these surveys. Where the time goes (70 x 70 maze, `opt-level = 3`): the
elimination leaves a core of 1,789 of 14,559 groups (25,422 edges), factored
in about 7 ms; each of the three passes takes about 6 ms, nearly all of it the
conjugate gradient (about 225 iterations per axis); the third pass's
corrections are below 1e-11 m. A tree-like survey leaves no core at all (the
20k soft gate: 19,986 groups eliminated, factoring 5 ms, each pass 0.3 ms).

### Why not a library sparse solver?

A sparse Cholesky (LDL^T) factorization from a library would also work.
However:
- It requires a sparse matrix library (e.g., `sprs`, `cholmod` bindings) - an
  extra dependency.
- The bounded elimination above is a sparse factorization of exactly the shape
  a cave survey has (trees, chains and a few loops), in positive arithmetic, in
  a hundred lines of pure Rust with no dependencies beyond `std`.

### Profiling results

On a project with ~500 lines (~10,000 stations):
- **Before (dense LU):** `adjust_least_squares` consumed 96.78% of CPU for
  multiple minutes. The inner loop was a Gaussian elimination kernel
  (movsd/mulsd/subsd) operating on a dense n×n matrix.
- **After (sparse PCG):** `adjust_least_squares` dropped to 5.14% of CPU. The
  hotspots shifted to normal app activity (image resizing, tile rendering).

## Frozen Shot Detection - Tarjan's Bridge-Finding

**File:** `src-tauri/crates/models/src/survey/frozen.rs`, `Project::frozen_shots()`

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

**Complexity:** O(S × V) - for 500 lines with ~20 shots each, this means
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
2. **Build adjacency list** with edge metadata (is_shot, line_id, shot_index)
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
**simple graph**: shots connect consecutive intra-line stations, merges
connect inter-line stations, and closures connect non-adjacent stations.
No two edge types can create parallel edges between the same station pair, so
the multigraph handling is purely defensive.

### Profiling results

On a project with ~500 lines, undo operations that previously stalled due to
`compute_frozen_shots()` dominating CPU no longer show the function in profiling
hotspots.

## The edit pipeline, and the benchmark against 0.6.0

**Where one edit's time goes.** Every recorded edit runs one pipeline
(`AppState::run_recorded`, `core/state/commands.rs`): the survey is cloned (the
rollback and the undo capture read it), the command's model operation runs its
own protocol (a working copy, the anchor pass, validation; `survey/edit.rs`),
the result is validated, and the changeset is captured by a diff. Each step is
O(survey): on the 20,000-shot gate survey a clone is about 3 ms, `validate`
about 2.5 ms (12 ms before its fast paths) and the capture under 1 ms
(`perf_parts_20k`). That floor is why a
one-cell edit costs milliseconds where 0.6.0 took a tenth of one; the refresh
that follows every edit (`get_project_state`) costs more on both sides, and an
undo or redo (a changeset step) costs a fraction of 0.6.0's.

**Shot edits are one pass per command.** `UpdateShot`, alone or as a batch's
run of them (a data-table paste), goes through one `ShotEditRun`
(`survey/edit/shot_run.rs`). Each update keeps exactly the semantics of
`Project::update_shot` (the same refusals at the same cell, the same stored
model to the bit: `shot_run_tests.rs`, and the metamorphic "a batch is its
commands in order" pair), but the run keeps the raw layout of the current state
and updates it after each cell instead of taking a working copy, two layouts, a
frozen-shot computation and a validation per cell: a datum-held piece is
re-walked downstream of the edited line (`Project::fold_line` is the one
arithmetic of both walks), a free piece is re-walked from its new placement when
next read, and the frozen set is recomputed only when a closure's alignment
changed. A paste of N cells costs one survey pass plus N small walks.

**Also:** `validate` uses sorted fast paths and a dense union-find (the same
reports, pinned by a differential test); the gas reach field runs on the
revision's cached layout and a per-revision copy of the project, outside the
project lock, and the last field per revision is kept; Combine Lines judges its
refusal without the dialog's two dry-run plans and reads the pipeline's snapshot
instead of cloning again; a SpeleoDB-linked project rewrites its file when the
dirty bit flips, not on every edit.

**Running the comparison.** `perf_bench_5k` / `perf_bench_20k`
(`core/state/perf_bench_tests.rs`, ignored) time the `AppState` entry points the
IPC commands call, on the size gate's surveys, median of
`STALAGMIGHT_PERF_RUNS` runs; `STALAGMIGHT_PERF_REPORT` appends the rows to a
file. The 0.6.0 side is the same file dropped into a release/0.6.0 checkout (it
builds the survey with 0.6.0's own model and saves it with 0.6.0's save) and
pointed at the same `STALAGMIGHT_PERF_LEGACY_DIR`, so both open one legacy
file:

```bash
cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  perf_bench -- --ignored --nocapture --test-threads=1
```

**Results** (2026-11-20, release builds, a 16-core host under load from other
builds, so the ratios matter more than the milliseconds; do / undo / redo):

| Operation | 0.6.0, 5k | now, 5k | ratio | 0.6.0, 20k | now, 20k | ratio |
|---|---|---|---|---|---|---|
| open (0.6.0: its file; now: upgrade-open of that file) | 17.5 | 38.6 | 2.20 | 80.1 | 197 | 2.46 |
| open a version 2 file | 17.5 | 9.8 | 0.56 | 80.1 | 45.1 | 0.56 |
| binary envelope encode | 1.52 | 0.57 | 0.37 | 8.59 | 3.06 | 0.36 |
| save (0.6.0: one-sync SQLite; now: v2) | 29.2 | 5.83 | 0.20 | 125 | 25.2 | 0.20 |
| get_project_state (+ JSON) | 8.05 | 7.07 | 0.88 | 40.8 | 32.1 | 0.79 |
| gas reach field | 3.94 | 1.76 | 0.45 | 23.0 | 8.82 | 0.38 |
| update shot (comment) | 0.05 / 0.93 / 0.93 | 1.49 / 0.03 / 0.02 | 28 | 0.14 / 4.22 / 4.03 | 9.25 / 0.09 / 0.04 | 66 |
| update shot (length) | 0.62 / 1.03 / 1.13 | 2.19 / 0.03 / 0.02 | 3.6 | 2.42 / 4.65 / 4.70 | 12.9 / 0.09 / 0.04 | 5.3 |
| insert shot | 8.95 / 2.43 / 1.14 | 4.76 / 0.03 / 0.02 | 0.53 | 42.8 / 10.5 / 5.08 | 26.8 / 0.10 / 0.04 | 0.63 |
| delete shot | 15.5 / 12.1 / 12.4 | 7.43 / 0.03 / 0.02 | 0.48 | 70.3 / 54.2 / 54.5 | 36.4 / 0.09 / 0.04 | 0.52 |
| split line | 16.6 / 13.8 / 13.6 | 3.71 / 0.04 / 0.02 | 0.22 | 76.0 / 61.0 / 59.7 | 21.5 / 0.09 / 0.05 | 0.28 |
| combine lines | 21.0 / 15.3 / 15.3 | 12.6 / 0.05 / 0.03 | 0.60 | 94.6 / 67.5 / 65.4 | 68.5 / 0.12 / 0.06 | 0.72 |
| merge stations | 10.4 / 11.2 / 11.0 | 4.37 / 0.85 / 0.84 | 0.42 | 44.6 / 52.2 / 52.4 | 26.5 / 4.76 / 4.25 | 0.60 |
| detach | 10.1 / 11.3 / 11.1 | 3.46 / 0.85 / 0.85 | 0.34 | 43.2 / 52.3 / 50.9 | 21.9 / 4.62 / 4.14 | 0.51 |
| move station | 11.1 / 12.9 / 12.1 | 3.96 / 0.03 / 0.02 | 0.36 | 42.9 / 50.7 / 50.1 | 23.6 / 0.10 / 0.04 | 0.55 |
| datum edit (entrance GPS) | 0.67 / 1.03 / 1.17 | 3.15 / 0.58 / 0.58 | 4.7 | 2.41 / 4.51 / 5.17 | 20.0 / 3.17 / 2.85 | 8.3 |
| reverse line | 13.6 / 14.3 / 14.1 | 3.84 / 0.03 / 0.02 | 0.28 | 55.0 / 58.9 / 58.5 | 23.9 / 0.10 / 0.04 | 0.43 |
| paste 1,000 cells (LRUD) | 22.5 / 25.8 / 26.0 | 2.36 / 0.32 / 0.31 | 0.10 | 68.1 / 65.8 / 64.9 | 11.1 / 0.44 / 0.36 | 0.16 |
| paste 1,000 cells (length) | 202 / 26.6 / 25.8 | 24.1 / 0.34 / 0.31 | 0.12 | 764 / 66.8 / 66.6 | 27.1 / 0.43 / 0.35 | 0.04 |
| Compass import (50 / 200 surveys) | 23.4 / 33.9 / 37.2 | 7.56 / 0.90 / 0.98 | 0.32 | 111 / 164 / 181 | 33.7 / 5.02 / 4.55 | 0.30 |

Before this work the 1,000-cell length paste took 13.2 s (5k) and 31.8 s (20k)
in release, and Combine Lines 132 ms and 527 ms.

**Still slower than 0.6.0, and why:**
- *Opening a legacy file* (once per file: the first save writes version 2, and
  a version 2 open is about half 0.6.0's). It is the frozen 0.6.0 reader
  itself plus the migration, which converts and then verifies the result
  against the legacy positions and both Closures Computed Views (plan 7.3).
- *A single shot edit and a datum edit* pay the pipeline floor above (a clone,
  one or two validations, a capture: about 9 ms on 20k), where 0.6.0 patched
  one shot and kept no undo snapshot. The edit plus the refresh that follows it
  is within 7% of 0.6.0's on both surveys for shot edits, and within 21% for a
  datum GPS edit; the undo and redo of a shot edit are 30 to 120 times faster
  (of a datum edit, 1.4 to 1.8 times). Getting
  the edit itself under 0.6.0's needs the survey shared copy-on-write (lines and
  stations behind `Arc`, so the snapshot and the capture cost what the edit
  touched), which is a model-wide change.

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
| Structural edit (insert / delete / merge / split / reorder, and their undos) | vertex slots move, so a per-line re-emit lands in the wrong range |
| An edit that moves the survey bbox extent | the mercator anchor shifts, so every anchor-relative position changes |
| GPS-origin change | same, via reprojection |
| Issue-severity coloring active | severity arrays are recomputed per flush and severity is non-local |
| Move-comment tint active | per-feature comment ownership is not tracked by the diff |
| More than half the lines dirty | many `bufferSubData` calls lose to one rebuild (`incremental.ts:197`) |

Everything else takes a fast path: an edit that leaves the SoA layout byte-stable
re-emits only the dirty lines' vertex ranges, and a pure global recolor
(analytical overlay enter/exit or recompute, theme swap, depth toggle)
re-resolves and re-uploads only the four color buffers, skipping reprojection and
tessellation entirely.

**Label builds** (`customRender/labelController.ts`) do glyph shaping plus a
zoom-ladder greedy declutter, which is the heaviest CPU pass in the renderer.
They are scoped to a padded viewport and emit a slack zoom window, and they
rebuild on camera settle (`moveend` / `zoomend`, coalesced and deferred off the
render stack) only when that padded viewport is exited or the slack runs out. A
modest zoom-in inside the slack window needs no rebuild.

**Hit-testing** runs per pointer move through
`SurveyLayer.hitCandidateLines`, since `queryRenderedFeatures` returns nothing
from a custom layer. `surveyLayer/geometryIndex.ts` caches projected line
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
Measured on the fractal fixture (32 lines / 116 stations, 300 jumps):
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

**`src/hooks/useFpsMonitor.ts` is not running.** It implements a rolling
`requestAnimationFrame` average with hysteresis thresholds, but it has no
consumer; the live FPS readout is `useMapDevStats`' own loop. Do not wire
adaptive work to it without wiring the hook up first.
