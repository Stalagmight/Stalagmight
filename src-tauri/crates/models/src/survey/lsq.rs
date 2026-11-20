//! The least-squares linear algebra of the Closures Computed View.
//!
//! **The system.** Per axis, one grounded weighted Laplacian in the
//! corrections `d` the solve adds to its reference positions (each free
//! group's lowest member's raw position):
//!
//! ```text
//! sum_j w_ij (d_i - d_j) + g_i d_i = r_i
//! ```
//!
//! with `w_ij > 0` the summed weights of the shots between free groups `i`
//! and `j`, `g_i >= 0` the weights tying group `i` to held positions (shots
//! to held groups, soft GPS fixes) and `r_i` the misclosure currents: each
//! shot's weight times what its reference ends do not satisfy of its vector,
//! each soft fix's weight times its offset. The caller assembles `w`, `g` and
//! `r` from differences of nearby positions, never from absolute
//! coordinates.
//!
//! **Why this form (migration fuzz seed 34022454).** 0.6.0 (and this module
//! until then) solved for absolute coordinates, `A x = A^T W b`, with the
//! diagonal stored as a float sum and every elimination step subtracting
//! from it (`d_v -= w^2 / p_u`). Both lose what a survey that mixes shot
//! weights 1e14 apart needs: the right-hand side carries `eps x weight x
//! coordinate` of rounding (a 1e5 m coordinate under a 3e8 weight is 1e-2 of
//! spurious current), the diagonal's rounding is a leak to ground of `eps x
//! weight`, and a part of the survey hanging off one weak (100 km) shot turns
//! either into hundreds of metres: the seed's exact solution moves no
//! station more than 10.7 m, the absolute solve moved it 809 m. Here the
//! right-hand side is the misclosure (exact for a satisfied shot), the
//! operator is applied as differences `d_i - d_j` (a rigid shift of a part
//! is exactly no current), and elimination only adds positive terms (the
//! pivot `p_u = g_u + sum w_uv` is summed afresh, the neighbour's ground
//! grows by `w_uv g_u / p_u`, the fill-in is `w_ua w_ub / p_u`), so no step
//! cancels. In exact arithmetic it is the same minimiser.
//!
//! **The solve.** Exact Gaussian elimination of every node whose current
//! degree is at most 2 (leaf back-substitution and series reduction), both
//! axes in lockstep; the surviving 2-core (about the cycle space) is solved
//! with Jacobi-PCG from zero corrections (a null-space direction keeps its
//! reference position), then the peeled nodes are recovered in reverse
//! order. The working adjacency is a `BTreeMap` per node, so every float sum
//! runs in node order and the solve is bit-reproducible (#413 finding 21).
//! History of the peeling: #81.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// The largest current degree a node is eliminated at exactly; a node of a
/// higher degree is left to the core's conjugate gradient (it bounds the
/// fill-in of one step to `MAX_PEEL_DEGREE^2 / 2` edges).
const MAX_PEEL_DEGREE: usize = 8;

/// One exact elimination step: the node, its neighbours at that point with
/// their weight ratios `w_uv / p_u` per axis, and its pivots.
struct Step {
    u: usize,
    nbrs: Vec<(usize, f64, f64)>,
    px: f64,
    py: f64,
}

/// The two grounded weighted-Laplacian systems (module docs), factored: the
/// exact elimination steps in order and the core left to the conjugate
/// gradient. The right-hand side comes later ([`Self::solve`]), so the
/// solve's refinement passes reuse one factoring.
pub(crate) struct Grounded {
    n: usize,
    steps: Vec<Step>,
    /// The core's nodes, ascending.
    core: Vec<usize>,
    /// The core's edges, `(local j, w_x, w_y)`.
    core_adj: Vec<Vec<(usize, f64, f64)>>,
    core_ground: [Vec<f64>; 2],
}

