//! The GNU Gama gold standard on the station-graph least squares.
//!
//! Port of `production_matches_gama_golden` (`models/src/project/tests.rs`):
//! the per-axis closure solve is a 1-D levelling network, which gama-local
//! adjusts exactly; its adjusted coordinates are frozen in
//! `src-tauri/tests/fixtures/gama_golden.json` (keyed by line id `s` and chain
//! index `i`). The networks are rebuilt here directly on the new model, with
//! the same seeded generators and in the same order, so the shots are
//! bit-identical to the legacy builders: a merge at two line starts is one
//! junction station, every other merge of the grid is a shared station, and
//! every closure is a `Batch` closure.

use std::collections::BTreeMap;

use crate::config::ClosureAdjustmentConfig;
use crate::{GpsCoordinates, LineId, ShotData, StationId, StationRef};

use super::test_helpers::{meta, project};
use super::{ClosureStrategy, Project, ZeroDatum};

#[derive(serde::Deserialize)]
struct GoldStation {
    s: u32,
    i: u32,
    x: f64,
    y: f64,
}

/// Today's xorshift in [0, 1).
fn xorshift01(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 11) as f64) / ((1u64 << 53) as f64)
}

fn shot(az: f64, len: f64) -> ShotData {
    ShotData {
        azimuth_in: az,
        azimuth_out: az,
        length: len,
        ..ShotData::default()
    }
}

fn drift_shots(st: &mut u64, n: usize, base_az: f64) -> Vec<ShotData> {
    (0..n)
        .map(|_| {
            let az = base_az + (xorshift01(st) - 0.5) * 80.0;
            let len = 4.0 + xorshift01(st) * 6.0;
            shot(az, len)
        })
        .collect()
}

/// A network under construction: stations minted up front, lines over them.
struct Net {
    p: Project,
}

impl Net {
    fn new() -> Self {
        Self { p: project() }
    }

    fn station(&mut self) -> StationId {
        let id = self.p.mint_station_id().unwrap_or_default();
        let _ = self
            .p
            .insert_station(super::Station::new(id, format!("s{id}")));
        id
    }

    /// A line from `start` through fresh stations.
    fn line_from(
        &mut self,
        start: StationId,
        shots: Vec<ShotData>,
        quality: f64,
    ) -> (LineId, Vec<StationId>) {
        let mut chain = vec![start];
        for _ in 0..shots.len() {
            let s = self.station();
            chain.push(s);
        }
        self.line_on(&chain, shots, quality)
    }

    fn line_on(
        &mut self,
        chain: &[StationId],
        shots: Vec<ShotData>,
        quality: f64,
    ) -> (LineId, Vec<StationId>) {
        let name = format!("L{}", self.p.next_line_id());
        let mut m = meta(&name);
        m.quality = quality;
        let id = match self.p.add_line_on_stations(m, chain, shots) {
            Ok(id) => id,
            Err(error) => panic!("{error}"),
        };
        (id, chain.to_vec())
    }

    fn datum_at_origin(&mut self, station: StationId) {
        self.p.push_datum(ZeroDatum::new(
            station,
            GpsCoordinates {
                latitude: 0.0,
                longitude: 0.0,
                depth_reference: 0.0,
            },
        ));
    }

    fn closure(&mut self, a: StationId, b: StationId) {
        self.p
            .push_closure(a, None, b, None, ClosureStrategy::Batch);
    }

    fn done(self) -> Project {
        let violations = self.p.validate();
        assert!(violations.is_empty(), "{violations:?}");
        self.p
    }
}

/// Two lines from one datum station, closed at their far ends.
fn two_paths(a_shots: Vec<ShotData>, b_shots: Vec<ShotData>) -> Project {
    let mut net = Net::new();
    let o = net.station();
    let (_, a) = net.line_from(o, a_shots, 1.0);
    let (_, b) = net.line_from(o, b_shots, 1.0);
    net.datum_at_origin(o);
    net.closure(a[a.len() - 1], b[b.len() - 1]);
    net.done()
}

