//! [#576 NF-W4U-3] The placement crossing check, split out of
//! `placement_suggest.rs`: the exact segment test, the A2 grid
//! (`CrossingPrefilter`) that hands each placed import segment only the
//! existing segments within a proven rounding margin, and the `CrossingIndex`
//! that skips, per section pair, what it can PROVE the segment test passes.

use super::{charge_work, SuggestionConfig, TracePoint};

/// Below this `|denominator|` [`segment_intersection_params`] calls two
/// segments parallel and reports no crossing. Shared with
/// [`crossing_prefilter_margin`], whose bound is derived from it.
const PARALLEL_DENOM_EPS: f64 = 1e-9;

pub(super) fn segment_intersection_params(
    a0: TracePoint,
    a1: TracePoint,
    b0: TracePoint,
    b1: TracePoint,
) -> Option<(f64, f64)> {
    const ENDPOINT_EPS: f64 = 1e-6;

    let ax = a1.x - a0.x;
    let ay = a1.y - a0.y;
    let bx = b1.x - b0.x;
    let by = b1.y - b0.y;
    let denom = ax * by - ay * bx;
    if denom.abs() < PARALLEL_DENOM_EPS {
        return None;
    }

    let cx = b0.x - a0.x;
    let cy = b0.y - a0.y;
    let t = (cx * by - cy * bx) / denom;
    let u = (cx * ay - cy * ax) / denom;

    if t > ENDPOINT_EPS && t < 1.0 - ENDPOINT_EPS && u > ENDPOINT_EPS && u < 1.0 - ENDPOINT_EPS {
        Some((t, u))
    } else {
        None
    }
}

pub(super) fn interpolate_depth(a: TracePoint, b: TracePoint, t: f64) -> f64 {
    a.depth + (b.depth - a.depth) * t
}

/// Does this (import, existing) segment pair cross with less than the minimum
/// vertical clearance? The exact test, unchanged from the exhaustive loop.
pub(super) fn pair_crosses_too_close(
    import_a: TracePoint,
    import_b: TracePoint,
    existing_a: TracePoint,
    existing_b: TracePoint,
    config: &SuggestionConfig,
) -> bool {
    let Some((import_t, existing_t)) =
        segment_intersection_params(import_a, import_b, existing_a, existing_b)
    else {
        return false;
    };
    let import_depth = interpolate_depth(import_a, import_b, import_t);
    let existing_depth = interpolate_depth(existing_a, existing_b, existing_t);
    (import_depth - existing_depth).abs() < config.min_crossing_depth_clearance
}

/// The largest `|dx|` or `|dy|` of any one segment of `trace`.
pub(super) fn max_segment_extent(trace: &[TracePoint]) -> f64 {
    trace.windows(2).fold(0.0, |acc: f64, w| {
        acc.max((w[1].x - w[0].x).abs())
            .max((w[1].y - w[0].y).abs())
    })
}

pub(super) fn trace_is_finite(trace: &[TracePoint]) -> bool {
    trace.iter().all(|p| p.x.is_finite() && p.y.is_finite())
}

