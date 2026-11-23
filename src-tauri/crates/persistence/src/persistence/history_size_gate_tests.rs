//! The undo size gate on the station-graph model (plan 8.1), ported from
//! `core/state/undo_size_gate_tests.rs`: on the same synthetic 5,000- and
//! 20,000-shot surveys, built with the survey API, every gated action's
//! history entry (the persisted JSON, label included) is at most 1% of the
//! `.stalx` v2 file of the survey (`stalx_v2::encode`, empty history), a
//! paste costs a fixed part plus at most 40 bytes per changed cell, and
//! every undo and redo lands on the recorded state to the bit: in the
//! session, and through the persisted entry. On the 5,000-shot survey the
//! v2 bytes of every state are compared too.
//!
//! A changeset is symmetric (one entry serves the undo and the redo stack),
//! so each action has one size. Exempt by construction, as before: the
//! actions that bring whole-file data in (Add .stalx, an import, Merge
//! Projects).
//!
//! `STALAGMIGHT_UNDO_SIZE_REPORT=<path>` appends the table there; it is also
//! printed (`--nocapture`).

use std::fmt::Write as _;
use std::time::Instant;

use stalagmight_models::survey::changeset::testkit::{
    entry_text, gate_shot, gate_survey, gps, persisted, same_state, session_ids_sound, set_credit,
    GateSurvey, SHOTS_PER_LINE,
};
use stalagmight_models::survey::changeset::{capture, redo, undo};
use stalagmight_models::survey::{
    LineMeta, Project, RangeDeleteMode, ShotPlace, SplitOptions, StartAt,
};
use stalagmight_models::{
    CombineLinesOptions, CombinePick, LineId, LineUpdates, Section, ShotKind, ShotUpdates, SplitAt,
    StationRef,
};

use super::stalx_v2::{encode, Extras, Mode};

/// The cap: an entry may be at most this fraction of the v2 file.
const MAX_FRACTION: f64 = 0.01;

type Action = fn(&mut Project, &GateSurvey) -> Result<(), String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn v2_bytes(p: &Project) -> Vec<u8> {
    encode(p, &Extras::default(), Mode::Normal).expect("a valid model encodes")
}

fn meta(name: &str, section: Option<u32>) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#ff0000".to_string(),
        section_id: section,
        ..LineMeta::default()
    }
}

fn shot_id(p: &Project, line: LineId, index: usize) -> stalagmight_models::ShotId {
    p.line(line).expect("a gate line").shots[index].id
}

fn station(p: &Project, line: LineId, index: usize) -> u32 {
    p.line_stations(line).expect("a gate line")[index]
}

fn twenty(seed: usize) -> Vec<stalagmight_models::ShotData> {
    (0..20).map(|i| gate_shot(seed, i)).collect()
}

fn update(
    p: &mut Project,
    line: LineId,
    index: usize,
    updates: &ShotUpdates,
) -> Result<(), String> {
    let id = shot_id(p, line, index);
    p.update_shot(line, id, updates).map(|_| ()).map_err(err)
}

fn split_options() -> SplitOptions {
    SplitOptions {
        name_a: "A".to_string(),
        name_b: "B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
    }
}

fn range(
    p: &mut Project,
    line: LineId,
    a: usize,
    b: usize,
    mode: &RangeDeleteMode,
) -> Result<(), String> {
    let (a, b) = (shot_id(p, line, a), shot_id(p, line, b));
    p.delete_shot_range(line, a, b, mode)
        .map(|_| ())
        .map_err(err)
}

fn split_mode() -> RangeDeleteMode {
    RangeDeleteMode::Split {
        name_a: "A".to_string(),
        name_b: "B".to_string(),
    }
}

