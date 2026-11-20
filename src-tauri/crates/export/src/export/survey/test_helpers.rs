//! Synthetic station-graph surveys for the exporter tests. Fully synthetic:
//! fixed creation time, uuids derived from a counter, so the same survey is
//! built to the same bytes every time.

use chrono::{TimeZone, Utc};

use crate::models::survey::{ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum};
use crate::models::{GpsCoordinates, LineId, ShotData, StationId};

/// A v4-shaped uuid derived from `n` (so the SpeleoDB sidecar keeps it).
pub(crate) fn uuid_for(n: u32) -> String {
    let mut bytes = [0u8; 16];
    bytes[..4].copy_from_slice(&n.to_le_bytes());
    bytes[4] = 0x5a;
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// One shot: both azimuth readings, a slope length, a depth pair, and a uuid
/// derived from `n`.
pub(crate) fn shot(n: u32, azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        uuid: Some(uuid_for(n)),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..ShotData::default()
    }
}

/// `count` shots from uuid counter `first`, each `length` long on `azimuth`,
/// descending `step` per shot from `depth`.
pub(crate) fn run(
    first: u32,
    count: u32,
    azimuth: f64,
    length: f64,
    depth: f64,
    step: f64,
) -> Vec<ShotData> {
    (0..count)
        .map(|i| {
            let d = depth + step * f64::from(i);
            shot(first + i, azimuth, length, d, d + step)
        })
        .collect()
}

pub(crate) fn meta(name: &str, color: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: color.to_string(),
        ..LineMeta::default()
    }
}

/// A GPS position `east_m` / `north_m` metres from the synthetic base.
pub(crate) fn gps_at(east_m: f64, north_m: f64) -> GpsCoordinates {
    let lat = 20.5 + north_m / 111_320.0;
    let lon = -87.25 + east_m / (111_320.0 * 20.5_f64.to_radians().cos());
    GpsCoordinates {
        latitude: lat,
        longitude: lon,
        depth_reference: 0.0,
    }
}

/// An empty project with a fixed creation instant.
pub(crate) fn project(name: &str) -> Project {
    Project::new_at(
        name,
        Some("Synthetic".to_string()),
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .single()
            .unwrap_or_default(),
    )
}

/// The stations and lines of [`cave`].
pub(crate) struct Cave {
    pub(crate) project: Project,
    pub(crate) trunk: LineId,
    pub(crate) north: LineId,
    pub(crate) back: LineId,
    /// Trunk's chain (`T0..T4`).
    pub(crate) t: Vec<StationId>,
    /// North's chain (`N0 = T2 .. N3`).
    pub(crate) n: Vec<StationId>,
    /// Back's chain (`B0, B1, T4`): it ENDS at the junction.
    pub(crate) b: Vec<StationId>,
    /// Mid's chain (`M0, N2, M2`): it passes the junction mid-line.
    pub(crate) m: Vec<StationId>,
    /// Island's chain (`E0..E3`), its datum on `E2`.
    pub(crate) e: Vec<StationId>,
    /// Free's chain (`F0..F2`), placed at (10, 20).
    pub(crate) f: Vec<StationId>,
}

fn chain(p: &Project, line: LineId) -> Vec<StationId> {
    p.line(line)
        .map(|l| l.stations().collect())
        .unwrap_or_default()
}

