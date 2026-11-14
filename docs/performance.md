# Performance

This document covers key algorithmic decisions and their performance
characteristics, for both the Rust backend and the WebGL frontend.

## Loop Closure Adjustment - cave-closure

The active horizontal solver is `cave_closure::horizontal::HorizontalProblem`.
The adapter in `src-tauri/crates/models/src/project/lsq.rs` owns application
station groups, controls, and shot/section reconstruction. The crate owns
uncertainty propagation, sparse normal equations, reduction, and PCG.

Both axes share an ordered sparse graph but have independent weights. The solver
peels degree-at-most-two nodes with exact scalar Schur complements, leaving a small
core for warm-started Jacobi-PCG. The pivot guard is relative to the original
diagonal on both axes. Floating axes keep the translation selected by the measured
warm start; no centroid gauges, variance floors, or additional refinement are
introduced. PCG uses the original initial-residual stopping test and caps iterations
at the smaller of core size and 10,000. Back-substitution reconstructs all groups.

Preparation and reconstruction remain component-scoped linear passes. Long loops
reduce in near-linear time rather than requiring a full-system iteration per unit
of loop diameter. Interconnected loop cores cost iterations times their sparse size.
There is no dense matrix allocation or native sparse-direct backend in this path.

The ignored `profile_closure_solve_large` harness measures end-to-end scaling on
synthetic loops. Run it in release mode on an otherwise quiet machine. It includes
application reconstruction, rather than timing only the numerical kernel. Commands
are in the testing recipes.

A local macOS ARM64 comparison measured the following medians (five warmed runs,
including project cloning and reconstruction). These are observations, not absolute
time guarantees. Migration qualification compared both implementations in the same
run; the old implementation and its comparison harness have since been removed.

| Shots per loop leg | Original (ms) | cave-closure (ms) |
| --- | ---: | ---: |
| 1,500 | 3.62 | 3.58 |
| 3,000 | 6.69 | 6.91 |
| 6,000 | 14.98 | 14.89 |
| 12,000 | 31.63 | 32.84 |

The largest case was 3.8 percent slower, within the 25 percent ceiling, and retained
near-linear scaling. The migration comparison observed zero coordinate difference
on its synthetic network roster; the independent mathematical tests retain their
stricter tolerances alongside the 0.30 metre migration ceiling.

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

**`src/hooks/useFpsMonitor.ts` is not running.** It implements a rolling
`requestAnimationFrame` average with hysteresis thresholds, but it has no
consumer; the live FPS readout is `useMapDevStats`' own loop. Do not wire
adaptive work to it without wiring the hook up first.
