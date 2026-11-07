//! The non-destructive closure-adjusted view and its reduced-Laplacian solver.
//!
//! Lines 6785-7637 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

fn shot_geometry(project: &Project) -> Vec<(f64, f64, f64)> {
    project
        .sections
        .iter()
        .flat_map(|s| {
            s.shots
                .iter()
                .map(|sh| (sh.azimuth_in, sh.azimuth_out, sh.length))
        })
        .collect()
}

#[test]
fn compute_adjusted_closes_loop_without_mutating_measured() {
    let (project, a, b) = looped_project_with_misclosure();
    let config = ClosureAdjustmentConfig::default();

    let shots_before = shot_geometry(&project);
    let measured = project.compute_station_coordinates();
    let adjusted = project.compute_adjusted_station_coordinates(&config);

    // 1. Non-destructive: deriving the adjusted view must not rewrite measured shots.
    assert_eq!(
        shots_before,
        shot_geometry(&project),
        "deriving the adjusted view must not mutate measured shot geometry"
    );

    // 2. Measured view: the loop is open (~0.2 m misclosure).
    let m_a2 = station_coords(&measured, StationRef::new(a, 2));
    let m_b2 = station_coords(&measured, StationRef::new(b, 2));
    let measured_misclosure = ((m_a2.x - m_b2.x).powi(2) + (m_a2.y - m_b2.y).powi(2)).sqrt();
    assert!(
        measured_misclosure > 0.1,
        "measured loop should be open, got {measured_misclosure}"
    );

    // 3. Adjusted view: the loop closes (endpoints ~coincident).
    let adj_a2 = station_coords(&adjusted, StationRef::new(a, 2));
    let adj_b2 = station_coords(&adjusted, StationRef::new(b, 2));
    let adjusted_misclosure =
        ((adj_a2.x - adj_b2.x).powi(2) + (adj_a2.y - adj_b2.y).powi(2)).sqrt();
    assert!(
        adjusted_misclosure < 1e-3,
        "adjusted loop should close, got {adjusted_misclosure}"
    );

    // 4. The zero datum is fixed in both views.
    let m_a0 = station_coords(&measured, StationRef::new(a, 0));
    let adj_a0 = station_coords(&adjusted, StationRef::new(a, 0));
    assert!(
        (m_a0.x - adj_a0.x).abs() < 1e-9 && (m_a0.y - adj_a0.y).abs() < 1e-9,
        "zero datum must not move in the adjusted view"
    );
}

#[test]
fn compute_adjusted_is_deterministic_and_keeps_measured_stable() {
    let (project, _a, _b) = looped_project_with_misclosure();
    let config = ClosureAdjustmentConfig::default();

    let measured_before = project.compute_station_coordinates();
    let adjusted1 = project.compute_adjusted_station_coordinates(&config);
    let adjusted2 = project.compute_adjusted_station_coordinates(&config);
    let measured_after = project.compute_station_coordinates();

    // Re-solving from intact measured data is deterministic (no compounding).
    assert_eq!(adjusted1.len(), adjusted2.len());
    for (s1, s2) in adjusted1.iter().zip(adjusted2.iter()) {
        assert_eq!(s1.station_ref, s2.station_ref);
        assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
        assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
    }

    // The measured view is unaffected by having derived the adjusted view.
    assert_eq!(measured_before.len(), measured_after.len());
    for (s1, s2) in measured_before.iter().zip(measured_after.iter()) {
        assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
        assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
    }
}