impl Grounded {
    /// Factor the systems with ground weights `ground_x` / `ground_y` and
    /// the symmetric edges `adj` (`(j, w_x, w_y)`, positive weights, or zero
    /// on an axis the shot does not observe).
    pub(crate) fn factor(
        ground_x: &[f64],
        ground_y: &[f64],
        adj: &[Vec<(usize, f64, f64)>],
    ) -> Self {
        let n = ground_x.len();
        let mut gx = ground_x.to_vec();
        let mut gy = ground_y.to_vec();
        let mut nbr: Vec<BTreeMap<usize, (f64, f64)>> = vec![BTreeMap::new(); n];
        for (i, row) in adj.iter().enumerate() {
            for &(j, wx, wy) in row {
                let e = nbr[i].entry(j).or_insert((0.0, 0.0));
                e.0 += wx;
                e.1 += wy;
            }
        }

        let mut eliminated = vec![false; n];
        let mut steps: Vec<Step> = Vec::new();
        // Lowest current degree first, then lowest node: deterministic, and
        // the degree-1 and degree-2 nodes (spurs and chains) go before any
        // node whose elimination fills in.
        let mut heap: BinaryHeap<Reverse<(usize, usize)>> = (0..n)
            .filter(|&i| nbr[i].len() <= MAX_PEEL_DEGREE)
            .map(|i| Reverse((nbr[i].len(), i)))
            .collect();
        while let Some(Reverse((degree, u))) = heap.pop() {
            if eliminated[u] || nbr[u].len() != degree {
                continue;
            }
            let px = nbr[u].values().fold(gx[u], |s, w| s + w.0);
            let py = nbr[u].values().fold(gy[u], |s, w| s + w.1);
            // A zero pivot is an axis with nothing holding it (a floating
            // part): left to the core, where it keeps its reference.
            if !(px > 0.0 && px.is_finite() && py > 0.0 && py.is_finite()) {
                continue;
            }
            let weights: Vec<(usize, f64, f64)> =
                nbr[u].iter().map(|(&v, &(wx, wy))| (v, wx, wy)).collect();
            eliminated[u] = true;
            let (gux, guy) = (gx[u], gy[u]);
            let mut nbrs = Vec::with_capacity(weights.len());
            for &(v, wx, wy) in &weights {
                nbr[v].remove(&u);
                let (tx, ty) = (wx / px, wy / py);
                gx[v] += tx * gux;
                gy[v] += ty * guy;
                nbrs.push((v, tx, ty));
            }
            // Fill-in between each pair of neighbours: `w_ua w_ub / p_u`.
            for (k, &(a, tax, tay)) in nbrs.iter().enumerate() {
                for &(b, wbx, wby) in &weights[k + 1..] {
                    let (fx, fy) = (tax * wbx, tay * wby);
                    let e = nbr[a].entry(b).or_insert((0.0, 0.0));
                    e.0 += fx;
                    e.1 += fy;
                    let e = nbr[b].entry(a).or_insert((0.0, 0.0));
                    e.0 += fx;
                    e.1 += fy;
                }
            }
            for &(v, _, _) in &nbrs {
                if nbr[v].len() <= MAX_PEEL_DEGREE {
                    heap.push(Reverse((nbr[v].len(), v)));
                }
            }
            steps.push(Step { u, nbrs, px, py });
        }

        let core: Vec<usize> = (0..n).filter(|&i| !eliminated[i]).collect();
        let mut local = vec![usize::MAX; n];
        for (li, &g) in core.iter().enumerate() {
            local[g] = li;
        }
        let core_adj: Vec<Vec<(usize, f64, f64)>> = core
            .iter()
            .map(|&g| {
                nbr[g]
                    .iter()
                    .filter(|(&v, _)| local[v] != usize::MAX)
                    .map(|(&v, &(wx, wy))| (local[v], wx, wy))
                    .collect()
            })
            .collect();
        let core_ground = [
            core.iter().map(|&g| gx[g]).collect(),
            core.iter().map(|&g| gy[g]).collect(),
        ];
        Self {
            n,
            steps,
            core,
            core_adj,
            core_ground,
        }
    }