/// A survey with every shape the exporters walk: a trunk with a datum (the
/// frame) at its start, a branch off a mid-line junction (North), a line
/// that ends at a junction (Back), a line passing a junction mid-line (Mid),
/// a Batch loop closure (North's end to Back's start), a GPS fix (Mid's end),
/// a second cave piece whose datum is mid-line (Island), a datum-less free
/// piece (Free), station comments (one on a junction, one on the frame
/// station, which becomes a START) and a renamed junction.
pub(crate) fn cave() -> Cave {
    let mut p = project("Synthetic cave");
    let ok = |r: Result<LineId, crate::models::survey::BuildError>| match r {
        Ok(id) => id,
        Err(e) => panic!("build refused: {e}"),
    };
    let mut trunk_shots = run(0, 4, 90.0, 10.0, 5.0, 1.0);
    for s in &mut trunk_shots {
        (s.left, s.right, s.up, s.down) = (Some(1.0), Some(2.0), Some(0.5), Some(1.5));
    }
    trunk_shots[1].comment = Some("shot comment".to_string());
    let trunk = ok(p.add_line(meta("Trunk", "#336699"), StartAt::New, trunk_shots));
    let t = chain(&p, trunk);
    let north = ok(p.add_line(
        meta("North", "#aa5500"),
        StartAt::Station(t[2]),
        run(10, 3, 0.0, 8.0, 7.0, 0.5),
    ));
    let n = chain(&p, north);

    let b0 = p.mint_station("0").unwrap_or_default();
    let b1 = p.mint_station("1").unwrap_or_default();
    let mut back_shots = run(20, 2, 135.0, 6.0, 8.5, 0.25);
    back_shots[0].pitch_in = Some(4.0);
    let back = ok(p.add_line_on_stations(meta("Back", "#00aa55"), &[b0, b1, t[4]], back_shots));
    let b = chain(&p, back);

    let m0 = p.mint_station("0").unwrap_or_default();
    let m2 = p.mint_station("9").unwrap_or_default();
    let mid = ok(p.add_line_on_stations(
        meta("Mid", "#5500aa"),
        &[m0, n[2], m2],
        run(30, 2, 270.0, 5.0, 8.0, 0.0),
    ));
    let m = chain(&p, mid);

    let island = ok(p.add_line(
        meta("Island", "#999999"),
        StartAt::New,
        run(40, 3, 45.0, 7.0, 2.0, 1.0),
    ));
    let e = chain(&p, island);
    let free = ok(p.add_line(
        meta("Free", "#123456"),
        StartAt::New,
        run(50, 2, 200.0, 4.0, 3.0, 0.5),
    ));
    let f = chain(&p, free);

    p.datums.push(ZeroDatum {
        via: Some(trunk),
        entrance: Some(true),
        ..ZeroDatum::new(t[0], gps_at(0.0, 0.0))
    });
    p.datums.push(ZeroDatum {
        via: Some(island),
        ..ZeroDatum::new(e[2], gps_at(300.0, 200.0))
    });
    p.datums.push(ZeroDatum {
        via: Some(mid),
        ..ZeroDatum::new(m[2], gps_at(10.0, 30.0))
    });
    if let Err(e) = p.set_placement(f[0], 10.0, 20.0) {
        panic!("placement refused: {e}");
    }
    p.push_closure(n[3], Some(north), b[0], Some(back), ClosureStrategy::Batch);

    if let Some(s) = p.station_mut(t[2]) {
        s.name = "Hub".to_string();
        s.comment = Some("junction comment".to_string());
    }
    if let Some(s) = p.station_mut(t[0]) {
        s.comment = Some("entrance comment".to_string());
    }
    if let Some(s) = p.station_mut(m[0]) {
        s.comment = Some("mid comment".to_string());
    }
    let violations = p.validate();
    assert!(
        violations.is_empty(),
        "synthetic cave invalid: {violations:?}"
    );
    Cave {
        project: p,
        trunk,
        north,
        back,
        t,
        n,
        b,
        m,
        e,
        f,
    }
}

/// A slot: `(line, chain index)`.
pub(crate) type At = (LineId, u32);

/// A survey built line by line, the way the exporter tests describe one: a
/// free line is placed at the origin (as 0.6.0 placed an unconnected line),
/// a branch starts on an existing station, a merge joins two stations (the
/// piece of the first moves onto the second), and every edit goes through
/// the model's own operations, so the result always validates.
pub(crate) struct Survey {
    pub(crate) project: Project,
}

