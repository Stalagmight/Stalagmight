//! The precision of the Closures Computed View's solve (migration fuzz farm
//! seed 34022454): a part of the survey that hangs off one weak shot, with
//! weights 1e14 apart inside it. Synthetic data only.
//!
//! In the exact least squares a shot that is the only link between a part
//! and everything holding it carries no current when every misclosure is
//! inside that part, so the part's attachment station stays where the raw
//! layout puts it and no station moves further than the misclosures (the
//! maximum principle, `migrate_fuzz_tests::computed_view_within_its_misclosures`).
//! Solved for absolute coordinates, the right-hand side's rounding (a
//! 1e5 m coordinate under a 3e8 weight) and the diagonal's rounding were
//! spurious currents the weak link turned into hundreds of metres.

use crate::config::ClosureAdjustmentConfig;
use crate::{GpsCoordinates, ShotData, StationId};

use super::test_helpers::{add, at, flat, meta, project};
use super::{ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum};

/// Line quality 0.25: the floored sigma of a zero-length shot on a grid axis
/// is 5.5e-5 m (a weight of 3.4e8), and a 100 km shot's sigma across it is
/// 436 m (a weight of 5e-6).
const FINE: f64 = 0.25;

fn line(p: &mut Project, start: StationId, quality: f64, shots: Vec<ShotData>) -> u32 {
    let name = format!("L{}", p.next_line_id());
    match p.add_line(
        LineMeta {
            quality,
            ..meta(&name)
        },
        StartAt::Station(start),
        shots,
    ) {
        Ok(id) => id,
        Err(error) => panic!("add_line refused: {error}"),
    }
}

/// A datum at the origin, a 3 m shot east, then (quality 0.25) a zero-length
/// connector, 100 km north and 72.4 km east to `C`; from `C` a four-shot
/// loop whose far end `H` a `Tie` closure ties back to `C` (an 11.4 m
/// misclosure, every misclosure of the survey), and from `H` a zero-length
/// connector. Returns the project, `C` and the misclosure.
fn far_part_on_one_weak_link() -> (Project, StationId, f64) {
    let mut p = project();
    let start = add(&mut p, StartAt::New, vec![flat(90.0, 3.0)]);
    let origin = at(&p, start, 0);
    p.push_datum(ZeroDatum::new(
        origin,
        GpsCoordinates {
            latitude: 20.0,
            longitude: -87.0,
            depth_reference: 0.0,
        },
    ));
    let east = at(&p, start, 1);
    let bridge = line(
        &mut p,
        east,
        FINE,
        vec![flat(0.0, 0.0), flat(0.0, 100_000.0), flat(90.0, 72_398.75)],
    );
    let c = at(&p, bridge, 3);
    let lap = line(
        &mut p,
        c,
        1.0,
        vec![
            flat(68.13, 9.818),
            flat(264.5, 11.97),
            flat(116.7, 3.285),
            flat(124.35, 11.84),
        ],
    );
    let h = at(&p, lap, 4);
    line(&mut p, h, FINE, vec![flat(0.0, 0.0)]);
    p.push_closure(c, None, h, None, ClosureStrategy::Tie);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    let raw = p.raw_positions();
    let (Some(rc), Some(rh)) = (raw.xy(c), raw.xy(h)) else {
        panic!("C and H are laid out");
    };
    (p, c, (rc[0] - rh[0]).hypot(rc[1] - rh[1]))
}

#[test]
fn a_part_hanging_off_one_weak_shot_moves_only_by_its_own_misclosure() {
    let (p, c, misclosure) = far_part_on_one_weak_link();
    assert!(misclosure > 10.0 && misclosure < 12.0, "{misclosure}");
    let raw = p.raw_positions();
    let adjusted = p.adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default());
    assert!(adjusted.failures().is_empty(), "{:?}", adjusted.failures());
    let moved = |s: StationId| -> f64 {
        let (Some(r), Some(q)) = (raw.xy(s), adjusted.xy(s)) else {
            panic!("station {s} is positioned");
        };
        (q[0] - r[0]).hypot(q[1] - r[1])
    };
    // The 100 km shot is C's only link to the datum and the misclosure is
    // all beyond it: it carries nothing, so C stays put.
    assert!(moved(c) < 1e-6, "C moved {} m", moved(c));
    let largest = raw.iter().map(|(s, _)| moved(s)).fold(0.0_f64, f64::max);
    assert!(
        largest <= misclosure + 1e-6,
        "a station moved {largest} m; the misclosure is {misclosure} m"
    );
    // H closes onto C: the loop takes the whole misclosure.
    assert!(largest > misclosure - 1e-6, "{largest}");
}
