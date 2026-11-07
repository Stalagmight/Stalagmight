//! Closed-form golden fixtures and the independent rigorous reference for the closure adjustment.
//!
//! Lines 13210-13887 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// ── Closed-form golden fixtures for the closure adjustment ───────────────────
//
// KNOWN-ANSWER regression guards for the weighted least-squares closure solve
// (`compute_adjusted_station_coordinates`). Each fixture pins the EXACT adjusted
// coordinates a correct adjustment must produce, derived in closed form from the
// normal equations - NOT by running the solver.
//
// These COMPLEMENT the in-repo suite that proves the solver finds the true
// minimiser of the *production-assembled* system (the `‖A·x-b‖∞` optimality
// guard inside `adjust_least_squares`, the dense reduced-Laplacian check, and
// `adjust_random_geometries_are_optimal_and_well_formed`). What those cannot
// catch is a wrong WEIGHT MODEL: they all inherit production's σ→weight assembly,
// so a bug there is self-consistent and passes every one of them. The
// weighted-mean star below re-derives the weights independently from the
// documented propagation and checks the resulting coordinates, closing that gap
// for arbitrary star geometries.
//
// Model (see docs/closures.md): a 2-D, horizontal-only, per-axis DECOUPLED
// weighted least squares. Each shot contributes independent X/Y observations of
// the measured Δ, weight w = 1/σ² with σ propagated from the base sigmas and
// scaled by section quality:
//     σ_len' = σ_len·quality,   σ_az' = σ_az·quality
//     σ_dx = √((sin·σ_len')² + (len·cos·σ_az')²)
//     σ_dy = √((cos·σ_len')² + (len·sin·σ_az')²)
// For a shot from a FIXED group to an unknown group the normal equation reduces
// to the classic Gauss weighted mean: the unknown lands at the weight-weighted
// average of each neighbour's predicted position (fixed_pos + measured Δ).
//
// Default config → σ_len = 0.05 m, σ_az = 1.0°, section quality = 1.0.

/// Assert an adjusted station coordinate matches a closed-form `(x, y)` within
/// `tol` metres (Z is not adjusted, so only the horizontal plane is checked).
fn golden_assert_xy(got: Coordinates, want: (f64, f64), tol: f64, label: &str) {
    assert!(
        (got.x - want.0).abs() < tol && (got.y - want.1).abs() < tol,
        "{label}: adjusted ({:.6}, {:.6}) != closed-form ({:.6}, {:.6}) (tol {tol:e})",
        got.x,
        got.y,
        want.0,
        want.1,
    );
}

#[test]
fn golden_symmetric_v_resolves_to_axis_of_symmetry() {
    // Two single-shot legs from a shared, datum-anchored origin, joined by a
    // closure:
    //     leg A: azimuth 80°,  10 m → predicts (10·sin80°,  10·cos80°)
    //     leg B: azimuth 100°, 10 m → predicts (10·sin80°, -10·cos80°)
    // The legs are mirror images across the East axis, so |sin| and |cos| - and
    // hence the X/Y weights - are identical. The ~3.47 m vertical (North)
    // disagreement must average to the axis of symmetry: Y = 0, with X left at the
    // common predicted value (both legs agree on X, so it is not corrected).
    // Config-INDEPENDENT: the equal weights cancel, so the answer does not depend
    // on σ_len / σ_az at all.
    let mut project = Project::new("GoldenSymV".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(80.0, 80.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(100.0, 100.0, 10.0, 0.0, 0.0)],
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
            StationRef::new(a, 1),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let adjusted =
        project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());

    let expected_x = 10.0 * 80.0_f64.to_radians().sin();
    golden_assert_xy(
        station_coords(&adjusted, StationRef::new(a, 1)),
        (expected_x, 0.0),
        1e-6,
        "A endpoint",
    );
    golden_assert_xy(
        station_coords(&adjusted, StationRef::new(b, 1)),
        (expected_x, 0.0),
        1e-6,
        "B endpoint",
    );
    // The closure must actually fuse the two endpoints.
    let adj_a = station_coords(&adjusted, StationRef::new(a, 1));
    let adj_b = station_coords(&adjusted, StationRef::new(b, 1));
    assert!(
        (adj_a.x - adj_b.x).abs() < 1e-6 && (adj_a.y - adj_b.y).abs() < 1e-6,
        "closure endpoints coincide"
    );
    // The datum stays pinned at the origin.
    golden_assert_xy(
        station_coords(&adjusted, StationRef::new(a, 0)),
        (0.0, 0.0),
        1e-9,
        "datum",
    );
}