fn axis_aligned_loop() -> Project {
    two_paths(
        vec![shot(90.0, 10.0), shot(0.0, 10.0)],
        vec![shot(0.0, 10.0), shot(90.0, 10.2)],
    )
}

fn oblique_loop(len_b: f64) -> Project {
    two_paths(
        vec![shot(30.0, 10.0), shot(90.0, 10.0)],
        vec![shot(90.0, 10.0), shot(30.0, len_b)],
    )
}

fn fuzz_loop(seed: u64) -> Project {
    let mut st = seed | 1;
    let na = 2 + (xorshift01(&mut st) * 3.0) as usize;
    let nb = 2 + (xorshift01(&mut st) * 3.0) as usize;
    let mk = |st: &mut u64, n: usize| -> Vec<ShotData> {
        (0..n)
            .map(|_| {
                let az = xorshift01(st) * 360.0;
                let len = 5.0 + xorshift01(st) * 10.0;
                shot(az, len)
            })
            .collect()
    };
    let a = mk(&mut st, na);
    let b = mk(&mut st, nb);
    two_paths(a, b)
}

fn big_loop(n: usize, seed: u64) -> Project {
    let mut st = seed | 1;
    let a = drift_shots(&mut st, n, 60.0);
    let b = drift_shots(&mut st, n, 30.0);
    two_paths(a, b)
}

fn ladder(segs: usize, seg: usize, seed: u64) -> Project {
    let n = segs * seg;
    let mut st = seed | 1;
    let mut net = Net::new();
    let o = net.station();
    let (_, a) = net.line_from(o, drift_shots(&mut st, n, 90.0), 1.0);
    let (_, b) = net.line_from(o, drift_shots(&mut st, n, 90.0), 1.0);
    net.datum_at_origin(o);
    for k in 1..=segs {
        net.closure(a[k * seg], b[k * seg]);
    }
    net.done()
}

/// Rows (mostly east) and columns (mostly north): row 0 shares its stations
/// with every column's start, column 0 shares its stations with every row's
/// start; every interior node is two stations joined by a closure.
fn grid(rows: usize, cols: usize, seed: u64) -> Project {
    let mut st = seed | 1;
    let row_shots: Vec<Vec<ShotData>> = (0..rows)
        .map(|_| drift_shots(&mut st, cols - 1, 90.0))
        .collect();
    let col_shots: Vec<Vec<ShotData>> = (0..cols)
        .map(|_| drift_shots(&mut st, rows - 1, 0.0))
        .collect();
    let mut net = Net::new();
    let row0: Vec<StationId> = (0..cols).map(|_| net.station()).collect();
    let col0: Vec<StationId> = std::iter::once(row0[0])
        .chain((1..rows).map(|_| net.station()))
        .collect();
    let mut row_chains: Vec<Vec<StationId>> = vec![row0.clone()];
    for &start in &col0[1..] {
        let mut chain = vec![start];
        chain.extend((1..cols).map(|_| net.station()));
        row_chains.push(chain);
    }
    let mut col_chains: Vec<Vec<StationId>> = vec![col0.clone()];
    for &start in &row0[1..] {
        let mut chain = vec![start];
        chain.extend((1..rows).map(|_| net.station()));
        col_chains.push(chain);
    }
    for (chain, shots) in row_chains.iter().zip(row_shots) {
        net.line_on(chain, shots, 1.0);
    }
    for (chain, shots) in col_chains.iter().zip(col_shots) {
        net.line_on(chain, shots, 1.0);
    }
    net.datum_at_origin(row0[0]);
    for r in 1..rows {
        for c in 1..cols {
            net.closure(row_chains[r][c], col_chains[c][r]);
        }
    }
    net.done()
}