    /// The corrections `(d_x, d_y)` for the misclosure currents `rhs_x` /
    /// `rhs_y`.
    pub(crate) fn solve(&self, rhs_x: &[f64], rhs_y: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let mut rx = rhs_x.to_vec();
        let mut ry = rhs_y.to_vec();
        let mut at: Vec<(f64, f64)> = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let (rux, ruy) = (rx[step.u], ry[step.u]);
            at.push((rux, ruy));
            for &(v, tx, ty) in &step.nbrs {
                rx[v] += tx * rux;
                ry[v] += ty * ruy;
            }
        }
        let mut sol_x = vec![0.0; self.n];
        let mut sol_y = vec![0.0; self.n];
        if !self.core.is_empty() {
            let pick =
                |values: &[f64]| -> Vec<f64> { self.core.iter().map(|&g| values[g]).collect() };
            let csx = pcg_axis(&self.core_ground[0], &pick(&rx), &self.core_adj, 0);
            let csy = pcg_axis(&self.core_ground[1], &pick(&ry), &self.core_adj, 1);
            for (li, &g) in self.core.iter().enumerate() {
                sol_x[g] = csx[li];
                sol_y[g] = csy[li];
            }
        }
        for (step, &(rux, ruy)) in self.steps.iter().zip(&at).rev() {
            let mut sx = rux / step.px;
            let mut sy = ruy / step.py;
            for &(v, tx, ty) in &step.nbrs {
                sx += tx * sol_x[v];
                sy += ty * sol_y[v];
            }
            sol_x[step.u] = sx;
            sol_y[step.u] = sy;
        }
        (sol_x, sol_y)
    }
}

/// Jacobi-preconditioned conjugate gradient for one axis (`axis` selects the
/// weight) of the reduced core, from zero corrections (a null-space
/// direction keeps its reference position), converging relative to the
/// right-hand side. The operator is applied in difference form (module
/// docs).
fn pcg_axis(ground: &[f64], rhs: &[f64], adj: &[Vec<(usize, f64, f64)>], axis: usize) -> Vec<f64> {
    let n = rhs.len();
    let weight = |e: &(usize, f64, f64)| if axis == 0 { e.1 } else { e.2 };
    let apply = |x: &[f64]| -> Vec<f64> {
        (0..n)
            .map(|i| {
                adj[i]
                    .iter()
                    .fold(ground[i] * x[i], |s, e| s + weight(e) * (x[i] - x[e.0]))
            })
            .collect()
    };
    let max_iter = n.min(10_000);
    let tol = 1e-12;
    // A node nothing weighs on this axis keeps its reference position: its
    // equation (`0 = r`) is dropped.
    let inv_diag: Vec<f64> = (0..n)
        .map(|i| {
            let d = adj[i].iter().fold(ground[i], |s, e| s + weight(e));
            if d > 0.0 && d.is_finite() {
                1.0 / d
            } else {
                0.0
            }
        })
        .collect();

    let mut x = vec![0.0; n];
    let mut r: Vec<f64> = rhs
        .iter()
        .zip(&inv_diag)
        .map(|(&b, &m)| if m > 0.0 { b } else { 0.0 })
        .collect();
    let mut z: Vec<f64> = r.iter().zip(&inv_diag).map(|(&ri, &mi)| ri * mi).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(&ri, &zi)| ri * zi).sum();
    let r0_norm_sq: f64 = r.iter().map(|&v| v * v).sum();
    if r0_norm_sq == 0.0 {
        return x;
    }
    for _ in 0..max_iter {
        let ap = apply(&p);
        let pap: f64 = p.iter().zip(&ap).map(|(&pi, &api)| pi * api).sum();
        if !(pap > 0.0 && pap.is_finite()) {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        let r_norm_sq: f64 = r.iter().map(|&v| v * v).sum();
        if r_norm_sq < tol * tol * r0_norm_sq {
            break;
        }
        for i in 0..n {
            z[i] = r[i] * inv_diag[i];
        }
        let rz_new: f64 = r.iter().zip(&z).map(|(&ri, &zi)| ri * zi).sum();
        let beta = rz_new / rz;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        rz = rz_new;
    }
    x
}
