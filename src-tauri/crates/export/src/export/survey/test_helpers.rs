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
    pub(crate) mid: LineId,
    pub(crate) island: LineId,
    pub(crate) free: LineId,
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
        mid,
        island,
        free,
        t,
        n,
        b,
        m,
        e,
        f,
    }
}