impl Survey {
    /// An empty survey named `name`, with no location.
    pub(crate) fn new(name: &str) -> Self {
        Self::located(name, None)
    }

    /// An empty survey with a location and a fixed creation instant.
    pub(crate) fn located(name: &str, location: Option<&str>) -> Self {
        let mut project = project(name);
        project.location = location.map(str::to_string);
        Self { project }
    }

    /// A line starting on a new station, as a free piece placed at the
    /// origin.
    pub(crate) fn line(&mut self, meta: LineMeta, shots: Vec<ShotData>) -> LineId {
        let line = match self.project.add_line(meta, StartAt::New, shots) {
            Ok(id) => id,
            Err(e) => panic!("line refused: {e}"),
        };
        let start = self.station(line, 0);
        if let Err(e) = self.project.set_placement(start, 0.0, 0.0) {
            panic!("placement refused: {e}");
        }
        line
    }

    /// A line named `name` (colour `color`) starting on a new station.
    pub(crate) fn named(&mut self, name: &str, color: &str, shots: Vec<ShotData>) -> LineId {
        self.line(meta(name, color), shots)
    }

    /// A line starting on the existing station at `from`.
    pub(crate) fn branch(&mut self, meta: LineMeta, from: At, shots: Vec<ShotData>) -> LineId {
        let start = self.station(from.0, from.1);
        match self.project.add_line(meta, StartAt::Station(start), shots) {
            Ok(id) => id,
            Err(e) => panic!("branch refused: {e}"),
        }
    }

    /// The station at slot `(line, index)`.
    pub(crate) fn station(&self, line: LineId, index: u32) -> StationId {
        match self
            .project
            .station_at(crate::models::StationRef::new(line, index))
        {
            Some(id) => id,
            None => panic!("no station at ({line}, {index})"),
        }
    }

    /// Merge the station at `a` with the one at `b` (the model's Merge
    /// Stations: `a`'s piece moves onto `b` unless that would move a placed
    /// piece).
    pub(crate) fn merge(&mut self, a: At, b: At) {
        let (a, b) = (self.station(a.0, a.1), self.station(b.0, b.1));
        if let Err(e) = self.project.merge_stations(a, b) {
            panic!("merge refused: {e}");
        }
    }

    /// Set a zero datum on the slot `at`.
    pub(crate) fn datum(&mut self, at: At, gps: GpsCoordinates) {
        let slot = crate::models::StationRef::new(at.0, at.1);
        if let Err(e) = self.project.set_datum_at(slot, gps, None) {
            panic!("datum refused: {e}");
        }
    }

    /// A loop closure between the slots `a` and `b`.
    pub(crate) fn closure(&mut self, a: At, b: At, strategy: ClosureStrategy) {
        let (sa, sb) = (self.station(a.0, a.1), self.station(b.0, b.1));
        self.project
            .push_closure(sa, Some(a.0), sb, Some(b.0), strategy);
    }

    /// Set the comment of the station at `at`.
    pub(crate) fn comment(&mut self, at: At, text: &str) {
        let id = self.station(at.0, at.1);
        if let Some(station) = self.project.station_mut(id) {
            station.comment = Some(text.to_string());
        }
    }

    /// Rename the station at `at`.
    pub(crate) fn rename(&mut self, at: At, name: &str) {
        let id = self.station(at.0, at.1);
        if let Some(station) = self.project.station_mut(id) {
            station.name = name.to_string();
        }
    }

    /// The finished survey (it must validate).
    pub(crate) fn build(self) -> Project {
        let violations = self.project.validate();
        assert!(violations.is_empty(), "test survey invalid: {violations:?}");
        self.project
    }
}

/// A shot with both azimuth readings `azimuth`, `length` long, flat at
/// depth 0 (no uuid: the model mints one).
pub(crate) fn flat(length: f64, azimuth: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..ShotData::default()
    }
}