/// [#576 A2] How far apart two segments' bounding boxes may be while
/// [`segment_intersection_params`] could STILL report them crossing, for
/// segments whose `|dx|`, `|dy|` are at most `max_extent`; `None` when no such
/// bound holds and every pair has to be tested.
///
/// Mathematically, segments with disjoint boxes cannot cross. The computed
/// test can, though, when the two are nearly parallel: `t = N / D` with
/// `|D| >= PARALLEL_DENOM_EPS`, and the rounding in `N` and `D` is relative to
/// `|c||b|` and `|a||b|`. So the prefilter skips a pair only when its boxes
/// are further apart than this margin, and the margin is a proven bound on
/// what rounding can bridge - which is what makes the filtered loop return
/// exactly what the exhaustive one did, bit for bit.
///
/// The bound. Let `L` bound every component of `a` and `b`, and let the boxes
/// be `G` apart along some axis. The true crossing point `P` is within
/// `dist(t, [0, 1]) * L` of `a`'s box and `dist(u, [0, 1]) * L` of `b`'s along
/// that axis, and those two distances add to at least `G`, so one of the TRUE
/// `t`, `u` is at least `G / (2L)` outside `[0, 1]`. The computed one lands
/// inside `(0, 1)` only if rounding moved it that far. With `gamma` bounding
/// the relative error of a product-difference (about `4u`; `16u` is used) and
/// `|c| <= 2L + G` for boxes that close, `|D_hat - D| / |D_hat| <= r =
/// 2 gamma L^2 / EPS` and `|N_hat - N| / |D_hat| <= gamma 2L (2L + G) / EPS`,
/// so `|t_hat - t| <= (s + r + u) / (1 - r)`. Requiring that to reach
/// `G / (2L)` gives `G <= (12 gamma L^3 / EPS + 2 L u) / (1 - 6 gamma L^2 /
/// EPS)`, valid while the denominator stays positive. The returned margin is
/// twice that, plus a micrometre for the rounding of the box arithmetic
/// itself, and the bound is only used while `6 gamma L^2 / EPS <= 1/2`
/// (segments up to ~216 m): past that, `None`.
pub(super) fn crossing_prefilter_margin(max_extent: f64) -> Option<f64> {
    const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0;
    const GAMMA: f64 = 16.0 * UNIT_ROUNDOFF;
    // A millimetre of slack: translating the import moves its endpoints by a
    // rounding, so a placed segment can be a hair longer than measured here.
    let l = max_extent + 1e-3;
    if !l.is_finite() {
        return None;
    }
    let shrink = 6.0 * GAMMA * l * l / PARALLEL_DENOM_EPS;
    if shrink > 0.5 {
        return None;
    }
    let bridgeable =
        (12.0 * GAMMA * l.powi(3) / PARALLEL_DENOM_EPS + 2.0 * l * UNIT_ROUNDOFF) / (1.0 - shrink);
    Some(2.0 * bridgeable + 1e-6)
}

/// `[min_x, max_x, min_y, max_y]` of a segment.
pub(super) fn segment_box(a: TracePoint, b: TracePoint) -> [f64; 4] {
    [a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y)]
}

/// [#576 A2] An existing section's trace segments bucketed into a uniform
/// grid, built once per (import, existing) section pair, so the crossing
/// check visits the existing segments NEAR each placed import segment instead
/// of all of them. The exhaustive loop tested every import segment against
/// every existing one for every surviving candidate and merge offset; on two
/// straight 2,000-shot sections that was 370 s.
pub(super) struct CrossingPrefilter {
    /// See [`crossing_prefilter_margin`].
    margin: f64,
    /// The longest segment extent the margin was derived for; sizes the cells.
    extent: f64,
    origin_x: f64,
    origin_y: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    /// CSR buckets: the segments of cell `(cx, cy)` are
    /// `entries[starts[cy * nx + cx]..starts[cy * nx + cx + 1]]`.
    starts: Vec<usize>,
    entries: Vec<usize>,
    boxes: Vec<[f64; 4]>,
}

impl CrossingPrefilter {
    /// `None` when the exact bound does not apply (non-finite coordinates, or
    /// segments too long for [`crossing_prefilter_margin`]): the caller then
    /// tests every pair.
    pub(super) fn build(
        import_trace: &[TracePoint],
        existing_trace: &[TracePoint],
    ) -> Option<Self> {
        if !trace_is_finite(import_trace) || !trace_is_finite(existing_trace) {
            return None;
        }
        let extent = max_segment_extent(import_trace).max(max_segment_extent(existing_trace));
        let margin = crossing_prefilter_margin(extent)?;
        let boxes: Vec<[f64; 4]> = existing_trace
            .windows(2)
            .map(|w| segment_box(w[0], w[1]))
            .collect();
        let every: Vec<usize> = (0..boxes.len()).collect();
        Self::over(margin, extent, boxes, &every)
    }

    /// [#576 NF-W4U-3] The same grid (same margin, same cell rule) holding
    /// only `segments` of the existing trace, ascending; `None` when empty.
    fn subset(&self, segments: &[usize]) -> Option<Self> {
        Self::over(self.margin, self.extent, self.boxes.clone(), segments)
    }

