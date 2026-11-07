//! The GNU Gama gold-standard fixtures for the closure adjustment.
//!
//! Lines 13888-14615 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// ── GNU Gama gold-standard fixtures ──────────────────────────────────────────
//
// Production's closure solve is DECOUPLED per axis, and each axis minimises
// Σ w·(Δ_meas - (p_j - p_i))² - i.e. exactly a 1-D LEVELLING network. GNU Gama
// (gama-local) adjusts levelling networks natively, linearly and exactly, so it is
// an independent GOLD STANDARD: we emit each network's X- and Y-axis as a Gama
// levelling network, run gama-local ONCE offline, and freeze the adjusted
// coordinates in tests/fixtures/gama_golden.json. `production_matches_gama_golden`
// then checks production against the frozen Gama numbers with NO gama dependency.
//
// Regenerate after changing the networks or the weight model:
//   GAMA_LOCAL=/path/to/gama-local cargo test --manifest-path src-tauri/Cargo.toml \
//       --lib generate_gama_golden_fixtures -- --ignored --nocapture
// The frozen fixture was produced with gama-local 2.33; use that version (or
// verify a newer one regenerates byte-identically) when regenerating.

#[derive(serde::Serialize, serde::Deserialize)]
struct GoldStation {
    s: u32,
    i: u32,
    x: f64,
    y: f64,
}

/// Deterministic xorshift in [0, 1); seedable so fixtures are reproducible.
fn xorshift01(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 11) as f64) / ((1u64 << 53) as f64)
}

/// A fuzzed two-path loop: two sections of random oblique legs from a shared,
/// datum-anchored origin, joined at their far ends by a closure. Single-component,
/// single-datum, no spurs - every station lies on the loop. Misclosure is whatever
/// the random geometry produces (irrelevant to the per-axis levelling gold solve).
fn fuzz_loop(seed: u64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("fuzz_{seed}"), None);
    let na = 2 + (xorshift01(&mut st) * 3.0) as usize;
    let nb = 2 + (xorshift01(&mut st) * 3.0) as usize;
    let mkshots = |st: &mut u64, n: usize| -> Vec<ShotData> {
        (0..n)
            .map(|_| {
                let az = xorshift01(st) * 360.0;
                let len = 5.0 + xorshift01(st) * 10.0;
                shot(az, az, len, 0.0, 0.0)
            })
            .collect()
    };
    let a = p
        .add_section(SectionData {
            shots: mkshots(&mut st, na),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: mkshots(&mut st, nb),
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, na as u32),
        StationRef::new(b, nb as u32),
        ClosureStrategy::Batch,
    )
    .unwrap();
    p
}

/// Random oblique shots with a gentle eastward drift (keeps coordinates bounded at
/// scale while leaving a real misclosure).
fn drift_shots(st: &mut u64, n: usize, base_az: f64) -> Vec<ShotData> {
    (0..n)
        .map(|_| {
            let az = base_az + (xorshift01(st) - 0.5) * 80.0;
            let len = 4.0 + xorshift01(st) * 6.0;
            shot(az, az, len, 0.0, 0.0)
        })
        .collect()
}

/// One big loop: two paths of `n` shots from a merged datum origin joined by a
/// single closure at the far end. ~2n+1 stations - pure-scale stress for the O(n)
/// solve.
fn big_loop(n: usize, seed: u64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("loop_{}", 2 * n + 1), None);
    let a = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 60.0),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 30.0),
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, n as u32),
        StationRef::new(b, n as u32),
        ClosureStrategy::Batch,
    )
    .unwrap();
    p
}