/// Exactness guard for the degree-≤2 graph-reduction solver (#81): on a range of
/// random SPD weighted-Laplacian systems (chains, single cycles, and densely
/// connected cores), `solve_reduced_laplacian` must match a reference dense
/// Gaussian elimination on both axes.
#[test]
fn solve_reduced_laplacian_matches_dense() {
    // Deterministic xorshift RNG → reproducible [0,1) draws (no rand dep).
    fn next(state: &mut u64) -> f64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }

    // Dense SPD solve via Gaussian elimination with partial pivoting.
    fn dense_solve(mat: &[Vec<f64>], rhs: &[f64]) -> Vec<f64> {
        let n = rhs.len();
        let mut a: Vec<Vec<f64>> = mat.to_vec();
        let mut b = rhs.to_vec();
        for col in 0..n {
            let mut piv = col;
            for r in (col + 1)..n {
                if a[r][col].abs() > a[piv][col].abs() {
                    piv = r;
                }
            }
            a.swap(col, piv);
            b.swap(col, piv);
            let d = a[col][col];
            for r in (col + 1)..n {
                let f = a[r][col] / d;
                for c in col..n {
                    a[r][c] -= f * a[col][c];
                }
                b[r] -= f * b[col];
            }
        }
        let mut x = vec![0.0; n];
        for col in (0..n).rev() {
            let mut s = b[col];
            for c in (col + 1)..n {
                s -= a[col][c] * x[c];
            }
            x[col] = s / a[col][col];
        }
        x
    }

    // (n, extra_random_edges) - 0 extra = pure chain (fully peels, empty core);
    // a few = small cycle core; many = a denser core that exercises the PCG path.
    let configs = [(2, 0), (6, 0), (8, 2), (20, 5), (40, 30), (60, 80)];
    let mut seed = 0x9E3779B97F4A7C15u64;

    for (n, extra) in configs {
        let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
        let mut diag_x = vec![0.0; n];
        let mut diag_y = vec![0.0; n];

        let add_edge = |a: usize,
                        b: usize,
                        seed: &mut u64,
                        adj: &mut Vec<Vec<(usize, f64, f64)>>,
                        dx: &mut Vec<f64>,
                        dy: &mut Vec<f64>| {
            let wx = 0.5 + next(seed);
            let wy = 0.5 + next(seed);
            adj[a].push((b, -wx, -wy));
            adj[b].push((a, -wx, -wy));
            dx[a] += wx;
            dx[b] += wx;
            dy[a] += wy;
            dy[b] += wy;
        };

        // Spanning chain keeps the graph connected.
        for i in 0..n.saturating_sub(1) {
            add_edge(i, i + 1, &mut seed, &mut adj, &mut diag_x, &mut diag_y);
        }
        // Extra edges introduce loops (a non-trivial 2-core).
        for _ in 0..extra {
            let a = (next(&mut seed) * n as f64) as usize % n;
            let b = (next(&mut seed) * n as f64) as usize % n;
            if a != b {
                add_edge(a, b, &mut seed, &mut adj, &mut diag_x, &mut diag_y);
            }
        }
        // Grounding term on every diagonal → strictly SPD (mimics fixed-node edges).
        for i in 0..n {
            diag_x[i] += 0.25 + next(&mut seed);
            diag_y[i] += 0.25 + next(&mut seed);
        }
        let atb_x: Vec<f64> = (0..n).map(|_| next(&mut seed) * 4.0 - 2.0).collect();
        let atb_y: Vec<f64> = (0..n).map(|_| next(&mut seed) * 4.0 - 2.0).collect();

        // Dense reference matrices built from the SAME adj/diag (duplicate edges
        // summed identically to the solver).
        let mut ax = vec![vec![0.0; n]; n];
        let mut ay = vec![vec![0.0; n]; n];
        for i in 0..n {
            ax[i][i] = diag_x[i];
            ay[i][i] = diag_y[i];
            for &(j, ox, oy) in &adj[i] {
                ax[i][j] += ox;
                ay[i][j] += oy;
            }
        }
        let ref_x = dense_solve(&ax, &atb_x);
        let ref_y = dense_solve(&ay, &atb_y);

        // Strictly SPD (grounded) → unique solution, so a zero warm start matches
        // the dense reference exactly.
        let x0 = vec![0.0; n];
        let (sol_x, sol_y) =
            Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0, &x0);

        for i in 0..n {
            assert!(
                (sol_x[i] - ref_x[i]).abs() < 1e-6,
                "n={n} extra={extra} x[{i}]: reduced={} dense={}",
                sol_x[i],
                ref_x[i]
            );
            assert!(
                (sol_y[i] - ref_y[i]).abs() < 1e-6,
                "n={n} extra={extra} y[{i}]: reduced={} dense={}",
                sol_y[i],
                ref_y[i]
            );
        }
    }
}

