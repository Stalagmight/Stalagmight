//! The least-squares linear algebra of the Closures Computed View.
//!
//! Copied from `models/src/project/lsq.rs` (`Project::solve_reduced_laplacian`
//! and `Project::pcg_axis`, private there) unchanged apart from being free
//! functions and indexing the core with a vector instead of a `HashMap`
//! lookup (a lookup only; no iteration order reaches a result). The old copy
//! is deleted with `models::project`; see its doc comments for the history
//! (#81 exact peeling, #413 finding 21 determinism, Codex #86 the warm-start
//! residual reference).

use std::collections::{BTreeMap, VecDeque};

/// Solve the two SPD weighted-Laplacian systems `A x = atb_x` and
/// `A y = atb_y` sharing the sparsity `adj` (symmetric off-diagonal entries
/// `(j, off_x, off_y)`, the negative edge weights).
///
/// Exact Gaussian elimination of every node whose current off-diagonal degree
/// is at most 2 (leaf back-substitution and series reduction), carried for
/// both axes in lockstep; the surviving 2-core (about the cycle space) is
/// solved with Jacobi-PCG warm-started at `x0`, then the peeled nodes are
/// recovered in reverse order. The working adjacency is a `BTreeMap` per node,
/// so every float sum runs in node order and the solve is bit-reproducible.
pub(crate) fn solve_reduced_laplacian(
    n: usize,
    diag_x: &[f64],
    diag_y: &[f64],
    atb_x: &[f64],
    atb_y: &[f64],
    adj: &[Vec<(usize, f64, f64)>],
    x0_x: &[f64],
    x0_y: &[f64],
) -> (Vec<f64>, Vec<f64>) {
    // A node is only eliminated when its current pivot is safely positive
    // relative to its original diagonal; a collapsed pivot means a null-space
    // (floating) axis, left to the warm-started PCG.
    const PIVOT_REL: f64 = 1e-9;

    struct Elim {
        u: usize,
        nbrs: Vec<(usize, f64, f64)>,
        dxu: f64,
        dyu: f64,
        cxu: f64,
        cyu: f64,
    }

    let mut dx = diag_x.to_vec();
    let mut dy = diag_y.to_vec();
    let mut cx = atb_x.to_vec();
    let mut cy = atb_y.to_vec();
    let mut nbr: Vec<BTreeMap<usize, (f64, f64)>> = vec![BTreeMap::new(); n];
    for (i, row) in adj.iter().enumerate() {
        for &(j, ox, oy) in row {
            let e = nbr[i].entry(j).or_insert((0.0, 0.0));
            e.0 += ox;
            e.1 += oy;
        }
    }

    let mut eliminated = vec![false; n];
    let mut order: Vec<Elim> = Vec::new();
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| nbr[i].len() <= 2).collect();
    while let Some(u) = queue.pop_front() {
        if eliminated[u] || nbr[u].len() > 2 {
            continue;
        }
        let (pdx, pdy) = (dx[u], dy[u]);
        if pdx <= PIVOT_REL * diag_x[u] || pdy <= PIVOT_REL * diag_y[u] {
            continue;
        }
        let nbrs: Vec<(usize, f64, f64)> =
            nbr[u].iter().map(|(&v, &(ox, oy))| (v, ox, oy)).collect();
        let (pcx, pcy) = (cx[u], cy[u]);
        eliminated[u] = true;
        order.push(Elim {
            u,
            nbrs: nbrs.clone(),
            dxu: pdx,
            dyu: pdy,
            cxu: pcx,
            cyu: pcy,
        });
        for &(v, _, _) in &nbrs {
            nbr[v].remove(&u);
        }
        for &(v, ox, oy) in &nbrs {
            cx[v] -= ox * pcx / pdx;
            cy[v] -= oy * pcy / pdy;
            dx[v] -= ox * ox / pdx;
            dy[v] -= oy * oy / pdy;
        }
        for a in 0..nbrs.len() {
            for b in (a + 1)..nbrs.len() {
                let (i, oxa, oya) = nbrs[a];
                let (j, oxb, oyb) = nbrs[b];
                let fx = -(oxa * oxb) / pdx;
                let fy = -(oya * oyb) / pdy;
                let e = nbr[i].entry(j).or_insert((0.0, 0.0));
                e.0 += fx;
                e.1 += fy;
                let e = nbr[j].entry(i).or_insert((0.0, 0.0));
                e.0 += fx;
                e.1 += fy;
            }
        }
        for &(v, _, _) in &nbrs {
            if !eliminated[v] && nbr[v].len() <= 2 {
                queue.push_back(v);
            }
        }
    }

    let mut sol_x = vec![0.0; n];
    let mut sol_y = vec![0.0; n];
    let core: Vec<usize> = (0..n).filter(|&i| !eliminated[i]).collect();
    if !core.is_empty() {
        let k = core.len();
        let mut local = vec![usize::MAX; n];
        for (li, &g) in core.iter().enumerate() {
            local[g] = li;
        }
        let cdx: Vec<f64> = core.iter().map(|&g| dx[g]).collect();
        let cdy: Vec<f64> = core.iter().map(|&g| dy[g]).collect();
        let crx: Vec<f64> = core.iter().map(|&g| cx[g]).collect();
        let cry: Vec<f64> = core.iter().map(|&g| cy[g]).collect();
        let mut cadj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); k];
        for (li, &g) in core.iter().enumerate() {
            for (&v, &(ox, oy)) in &nbr[g] {
                if local[v] != usize::MAX {
                    cadj[li].push((local[v], ox, oy));
                }
            }
        }
        let cx0: Vec<f64> = core.iter().map(|&g| x0_x[g]).collect();
        let cy0: Vec<f64> = core.iter().map(|&g| x0_y[g]).collect();
        let csx = pcg_axis(k, &cdx, &crx, &cadj, 0, &cx0);
        let csy = pcg_axis(k, &cdy, &cry, &cadj, 1, &cy0);
        for (li, &g) in core.iter().enumerate() {
            sol_x[g] = csx[li];
            sol_y[g] = csy[li];
        }
    }

    for rec in order.iter().rev() {
        let mut sx = rec.cxu;
        let mut sy = rec.cyu;
        for &(v, ox, oy) in &rec.nbrs {
            sx -= ox * sol_x[v];
            sy -= oy * sol_y[v];
        }
        sol_x[rec.u] = sx / rec.dxu;
        sol_y[rec.u] = sy / rec.dyu;
    }
    (sol_x, sol_y)
}