#[expect(clippy::too_many_lines, reason = "one entry per gated action")]
fn actions() -> Vec<(&'static str, Action)> {
    vec![
        ("create line (20 shots)", |p, _| {
            p.create_line(
                meta("New", None),
                StartAt::New,
                twenty(7_777),
                Some([50.0, 60.0]),
            )
            .map(|_| ())
            .map_err(err)
        }),
        ("rename line", |p, g| {
            let updates = LineUpdates {
                name: Some(Some("Renamed".to_string())),
                ..LineUpdates::default()
            };
            p.update_line_meta(g.mid(), &updates)
                .map(|_| ())
                .map_err(err)
        }),
        ("delete line", |p, g| {
            p.delete_line(g.mid()).map(|_| ()).map_err(err)
        }),
        ("reverse line", |p, g| {
            p.reverse_line(g.mid(), false).map(|_| ()).map_err(err)
        }),
        ("reverse line (1 shot)", |p, g| {
            p.reverse_line(g.leaf, true).map(|_| ()).map_err(err)
        }),
        ("insert shot", |p, g| {
            let after = shot_id(p, g.mid(), 20);
            p.create_shot(g.mid(), gate_shot(7_000, 1), ShotPlace::After(after))
                .map(|_| ())
                .map_err(err)
        }),
        ("edit shot length", |p, g| {
            let updates = ShotUpdates {
                length: Some(12.5),
                ..ShotUpdates::default()
            };
            update(p, g.lines[1], 5, &updates)
        }),
        ("edit shot comment", |p, g| {
            let updates = ShotUpdates {
                comment: Some(Some("edited".to_string())),
                ..ShotUpdates::default()
            };
            update(p, g.mid(), 5, &updates)
        }),
        ("set shot kind", |p, g| {
            let id = shot_id(p, g.mid(), 7);
            p.set_shot_kind(g.mid(), id, ShotKind::Jump)
                .map(|_| ())
                .map_err(err)
        }),
        ("delete shot", |p, g| {
            let id = shot_id(p, g.mid(), 25);
            p.delete_shot(g.mid(), id).map(|_| ()).map_err(err)
        }),
        ("edit credit (one line)", |p, g| {
            set_credit(p, g.mid(), 0..SHOTS_PER_LINE, "Surveyor");
            Ok(())
        }),
        ("merge stations", |p, g| {
            let (a, b) = (station(p, g.island, 0), station(p, g.mid(), 10));
            p.merge_keeping_stationary(a, b).map(|_| ()).map_err(err)
        }),
        ("unmerge stations", |p, g| {
            p.unmerge_station(StationRef::new(g.mid(), 0), crate::models::survey::NameAnswers::RuleL, None)
                .map(|_| ())
                .map_err(err)
        }),
        ("unmerge station pair", |p, g| {
            let parent = g.lines[(g.mid_index() - 1) / 2];
            let at = 1 + (g.mid_index() * 7) % (SHOTS_PER_LINE - 1);
            p.unmerge_station_pair(StationRef::new(parent, at as u32), StationRef::new(g.mid(), 0), crate::models::survey::NameAnswers::RuleL, None)
            .map(|_| ())
            .map_err(err)
        }),
        ("create loop closure", |p, g| {
            let next = g.lines[g.mid_index() + 1];
            let (a, b) = (station(p, g.mid(), SHOTS_PER_LINE), station(p, next, 4));
            p.create_closure(
                a,
                Some(g.mid()),
                b,
                Some(next),
                stalagmight_models::survey::ClosureStrategy::Move,
            )
            .map(|_| ())
            .map_err(err)
        }),
        ("remove loop closure", |p, _| {
            let id = p.closures.first().ok_or("no closure")?.id;
            p.remove_closure(id).map(|_| ()).map_err(err)
        }),
        ("set zero datum (GPS fix)", |p, g| {
            let s = station(p, g.mid(), 20);
            p.set_datum(s, Some(g.mid()), gps(20.501, -87.249), Some(5.0))
                .map(|_| ())
                .map_err(err)
        }),
        ("set zero datum (places the island)", |p, g| {
            let s = station(p, g.island, 0);
            p.set_datum(s, Some(g.island), gps(20.6, -87.3), None)
                .map(|_| ())
                .map_err(err)
        }),
        ("remove zero datum", |p, _| {
            p.remove_datum(0).map(|_| ()).map_err(err)
        }),
        ("station comment", |p, g| {
            let s = station(p, g.mid(), 12);
            p.station_mut(s).ok_or("no station")?.comment = Some("restriction".to_string());
            Ok(())
        }),
        ("move line", |p, g| {
            p.move_line(g.island, [3.0, -2.0, 0.0])
                .map(|_| ())
                .map_err(err)
        }),
        ("move segment", |p, g| {
            p.move_segment(g.island, [1.5, 0.5, 0.0])
                .map(|_| ())
                .map_err(err)
        }),
        ("move station", |p, g| {
            let s = station(p, g.mid(), 30);
            p.move_station(s, [0.4, 0.3, 0.0]).map(|_| ()).map_err(err)
        }),
        ("move comment", |p, g| {
            let text = p.line(g.mid()).ok_or("no line")?.shots[10].comment.clone();
            let clear = ShotUpdates {
                comment: Some(None),
                ..ShotUpdates::default()
            };
            update(p, g.mid(), 10, &clear)?;
            let set = ShotUpdates {
                comment: Some(text),
                ..ShotUpdates::default()
            };
            update(p, g.mid(), 11, &set)
        }),
        ("paste 30 cells (batch)", |p, g| {
            for i in 0..30 {
                let updates = ShotUpdates {
                    left: Some(Some(2.25)),
                    ..ShotUpdates::default()
                };
                update(p, g.mid(), i, &updates)?;
            }
            Ok(())
        }),
        ("project settings", |p, _| {
            p.name = "Gate renamed".to_string();
            p.settings.validation.warn_shot_length_max = 60.0;
            Ok(())
        }),
        ("surface elevation", |p, _| {
            p.settings.surface_elevation = Some(12.0);
            Ok(())
        }),
        ("split line here", |p, g| {
            p.split_line(
                g.mid(),
                SplitAt::Station { station_index: 25 },
                &split_options(),
            )
            .map(|_| ())
            .map_err(err)
        }),
        ("combine lines", |p, g| {
            let candidate = p
                .combine_candidates(StationRef::new(g.leaf, 0))
                .into_iter()
                .find(|c| c.blocked_reason.is_none())
                .ok_or("the leaf hangs off a line, so it can combine")?;
            let options = CombineLinesOptions {
                name: "Combined".to_string(),
                color: "#333333".to_string(),
                swap_lr: false,
                comment_pick: CombinePick::Kept,
                name_pick: CombinePick::Kept,
                join: None,
            };
            p.combine_lines(candidate.line_a, candidate.line_b, &options)
                .map(|_| ())
                .map_err(err)
        }),
        ("insert station mid-shot", |p, g| {
            let id = shot_id(p, g.mid(), 15);
            p.split_shot_at_distance(g.mid(), id, 1.0, Some("gate-inserted".to_string()))
                .map(|_| ())
                .map_err(err)
        }),
        ("delete shot and split", |p, g| {
            range(p, g.mid(), 25, 25, &split_mode())
        }),
        ("delete shot range (split)", |p, g| {
            range(p, g.mid(), 20, 24, &split_mode())
        }),
        ("delete shot range (shift)", |p, g| {
            range(p, g.mid(), 20, 24, &RangeDeleteMode::Shift)
        }),
        ("delete last shot and line", |p, g| {
            p.delete_line(g.leaf).map(|_| ()).map_err(err)
        }),
        ("rename station", |p, g| {
            let s = station(p, g.mid(), 10);
            if !p.conflicts_with_name(s, "10 renamed").is_empty() {
                return Err("name taken".to_string());
            }
            p.station_mut(s).ok_or("no station")?.name = "10 renamed".to_string();
            Ok(())
        }),
        ("create line into a section", |p, g| {
            p.create_line(
                meta("Grouped", Some(g.section)),
                StartAt::New,
                twenty(7_778),
                Some([70.0, 80.0]),
            )
            .map(|_| ())
            .map_err(err)
        }),
        ("create line and merge into a section", |p, g| {
            let s = station(p, g.mid(), 30);
            p.create_line(
                meta("Snapped", Some(g.section)),
                StartAt::Station(s),
                twenty(7_779),
                None,
            )
            .map(|_| ())
            .map_err(err)
        }),
        ("create section (with a line)", |p, g| {
            let id = p.mint_section_id().map_err(err)?;
            p.sections.push(Section {
                id,
                name: "Lower level".to_string(),
                color: "#123456".to_string(),
            });
            p.line_mut(g.mid()).ok_or("no line")?.section_id = Some(id);
            Ok(())
        }),
        ("rename section", |p, _| {
            p.sections.first_mut().ok_or("no Section")?.name = "Renamed level".to_string();
            Ok(())
        }),
        ("move 20 lines to a section", |p, g| {
            for id in &g.lines[20..40] {
                p.line_mut(*id).ok_or("no line")?.section_id = Some(g.section);
            }
            Ok(())
        }),
        ("apply section colour", |p, g| {
            let color = p.sections.first().ok_or("no Section")?.color.clone();
            for line in p.lines_mut() {
                if line.section_id == Some(g.section) {
                    line.color.clone_from(&color);
                }
            }
            Ok(())
        }),
        ("delete section", |p, g| {
            p.sections.retain(|s| s.id != g.section);
            for line in p.lines_mut() {
                if line.section_id == Some(g.section) {
                    line.section_id = None;
                }
            }
            Ok(())
        }),
    ]
}