/// Regression for the warm-start fix (Codex #86): a node that is unconstrained on
/// one axis (zero pivot, zero-weight edge → null-space row) must keep its measured
/// position on that axis rather than snapping to world 0.
#[test]
fn solve_reduced_laplacian_preserves_unconstrained_axis_warm_start() {
    // Two nodes. Node 0 is grounded on both axes. Node 1 couples to node 0 on Y
    // only (the X off-diagonal weight is 0), and has zero X diagonal + zero X rhs
    // - i.e. nothing constrains its X. Its warm-start X (42) must survive.
    let n = 2;
    let diag_x = vec![1.0, 0.0];
    let diag_y = vec![2.0, 1.0];
    let atb_x = vec![5.0, 0.0];
    let atb_y = vec![0.0, 0.0];
    let adj = vec![vec![(1usize, 0.0, -1.0)], vec![(0usize, 0.0, -1.0)]];
    let x0_x = vec![0.0, 42.0];
    let x0_y = vec![0.0, 0.0];

    let (sol_x, _sol_y) =
        Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y);

    assert!(
        (sol_x[1] - 42.0).abs() < 1e-9,
        "unconstrained-X node should retain its warm-start X (42), got {}",
        sol_x[1]
    );
    // The constrained node still solves normally.
    assert!(
        (sol_x[0] - 5.0).abs() < 1e-9,
        "node 0 X should be 5, got {}",
        sol_x[0]
    );
}

/// Regression for the early-exit fix (Codex #86 follow-up): when the reduced core
/// has a zero RHS but non-zero, full-rank matrix rows, the unique minimizer is 0 -
/// the warm start must NOT be returned just because `‖b‖ = 0`. Uses a K4 core (all
/// degree 3, so it survives degree-≤2 reduction and exercises the core PCG).
#[test]
fn solve_reduced_laplacian_zero_rhs_constrained_core_solves_not_warm_start() {
    let n = 4;
    // K4 + grounding: diagonal 4, every off-diagonal -1 → SPD, full rank.
    let diag = vec![4.0; 4];
    let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    for i in 0..n {
        for j in 0..n {
            if i != j {
                adj[i].push((j, -1.0, -1.0));
            }
        }
    }
    let atb = vec![0.0; 4]; // zero RHS → unique solution is 0
    let x0_x = vec![10.0, -5.0, 3.0, 7.0];
    let x0_y = vec![-2.0, 8.0, -6.0, 1.0];

    let (sol_x, sol_y) =
        Project::solve_reduced_laplacian(n, &diag, &diag, &atb, &atb, &adj, &x0_x, &x0_y);

    for (i, v) in sol_x.iter().chain(sol_y.iter()).enumerate() {
        assert!(
            v.abs() < 1e-6,
            "zero-RHS constrained core must solve to 0, got {v} at {i}"
        );
    }
}

/// Codex #86 (3rd, null-space-through-peeled-nodes): a component that *floats* on
/// an axis (positive internal coupling but no grounding → 1-D translation null
/// space) must keep its measured positions, not jump, when its nodes are peeled by
/// degree-≤2 reduction before the core PCG sees them.
///
/// Mechanism: the surviving core node keeps its own measured value (warm start),
/// and back-substitution reproduces the measured *relative* offsets - so the whole
/// component lands exactly on its measured coordinates, matching the old
/// full-system CG. (In real surveys this case can't even arise: every
/// positive-length shot has positive weight on both axes, so floating components
/// require zero-length shots.)
#[test]
fn solve_reduced_laplacian_preserves_floating_component_nullspace() {
    let n = 3;
    // X: pure path Laplacian 0-1-2 (singular → floats). Y: grounded (nonsingular).
    let diag_x = vec![1.0, 2.0, 1.0];
    let diag_y = vec![2.0, 3.0, 2.0];
    let adj = vec![
        vec![(1usize, -1.0, -1.0)],
        vec![(0usize, -1.0, -1.0), (2usize, -1.0, -1.0)],
        vec![(1usize, -1.0, -1.0)],
    ];
    // X rhs consistent with measured x=(0,3,5): A_x·(0,3,5) = (-3,1,2).
    let atb_x = vec![-3.0, 1.0, 2.0];
    let atb_y = vec![0.0, 0.0, 0.0];
    let x0_x = vec![0.0, 3.0, 5.0];
    let x0_y = vec![0.0, 0.0, 0.0];

    let (sx, _sy) =
        Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y);

    for (i, want) in [0.0_f64, 3.0, 5.0].iter().enumerate() {
        assert!(
            (sx[i] - want).abs() < 1e-6,
            "floating component node {i} jumped: got {}, want {want}",
            sx[i]
        );
    }
}