    /// The grid over `segments` (ascending indices into `boxes`).
    fn over(margin: f64, extent: f64, boxes: Vec<[f64; 4]>, segments: &[usize]) -> Option<Self> {
        let (origin_x, max_x, origin_y, max_y) = segments.iter().fold(
            (
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ),
            |(lx, hx, ly, hy), &k| {
                let b = &boxes[k];
                (lx.min(b[0]), hx.max(b[1]), ly.min(b[2]), hy.max(b[3]))
            },
        );
        if segments.is_empty() {
            return None;
        }
        let (width, height) = (max_x - origin_x, max_y - origin_y);
        // At least one segment's extent and both margins wide, so a query
        // spans a handful of cells; and no finer than ~4 cells per segment
        // over the trace's box, so the grid stays linear in the section.
        let cell = extent
            .max(2.0 * margin)
            .max(1.0)
            .max((width * height / (4.0 * segments.len() as f64)).sqrt());
        let nx = (width / cell).floor() as usize + 1;
        let ny = (height / cell).floor() as usize + 1;
        let mut grid = Self {
            margin,
            extent,
            origin_x,
            origin_y,
            cell,
            nx,
            ny,
            starts: vec![0; nx * ny + 1],
            entries: Vec::new(),
            boxes,
        };
        // Counting pass, then fill: two walks of the same cell ranges.
        for &segment in segments {
            let (x0, x1, y0, y1) = grid.cell_range(&grid.boxes[segment]);
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    grid.starts[cy * nx + cx + 1] += 1;
                }
            }
        }
        for i in 1..grid.starts.len() {
            grid.starts[i] += grid.starts[i - 1];
        }
        let mut fill = grid.starts.clone();
        grid.entries = vec![0; grid.starts[nx * ny]];
        for &segment in segments {
            let (x0, x1, y0, y1) = grid.cell_range(&grid.boxes[segment]);
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    let slot = &mut fill[cy * nx + cx];
                    grid.entries[*slot] = segment;
                    *slot += 1;
                }
            }
        }
        Some(grid)
    }

    /// The cell column of `x`, clamped into the grid.
    fn column(&self, x: f64) -> usize {
        (((x - self.origin_x) / self.cell).floor().max(0.0) as usize).min(self.nx - 1)
    }

    fn row(&self, y: f64) -> usize {
        (((y - self.origin_y) / self.cell).floor().max(0.0) as usize).min(self.ny - 1)
    }

    /// Inclusive cell ranges `(x0, x1, y0, y1)` a box covers, clamped.
    fn cell_range(&self, b: &[f64; 4]) -> (usize, usize, usize, usize) {
        (
            self.column(b[0]),
            self.column(b[1]),
            self.row(b[2]),
            self.row(b[3]),
        )
    }

    /// A placed segment's box grown by the margin: what a query covers.
    pub(super) fn query(&self, placed_box: [f64; 4]) -> [f64; 4] {
        [
            placed_box[0] - self.margin,
            placed_box[1] + self.margin,
            placed_box[2] - self.margin,
            placed_box[3] + self.margin,
        ]
    }

    /// Does `query` lie wholly outside the grid, so it can meet no segment?
    pub(super) fn misses(&self, query: &[f64; 4]) -> bool {
        let grid_hi_x = self.origin_x + self.cell * self.nx as f64;
        let grid_hi_y = self.origin_y + self.cell * self.ny as f64;
        query[1] < self.origin_x
            || query[0] > grid_hi_x
            || query[3] < self.origin_y
            || query[2] > grid_hi_y
    }

    /// Does any existing segment the prefilter cannot rule out cross the
    /// placed import segment `(a, b)` too closely? Every existing segment
    /// whose box comes within `margin` of the import segment's box is tested
    /// exactly once; every other one is provably one the exact test would
    /// pass (see [`crossing_prefilter_margin`]).
    fn segment_violates(
        &self,
        existing_trace: &[TracePoint],
        import_idx: usize,
        a: TracePoint,
        b: TracePoint,
        ignored_pairs: &[(usize, usize)],
        config: &SuggestionConfig,
    ) -> bool {
        let query = self.query(segment_box(a, b));
        if self.misses(&query) {
            return false;
        }
        let (x0, x1, y0, y1) = self.cell_range(&query);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let cell = cy * self.nx + cx;
                for &existing_idx in &self.entries[self.starts[cell]..self.starts[cell + 1]] {
                    charge_work(1);
                    let e = &self.boxes[existing_idx];
                    if e[0] > query[1] || e[1] < query[0] || e[2] > query[3] || e[3] < query[2] {
                        continue;
                    }
                    // A segment spanning several cells is met in each; test it
                    // only in the cell holding the low corner of the overlap,
                    // which lies inside both boxes and so in exactly one
                    // visited cell.
                    if self.column(query[0].max(e[0])) != cx || self.row(query[2].max(e[2])) != cy {
                        continue;
                    }
                    if ignored_pairs.contains(&(import_idx, existing_idx)) {
                        continue;
                    }
                    if pair_crosses_too_close(
                        a,
                        b,
                        existing_trace[existing_idx],
                        existing_trace[existing_idx + 1],
                        config,
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// [#576 NF-W4U-3] A stretch of consecutive import segments (`start..end`)
/// whose directions all lie within `radius` of `(cx, cy)`: a straight run.
pub(super) struct StraightRun {
    start: usize,
    end: usize,
    cx: f64,
    cy: f64,
    /// Bounds `|A - (cx, cy)|` for the EXACT direction `A = P1 - P0` of every
    /// segment in the run, not just the rounded one.
    radius: f64,
}

/// Runs shorter than this stay on the full grid: proving one parallel costs a
/// pass over the existing section, which pays only on long straight stretches.
const MIN_STRAIGHT_RUN: usize = 16;
/// At most this many runs (the longest) are proven per import section.
const MAX_STRAIGHT_RUNS: usize = 64;
/// A run's directions stay within `STRAIGHT_RUN_SPREAD / max(|c|, 1)` metres
/// of its first one - near the rounding noise of a dead-straight trace, far
/// below any surveyed bend. Only a grouping choice: [`run_parallel_to`] proves
/// each run with its own measured radius, so a looser value is never wrong,
/// just less often provable.
const STRAIGHT_RUN_SPREAD: f64 = 1e-10;
/// Placed import segments are tested in chunks of at most this many, so a
/// chunk whose box misses its lane's grid is skipped in one test.
const CROSSING_CHUNK: usize = 32;

/// The straight runs of an import trace worth proving parallel, in trace order.
pub(super) fn straight_runs(import_trace: &[TracePoint]) -> Vec<StraightRun> {
    const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0;
    let direction = |j: usize| {
        (
            import_trace[j + 1].x - import_trace[j].x,
            import_trace[j + 1].y - import_trace[j].y,
        )
    };
    let segments = import_trace.len().saturating_sub(1);
    let mut runs = Vec::new();
    let mut start = 0;
    while start < segments {
        let (cx, cy) = direction(start);
        let tolerance = STRAIGHT_RUN_SPREAD / cx.hypot(cy).max(1.0);
        let (mut spread, mut size) = (0.0_f64, cx.abs().max(cy.abs()));
        let mut end = start + 1;
        while end < segments {
            let (ax, ay) = direction(end);
            let off = (ax - cx).hypot(ay - cy);
            // A NaN direction ends the run too.
            if off.is_nan() || off > tolerance {
                break;
            }
            spread = spread.max(off);
            size = size.max(ax.abs()).max(ay.abs());
            end += 1;
        }
        if end - start >= MIN_STRAIGHT_RUN && size.is_finite() {
            runs.push(StraightRun {
                start,
                end,
                cx,
                cy,
                // The measured spread, plus the rounding of each `P1 - P0` and
                // of measuring it.
                radius: spread * (1.0 + 1e-6) + 4.0 * UNIT_ROUNDOFF * size,
            });
        }
        start = end;
    }
    if runs.len() > MAX_STRAIGHT_RUNS {
        runs.sort_by_key(|r| (std::cmp::Reverse(r.end - r.start), r.start));
        runs.truncate(MAX_STRAIGHT_RUNS);
        runs.sort_by_key(|r| r.start);
    }
    runs
}

/// [#576 NF-W4U-3] Is every segment of `run` provably PARALLEL to the
/// existing segment with direction `(bx, by)` (computed as
/// [`segment_intersection_params`] computes it), for every translation the
/// crossing check can apply? Then that test returns `None` for each such pair,
/// so the pair never violates the clearance, whichever candidate is checked.
///
/// The bound. A placed import segment's endpoints are `fl(P + d)`, and the
/// segment test computes `a = fl(fl(P1 + d) - fl(P0 + d))` and
/// `D = fl(fl(ax by) - fl(ay bx))`, calling the pair parallel when `|D| <
/// PARALLEL_DENOM_EPS`. With `u` the unit roundoff and `X` a bound on every
/// `|P + d|` coordinate: `|ax - Ax| <= ex = 3u (X + |Ax|)` for the exact
/// direction `A = P1 - P0` (two rounded sums, one rounded difference), and
/// likewise `ey`; `|D| <= (1 + u) (|a x b| + u |ax| |by| + u |ay| |bx|)`; `|a x
/// b| <= |A x b| + ex |by| + ey |bx|`; and with `|A - c| <= r`, `|A x b| <=
/// |c x b| + r |b|` and `|Ax| <= |cx| + r`. The sum of those terms, plus the
/// rounding of computing `c x b`, is evaluated below; every other rounding in
/// it is relative and well inside the `1 + 1e-6` factor, and comparing against
/// HALF the threshold leaves the rest. `X`: `d = fl(E - I)` for stations of the
/// two traces, so `|P + d| <= 2 max|I| + max|E|`, inflated the same way.
fn run_parallel_to(run: &StraightRun, bx: f64, by: f64, coord_bound: f64) -> bool {
    const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0;
    let u = UNIT_ROUNDOFF;
    let (abx, aby) = (bx.abs(), by.abs());
    let (acx, acy, r) = (run.cx.abs(), run.cy.abs(), run.radius);
    let cross = (run.cx * by - run.cy * bx).abs() + 3.0 * u * (acx * aby + acy * abx);
    let ex = 3.0 * u * (coord_bound + acx + r);
    let ey = 3.0 * u * (coord_bound + acy + r);
    let bound = cross
        + r * bx.hypot(by)
        + ex * aby
        + ey * abx
        + u * ((acx + r + ex) * aby + (acy + r + ey) * abx);
    // Written so a NaN bound proves nothing.
    bound * (1.0 + 1e-6) < 0.5 * PARALLEL_DENOM_EPS
}

/// Where a chunk of placed import segments is tested.
pub(super) enum CrossingLane {
    /// Walk this grid (every existing segment, or only those a straight run
    /// is not proven parallel to).
    Grid(CrossingPrefilter),
    /// The grid's bound does not hold: test these existing segments one by
    /// one (ascending).
    Every(Vec<usize>),
}

pub(super) struct CrossingChunk {
    /// Import segments `start..end`.
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) lane: usize,
    /// `[min_x, max_x, min_y, max_y]` of the chunk's unplaced points.
    pub(super) points_box: [f64; 4],
}

/// [#576 NF-W4U-3] What the crossing check walks for one (import, existing)
/// section pair, built once per pair. It returns exactly what testing every
/// placed import segment against the full grid returned (and so, by A2, what
/// the exhaustive loop did): the verdict is "no import segment violates", and
/// only segments that provably cannot violate are left out.
///
/// - A straight run proven parallel to EVERY existing segment
///   ([`run_parallel_to`]) has no chunk at all: each of its pairs is parallel
///   to the segment test, whatever the translation.
/// - A run proven parallel to SOME existing segments walks a lane grid built
///   over the others only - the same margin, the same cell rule, the same
///   per-entry test - so every pair the lane leaves out is either proven
///   parallel or, by A2, outside the margin.
/// - A chunk is skipped when its placed points' box, grown by the margin,
///   misses its lane's grid. Rounding is monotone, so `fl(min + d) <=
///   fl(p + d) <= fl(max + d)` for each point, and that box contains every
///   member segment's query box: each `segment_violates` would have returned
///   `false` at its first test.
///
/// This is what removes the collinear worst case, where every start index
/// survives the matcher and each candidate used to walk the whole placed
/// import through the grid.
pub(super) struct CrossingIndex {
    pub(super) lanes: Vec<CrossingLane>,
    pub(super) chunks: Vec<CrossingChunk>,
}

impl CrossingIndex {
    pub(super) fn build(
        import_trace: &[TracePoint],
        existing_trace: &[TracePoint],
        runs: &[StraightRun],
    ) -> Self {
        let existing_segments = existing_trace.len().saturating_sub(1);
        let import_segments = import_trace.len().saturating_sub(1);
        let full = CrossingPrefilter::build(import_trace, existing_trace);
        // Lane per import segment (0 = the full grid); `None` = proven
        // parallel to every existing segment.
        let mut lane_of: Vec<Option<usize>> = vec![Some(0); import_segments];
        let mut sparse: Vec<CrossingLane> = Vec::new();
        if !runs.is_empty() && trace_is_finite(import_trace) && trace_is_finite(existing_trace) {
            let max_abs = |t: &[TracePoint]| {
                t.iter()
                    .fold(0.0_f64, |m, p| m.max(p.x.abs()).max(p.y.abs()))
            };
            let coord_bound =
                (2.0 * max_abs(import_trace) + max_abs(existing_trace)) * (1.0 + 1e-6);
            let directions: Vec<(f64, f64)> = existing_trace
                .windows(2)
                .map(|w| (w[1].x - w[0].x, w[1].y - w[0].y))
                .collect();
            for run in runs {
                // The existing segments this run is NOT proven parallel to; a
                // lane over more than half of them is not worth building.
                let mut open = Vec::new();
                for (k, &(bx, by)) in directions.iter().enumerate() {
                    if !run_parallel_to(run, bx, by, coord_bound) {
                        open.push(k);
                        if 2 * open.len() > existing_segments {
                            break;
                        }
                    }
                }
                let lane = if open.is_empty() {
                    None
                } else if 2 * open.len() > existing_segments {
                    Some(0)
                } else {
                    sparse.push(match &full {
                        Some(grid) => match grid.subset(&open) {
                            Some(lane) => CrossingLane::Grid(lane),
                            None => CrossingLane::Every(open),
                        },
                        None => CrossingLane::Every(open),
                    });
                    Some(sparse.len())
                };
                lane_of[run.start..run.end].fill(lane);
            }
        }
        let mut lanes = Vec::with_capacity(1 + sparse.len());
        lanes.push(match full {
            Some(grid) => CrossingLane::Grid(grid),
            None => CrossingLane::Every((0..existing_segments).collect()),
        });
        lanes.extend(sparse);

        let mut chunks = Vec::new();
        let mut start = 0;
        while start < import_segments {
            let lane = lane_of[start];
            let mut end = start + 1;
            while end < import_segments
                && lane_of[end] == lane
                && (lane.is_none() || end - start < CROSSING_CHUNK)
            {
                end += 1;
            }
            if let Some(lane) = lane {
                let points_box = import_trace[start..=end].iter().fold(
                    [
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                    ],
                    |b, p| [b[0].min(p.x), b[1].max(p.x), b[2].min(p.y), b[3].max(p.y)],
                );
                chunks.push(CrossingChunk {
                    start,
                    end,
                    lane,
                    points_box,
                });
            }
            start = end;
        }
        Self { lanes, chunks }
    }
}

pub(super) fn placement_crossing_clearance_ok(
    import_trace: &[TracePoint],
    existing_trace: &[TracePoint],
    index: &CrossingIndex,
    import_station_index: usize,
    existing_station_index: usize,
    ignored_pairs: &[(usize, usize)],
    config: &SuggestionConfig,
) -> bool {
    if import_station_index >= import_trace.len() || existing_station_index >= existing_trace.len()
    {
        return false;
    }

    let dx = existing_trace[existing_station_index].x - import_trace[import_station_index].x;
    let dy = existing_trace[existing_station_index].y - import_trace[import_station_index].y;

    // The placed import is translated on the fly - the same `translated`
    // arithmetic, so the same coordinates - rather than copied per call.
    for chunk in &index.chunks {
        let lane = &index.lanes[chunk.lane];
        if let CrossingLane::Grid(grid) = lane {
            let b = chunk.points_box;
            if grid.misses(&grid.query([b[0] + dx, b[1] + dx, b[2] + dy, b[3] + dy])) {
                continue;
            }
        }
        for import_idx in chunk.start..chunk.end {
            let import_a = import_trace[import_idx].translated(dx, dy);
            let import_b = import_trace[import_idx + 1].translated(dx, dy);
            let violates = match lane {
                CrossingLane::Grid(grid) => grid.segment_violates(
                    existing_trace,
                    import_idx,
                    import_a,
                    import_b,
                    ignored_pairs,
                    config,
                ),
                CrossingLane::Every(segments) => segments.iter().any(|&existing_idx| {
                    charge_work(1);
                    !ignored_pairs.contains(&(import_idx, existing_idx))
                        && pair_crosses_too_close(
                            import_a,
                            import_b,
                            existing_trace[existing_idx],
                            existing_trace[existing_idx + 1],
                            config,
                        )
                }),
            };
            if violates {
                return false;
            }
        }
    }

    true
}