/// One LRUD cell pasted into every shot of `lines`, each value different.
fn paste_left(p: &mut Project, lines: &[LineId]) -> Result<usize, String> {
    let mut k = 0usize;
    for line in lines {
        let n = p.line(*line).ok_or("no line")?.shots.len();
        for i in 0..n {
            let updates = ShotUpdates {
                left: Some(Some(1.0 + k as f64 * 0.01)),
                ..ShotUpdates::default()
            };
            update(p, *line, i, &updates)?;
            k += 1;
        }
    }
    Ok(k)
}

/// Edits of a thousand shots or stations at once, on the 20,000-shot survey.
fn large_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("paste 1,000 cells (batch)", |p, g| {
            paste_left(p, &g.lines[180..200]).map(|_| ())
        }),
        ("rename 1,000 stations (batch)", |p, g| {
            for line in &g.lines[200..220] {
                for index in 1..=SHOTS_PER_LINE {
                    let s = station(p, *line, index);
                    p.station_mut(s).ok_or("no station")?.name = format!("P{line}.{index}");
                }
            }
            Ok(())
        }),
    ]
}

/// Credit edits over a long line and a thousand shots, and reversing the
/// long line, on the 5,000-shot survey with a 2,000-shot line added.
fn long_line_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("edit credit (2,000-shot line)", |p, g| {
            let line = g.long.ok_or("the long line")?;
            set_credit(p, line, 0..2_000, "Surveyor");
            Ok(())
        }),
        ("edit credit (1,000 shots, 40 lines)", |p, g| {
            for line in &g.lines[40..80] {
                set_credit(p, *line, 10..35, "Surveyor");
            }
            Ok(())
        }),
        ("reverse line (2,000 shots)", |p, g| {
            let line = g.long.ok_or("the long line")?;
            p.reverse_line(line, true).map(|_| ()).map_err(err)
        }),
    ]
}

