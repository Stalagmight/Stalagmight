//! [#756] Combine Lines beside a line held to the rest by loop closures
//! alone, through `AppState`, on the station graph. The issue's survey is
//! not a valid model any more (a closure may tie two pieces only when both
//! hold a datum, plan 2 invariant 7), so a `.stalx` v2 file cannot hold it:
//! the encoder refuses it and so does an open. What a survey the app leaves
//! can show is a snap torn into a part a zero datum holds, which the plan
//! gives the datum's reason.
//!
//! The model-level ports are `models::survey::edit::combine_port_misc_tests`
//! (`the_issue_survey_is_refused_and_the_sweep_settles_it`,
//! `a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum`).
//!
//! Fate of each 0.6.0 test: `the_issue_survey_written_to_a_file_opens_settled`
//! is adapted as `the_issue_survey_cannot_be_written_or_opened_as_a_v2_file`
//! (a load never heals, plan 6.6: the file is refused, not settled; a legacy
//! file holding it is settled by the frozen 0.6.0 open the migration runs);
//! `a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum_through_the_commands`
//! is ported on a survey built with the model's own operations (the Create
//! Line, Move, Merge, Set Zero Datum and snap commands are other packages').
//!
//! All data is synthetic: projects built in memory, files in a temporary
//! directory.

use super::topology_kit::{
    apart, assert_undo_redo_exact, close, empty, gps, hold, line_at, line_from, open, positions,
    project, save_close_reopen, shot, slot, snapshot, st,
};
use super::AppState;
use crate::core::Command;
use crate::models::survey::{ClosureStrategy, Project};
use crate::models::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, LineEnd, LineId, ShotData, StationId,
    TornTieKind, TornTieReason,
};
use crate::persistence::stalx_v2::{self, EncodeError, Extras, Mode};

fn raw(p: &Project, station: StationId) -> [f64; 2] {
    p.raw_positions().xy(station).unwrap()
}

/// A one-shot line `name` from station `from` to where `to` is.
fn bridge(p: &mut Project, name: &str, from: StationId, to: StationId) -> LineId {
    let (a, b) = (raw(p, from), raw(p, to));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let azimuth = dx.atan2(dy).to_degrees().rem_euclid(360.0);
    let leg = ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: dx.hypot(dy),
        ..ShotData::default()
    };
    line_from(p, name, from, vec![leg])
}

/// The issue's survey: K's end and O's start 5 mm apart, tied by a loop
/// closure; N on K's start, M on O's end (junctions); Z snapped to O's
/// station 1 and W starting on Z's end, which an ordinary loop closure ties
/// to M. K, O and Z are three pieces with no datum, held together by
/// closures alone.
fn issue_survey() -> Project {
    let mut p = empty("756");
    let k = line_at(
        &mut p,
        "K",
        [100.0, -40.0],
        vec![shot(90.0, 10.0), shot(80.0, 5.0)],
    );
    let k2 = raw(&p, st(&p, k, 2));
    let o = line_at(
        &mut p,
        "O",
        [k2[0] + 0.004, k2[1] - 0.003],
        vec![shot(0.0, 4.0), shot(300.0, 6.0)],
    );
    let (k0, o2) = (st(&p, k, 0), st(&p, o, 2));
    line_from(&mut p, "N", k0, vec![shot(0.0, 3.0)]);
    let m = line_from(&mut p, "M", o2, vec![shot(90.0, 3.0)]);
    let o1 = raw(&p, st(&p, o, 1));
    let z = line_at(&mut p, "Z", o1, vec![shot(180.0, 5.0)]);
    let z1 = st(&p, z, 1);
    let w = line_from(&mut p, "W", z1, vec![shot(90.0, 4.0)]);
    for (a, b, strategy) in [
        (slot(o, 1), slot(z, 0), ClosureStrategy::Move),
        (slot(w, 1), slot(m, 1), ClosureStrategy::Batch),
        (slot(k, 2), slot(o, 0), ClosureStrategy::Batch),
    ] {
        let (sa, sb) = (
            st(&p, a.line_id, a.station_index),
            st(&p, b.line_id, b.station_index),
        );
        p.push_closure(sa, Some(a.line_id), sb, Some(b.line_id), strategy);
    }
    p
}

/// A load never heals (plan 6.6): the issue's survey breaks invariant 7, so
/// the writer refuses to save it and a file that holds it anyway (edited by
/// hand) is refused at open, untouched, by the corrupt-file route. The
/// orphan-closure sweep settles it the way 0.6.0's open did (the snap and
/// the join closure become junctions, W's closure to M stays), and the
/// settled survey opens.
#[test]
fn the_issue_survey_cannot_be_written_or_opened_as_a_v2_file() {
    let p = issue_survey();
    assert!(
        !p.validate().is_empty(),
        "the issue's survey is not a valid model"
    );
    let refused = stalx_v2::encode(&p, &Extras::default(), Mode::Normal);
    assert!(
        matches!(refused, Err(EncodeError::Invalid { .. })),
        "{refused:?}"
    );

    let mut settled = p.clone();
    let report = settled.sweep_orphan_closures();
    assert_eq!(report.promoted.len(), 2, "the snap and the join closure");
    assert!(settled.validate().is_empty(), "{:?}", settled.validate());
    let w_to_m = p.closures[1].id;
    assert_eq!(
        settled.closures.iter().map(|c| c.id).collect::<Vec<_>>(),
        [w_to_m]
    );
    let opened = open(&settled);
    assert!(project(&opened.state).validate().is_empty());

    // Edited by hand: the survey without its closures is valid (each piece
    // has its placement); put the three closures into its file.
    let mut bare = p.clone();
    bare.closures.clear();
    let bare = open(&bare);
    let text = std::fs::read_to_string(&bare.path).unwrap();
    let after_placements = {
        let at = text.find("\"placements\":[\n").unwrap();
        at + text[at..].find("\n],\n").unwrap() + "\n],\n".len()
    };
    let closures: Vec<String> = p
        .closures
        .iter()
        .map(|c| {
            let strategy = if c.strategy == ClosureStrategy::Move { "move" } else { "batch" };
            format!(
                "{{\"id\":{},\"a\":{},\"a_via\":{},\"b\":{},\"b_via\":{},\"strategy\":\"{strategy}\"}}",
                c.id,
                c.a,
                c.a_via.unwrap(),
                c.b,
                c.b_via.unwrap(),
            )
        })
        .collect();
    let edited = format!(
        "{}\"closures\":[\n{}\n],\n{}",
        &text[..after_placements],
        closures.join(",\n"),
        &text[after_placements..]
    );
    let path = bare.path.with_file_name("hand-edited.stalx");
    std::fs::write(&path, edited.as_bytes()).unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: path.clone() });
    assert!(!r.success, "a hand-edited invalid file opens");
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .contains("cannot be opened normally"),
        "{:?}",
        r.error
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        edited,
        "the file is untouched"
    );
    assert!(state.get_project_state().is_none());
}