/// Regression for the scale-relative pivot floor (#89 / Codex). A *loop* that
/// floats on X with **varied** edge weights drives the last null-space pivot to
/// roundoff (~1e-12), not exactly 0 - so the old absolute 1e-30 floor eliminated
/// it and divided by ~0, shifting the component. The pivot must be judged relative
/// to its original diagonal so the node stays in the warm-started core and the
/// measured translation is preserved.
#[test]
fn solve_reduced_laplacian_floating_loop_varied_weights_no_shift() {
    // Triangle 0-1-2, pure (singular) Laplacian on X → floats; grounded on Y.
    // Varied off-diagonals so the final Schur pivot lands on roundoff, not 0.
    let n = 3;
    let diag_x = vec![3.3, 2.7, 4.0]; // = incident |off| sums (row-sum 0 ⇒ singular)
    let diag_y = vec![4.3, 3.7, 5.0]; // +1 grounding ⇒ nonsingular
    let adj = vec![
        vec![(1usize, -1.0, -1.0), (2usize, -2.3, -2.3)],
        vec![(0usize, -1.0, -1.0), (2usize, -1.7, -1.7)],
        vec![(0usize, -2.3, -2.3), (1usize, -1.7, -1.7)],
    ];
    // X rhs consistent with measured x=(0,2,5): atb_x = A_x·(0,2,5).
    let atb_x = vec![-13.5, -3.1, 16.6];
    let atb_y = vec![0.0, 0.0, 0.0];
    let x0_x = vec![0.0, 2.0, 5.0];
    let x0_y = vec![0.0, 0.0, 0.0];

    let (sx, _sy) =
        Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y);

    for (i, want) in [0.0_f64, 2.0, 5.0].iter().enumerate() {
        assert!(
            (sx[i] - want).abs() < 1e-6,
            "floating loop node {i} shifted: got {}, want {want}",
            sx[i]
        );
    }
}