/// Build a "star": one single-shot leg per `legs` entry `(azimuth°, length,
/// quality)`, all radiating from a shared datum-anchored origin to one common
/// closure-fused endpoint P. Returns P's adjusted coordinates from the production
/// solve.
fn solve_weighted_mean_star(legs: &[(f64, f64, f64)]) -> Coordinates {
    let mut project = Project::new("WeightedStar".to_string(), None);
    let sids: Vec<SectionId> = legs
        .iter()
        .map(|&(az, len, quality)| {
            project
                .add_section(SectionData {
                    shots: vec![shot(az, az, len, 0.0, 0.0)],
                    quality,
                    ..Default::default()
                })
                .id
        })
        .collect();
    project
        .set_zero_datum(
            StationRef::new(sids[0], 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    for &sid in &sids[1..] {
        project
            .merge_stations(StationRef::new(sids[0], 0), StationRef::new(sid, 0))
            .unwrap();
    }
    for &sid in &sids[1..] {
        project
            .create_loop_closure(
                StationRef::new(sids[0], 1),
                StationRef::new(sid, 1),
                ClosureStrategy::Batch,
            )
            .unwrap();
    }
    let adjusted =
        project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    station_coords(&adjusted, StationRef::new(sids[0], 1))
}

/// Independently re-derive the closed-form weighted-mean optimum for a star from
/// the documented σ→weight propagation (NOT the production assembly). With the
/// origin fixed at (0,0) the decoupled normal equations give, per axis,
/// `P = Σ w·Δ / Σ w`.
fn weighted_mean_star_expected(
    legs: &[(f64, f64, f64)],
    config: &ClosureAdjustmentConfig,
) -> (f64, f64) {
    let (mut num_x, mut den_x, mut num_y, mut den_y) = (0.0, 0.0, 0.0, 0.0);
    for &(az_deg, len, raw_quality) in legs {
        // Clamp quality exactly as production's adjust_least_squares does, so the
        // weight model also matches for quality <= 0 / non-finite (otherwise
        // s_len = 0 → σ = 0 → w = ∞ → NaN and the assertion false-fails).
        let quality = if raw_quality.is_finite() && raw_quality > 0.0 {
            raw_quality
        } else {
            1.0
        };
        let az = az_deg.to_radians();
        let s_len = config.base_sigma_length_m * quality;
        let s_az = config.base_sigma_azimuth_deg.to_radians() * quality;
        let sigma_dx = ((az.sin() * s_len).powi(2) + (len * az.cos() * s_az).powi(2)).sqrt();
        let sigma_dy = ((az.cos() * s_len).powi(2) + (len * az.sin() * s_az).powi(2)).sqrt();
        let w_x = 1.0 / (sigma_dx * sigma_dx);
        let w_y = 1.0 / (sigma_dy * sigma_dy);
        // Star legs are flat (depth_in == depth_out), so horizontal_length() ==
        // shot.length and Δ = len·(sin, cos). Production uses horizontal_length for Δ
        // but SLOPE length for the σ azimuth term; they coincide only under this
        // flat-leg assumption - split them if a sloped leg is ever added here.
        let dx = len * az.sin();
        let dy = len * az.cos();
        num_x += w_x * dx;
        den_x += w_x;
        num_y += w_y * dy;
        den_y += w_y;
    }
    (num_x / den_x, num_y / den_y)
}

#[test]
fn golden_weight_model_star_matches_sigma_propagation() {
    // The closure-fused endpoint of a star is the Gauss weighted mean of each
    // leg's independent position fix, with weights from the σ→weight propagation.
    // Matching the production solve across diverse leg geometries - varied
    // azimuths, lengths, and section qualities - pins the weight MODEL
    // independently of production's normal-equation assembly. This is the one
    // thing #86's optimality/dense checks structurally cannot verify: they reuse
    // the same assembly, so a wrong weight formula is self-consistent there.
    let config = ClosureAdjustmentConfig::default();
    let cases: &[&[(f64, f64, f64)]] = &[
        &[(90.0, 10.0, 1.0), (0.0, 10.0, 1.0)], // east + north, isotropic quality
        &[(90.0, 10.0, 1.0), (0.0, 10.0, 3.0)], // north leg down-weighted by quality
        &[(90.0, 10.0, 1.0), (180.0, 6.0, 1.0)], // opposed legs, unequal length
        &[(90.0, 10.0, 1.0), (0.0, 8.0, 1.0), (45.0, 12.0, 2.0)], // three legs, mixed dir/len/quality
        &[(30.0, 7.0, 1.0), (200.0, 9.0, 1.5), (300.0, 5.0, 0.5)], // arbitrary 3-leg star
    ];
    for legs in cases {
        let got = solve_weighted_mean_star(legs);
        let (ex, ey) = weighted_mean_star_expected(legs, &config);
        assert!(
            (got.x - ex).abs() < 1e-6 && (got.y - ey).abs() < 1e-6,
            "legs {legs:?}: adjusted ({:.6}, {:.6}) != weighted mean ({ex:.6}, {ey:.6})",
            got.x,
            got.y,
        );
    }
}

// ── Independent rigorous reference (the in-process Gama equivalent) ───────────
//
// gama-local is not available in CI, so this in-process reference plays its role:
// a from-scratch weighted least-squares adjustment that makes NONE of production's
// modelling shortcuts. Production minimises, per shot,
//     w_dx·(Δx_meas - Δx)² + w_dy·(Δy_meas - Δy)²
// i.e. CARTESIAN coordinate-difference observations with DIAGONAL weights, X and Y
// DECOUPLED, linearised ONCE at the measured geometry. This reference instead
// minimises the RAW POLAR residuals
//     w_dist·(dist_meas - |Pj-Pi|)² + w_az·(az_meas - az(Pj-Pi))²
// with the X/Y-COUPLED Jacobian, ITERATED to convergence (Gauss-Newton). It keeps
// the dx-dy covariance production drops and re-linearises every step, so agreement
// is independent evidence that production's model is a sound approximation of
// rigorous LSQ - the role the external Gama oracle would play.
//
// The two are NOT bit-identical: the gap is the first-order error of production's
// approximation and grows with the angular misclosure of the network. The cases
// in `model_sound_matches_rigorous_polar_reference` stay in the small-misclosure
// regime real surveys operate in.

/// Solve a small dense `a·x = b` by Gaussian elimination with partial pivoting.
fn dense_solve_gauss(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-15 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    Some((0..n).map(|i| b[i] / a[i][i]).collect())
}

/// Rigorous coupled-polar Gauss-Newton adjustment of a single-component,
/// single-datum network. Returns each station's adjusted (x, y).
fn rigorous_polar_reference(
    project: &Project,
    config: &ClosureAdjustmentConfig,
) -> HashMap<StationRef, (f64, f64)> {
    use std::f64::consts::PI;

    fn find(p: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
        let mut r = x;
        while p[&r] != r {
            r = p[&r];
        }
        let mut c = x;
        while p[&c] != r {
            let nx = p[&c];
            p.insert(c, r);
            c = nx;
        }
        r
    }
    fn accumulate(rows: &[(usize, f64)], w: f64, r: f64, nmat: &mut [Vec<f64>], gvec: &mut [f64]) {
        for &(ia, ca) in rows {
            gvec[ia] += w * ca * r;
            for &(ib, cb) in rows {
                nmat[ia][ib] += w * ca * cb;
            }
        }
    }

    let mut stations: Vec<StationRef> = Vec::new();
    for s in &project.sections {
        for idx in 0..=s.shots.len() {
            stations.push(StationRef::new(s.id, idx as u32));
        }
    }
    let mut parent: HashMap<StationRef, StationRef> = stations.iter().map(|&s| (s, s)).collect();
    let fuse = |parent: &mut HashMap<StationRef, StationRef>, a: StationRef, b: StationRef| {
        if parent.contains_key(&a) && parent.contains_key(&b) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    };
    for m in &project.station_merges {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        ) {
            fuse(&mut parent, a, b);
        }
    }
    for lc in &project.loop_closures {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&lc.station_a),
            project.resolve_key(&lc.station_b),
        ) {
            fuse(&mut parent, a, b);
        }
    }

    let mut group_members: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
    for &s in &stations {
        let r = find(&mut parent, s);
        group_members.entry(r).or_default().push(s);
    }
    // Initial position per group: mean of members' measured coords.
    let mut pos: HashMap<StationRef, (f64, f64)> = HashMap::new();
    for (root, members) in &group_members {
        let (mut sx, mut sy, mut k) = (0.0, 0.0, 0.0);
        for &m in members {
            if let Some(c) = project.compute_station_raw_coords(m) {
                sx += c.x;
                sy += c.y;
                k += 1.0;
            }
        }
        pos.insert(
            *root,
            if k > 0.0 {
                (sx / k, sy / k)
            } else {
                (0.0, 0.0)
            },
        );
    }
    // Fixed group = the datum's group, pinned at its measured position. This oracle
    // anchors exactly ONE datum, so it only handles single-datum, single-component
    // networks; a multi-datum / datum-less / multi-component input would leave a
    // gauge-free translation null space. Reject those up front instead of silently
    // returning un-adjusted coordinates as the "rigorous" reference.
    assert_eq!(
        project.zero_datums.len(),
        1,
        "rigorous_polar_reference supports single-datum networks only (got {})",
        project.zero_datums.len()
    );
    let datum = project
        .zero_datums
        .first()
        .and_then(|z| project.resolve_key(&z.station_ref));
    let fixed_root = datum.map(|d| find(&mut parent, d));
    assert!(
        fixed_root.is_some(),
        "rigorous_polar_reference: datum did not resolve to a group"
    );
    if let (Some(fr), Some(d)) = (fixed_root, datum) {
        if let Some(c) = project.compute_station_raw_coords(d) {
            pos.insert(fr, (c.x, c.y));
        }
    }

    let mut uidx: HashMap<StationRef, usize> = HashMap::new();
    for root in group_members.keys() {
        if Some(*root) != fixed_root {
            let n = uidx.len();
            uidx.insert(*root, n);
        }
    }
    let nvar = uidx.len() * 2;

    struct Obs {
        gi: StationRef,
        gj: StationRef,
        dist: f64,
        az: f64,
        w_dist: f64,
        w_az: f64,
    }
    let mut obs: Vec<Obs> = Vec::new();
    for s in &project.sections {
        let q = if s.quality.is_finite() && s.quality > 0.0 {
            s.quality
        } else {
            1.0
        };
        let s_len = config.base_sigma_length_m * q;
        let s_az = config.base_sigma_azimuth_deg.to_radians() * q;
        for (idx, shot) in s.shots.iter().enumerate() {
            let gi = find(&mut parent, StationRef::new(s.id, idx as u32));
            let gj = find(&mut parent, StationRef::new(s.id, (idx + 1) as u32));
            if gi == gj {
                continue;
            }
            obs.push(Obs {
                gi,
                gj,
                dist: shot.horizontal_length(),
                az: shot.average_azimuth_measured().to_radians(),
                w_dist: 1.0 / (s_len * s_len),
                w_az: 1.0 / (s_az * s_az),
            });
        }
    }

    let wrap = |a: f64| -> f64 {
        let mut x = a % (2.0 * PI);
        if x > PI {
            x -= 2.0 * PI;
        } else if x < -PI {
            x += 2.0 * PI;
        }
        x
    };

    let mut converged = nvar == 0;
    if nvar > 0 {
        for _iter in 0..200 {
            let mut nmat = vec![vec![0.0; nvar]; nvar];
            let mut gvec = vec![0.0; nvar];
            for o in &obs {
                let pi = pos[&o.gi];
                let pj = pos[&o.gj];
                let dx = pj.0 - pi.0;
                let dy = pj.1 - pi.1;
                let d = (dx * dx + dy * dy).sqrt();
                if d < 1e-9 {
                    continue;
                }
                let r_dist = o.dist - d;
                let r_az = wrap(o.az - dx.atan2(dy));
                let ui = uidx.get(&o.gi).copied();
                let uj = uidx.get(&o.gj).copied();
                let mut dist_row: Vec<(usize, f64)> = Vec::new();
                let mut az_row: Vec<(usize, f64)> = Vec::new();
                if let Some(uj) = uj {
                    dist_row.push((2 * uj, dx / d));
                    dist_row.push((2 * uj + 1, dy / d));
                    az_row.push((2 * uj, dy / (d * d)));
                    az_row.push((2 * uj + 1, -dx / (d * d)));
                }
                if let Some(ui) = ui {
                    dist_row.push((2 * ui, -dx / d));
                    dist_row.push((2 * ui + 1, -dy / d));
                    az_row.push((2 * ui, -dy / (d * d)));
                    az_row.push((2 * ui + 1, dx / (d * d)));
                }
                accumulate(&dist_row, o.w_dist, r_dist, &mut nmat, &mut gvec);
                accumulate(&az_row, o.w_az, r_az, &mut nmat, &mut gvec);
            }
            let Some(delta) = dense_solve_gauss(nmat, gvec) else {
                panic!("rigorous_polar_reference: singular normal matrix (degenerate or unanchored geometry)")
            };
            let mut maxd = 0.0_f64;
            for (root, &u) in &uidx {
                if let Some(p) = pos.get_mut(root) {
                    p.0 += delta[2 * u];
                    p.1 += delta[2 * u + 1];
                }
                maxd = maxd.max(delta[2 * u].abs()).max(delta[2 * u + 1].abs());
            }
            if maxd < 1e-11 {
                converged = true;
                break;
            }
        }
    }
    // The reference is only meaningful at the true optimum; never hand back a
    // stalled iterate as the "rigorous" answer.
    assert!(
        converged,
        "rigorous_polar_reference: Gauss-Newton did not converge within 200 iterations"
    );

    let mut out = HashMap::new();
    for (root, members) in &group_members {
        let p = pos[root];
        for &m in members {
            out.insert(m, p);
        }
    }
    out
}