/// The one change a survey the app leaves can show (#756): two surveys each
/// held by a zero datum (N's far end on K's side, D's far end on O's side,
/// D leaving O's end and coming back onto O's station 1, snapped there) and
/// a loop closure from K's end to O's start (allowed between two surveys
/// held by zero datums). Combining K and O keeping K, O moves and leaves the
/// junction at its end, and the snap from O into D's part tears too. Its
/// reason is the zero datum that holds D, and the dialog names D as the line
/// that stays. One undo restores it exactly; a save and a reopen settle
/// nothing more and move nothing.
#[test]
fn a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum() {
    let mut p = empty("Held");
    let k = line_at(
        &mut p,
        "K",
        [100.0, -40.0],
        vec![shot(90.0, 10.0), shot(80.0, 5.0)],
    );
    let k2 = raw(&p, st(&p, k, 2));
    let o = line_at(
        &mut p,
        "O",
        [k2[0] + 0.004, k2[1] - 0.003],
        vec![shot(0.0, 4.0), shot(300.0, 6.0)],
    );
    let (k0, o1, o2) = (st(&p, k, 0), st(&p, o, 1), st(&p, o, 2));
    let n = line_from(&mut p, "N", k0, vec![shot(0.0, 3.0)]);
    let d = bridge(&mut p, "D", o2, o1);
    let (n1, d1) = (st(&p, n, 1), st(&p, d, 1));
    hold(&mut p, n1, gps(20.0, -87.0));
    hold(&mut p, d1, gps(20.0001, -87.0));
    close(&mut p, slot(k, 2), slot(o, 0), ClosureStrategy::Batch);
    let snap = close(&mut p, slot(o, 1), slot(d, 1), ClosureStrategy::Move);
    let aligned = p.closure_alignment(&p.raw_positions());
    assert_eq!(aligned, [false, true], "setup: a snap");
    let opened = open(&p);
    let state = &opened.state;

    let candidates = state.combine_candidates(slot(k, 2)).unwrap();
    let candidate = candidates
        .iter()
        .find(|c| [c.line_a, c.line_b] == [k, o])
        .unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(o));
    let mut torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|t| {
            // The end the dialog names as held: the one that does not move.
            let held = (t.reason == TornTieReason::ZeroDatum)
                .then(|| {
                    [t.station_a, t.station_b]
                        .into_iter()
                        .find(|at| !plan.moved_lines.contains(&at.line_id))
                })
                .flatten();
            (t.kind, t.reason, held)
        })
        .collect();
    torn.sort_by_key(|(kind, ..)| *kind == TornTieKind::SnappedClosure);
    assert_eq!(
        torn,
        [
            (
                TornTieKind::Merge,
                TornTieReason::ZeroDatum,
                Some(slot(d, 0))
            ),
            (
                TornTieKind::SnappedClosure,
                TornTieReason::ZeroDatum,
                Some(slot(d, 1))
            ),
        ]
    );
    let before = snapshot(state);

    let result = state.execute_combine_lines(
        k,
        o,
        CombineLinesOptions {
            name: "Joined".to_string(),
            color: "#abcdef".to_string(),
            swap_lr: true,
            comment_pick: CombinePick::Kept,
            name_pick: CombinePick::Kept,
            join: Some(CombineJoinEnds {
                kept: LineEnd::End,
                other: LineEnd::Start,
            }),
        },
    );
    assert!(result.success, "{:?}", result.error);
    let combined = project(state);
    let torn_snap = combined.closures.iter().find(|c| c.id == snap).unwrap();
    assert_eq!(
        torn_snap.strategy,
        ClosureStrategy::Batch,
        "an ordinary closure now"
    );
    assert!(
        combined
            .closure_alignment(&combined.raw_positions())
            .iter()
            .all(|a| !a),
        "no snap is left"
    );
    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "datum-held snap");

    let saved = project(state);
    save_close_reopen(state, &opened.path);
    let reopened = project(state);
    assert_eq!(reopened.closures, saved.closures);
    let (was, now) = (positions(&saved), positions(&reopened));
    assert_eq!(was.len(), now.len());
    for ((at, p), (_, q)) in was.into_iter().zip(now) {
        assert!(apart(p, q) < 1e-9, "the reopen moved {at:?}");
    }
}