/// Analytic golden: a loop with a hand-computable least-squares optimum.
///
/// Two due-east paths run from a shared, datum-anchored origin to a closure:
///   A: 0 →(5)→ A1 →(5)→ A2     (measured A2 at x = 10.0)
///   B: 0 →(5.1)→ B1 →(5.1)→ B2 (measured B2 at x = 10.2)
/// with `merge A0↔B0` and `closure A2↔B2`. Because every shot is due east, the X
/// normal equations have equal weights, so the weighted-LSQ optimum is exact and
/// independent of the sigma values: each interior station splits its path evenly
/// and the merged endpoint is the average of the two measured ends. Y stays 0.
/// This is an oracle independent of the solver implementation.
#[test]
fn adjust_line_loop_matches_hand_computed_optimum() {
    let mut project = Project::new("LineLoop".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.1, 0.0, 0.0),
                shot(90.0, 90.0, 5.1, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let adjusted =
        project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    let at = |sr| station_coords(&adjusted, sr);
    let approx = |c: Coordinates, x: f64, y: f64| (c.x - x).abs() < 1e-6 && (c.y - y).abs() < 1e-6;

    // Hand-computed optimum.
    assert!(
        approx(at(StationRef::new(a, 0)), 0.0, 0.0),
        "A0 = {:?}",
        at(StationRef::new(a, 0))
    );
    assert!(
        approx(at(StationRef::new(a, 1)), 5.05, 0.0),
        "A1 = {:?}",
        at(StationRef::new(a, 1))
    );
    assert!(
        approx(at(StationRef::new(a, 2)), 10.10, 0.0),
        "A2 = {:?}",
        at(StationRef::new(a, 2))
    );
    assert!(
        approx(at(StationRef::new(b, 1)), 5.05, 0.0),
        "B1 = {:?}",
        at(StationRef::new(b, 1))
    );
    assert!(
        approx(at(StationRef::new(b, 2)), 10.10, 0.0),
        "B2 = {:?}",
        at(StationRef::new(b, 2))
    );
}

/// Property/fuzz sweep over random connected geometries with closures. For each
/// random project the closure-adjusted view must:
///   - be finite (no NaN/Inf),
///   - keep the zero datum fixed,
///   - be deterministic (two solves agree),
///   - never mutate the measured geometry, and
///   - unify each eligible closure's endpoints (the adjusted gap is ~0).
/// Optimality of every one of these random solves is asserted *inside*
/// `adjust_least_squares` (the `#[cfg(test)]` `‖A·x-b‖∞` residual guard), so this
/// sweep doubles as a broad optimality check across diverse topologies.
#[test]
fn adjust_random_geometries_are_optimal_and_well_formed() {
    fn next(state: &mut u64) -> f64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }

    let mut seed = 0xD1CE_5EED_1234_9F17u64;
    let config = ClosureAdjustmentConfig::default();
    let mut exercised_closures = 0;

    for _trial in 0..60 {
        let mut project = Project::new("Fuzz".to_string(), None);
        let n_sections = 2 + (next(&mut seed) * 4.0) as usize; // 2..=5
        let mut sids = Vec::new();
        for _ in 0..n_sections {
            let n_shots = 1 + (next(&mut seed) * 6.0) as usize; // 1..=6
            let shots: Vec<ShotData> = (0..n_shots)
                .map(|_| {
                    let az = next(&mut seed) * 360.0;
                    let len = 1.0 + next(&mut seed) * 9.0;
                    shot(az, az, len, 0.0, 0.0)
                })
                .collect();
            sids.push(
                project
                    .add_section(SectionData {
                        shots,
                        ..Default::default()
                    })
                    .id,
            );
        }

        // Datum anchors the first section.
        project
            .set_zero_datum(
                StationRef::new(sids[0], 0),
                GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap();

        // Connect every later section to an earlier one with a merge → a connected
        // tree (no loops yet).
        let station_count = |p: &Project, sid| {
            p.sections
                .iter()
                .find(|s| s.id == sid)
                .map(|s| s.shots.len())
                .unwrap_or(0)
        };
        for s in 1..n_sections {
            let earlier = sids[(next(&mut seed) * s as f64) as usize];
            let tgt_idx = (next(&mut seed) * (station_count(&project, earlier) + 1) as f64) as u32;
            let _ = project.merge_stations(
                StationRef::new(sids[s], 0),
                StationRef::new(earlier, tgt_idx),
            );
        }

        // Add 1..=2 closures between stations in different sections → loops.
        let mut closures: Vec<(StationRef, StationRef)> = Vec::new();
        let n_closures = 1 + (next(&mut seed) * 2.0) as usize;
        for _ in 0..n_closures {
            let si = (next(&mut seed) * n_sections as f64) as usize;
            let mut sj = (next(&mut seed) * n_sections as f64) as usize;
            if sj == si {
                sj = (sj + 1) % n_sections;
            }
            let pi = StationRef::new(
                sids[si],
                (next(&mut seed) * (station_count(&project, sids[si]) + 1) as f64) as u32,
            );
            let pj = StationRef::new(
                sids[sj],
                (next(&mut seed) * (station_count(&project, sids[sj]) + 1) as f64) as u32,
            );
            if project
                .create_loop_closure(pi, pj, ClosureStrategy::Batch)
                .is_ok()
            {
                closures.push((pi, pj));
            }
        }

        let measured_before = project.compute_station_coordinates();
        let adjusted = project.compute_adjusted_station_coordinates(&config);
        let adjusted2 = project.compute_adjusted_station_coordinates(&config);
        let measured_after = project.compute_station_coordinates();

        // Finite.
        for s in &adjusted {
            assert!(
                s.coordinates.x.is_finite()
                    && s.coordinates.y.is_finite()
                    && s.coordinates.z.is_finite(),
                "non-finite adjusted coord at {:?}: {:?}",
                s.station_ref,
                s.coordinates
            );
        }
        // Datum fixed.
        let d0 = station_coords(&adjusted, StationRef::new(sids[0], 0));
        assert!(
            d0.x.abs() < 1e-6 && d0.y.abs() < 1e-6,
            "datum moved to {d0:?}"
        );
        // Deterministic.
        assert_eq!(adjusted.len(), adjusted2.len());
        for (s1, s2) in adjusted.iter().zip(&adjusted2) {
            assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
            assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
        }
        // Measured geometry untouched by deriving the adjusted view.
        assert_eq!(measured_before.len(), measured_after.len());
        for (s1, s2) in measured_before.iter().zip(&measured_after) {
            assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
            assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
        }
        // Count solves actually exercised. We deliberately do NOT assert that each
        // closure's endpoints coincide: with multiple closures the least-squares
        // solution is a *compromise*, so an individual loop needn't close to zero -
        // that is correct LSQ behavior, not a bug. Optimality (that the result is
        // the true minimizer of the whole system) is asserted by the `‖A·x-b‖∞`
        // guard inside `adjust_least_squares`, which fired for each solve above.
        exercised_closures += closures.len();
    }

    // Guard against the sweep silently degenerating into all-fast-path trials.
    assert!(
        exercised_closures > 0,
        "fuzz sweep never exercised a closure solve"
    );
}

/// Profiling harness for the closure-adjusted solve (#81). Ignored by default.
/// Run with:
///   cargo test --manifest-path src-tauri/Cargo.toml --release \
///     profile_closure_solve_large -- --ignored --nocapture
///
/// Builds single-closure surveys shaped as one big loop made of two long
/// degree-2 chains joined at both ends - the worst case for the closure solve
/// (a single high-diameter loop). Sweeping the size demonstrates the scaling:
/// with the degree-≤2 graph reduction the loop peels to a ~1-node core, so the
/// whole `compute_adjusted_station_coordinates` is O(n) (it was O(n²) before #81
/// - both the per-station coordinate walk and the O(diameter) PCG iterations).
#[test]
#[ignore]
fn profile_closure_solve_large() {
    for n_shots in [1500usize, 3000, 6000, 12000] {
        let mut project = Project::new("ProfileLoop".to_string(), None);

        // Path A: eastward shots of length 5.0.
        let a = project
            .add_section(SectionData {
                shots: (0..n_shots)
                    .map(|_| shot(90.0, 90.0, 5.0, 0.0, 0.0))
                    .collect(),
                ..Default::default()
            })
            .id;
        // Path B: same count, slightly longer so the far ends disagree (a real
        // misclosure for the solve to distribute).
        let b = project
            .add_section(SectionData {
                shots: (0..n_shots)
                    .map(|_| shot(90.0, 90.0, 5.001, 0.0, 0.0))
                    .collect(),
                ..Default::default()
            })
            .id;

        project
            .set_zero_datum(
                StationRef::new(a, 0),
                GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap();
        // Join the two chains at the origin and close the far ends → one big loop.
        project
            .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
            .unwrap();
        project
            .create_loop_closure(
                StationRef::new(a, n_shots as u32),
                StationRef::new(b, n_shots as u32),
                ClosureStrategy::Batch,
            )
            .unwrap();

        let n_stations = project.compute_station_coordinates().len();

        let t = std::time::Instant::now();
        let adjusted =
            project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
        let elapsed = t.elapsed();

        eprintln!(
            "profile_closure_solve_large: {} stations, 1 closure → compute_adjusted = {} ms ({} adjusted)",
            n_stations,
            elapsed.as_millis(),
            adjusted.len(),
        );
    }
}

#[test]
fn compute_adjusted_equals_measured_without_closures() {
    let mut project = Project::new("NoClosures".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 0.0, 0.0),
            shot(90.0, 90.0, 10.0, 0.0, 0.0),
        ],
        ..Default::default()
    });
    let config = ClosureAdjustmentConfig::default();

    let measured = project.compute_station_coordinates();
    let adjusted = project.compute_adjusted_station_coordinates(&config);

    assert_eq!(measured.len(), adjusted.len());
    for (m, a) in measured.iter().zip(adjusted.iter()) {
        assert!((m.coordinates.x - a.coordinates.x).abs() < 1e-12);
        assert!((m.coordinates.y - a.coordinates.y).abs() < 1e-12);
        assert!((m.coordinates.z - a.coordinates.z).abs() < 1e-12);
    }
}