struct Row {
    action: &'static str,
    bytes: usize,
    /// Capture, undo and redo, in microseconds (reported, never asserted).
    micros: [u128; 3],
}

/// Run every action once from the survey, measure its entry, and check that
/// undo, redo and a second undo land exactly on the recorded states, in the
/// session and through the persisted entry.
fn measure(
    gate: &GateSurvey,
    v2_exact: bool,
    actions: Vec<(&'static str, Action)>,
) -> (Vec<Row>, Vec<String>) {
    let base = &gate.project;
    let base_v2 = v2_exact.then(|| v2_bytes(base));
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (action, run) in actions {
        let mut after = base.clone();
        if let Err(e) = run(&mut after, gate) {
            failed.push(format!("{action}: {e}"));
            continue;
        }
        let violations = after.validate();
        if !violations.is_empty() {
            failed.push(format!("{action}: invalid after: {violations:?}"));
            continue;
        }
        let started = Instant::now();
        let changes = capture(base, &after);
        let capture_us = started.elapsed().as_micros();
        if changes.is_empty() {
            failed.push(format!("{action}: changed nothing"));
            continue;
        }
        let bytes = entry_text(&changes, action).len();
        let after_v2 = v2_exact.then(|| v2_bytes(&after));
        let mut check = |live: &Project, expected: &Project, v2: Option<&Vec<u8>>, what: &str| {
            if let Err(e) = same_state(live, expected) {
                failed.push(format!("{action}: {what}: {e}"));
                return false;
            }
            if let Err(e) = session_ids_sound(live) {
                failed.push(format!("{action}: {what}: {e}"));
                return false;
            }
            if let Some(v2) = v2 {
                let mut normalized = live.clone();
                normalized.counters = expected.counters;
                if v2_bytes(&normalized) != *v2 {
                    failed.push(format!("{action}: {what}: the v2 bytes differ"));
                    return false;
                }
            }
            true
        };
        let mut timings = [capture_us, 0, 0];
        let mut ok = true;
        for (form, cs) in [
            ("session", changes.clone()),
            ("persisted", persisted(&changes)),
        ] {
            let mut live = after.clone();
            let started = Instant::now();
            if let Err(e) = undo(&mut live, &cs) {
                failed.push(format!("{action}: {form} undo refused: {e}"));
                ok = false;
                break;
            }
            let undo_us = started.elapsed().as_micros();
            ok &= check(&live, base, base_v2.as_ref(), &format!("{form} undo"));
            let started = Instant::now();
            if let Err(e) = redo(&mut live, &cs) {
                failed.push(format!("{action}: {form} redo refused: {e}"));
                ok = false;
                break;
            }
            let redo_us = started.elapsed().as_micros();
            ok &= check(&live, &after, after_v2.as_ref(), &format!("{form} redo"));
            if undo(&mut live, &cs).is_err() {
                failed.push(format!("{action}: {form} second undo refused"));
                ok = false;
                break;
            }
            ok &= check(
                &live,
                base,
                base_v2.as_ref(),
                &format!("{form} second undo"),
            );
            if form == "session" {
                timings[1] = undo_us;
                timings[2] = redo_us;
            }
        }
        if ok {
            rows.push(Row {
                action,
                bytes,
                micros: timings,
            });
        }
    }
    (rows, failed)
}

fn table(label: &str, file_bytes: usize, rows: &[Row]) -> String {
    let mut out = String::new();
    let cap = (file_bytes as f64 * MAX_FRACTION) as usize;
    let _ = writeln!(out, "{label}: v2 file {file_bytes} bytes, cap {cap} bytes");
    for row in rows {
        let _ = writeln!(
            out,
            "  {:<40} {:>8} B  {:>7.4}%   us capture/undo/redo {}/{}/{}",
            row.action,
            row.bytes,
            100.0 * row.bytes as f64 / file_bytes as f64,
            row.micros[0],
            row.micros[1],
            row.micros[2]
        );
    }
    out
}

#[expect(clippy::print_stdout, reason = "the size table is the test's report")]
fn report(text: &str) {
    println!("{text}");
    if let Ok(path) = std::env::var("STALAGMIGHT_UNDO_SIZE_REPORT") {
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        existing.push_str(text);
        std::fs::write(&path, existing).expect("write the size report");
    }
}

fn gate(gate: &GateSurvey, label: &str, v2_exact: bool, actions: Vec<(&'static str, Action)>) {
    let file_bytes = v2_bytes(&gate.project).len();
    let (rows, failed) = measure(gate, v2_exact, actions);
    let mut text = table(label, file_bytes, &rows);
    for failure in &failed {
        let _ = writeln!(text, "  NOT MEASURED {failure}");
    }
    report(&text);
    assert!(
        failed.is_empty(),
        "actions that did not run or did not restore:\n{text}"
    );
    let cap = file_bytes as f64 * MAX_FRACTION;
    let over: Vec<&Row> = rows.iter().filter(|r| r.bytes as f64 > cap).collect();
    assert!(over.is_empty(), "entries over 1% of the v2 file:\n{text}");
}

#[test]
fn every_history_entry_is_a_small_fraction_of_a_5k_shot_v2_file() {
    gate(&gate_survey(100, None), "5k shots", true, actions());
}

#[test]
fn every_history_entry_is_a_small_fraction_of_a_20k_shot_v2_file() {
    let mut all = actions();
    all.extend(large_actions());
    gate(&gate_survey(400, None), "20k shots", false, all);
}

/// On the 5k survey with a 2,000-shot line: a credit entry grows with the
/// runs it covers, not with the survey, so the smaller survey (and its
/// stricter cap) is the harder test.
#[test]
fn a_large_credit_edit_or_reverse_is_a_small_fraction_of_the_file() {
    gate(
        &gate_survey(100, Some(2_000)),
        "5k shots and a 2,000-shot line",
        true,
        long_line_actions(),
    );
}

/// A paste's entry grows with the cells it changed: past a fixed part, every
/// changed cell costs at most `BYTES_PER_CELL`, from 250 cells to 2,000.
#[test]
fn a_paste_entry_costs_a_bounded_number_of_bytes_per_changed_cell() {
    const FIXED_BYTES: usize = 4_000;
    const BYTES_PER_CELL: usize = 40;
    let gate = gate_survey(100, None);
    let mut text = String::from("paste entry per changed cell (5k shots)\n");
    for lines in [5, 40] {
        let lines = &gate.lines[10..10 + lines];
        let mut after = gate.project.clone();
        let cells = paste_left(&mut after, lines).expect("paste");
        let changes = capture(&gate.project, &after);
        let bytes = entry_text(&changes, "Update shots").len();
        let mut live = after.clone();
        undo(&mut live, &changes).expect("undo the paste");
        same_state(&live, &gate.project).expect("the undo of the paste");
        let _ = writeln!(
            text,
            "  {cells:>5} cells: {bytes:>6} B ({:.1} B/cell)",
            bytes as f64 / cells as f64
        );
        assert!(
            bytes <= FIXED_BYTES + BYTES_PER_CELL * cells,
            "a {cells}-cell paste's entry is {bytes} bytes, past {FIXED_BYTES} + {BYTES_PER_CELL} per cell"
        );
    }
    report(&text);
}