fn fuzz_paths(seed: u64, paths: usize, max_shots: usize, len_min: f64, len_max: f64) -> Project {
    let mut st = seed | 1;
    let mut net = Net::new();
    let o = net.station();
    let mut ends = Vec::new();
    for _ in 0..paths {
        let n = 2 + (xorshift01(&mut st) * (max_shots as f64 - 1.0)) as usize;
        let shots: Vec<ShotData> = (0..n)
            .map(|_| {
                let az = xorshift01(&mut st) * 360.0;
                let len = len_min + xorshift01(&mut st) * (len_max - len_min);
                shot(az, len)
            })
            .collect();
        let q = 0.5 + xorshift01(&mut st) * 2.5;
        let (_, chain) = net.line_from(o, shots, q);
        ends.push(chain[chain.len() - 1]);
    }
    net.datum_at_origin(o);
    for &end in &ends[1..] {
        net.closure(ends[0], end);
    }
    net.done()
}

fn build_star(legs: &[(f64, f64, f64)]) -> Project {
    let mut net = Net::new();
    let o = net.station();
    let ends: Vec<StationId> = legs
        .iter()
        .map(|&(az, len, q)| net.line_from(o, vec![shot(az, len)], q).1[1])
        .collect();
    net.datum_at_origin(o);
    for &end in &ends[1..] {
        net.closure(ends[0], end);
    }
    net.done()
}

/// The roster of `gama_golden_networks`, same names and parameters.
fn networks() -> Vec<(String, Project)> {
    let mut nets: Vec<(String, Project)> = vec![
        ("axis_aligned_loop".to_string(), axis_aligned_loop()),
        ("oblique_loop".to_string(), oblique_loop(10.5)),
        (
            "star_mixed".to_string(),
            build_star(&[(90.0, 10.0, 1.0), (0.0, 8.0, 1.0), (45.0, 12.0, 2.0)]),
        ),
        ("loop_101".to_string(), big_loop(50, 0x10001)),
        ("ladder_1001".to_string(), ladder(20, 25, 0x001A_DDE2)),
        ("grid_12x12".to_string(), grid(12, 12, 0x6121D)),
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
            fuzz_paths(0x00F1_A5C0, 2, 9, 0.3, 25.0),
        ),
    ];
    for seed in [0xA11CE_u64, 0xBEEF, 0x00C0_FFEE, 0x0D15_EA5E, 0x5EED] {
        nets.push((format!("fuzz_{seed}"), fuzz_loop(seed)));
    }
    nets
}

#[test]
fn station_graph_least_squares_matches_gama_golden() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/gama_golden.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("missing {path}"));
    let frozen: BTreeMap<String, Vec<GoldStation>> = serde_json::from_str(&data).unwrap();
    let config = ClosureAdjustmentConfig::default();
    let nets: BTreeMap<String, Project> = networks().into_iter().collect();
    assert!(!frozen.is_empty());
    assert_eq!(
        frozen.keys().collect::<Vec<_>>(),
        nets.keys().collect::<Vec<_>>(),
        "roster and frozen fixture networks differ"
    );
    let mut worst = 0.0_f64;
    for (name, rows) in &frozen {
        let project = &nets[name];
        let adjusted = project.adjusted_positions(&config);
        assert!(adjusted.failures().is_empty(), "{name}: the solve fails");
        let slots: BTreeMap<StationRef, [f64; 2]> = adjusted
            .slots(project)
            .into_iter()
            .map(|s| (s.slot, [s.x, s.y]))
            .collect();
        assert_eq!(rows.len(), slots.len(), "{name}: frozen station count");
        for r in rows {
            let [x, y] = slots[&StationRef::new(r.s, r.i)];
            let gap = (x - r.x).abs().max((y - r.y).abs());
            worst = worst.max(gap);
            assert!(
                gap < 1e-6,
                "{name} s{}:{}: ({x:.9}, {y:.9}) != gama gold ({:.9}, {:.9})",
                r.s,
                r.i,
                r.x,
                r.y
            );
        }
    }
    // Today's worst gap is about 2e-10 m; the port must stay in that class.
    assert!(worst < 1e-8, "worst gap {worst:e} m");
}