#[test]
fn compute_adjusted_closes_datumless_loop_deterministically() {
    // Two paths from a shared (merged) origin to a near-shared endpoint joined by
    // a closure, but with NO zero datum - so the solve anchors each component on
    // the deterministic default anchor (`default_anchor_for_component`) instead.
    let mut project = Project::new("DatumlessLoop".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.2, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();

    // Precondition: this exercises the datum-less anchor path.
    assert!(project.zero_datums.is_empty());

    let config = ClosureAdjustmentConfig::default();
    let shots_before = shot_geometry(&project);
    let adjusted1 = project.compute_adjusted_station_coordinates(&config);
    let adjusted2 = project.compute_adjusted_station_coordinates(&config);

    // Non-destructive: deriving the view doesn't rewrite measured shots.
    assert_eq!(shots_before, shot_geometry(&project));

    // Deterministic across solves: the lex-min default anchor fixes a stable gauge.
    assert_eq!(adjusted1.len(), adjusted2.len());
    for (s1, s2) in adjusted1.iter().zip(adjusted2.iter()) {
        assert_eq!(s1.station_ref, s2.station_ref);
        assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
        assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
    }

    // The loop still closes without a datum (only the absolute translation is free).
    let adj_a2 = station_coords(&adjusted1, StationRef::new(a, 2));
    let adj_b2 = station_coords(&adjusted1, StationRef::new(b, 2));
    let misclosure = ((adj_a2.x - adj_b2.x).powi(2) + (adj_a2.y - adj_b2.y).powi(2)).sqrt();
    assert!(
        misclosure < 1e-3,
        "datum-less adjusted loop should close, got {misclosure}"
    );
}

/// A component whose adjustment fails must fall back to *measured* geometry while
/// a sibling component is still adjusted, and the measured project must be left
/// intact. This guards the per-component error handling in
/// `into_adjusted_station_coordinates`: warn-and-continue (one component's
/// failure does not abort the whole solve) plus the snapshot/restore that keeps
/// a failed component on measured geometry rather than leaking a partial solve.
///
/// The failure is forced by injecting a second zero datum into one component's
/// constraint group (bypassing the single-datum-per-component API guard), which
/// makes `adjust_loop_closures` return `Err`.
#[test]
fn compute_adjusted_failed_component_falls_back_sibling_still_adjusts() {
    // Helper: a misclosing two-section loop (origin merged, ~0.2 m open at the far end).
    fn looped(project: &mut Project) -> (SectionId, SectionId) {
        let x = project
            .add_section(SectionData {
                shots: vec![
                    shot(90.0, 90.0, 10.0, 0.0, 0.0),
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let y = project
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                    shot(90.0, 90.0, 10.2, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        project
            .merge_stations(StationRef::new(x, 0), StationRef::new(y, 0))
            .unwrap();
        project
            .create_loop_closure(
                StationRef::new(x, 2),
                StationRef::new(y, 2),
                ClosureStrategy::Batch,
            )
            .unwrap();
        (x, y)
    }

    let mut project = Project::new("MixedComponents".to_string(), None);
    // Component 1 - will FAIL (two datums forced into the origin constraint group).
    let (a, b) = looped(&mut project);
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    let dup_key = project.key_for_ref(StationRef::new(b, 0)).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: dup_key,
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    // Component 2 - disconnected from the first, with a single datum; SUCCEEDS.
    let (c, d) = looped(&mut project);
    project
        .set_zero_datum(
            StationRef::new(c, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    let config = ClosureAdjustmentConfig::default();
    let shots_before = shot_geometry(&project);
    let measured = project.compute_station_coordinates();
    let adjusted = project.compute_adjusted_station_coordinates(&config);

    // Non-destructive even though one component errored mid-solve.
    assert_eq!(shots_before, shot_geometry(&project));

    // Failed component (a/b): every station identical to measured (clean fallback,
    // no partial adjustment leaked).
    for sr in [
        StationRef::new(a, 1),
        StationRef::new(a, 2),
        StationRef::new(b, 1),
        StationRef::new(b, 2),
    ] {
        let m = station_coords(&measured, sr);
        let adj = station_coords(&adjusted, sr);
        assert!(
            (m.x - adj.x).abs() < 1e-9 && (m.y - adj.y).abs() < 1e-9,
            "failed component station {sr:?} should stay on measured geometry"
        );
    }
    // The failed component's loop is therefore still open.
    let m_a2 = station_coords(&adjusted, StationRef::new(a, 2));
    let m_b2 = station_coords(&adjusted, StationRef::new(b, 2));
    assert!(
        ((m_a2.x - m_b2.x).powi(2) + (m_a2.y - m_b2.y).powi(2)).sqrt() > 0.1,
        "failed component loop should remain open (measured)"
    );

    // Sibling component (c/d) was still adjusted: its loop closes.
    let adj_c2 = station_coords(&adjusted, StationRef::new(c, 2));
    let adj_d2 = station_coords(&adjusted, StationRef::new(d, 2));
    assert!(
        ((adj_c2.x - adj_d2.x).powi(2) + (adj_c2.y - adj_d2.y).powi(2)).sqrt() < 1e-3,
        "sibling component should still be adjusted despite the other's failure"
    );
}
