//! Package RS-FX4 (plan 2026-11-23, kept and archived stations): records
//! carry the identity they were archived under, so a merge then a Detach
//! gives back a merged junction whole (model review finding 4). Fully
//! synthetic data.

use super::ops_test_helpers::{assert_valid, free_line, ok, run};
use super::test_helpers::{at, project};
use super::{LineId, Project, StationId};

/// Give each station of `line` the name `{prefix}{index}`.
fn name_line(p: &mut Project, line: LineId, prefix: &str) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
}

/// Give station `s` a comment and an external id of its own.
fn mark(p: &mut Project, s: StationId, text: &str) {
    let station = p.station_mut(s).unwrap();
    station.comment = Some(format!("{text} note"));
    station.external = Some(format!("{text}-ext"));
}

/// Every slot's node, line by line.
fn chains(p: &Project) -> Vec<(LineId, Vec<StationId>)> {
    p.lines()
        .iter()
        .map(|l| (l.id, l.stations().collect()))
        .collect()
}

/// Junction X (its own line LX native, LX2 named by X's record R from an
/// earlier merge) and a free line LY: `(project, lx, lx2, ly, x, r, y)`.
/// Every identity has a name, a comment and an external id of its own.
fn junction_and_line() -> (
    Project,
    LineId,
    LineId,
    LineId,
    StationId,
    StationId,
    StationId,
) {
    let mut p = project();
    let lx = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let lx2 = free_line(&mut p, [0.0, 50.0], run(90.0, 2, 10.0));
    let ly = free_line(&mut p, [0.0, 100.0], run(90.0, 2, 10.0));
    name_line(&mut p, lx, "X");
    name_line(&mut p, lx2, "R");
    name_line(&mut p, ly, "Y");
    let (x, r, y) = (at(&p, lx, 1), at(&p, lx2, 0), at(&p, ly, 1));
    mark(&mut p, x, "X");
    mark(&mut p, r, "R");
    mark(&mut p, y, "Y");
    ok(p.merge_stations(r, x, x, &[]));
    assert_valid(&p);
    assert_eq!(p.topology().lines_of(x), vec![lx, lx2]);
    assert!(p.record_naming(x, lx2).is_some_and(|rec| rec.id == r));
    (p, lx, lx2, ly, x, r, y)
}

/// Model review finding 4, the exact sequence: X (with R inside it) merged
/// into Y keeping Y, then Detach of LX. X comes back with LX and LX2, R
/// still inside it, and Y is as it was: every station byte for byte
/// (names, comments, externals, ids, records) and every slot. The same for
/// a Detach of LX2 (R's line: R was X's), and for either merge direction
/// (Y's piece moving or staying put). Before RS-FX4, R moved flat onto Y's
/// node, so X came back on LX alone and LX2 kept showing Y.
#[test]
fn detaching_a_line_of_a_merged_junction_brings_the_whole_junction_back() {
    let (before, lx, lx2, _ly, x, r, y) = junction_and_line();
    // `merge_stations(a, b, ..)` moves a's piece onto b's: X's piece moves
    // in the first, Y's in the second (the kept station is the moving one,
    // RS 4.1 step 3).
    for (a, b) in [(x, y), (y, x)] {
        let mut merged = before.clone();
        let out = ok(merged.merge_stations(a, b, y, &[]));
        assert_eq!((out.value.kept, out.value.archived), (y, x));
        assert_valid(&merged);
        assert_eq!(merged.shown_name(at(&merged, lx2, 0)), Some("Y1"));
        for line in [lx, lx2] {
            let naming = merged.detach_naming(y, line, None).unwrap();
            assert!(naming.restored);
            assert_eq!((naming.id, naming.name.as_str()), (x, "X1"));
            assert_eq!(naming.comment.as_deref(), Some("X note"));
            let other = if line == lx { lx2 } else { lx };
            assert_eq!(
                naming.together,
                vec![other],
                "the lines that come off together"
            );
            let mut q = merged.clone();
            let detached = ok(q.detach_line(y, line, &[], None));
            assert_eq!(detached.value.new_station, x);
            assert_valid(&q);
            assert_eq!(
                chains(&q),
                chains(&before),
                "detach of {line} after {a}->{b}"
            );
            assert_eq!(
                q.stations, before.stations,
                "detach of {line} after {a}->{b}"
            );
            // R is still X's record, and a second Detach gives it back.
            assert!(q.record_naming(x, lx2).is_some_and(|rec| rec.id == r));
            ok(q.detach_line(x, lx2, &[], None));
            assert_eq!(q.shown_name(at(&q, lx2, 0)), Some("R0"));
        }
    }
}