#[test]
fn model_soundness_oracle_reproduces_known_rigorous_optima() {
    // Validate the rigorous reference against optima we can pin by hand, so the
    // model-soundness comparison below rests on a trustworthy oracle.
    let config = ClosureAdjustmentConfig::default();

    // (1) An already-closed network has nothing to adjust → measured unchanged.
    let mut zero = Project::new("RefZero".to_string(), None);
    let za = zero
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let zb = zero
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    zero.set_zero_datum(
        StationRef::new(za, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    zero.merge_stations(StationRef::new(za, 0), StationRef::new(zb, 0))
        .unwrap();
    zero.create_loop_closure(
        StationRef::new(za, 2),
        StationRef::new(zb, 2),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let r0 = rigorous_polar_reference(&zero, &config);
    for m in &zero.compute_station_coordinates() {
        let (rx, ry) = r0[&m.station_ref];
        assert!(
            (m.coordinates.x - rx).abs() < 1e-6 && (m.coordinates.y - ry).abs() < 1e-6,
            "oracle moved an already-closed network at {:?}",
            m.station_ref,
        );
    }

    // (2) Symmetric V (legs az80°/az100°, 10 m): by symmetry the rigorous optimum
    // sits on the East axis at distance 10 → (10, 0). Production's once-linearised
    // cartesian model gives 10·sin80° ≈ 9.848 instead - the legitimate first-order
    // gap, which at this 10° angular misclosure is ~15 cm. This both validates the
    // oracle on a known answer and demonstrates the model gap is real and signed.
    let mut v = Project::new("RefSymV".to_string(), None);
    let va = v
        .add_section(SectionData {
            shots: vec![shot(80.0, 80.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let vb = v
        .add_section(SectionData {
            shots: vec![shot(100.0, 100.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    v.set_zero_datum(
        StationRef::new(va, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    v.merge_stations(StationRef::new(va, 0), StationRef::new(vb, 0))
        .unwrap();
    v.create_loop_closure(
        StationRef::new(va, 1),
        StationRef::new(vb, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let rv = rigorous_polar_reference(&v, &config);
    let (rx, ry) = rv[&StationRef::new(va, 1)];
    assert!(
        (rx - 10.0).abs() < 1e-4 && ry.abs() < 1e-4,
        "rigorous symmetric-V optimum ({rx:.6}, {ry:.6}) != (10, 0)"
    );
}

#[test]
fn model_sound_matches_rigorous_in_small_misclosure_regime() {
    // Production's decoupled, once-linearised solve must track the rigorous
    // coupled-polar adjustment in the regime real surveys operate in (well-closed
    // loops). The residual gap is the first-order error of production's
    // approximation: it grows with ANGULAR misclosure (see
    // model_soundness_oracle_reproduces_known_rigorous_optima, where a 10° angular
    // misclosure opens a ~15 cm gap) and, for oblique legs, with the dx-dy
    // covariance production drops. On ~0.1-0.2 m corrections the two agree to a few
    // millimetres or better - independent evidence the model is a sound LSQ
    // adjustment in regime. Tolerances are keyed per case: tight where legs are
    // axis-aligned (no dropped covariance), looser where they are oblique.
    let config = ClosureAdjustmentConfig::default();
    let cases: Vec<(&str, Project, f64)> = vec![
        (
            "axis-aligned 0.2 m loop",
            looped_project_with_misclosure().0,
            1e-3,
        ),
        ("oblique 1 cm loop", oblique_loop(10.01), 3e-3),
    ];
    for (name, project, tol) in cases {
        let prod = project.compute_adjusted_station_coordinates(&config);
        let reference = rigorous_polar_reference(&project, &config);
        let mut maxd = 0.0_f64;
        for (sr, (rx, ry)) in &reference {
            let p = station_coords(&prod, *sr);
            maxd = maxd.max((p.x - rx).abs()).max((p.y - ry).abs());
        }
        assert!(
            maxd < tol,
            "{name}: production vs rigorous polar LSQ differ by {maxd:.6} m (> {tol})"
        );
    }
}