/// A ladder: two rails of `segs*seg` shots from a merged datum origin, joined by a
/// "rung" closure every `seg` stations → `segs` STACKED loops sharing rails.
fn ladder(segs: usize, seg: usize, seed: u64) -> Project {
    let n = segs * seg;
    let mut st = seed | 1;
    let mut p = Project::new(format!("ladder_{}", 2 * n + 1), None);
    let a = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 90.0),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 90.0),
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    for k in 1..=segs {
        let idx = (k * seg) as u32;
        p.create_loop_closure(
            StationRef::new(a, idx),
            StationRef::new(b, idx),
            ClosureStrategy::Batch,
        )
        .unwrap();
    }
    p
}

/// A 2-D grid mesh: `rows` row-sections (mostly-East shots) and `cols`
/// column-sections (mostly-North shots), with every grid node (r,c) shared between
/// its row and column via a merge. A dense lattice of loops - one per cell.
fn grid(rows: usize, cols: usize, seed: u64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("grid_{rows}x{cols}"), None);
    let row_sec: Vec<u32> = (0..rows)
        .map(|_| {
            p.add_section(SectionData {
                shots: drift_shots(&mut st, cols - 1, 90.0),
                ..Default::default()
            })
            .id
        })
        .collect();
    let col_sec: Vec<u32> = (0..cols)
        .map(|_| {
            p.add_section(SectionData {
                shots: drift_shots(&mut st, rows - 1, 0.0),
                ..Default::default()
            })
            .id
        })
        .collect();
    p.set_zero_datum(
        StationRef::new(row_sec[0], 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    // Spanning tree of identity merges: row 0 ties every column, column 0 ties every
    // row. (Tree → each merge joins a new section, never an already-connected pair.)
    for c in 0..cols {
        p.merge_stations(
            StationRef::new(row_sec[0], c as u32),
            StationRef::new(col_sec[c], 0),
        )
        .unwrap();
    }
    for r in 1..rows {
        p.merge_stations(
            StationRef::new(row_sec[r], 0),
            StationRef::new(col_sec[0], r as u32),
        )
        .unwrap();
    }
    // Interior nodes are already path-connected, so identifying row r / col c there
    // CLOSES a cell loop - one closure per interior node.
    for r in 1..rows {
        for c in 1..cols {
            p.create_loop_closure(
                StationRef::new(row_sec[r], c as u32),
                StationRef::new(col_sec[c], r as u32),
                ClosureStrategy::Batch,
            )
            .unwrap();
        }
    }
    p
}

/// `paths` independent random sections from a merged datum origin, all fused at
/// their far ends by closures → `paths-1` independent loops, no spurs. Shots span
/// the FULL azimuth circle and a wide length band `[len_min, len_max]`, plus a
/// random per-section quality - deliberately harsher than the structured nets.
fn fuzz_paths(seed: u64, paths: usize, max_shots: usize, len_min: f64, len_max: f64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("wild_{seed:x}"), None);
    let sids: Vec<u32> = (0..paths)
        .map(|_| {
            let n = 2 + (xorshift01(&mut st) * (max_shots as f64 - 1.0)) as usize;
            let shots: Vec<ShotData> = (0..n)
                .map(|_| {
                    let az = xorshift01(&mut st) * 360.0;
                    let len = len_min + xorshift01(&mut st) * (len_max - len_min);
                    shot(az, az, len, 0.0, 0.0)
                })
                .collect();
            let q = 0.5 + xorshift01(&mut st) * 2.5;
            p.add_section(SectionData {
                shots,
                quality: q,
                ..Default::default()
            })
            .id
        })
        .collect();
    p.set_zero_datum(
        StationRef::new(sids[0], 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    for &sid in &sids[1..] {
        p.merge_stations(StationRef::new(sids[0], 0), StationRef::new(sid, 0))
            .unwrap();
    }
    let end_of = |p: &Project, sid: u32| {
        let n = p
            .sections
            .iter()
            .find(|s| s.id == sid)
            .map(|s| s.shots.len())
            .unwrap_or(0);
        StationRef::new(sid, n as u32)
    };
    let end0 = end_of(&p, sids[0]);
    for &sid in &sids[1..] {
        let e = end_of(&p, sid);
        p.create_loop_closure(end0, e, ClosureStrategy::Batch)
            .unwrap();
    }
    p
}

/// Star: single-shot legs `(az°, len, quality)` from a merged datum origin to one
/// closure-fused endpoint.
fn build_star(name: &str, legs: &[(f64, f64, f64)]) -> Project {
    let mut p = Project::new(name.to_string(), None);
    let sids: Vec<u32> = legs
        .iter()
        .map(|&(az, len, q)| {
            p.add_section(SectionData {
                shots: vec![shot(az, az, len, 0.0, 0.0)],
                quality: q,
                ..Default::default()
            })
            .id
        })
        .collect();
    p.set_zero_datum(
        StationRef::new(sids[0], 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    for &sid in &sids[1..] {
        p.merge_stations(StationRef::new(sids[0], 0), StationRef::new(sid, 0))
            .unwrap();
    }
    for &sid in &sids[1..] {
        p.create_loop_closure(
            StationRef::new(sids[0], 1),
            StationRef::new(sid, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();
    }
    p
}

/// The fixed roster of networks behind the Gama gold standard. Both the generator
/// and the CI check call this, so they see identical geometry.
fn gama_golden_networks() -> Vec<(String, Project)> {
    let mut nets: Vec<(String, Project)> = vec![
        (
            "axis_aligned_loop".to_string(),
            looped_project_with_misclosure().0,
        ),
        ("oblique_loop".to_string(), oblique_loop(10.5)),
        (
            "star_mixed".to_string(),
            build_star(
                "star_mixed",
                &[(90.0, 10.0, 1.0), (0.0, 8.0, 1.0), (45.0, 12.0, 2.0)],
            ),
        ),
        // Large scale and dense-redundancy topologies.
        ("loop_101".to_string(), big_loop(50, 0x10001)),
        ("ladder_1001".to_string(), ladder(20, 25, 0x1ADDE2)),
        ("grid_12x12".to_string(), grid(12, 12, 0x6121D)),
        // Harsh random multi-path bundles: full-circle azimuths, wide length bands.
        (
            "wild_2path".to_string(),
            fuzz_paths(0x21D2, 2, 7, 0.5, 40.0),
        ),
        (
            "wild_3path".to_string(),
            fuzz_paths(0x5A1A, 3, 5, 0.2, 30.0),
        ),
        (
            "wild_4path".to_string(),
            fuzz_paths(0xB17E, 4, 4, 1.0, 50.0),
        ),
        (
            "wild_long".to_string(),
            fuzz_paths(0xF1A5C0, 2, 9, 0.3, 25.0),
        ),
    ];
    for seed in [0xA11CE_u64, 0xBEEF, 0xC0FFEE, 0xD15EA5E, 0x5EED] {
        nets.push((format!("fuzz_{seed}"), fuzz_loop(seed)));
    }
    nets
}

/// Union-find over merges + closures → (station→group index, group→members).
fn station_groups(project: &Project) -> (HashMap<StationRef, usize>, Vec<Vec<StationRef>>) {
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
    let mut root_idx: HashMap<StationRef, usize> = HashMap::new();
    let mut members: Vec<Vec<StationRef>> = Vec::new();
    let mut gidx: HashMap<StationRef, usize> = HashMap::new();
    for &s in &stations {
        let r = find(&mut parent, s);
        let idx = *root_idx.entry(r).or_insert_with(|| {
            members.push(Vec::new());
            members.len() - 1
        });
        members[idx].push(s);
        gidx.insert(s, idx);
    }
    (gidx, members)
}

/// Reverse Cuthill-McKee ordering of the group graph. Gama's envelope solver cost
/// is `O(n·bandwidth²)`, and emitting points in section order leaves a huge
/// bandwidth on big loops; RCM keeps it near-constant so the gold solve scales.
/// Returns `rcm[k]` = the group emitted at position `k`.
fn bandwidth_order(n: usize, edges: &[(usize, usize)], start: usize) -> Vec<usize> {
    use std::collections::VecDeque;
    let mut adj = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a != b {
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    for nb in adj.iter_mut() {
        nb.sort_unstable();
        nb.dedup();
    }
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut seeds: Vec<usize> = (0..n).collect();
    seeds.sort_by_key(|&g| adj[g].len());
    seeds.retain(|&g| g != start);
    seeds.insert(0, start);
    let mut q = VecDeque::new();
    for &s in &seeds {
        if visited[s] {
            continue;
        }
        visited[s] = true;
        q.push_back(s);
        while let Some(u) = q.pop_front() {
            order.push(u);
            let mut nbrs: Vec<usize> = adj[u].iter().copied().filter(|&v| !visited[v]).collect();
            nbrs.sort_by_key(|&v| adj[v].len());
            for v in nbrs {
                visited[v] = true;
                q.push_back(v);
            }
        }
    }
    order.reverse();
    order
}

/// Emit one axis (0 = X, 1 = Y) of a network as a GNU Gama levelling network: one
/// point per group (datum group fixed) declared in `rcm` order, one `<dh>` per
/// inter-group shot with production's measured Δ and σ on that axis. `rank[g]` is
/// the gama point id of group `g`.
fn emit_leveling_gkf(
    project: &Project,
    config: &ClosureAdjustmentConfig,
    axis: usize,
    gidx: &HashMap<StationRef, usize>,
    rcm: &[usize],
    rank: &[usize],
    datum_group: usize,
    datum_coord: f64,
) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\"?>\n<gama-local xmlns=\"http://www.gnu.org/software/gama/gama-local\">\n");
    s.push_str("<network axes-xy=\"ne\" angles=\"right-handed\">\n");
    // tol-abs is the gross-error threshold in MM: gama removes any observation whose
    // misclosure exceeds it. Our fuzzed loops have metre-scale misclosures by design,
    // so set it huge to keep every observation in the adjustment.
    s.push_str(
        "<parameters sigma-apr=\"1.0\" conf-pr=\"0.95\" tol-abs=\"1e12\" sigma-act=\"apriori\"/>\n",
    );
    s.push_str("<points-observations>\n");
    for (k, &g) in rcm.iter().enumerate() {
        if g == datum_group {
            s.push_str(&format!(
                "<point id=\"g{k}\" z=\"{datum_coord:.12}\" fix=\"Z\"/>\n"
            ));
        } else {
            s.push_str(&format!("<point id=\"g{k}\" adj=\"Z\"/>\n"));
        }
    }
    s.push_str("<height-differences>\n");
    for sec in &project.sections {
        let q = if sec.quality.is_finite() && sec.quality > 0.0 {
            sec.quality
        } else {
            1.0
        };
        let s_len = config.base_sigma_length_m * q;
        let s_az = config.base_sigma_azimuth_deg.to_radians() * q;
        for (idx, shot) in sec.shots.iter().enumerate() {
            let gi = gidx[&StationRef::new(sec.id, idx as u32)];
            let gj = gidx[&StationRef::new(sec.id, (idx + 1) as u32)];
            if gi == gj {
                continue;
            }
            let az = shot.average_azimuth_measured().to_radians();
            let len = shot.length;
            let hlen = shot.horizontal_length();
            let (val, sigma) = if axis == 0 {
                (
                    hlen * az.sin(),
                    ((az.sin() * s_len).powi(2) + (len * az.cos() * s_az).powi(2)).sqrt(),
                )
            } else {
                (
                    hlen * az.cos(),
                    ((az.cos() * s_len).powi(2) + (len * az.sin() * s_az).powi(2)).sqrt(),
                )
            };
            s.push_str(&format!(
                "  <dh from=\"g{}\" to=\"g{}\" val=\"{val:.12}\" stdev=\"{sigma:.12}\"/>\n",
                rank[gi], rank[gj]
            ));
        }
    }
    s.push_str("</height-differences>\n</points-observations>\n</network>\n</gama-local>\n");
    s
}

/// Extract adjusted heights from gama-local `--xml` output: `id → Z`.
fn parse_gama_adjusted(xml: &str) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    let body = xml
        .split_once("<adjusted>")
        .and_then(|(_, r)| r.split_once("</adjusted>"))
        .map(|(b, _)| b)
        .unwrap_or("");
    for chunk in body.split("<point>").skip(1) {
        let id = chunk
            .split_once("<id>")
            .and_then(|(_, r)| r.split_once("</id>"))
            .map(|(v, _)| v.trim().to_string());
        let z = chunk
            .split_once("<Z>")
            .and_then(|(_, r)| r.split_once("</Z>"))
            .and_then(|(v, _)| v.trim().parse::<f64>().ok());
        if let (Some(id), Some(z)) = (id, z) {
            out.insert(id, z);
        }
    }
    out
}

/// Run gama on every network/axis and freeze the adjusted coordinates. Ignored by
/// default; needs `GAMA_LOCAL` pointing at a `gama-local` binary.
#[test]
#[ignore = "regenerates Gama gold fixtures; set GAMA_LOCAL=/path/to/gama-local"]
fn generate_gama_golden_fixtures() {
    use std::collections::BTreeMap;
    let gama = crate::test_support::require_env("GAMA_LOCAL");
    let config = ClosureAdjustmentConfig::default();
    let mut all: BTreeMap<String, Vec<GoldStation>> = BTreeMap::new();
    let mut worst = 0.0_f64;

    for (name, project) in gama_golden_networks() {
        let (gidx, members) = station_groups(&project);
        let datum = project
            .zero_datums
            .first()
            .and_then(|z| project.resolve_key(&z.station_ref))
            .unwrap();
        let dg = gidx[&datum];
        let datum_raw = project.compute_station_raw_coords(datum).unwrap();
        // Group adjacency from inter-group shots → reverse Cuthill-McKee point order.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for sec in &project.sections {
            for idx in 0..sec.shots.len() {
                let gi = gidx[&StationRef::new(sec.id, idx as u32)];
                let gj = gidx[&StationRef::new(sec.id, (idx + 1) as u32)];
                if gi != gj {
                    edges.push((gi, gj));
                }
            }
        }
        let rcm = bandwidth_order(members.len(), &edges, dg);
        let mut rank = vec![0usize; members.len()];
        for (k, &g) in rcm.iter().enumerate() {
            rank[g] = k;
        }
        let mut group_xy = vec![(0.0_f64, 0.0_f64); members.len()];
        for axis in 0..2 {
            let dcoord = if axis == 0 { datum_raw.x } else { datum_raw.y };
            let gkf = emit_leveling_gkf(&project, &config, axis, &gidx, &rcm, &rank, dg, dcoord);
            let tmp = std::env::temp_dir();
            let infile = tmp.join(format!("gama_{name}_{axis}.gkf"));
            let outfile = tmp.join(format!("gama_{name}_{axis}.out.xml"));
            std::fs::write(&infile, &gkf).unwrap();
            // --cov-band 0 skips the O(n²) a-posteriori covariance matrix (we only
            // need adjusted coordinates); without it gama is unusably slow at 10⁴ pts.
            let status = std::process::Command::new(&gama)
                .arg(&infile)
                .arg("--cov-band")
                .arg("0")
                .arg("--xml")
                .arg(&outfile)
                .status()
                .unwrap();
            assert!(status.success(), "gama-local failed on {name} axis {axis}");
            let adj = parse_gama_adjusted(&std::fs::read_to_string(&outfile).unwrap());
            for (g, item) in group_xy.iter_mut().enumerate() {
                let c = if g == dg {
                    dcoord
                } else {
                    *adj.get(&format!("g{}", rank[g]))
                        .unwrap_or_else(|| panic!("missing g{} for {name} axis {axis}", rank[g]))
                };
                if axis == 0 {
                    item.0 = c;
                } else {
                    item.1 = c;
                }
            }
        }
        // Sanity: production must already match the gold; record the worst gap.
        let prod = project.compute_adjusted_station_coordinates(&config);
        let mut rows = Vec::new();
        let mut net_worst = 0.0_f64;
        for (g, mem) in members.iter().enumerate() {
            for &sr in mem {
                let p = station_coords(&prod, sr);
                net_worst = net_worst
                    .max((p.x - group_xy[g].0).abs())
                    .max((p.y - group_xy[g].1).abs());
                rows.push(GoldStation {
                    s: sr.section_id,
                    i: sr.station_index,
                    x: group_xy[g].0,
                    y: group_xy[g].1,
                });
            }
        }
        println!(
            "[gama-golden] {name}: groups={} worst={net_worst:.3e} m",
            members.len()
        );
        // Refuse to freeze a fixture production doesn't already reproduce: a large
        // gap here is an emitter/model bug (wrong Δ/σ/sign), not a production
        // regression - writing it would bake a wrong gold standard.
        assert!(
            net_worst < 1e-5,
            "regeneration aborted: production diverged from freshly-emitted gama on {name} by {net_worst:.3e} m"
        );
        worst = worst.max(net_worst);
        rows.sort_by_key(|r| (r.s, r.i));
        all.insert(name, rows);
    }

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/gama_golden.json"
    );
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(&all).unwrap()).unwrap();
    println!("[gama-golden] wrote {path}; worst production-vs-gama gap = {worst:.3e} m");
}

#[test]
fn production_matches_gama_golden() {
    use std::collections::BTreeMap;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/gama_golden.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|_| {
        panic!("missing {path}; regenerate with GAMA_LOCAL=/path/to/gama-local cargo test generate_gama_golden_fixtures -- --ignored")
    });
    let frozen: BTreeMap<String, Vec<GoldStation>> = serde_json::from_str(&data).unwrap();
    let config = ClosureAdjustmentConfig::default();
    let nets: HashMap<String, Project> = gama_golden_networks().into_iter().collect();

    // Coverage guard: the frozen fixture and the live roster must name the SAME
    // networks, so adding one without rerunning the generator fails loudly instead
    // of being silently untested. (frozen is a BTreeMap, so its keys are sorted.)
    assert!(!frozen.is_empty(), "gama_golden.json is empty; regenerate");
    let mut roster_names: Vec<&String> = nets.keys().collect();
    roster_names.sort();
    let frozen_names: Vec<&String> = frozen.keys().collect();
    assert_eq!(
        frozen_names, roster_names,
        "roster and frozen fixture networks differ; rerun generate_gama_golden_fixtures"
    );

    for (name, rows) in &frozen {
        let project = nets
            .get(name)
            .unwrap_or_else(|| panic!("network {name} not in roster"));
        let prod = project.compute_adjusted_station_coordinates(&config);
        // Every production station must be frozen, so added/renumbered stations are
        // not silently left unchecked.
        assert_eq!(
            rows.len(),
            prod.len(),
            "{name}: frozen station count {} != production {}; regenerate",
            rows.len(),
            prod.len()
        );
        for r in rows {
            let p = station_coords(&prod, StationRef::new(r.s, r.i));
            // Worst observed gap across the roster is ~2e-10 m; 1e-6 leaves a wide
            // margin for cross-platform float variation while still being microns
            // against metre-scale corrections, so any real regression is caught.
            assert!(
                (p.x - r.x).abs() < 1e-6 && (p.y - r.y).abs() < 1e-6,
                "{name} s{}:{}: production ({:.6}, {:.6}) != gama gold ({:.6}, {:.6})",
                r.s,
                r.i,
                p.x,
                p.y,
                r.x,
                r.y,
            );
        }
    }
}