/// Jacobi-preconditioned conjugate gradient for one axis (`axis` selects the
/// off-diagonal weight) of the small reduced core, warm-started at `x0` (a
/// null-space direction stays at its measured value), converging relative to
/// the initial residual `|b - A x0|`.
fn pcg_axis(
    n: usize,
    diag: &[f64],
    rhs: &[f64],
    adj: &[Vec<(usize, f64, f64)>],
    axis: usize,
    x0: &[f64],
) -> Vec<f64> {
    let off = |e: (usize, f64, f64)| if axis == 0 { e.1 } else { e.2 };
    let spmv = |x: &[f64]| -> Vec<f64> {
        let mut y = vec![0.0; n];
        for i in 0..n {
            y[i] += diag[i] * x[i];
            for &edge in &adj[i] {
                y[i] += off(edge) * x[edge.0];
            }
        }
        y
    };
    let max_iter = n.min(10_000);
    let tol = 1e-12;
    let inv_diag: Vec<f64> = diag
        .iter()
        .map(|&d| if d.abs() > 1e-30 { 1.0 / d } else { 1.0 })
        .collect();

    let mut x = x0.to_vec();
    let ax0 = spmv(&x);
    let mut r: Vec<f64> = rhs.iter().zip(ax0.iter()).map(|(&b, &a)| b - a).collect();
    let mut z: Vec<f64> = r
        .iter()
        .zip(inv_diag.iter())
        .map(|(&ri, &mi)| ri * mi)
        .collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(z.iter()).map(|(&ri, &zi)| ri * zi).sum();
    let r0_norm_sq: f64 = r.iter().map(|&v| v * v).sum();
    if r0_norm_sq < tol * tol {
        return x;
    }
    for _ in 0..max_iter {
        let ap = spmv(&p);
        let pap: f64 = p.iter().zip(ap.iter()).map(|(&pi, &api)| pi * api).sum();
        if pap.abs() < 1e-30 {
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
        let rz_new: f64 = r.iter().zip(z.iter()).map(|(&ri, &zi)| ri * zi).sum();
        let beta = rz_new / rz;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        rz = rz_new;
    }
    x
}
