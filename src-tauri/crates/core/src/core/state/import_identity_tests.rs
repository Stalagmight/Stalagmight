//! Kept and archived stations at the import doors (RS 4.1, 9): a join keeps
//! the target and archives the file's own station there (name, comment,
//! external id) as a record naming the new line, comments never joined; the
//! loop closures an import makes archive an end by rule K with the allowed
//! fallback, collisions are lettered by rule L, and the receipt names it
//! all. Then Add .stalx and Merge Projects sources keep their records and
//! closure archives.
//!
//! Every survey is synthetic, built in code; files go to a temp dir.

use tempfile::TempDir;

use super::import_funnel::{self, ImportRequest, RefusedLine};
use super::project_add_tests::{add, at, fixed_time, meta, open_target, shot as fixed_shot};
use super::{AppState, VirtualShotMode};
use crate::core::{Command, CommandResult};
use crate::models::survey::{ClosureEnd, ClosureStrategy, Project, StartAt};
use crate::models::{
    ImportClosure, ImportLineConfig, ImportPlacement, ImportReceipt, LineId, ShotData, StationId,
    StationRef, MAX_STATION_ID,
};

// ----- kit -------------------------------------------------------------------

/// A level shot of `length` metres on `azimuth`.
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 5.0,
        depth_out: 5.0,
        ..Default::default()
    }
}

/// A line config at import index `index`, one shot per length, each turned
/// a little from the last so no two stations coincide.
fn line(index: u32, name: &str, lengths: &[f64], placement: ImportPlacement) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: index,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: lengths
            .iter()
            .enumerate()
            .map(|(i, &length)| shot(f64::from(u32::try_from(i).unwrap() * 37 % 360), length))
            .collect(),
        station_comments: Vec::new(),
        station_names: Vec::new(),
        station_external: Vec::new(),
        placement,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn pairs(entries: &[(u32, &str)]) -> Vec<(u32, String)> {
    entries
        .iter()
        .map(|(index, text)| (*index, (*text).to_string()))
        .collect()
}

/// Placed with its station `import_station` on the existing slot.
fn onto(line_id: LineId, index: u32, import_station: u32) -> ImportPlacement {
    ImportPlacement::FromExisting {
        import_station,
        existing_station: StationRef::new(line_id, index),
    }
}

/// Placed with its station 0 on station `index` of import line `line`.
fn onto_imported(line: u32, index: u32) -> ImportPlacement {
    ImportPlacement::FromImporting {
        import_station: 0,
        target_line_index: line,
        target_station_index: index,
    }
}

fn closure(from: (u32, u32), to: (u32, u32)) -> ImportClosure {
    ImportClosure {
        from_line_index: from.0,
        from_station_index: from.1,
        to_line_index: to.0,
        to_station_index: to.1,
    }
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Identities".to_string(),
                location: None,
            })
            .success
    );
    state
}

fn import(
    state: &AppState,
    lines: Vec<ImportLineConfig>,
    closures: Vec<ImportClosure>,
) -> CommandResult {
    let result = state.execute_import_lines(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    result
}

fn receipt(result: &CommandResult) -> ImportReceipt {
    result.import_receipt.clone().unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn line_id(project: &Project, name: &str) -> LineId {
    project
        .lines()
        .iter()
        .find(|l| l.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no line {name}"))
        .id
}

/// The node at station `index` of the line named `name`.
fn node(project: &Project, name: &str, index: usize) -> StationId {
    project
        .line(line_id(project, name))
        .unwrap()
        .station(index)
        .unwrap()
}

/// The names the line named `name` shows, in chain order.
fn shown(project: &Project, name: &str) -> Vec<String> {
    project
        .line(line_id(project, name))
        .unwrap()
        .stations()
        .map(|s| project.shown_name(s).unwrap().to_string())
        .collect()
}

fn assert_valid(project: &Project) {
    let violations = project.validate();
    assert!(violations.is_empty(), "{violations:?}");
}

/// A project holding `Main`: three 10 m shots, stations "0" to "3".
fn with_main() -> AppState {
    let state = new_state();
    import(
        &state,
        vec![line(
            0,
            "Main",
            &[10.0, 10.0, 10.0],
            ImportPlacement::Origin,
        )],
        Vec::new(),
    );
    state
}

// ----- joins (RS 4.1) ------------------------------------------------------------

/// The file's station at the join is the target's own (same name, no other
/// comment or external id): nothing is archived and the receipt is quiet.
#[test]
fn a_join_whose_file_station_is_the_targets_archives_nothing() {
    let state = with_main();
    let main = line_id(&project(&state), "Main");
    let mut branch = line(0, "Branch", &[2.0, 2.0], onto(main, 2, 0));
    branch.station_names = pairs(&[(0, "2"), (1, "b1"), (2, "b2")]);
    let result = import(&state, vec![branch], Vec::new());
    let p = project(&state);
    let junction = node(&p, "Main", 2);
    assert_eq!(node(&p, "Branch", 0), junction);
    assert!(p.station(junction).unwrap().archived.is_empty());
    assert_eq!(shown(&p, "Branch"), ["2", "b1", "b2"]);
    assert!(!p.is_foreign_slot(StationRef::new(line_id(&p, "Branch"), 0)));
    let receipt = receipt(&result);
    assert!(receipt.info.is_empty(), "{:?}", receipt.info);
    assert!(receipt.warnings.is_empty(), "{:?}", receipt.warnings);
    assert_valid(&p);
}

/// The file gives the joined station only a comment: the target keeps its
/// own (none), and the file's comment is kept in a record under the
/// target's name, so the line's slot there is foreign (its station is
/// archived) and Detach Line from Station brings the comment back.
#[test]
fn a_join_with_only_a_file_comment_archives_it_under_the_kept_name() {
    let state = with_main();
    let main = line_id(&project(&state), "Main");
    let mut branch = line(0, "Branch", &[2.0], onto(main, 1, 0));
    branch.station_comments = pairs(&[(0, "cairn"), (1, "end")]);
    let result = import(&state, vec![branch], Vec::new());
    let p = project(&state);
    let junction = node(&p, "Main", 1);
    let branch_id = line_id(&p, "Branch");
    let station = p.station(junction).unwrap();
    assert_eq!(station.comment, None, "comments are never joined");
    assert_eq!(station.archived.len(), 1);
    let record = &station.archived[0];
    assert_eq!(
        (
            record.name.as_str(),
            record.comment.as_deref(),
            &record.lines
        ),
        ("1", Some("cairn"), &vec![branch_id])
    );
    assert_eq!(
        p.station(node(&p, "Branch", 1)).unwrap().comment.as_deref(),
        Some("end"),
        "the line's own station takes its comment"
    );
    assert!(p.is_foreign_slot(StationRef::new(branch_id, 0)));
    assert_eq!(
        receipt(&result).info,
        vec![
            "Line \"Branch\" joins station \"Main / 1\" and shows it there; the file's own \
             station there, with its comment, is archived: Detach Line from Station brings it \
             back"
                .to_string()
        ]
    );
    let naming = p.detach_naming(junction, branch_id, None).unwrap();
    assert_eq!(
        (naming.id, naming.name.as_str(), naming.comment.as_deref()),
        (record.id, "1", Some("cairn"))
    );
    assert_valid(&p);
}

/// A reversed import: the file indexes its names, comments and external ids
/// in its own order, so the junction (chain position 0 of the reversed
/// line) takes the file's LAST station's, mirrored like every per-station
/// entry, into its record.
#[test]
fn a_reversed_join_archives_the_files_mirrored_station() {
    let state = with_main();
    let main = line_id(&project(&state), "Main");
    let mut branch = line(0, "Branch", &[2.0, 3.0], onto(main, 3, 0));
    branch.reverse = true;
    branch.station_names = pairs(&[(0, "far"), (2, "near")]);
    branch.station_comments = pairs(&[(0, "far note"), (2, "near note")]);
    branch.station_external = pairs(&[(2, "X-NEAR")]);
    import(&state, vec![branch], Vec::new());
    let p = project(&state);
    let junction = node(&p, "Main", 3);
    let records = &p.station(junction).unwrap().archived;
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0].name.as_str(),
            records[0].comment.as_deref(),
            records[0].external.as_deref()
        ),
        ("near", Some("near note"), Some("X-NEAR"))
    );
    let far = node(&p, "Branch", 2);
    assert_eq!(p.station(far).unwrap().name, "far");
    assert_eq!(p.station(far).unwrap().comment.as_deref(), Some("far note"));
    let main_station = p.station(junction).unwrap();
    assert_eq!(
        (
            main_station.comment.as_deref(),
            main_station.external.as_deref()
        ),
        (None, None),
        "the kept station takes nothing from the file"
    );
    assert_valid(&p);
}

/// A snapped line (`ManualSnap`) joins the same way: the target is kept and
/// the file's station archived.
#[test]
fn a_snapped_join_keeps_the_target_and_archives_the_files_station() {
    let state = with_main();
    let main = line_id(&project(&state), "Main");
    let mut branch = line(
        0,
        "Snap",
        &[2.0],
        ImportPlacement::ManualSnap {
            align_station_index: 0,
            snapped_to_station: StationRef::new(main, 2),
        },
    );
    branch.station_names = pairs(&[(0, "S0"), (1, "S1")]);
    import(&state, vec![branch], Vec::new());
    let p = project(&state);
    let junction = node(&p, "Main", 2);
    assert_eq!(p.station(junction).unwrap().name, "2");
    let records = &p.station(junction).unwrap().archived;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].name, "S0");
    assert_eq!(shown(&p, "Snap"), ["2", "S1"]);
    assert_valid(&p);
}

/// No Station ID is left for the file's station: the line still imports,
/// the record is dropped, and the receipt warns what was not kept.
#[test]
fn a_join_record_that_finds_no_station_id_is_dropped_and_reported() {
    let state = with_main();
    let mut p = project(&state);
    let main = line_id(&p, "Main");
    // One id left: the branch's own station takes it.
    p.counters.next_station_id = MAX_STATION_ID - 1;
    let mut branch = line(0, "Branch", &[2.0], onto(main, 1, 0));
    branch.station_names = pairs(&[(0, "Q"), (1, "Q1")]);
    branch.station_comments = pairs(&[(0, "lost")]);
    let configs = [branch];
    let targets = import_funnel::placement_targets(&configs);
    assert_eq!(targets, vec![StationRef::new(main, 1)]);
    let outcome = import_funnel::import_lines(
        &mut p,
        &ImportRequest {
            lines: &configs,
            closures: &[],
            file_hash: None,
            line_hashes: &[],
            updates: &[],
            mode: VirtualShotMode::Auto,
            surface_elevation: None,
            on_refused: RefusedLine::SkipAndReport,
        },
    )
    .unwrap();
    let junction = node(&p, "Main", 1);
    assert_eq!(outcome.line_ids.len(), 1, "the line is imported");
    assert!(p.station(junction).unwrap().archived.is_empty());
    assert_eq!(p.station(junction).unwrap().comment, None);
    assert_eq!(
        outcome.identity_warnings,
        vec![
            "Line \"Branch\" joins station \"Main / 1\" and shows it there; the file's own \
             station \"Q\" there, with its comment, was not kept: no Station ID is left for it"
                .to_string()
        ]
    );
    assert!(outcome.identity_notes.is_empty());
    assert_valid(&p);
}

/// A join onto a loop end a closure archives: the new line shows the kept
/// end's name there (RS 5.1), so a new station named like it takes the
/// letter, while the archived end's own name is free on the new line. The
/// file gives the joined station nothing of its own: nothing is archived.
#[test]
fn a_join_onto_a_closure_archived_station_shows_its_kept_end() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Main", &[10.0, 10.0, 10.0], ImportPlacement::Origin),
            line(1, "Side", &[3.0], onto_imported(0, 0)),
        ],
        vec![closure((1, 1), (0, 2))],
    );
    let p = project(&state);
    let side_end = node(&p, "Side", 1);
    assert!(p.is_closure_archived(side_end));
    assert_eq!(p.shown_name(side_end), Some("2"));
    assert_eq!(p.station(side_end).unwrap().name, "1");
    assert_eq!(
        receipt(&result).info,
        vec![
            "Loop closure: station \"Side / 1\" is shown as \"Main / 2\": Remove Closure brings \
             its own name back"
                .to_string()
        ]
    );
    let side = line_id(&p, "Side");

    let mut branch = line(0, "Branch", &[2.0, 2.0], onto(side, 1, 0));
    branch.station_names = pairs(&[(1, "2"), (2, "1")]);
    let result = import(&state, vec![branch], Vec::new());
    let p = project(&state);
    assert_eq!(node(&p, "Branch", 0), side_end);
    assert!(p.station(side_end).unwrap().archived.is_empty());
    assert_eq!(shown(&p, "Branch"), ["2", "2a", "1"]);
    assert_eq!(
        receipt(&result).warnings,
        vec![
            "Line \"Branch\" joins station \"2\", so its own station \"2\" was renamed \"2a\""
                .to_string()
        ]
    );
    assert_valid(&p);
}

// ----- closures (RS 9) ---------------------------------------------------------------

/// Rule K keeps the end on the longest line, whichever end the file names
/// first; the other end shows its name, and the receipt says so in a note.
/// One undo takes the closure (and its archive) away with the import.
#[test]
fn an_imported_closure_keeps_the_end_on_the_longest_line() {
    for reversed in [false, true] {
        let state = new_state();
        let pair = if reversed {
            closure((0, 2), (1, 1))
        } else {
            closure((1, 1), (0, 2))
        };
        import(
            &state,
            vec![
                line(0, "Main", &[10.0, 10.0, 10.0], ImportPlacement::Origin),
                line(1, "Side", &[3.0], onto_imported(0, 0)),
            ],
            vec![pair],
        );
        let p = project(&state);
        let (main_end, side_end) = (node(&p, "Main", 2), node(&p, "Side", 1));
        let c = p.closures[0];
        assert_eq!(
            c.archive(),
            Some((side_end, main_end)),
            "reversed={reversed}"
        );
        assert_eq!(
            c.archived,
            Some(if reversed {
                ClosureEnd::B
            } else {
                ClosureEnd::A
            })
        );
        assert_eq!(shown(&p, "Side"), ["0", "2"]);
        assert_valid(&p);

        assert!(state.undo().success);
        assert!(project(&state).closures.is_empty());
        assert!(state.redo().success);
        assert_eq!(project(&state).closures, p.closures, "redo brings it back");
    }
}

/// Both ends on one line: rule K ties, so the lower Station ID is kept and
/// the line shows it twice (N8 allows one identity twice on a line).
#[test]
fn an_imported_same_line_closure_shows_one_station_twice() {
    let state = new_state();
    let result = import(
        &state,
        vec![line(
            0,
            "Ring",
            &[10.0, 10.0, 10.0, 10.0],
            ImportPlacement::Origin,
        )],
        vec![closure((0, 4), (0, 0))],
    );
    let p = project(&state);
    assert_eq!(
        p.closures[0].archive(),
        Some((node(&p, "Ring", 4), node(&p, "Ring", 0)))
    );
    assert_eq!(shown(&p, "Ring"), ["0", "1", "2", "3", "0"]);
    assert_eq!(
        receipt(&result).info,
        vec![
            "Loop closure: station \"Ring / 4\" is shown as \"Ring / 0\": Remove Closure brings \
             its own name back"
                .to_string()
        ]
    );
    assert_valid(&p);
}

/// Rule K's end is already archived by an earlier closure (C1): the other
/// end is archived instead, when it may be; when neither may be, the closure
/// archives nothing and the receipt says it keeps both names.
#[test]
fn an_imported_closure_falls_back_to_the_allowed_end_then_to_none() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(
                0,
                "A",
                &[10.0, 10.0, 10.0, 10.0, 10.0, 10.0],
                ImportPlacement::Origin,
            ),
            line(1, "B", &[2.0], onto_imported(0, 0)),
            line(2, "C", &[2.0], onto_imported(0, 0)),
        ],
        vec![
            closure((1, 1), (0, 2)), // B/1 archived, shown as A/2
            closure((2, 1), (0, 4)), // C/1 archived, shown as A/4
            closure((1, 1), (0, 6)), // rule K: A/6 kept, B/1 taken: A/6 archived
            closure((1, 1), (2, 1)), // both ends taken: nothing archived
        ],
    );
    let p = project(&state);
    let (b1, c1) = (node(&p, "B", 1), node(&p, "C", 1));
    let (a2, a4, a6) = (node(&p, "A", 2), node(&p, "A", 4), node(&p, "A", 6));
    let archives: Vec<_> = p.closures.iter().map(|c| c.archive()).collect();
    assert_eq!(
        archives,
        vec![Some((b1, a2)), Some((c1, a4)), Some((a6, b1)), None]
    );
    assert_eq!(p.shown(a6), a2, "A/6 shows B/1, which shows A/2");
    assert_eq!(shown(&p, "A"), ["0", "1", "2", "3", "4", "5", "2"]);
    let receipt = receipt(&result);
    assert_eq!(receipt.closures_created, 4);
    assert_eq!(
        receipt.info.last().map(String::as_str),
        Some(
            "Loop closure between \"A / 2\" and \"A / 4\" shows both names: neither end can be \
             shown as the other"
        )
    );
    assert_valid(&p);
}

/// A closure whose kept end's name is the name of another station on the
/// archived end's line: that station (shown on one line, the kept end on
/// two) takes the letter by rule L, and the receipt warns.
#[test]
fn an_imported_closure_letters_a_collision_by_rule_l() {
    let state = new_state();
    let mut side = line(1, "Side", &[3.0, 3.0], onto_imported(0, 0));
    side.station_names = pairs(&[(1, "s1"), (2, "2")]);
    let result = import(
        &state,
        vec![
            line(0, "Main", &[10.0, 10.0, 10.0], ImportPlacement::Origin),
            side,
        ],
        vec![closure((1, 1), (0, 2))],
    );
    let p = project(&state);
    assert_eq!(shown(&p, "Side"), ["0", "2", "2a"]);
    assert_eq!(
        shown(&p, "Main"),
        ["0", "1", "2", "3"],
        "Main keeps its names"
    );
    assert_eq!(
        receipt(&result).warnings,
        vec![
            "Station \"Side / 2\" was renamed \"2a\": after a loop closure, line \"Side\" shows \
             another station named \"2\""
                .to_string()
        ]
    );
    assert_valid(&p);
}

/// A closure that hides a station the project already held is a warning,
/// not a note: the user knew that station by its name. The station is
/// named by its own line.
#[test]
fn an_imported_closure_that_hides_a_project_station_warns() {
    let state = new_state();
    import(
        &state,
        vec![line(0, "Old", &[1.0, 1.0], ImportPlacement::Origin)],
        Vec::new(),
    );
    let old = line_id(&project(&state), "Old");
    let result = import(
        &state,
        vec![
            line(0, "J", &[1.0], onto(old, 1, 0)),
            line(1, "L", &[10.0, 10.0, 10.0, 10.0], onto(old, 0, 0)),
        ],
        vec![closure((0, 0), (1, 3))],
    );
    let p = project(&state);
    let hidden = node(&p, "Old", 1);
    assert_eq!(p.closures[0].archive(), Some((hidden, node(&p, "L", 3))));
    assert_eq!(shown(&p, "Old"), ["0", "3", "2"]);
    let receipt = receipt(&result);
    assert_eq!(
        receipt.warnings,
        vec![
            "Loop closure: station \"Old / 1\", already in the project, is now shown as \
             \"L / 3\": Remove Closure brings its name back"
                .to_string()
        ]
    );
    assert!(receipt.info.is_empty(), "{:?}", receipt.info);
    assert_valid(&p);
}

/// The notes are capped like the warnings: a file can make one per join.
#[test]
fn receipt_notes_are_capped_like_warnings() {
    let mut receipt = ImportReceipt::empty();
    receipt.info = (0..60).map(|i| format!("note {i}")).collect();
    receipt.info.push("x".repeat(400));
    receipt.cap_warnings();
    assert_eq!(receipt.info.len(), 51);
    assert_eq!(receipt.info[50], "... and 11 more notes");
}

// ----- Add .stalx and Merge Projects sources (RS 4.4) ----------------------------------

/// A source holding a merge record (Side's own station 0, archived in Main's
/// station 1) and a closure archiving Side's last station.
fn source_with_archives() -> Project {
    let mut p = Project::new_at("Archives", None, fixed_time(40));
    let main = p
        .add_line(
            meta("Main"),
            StartAt::New,
            vec![
                fixed_shot("m-0", 0.0, 10.0, 1.0),
                fixed_shot("m-1", 30.0, 10.0, 1.0),
                fixed_shot("m-2", 60.0, 10.0, 1.0),
            ],
        )
        .unwrap();
    let side = p
        .add_line(
            meta("Side"),
            StartAt::New,
            vec![
                fixed_shot("s-0", 90.0, 4.0, 1.0),
                fixed_shot("s-1", 120.0, 4.0, 1.0),
            ],
        )
        .unwrap();
    let start = at(&p, main, 0);
    p.set_placement(start, 0.0, 0.0).unwrap();
    let side0 = at(&p, side, 0);
    p.station_mut(side0).unwrap().name = "S0".to_string();
    p.station_mut(side0).unwrap().comment = Some("side start".to_string());
    let main1 = at(&p, main, 1);
    p.merge_stations_auto(side0, main1, main1).unwrap();
    let (side2, main3) = (at(&p, side, 2), at(&p, main, 3));
    p.push_closure(side2, Some(side), main3, Some(main), ClosureStrategy::Batch);
    p.closures[0].archived = Some(ClosureEnd::A);
    let violations = p.validate();
    assert!(violations.is_empty(), "{violations:?}");
    p
}

/// What a project shows: per line, its name and the names it shows.
fn shown_lines(p: &Project) -> Vec<(String, Vec<String>)> {
    p.lines()
        .iter()
        .map(|l| {
            (
                l.name.clone().unwrap_or_default(),
                l.stations()
                    .map(|s| p.shown_name(s).unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

/// Add .stalx: the copy keeps the source's record (its id shifted past the
/// target's, its line remapped, its comment) and its closure archive, so
/// it shows exactly what the source shows; one undo takes it away.
#[test]
fn an_added_source_keeps_its_records_and_closure_archives() {
    let dir = TempDir::new().unwrap();
    let source = source_with_archives();
    let path = super::project_add_tests::write_v2(dir.path(), "source.stalx", &source);
    let (state, _) = open_target(dir.path(), false);
    let before = project(&state);
    let result = add(&state, &path);
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    assert_valid(&p);

    let base = before.counters.next_station_id;
    let source_main1 = at(&source, source.lines()[0].id, 1);
    let source_record = &source.station(source_main1).unwrap().archived[0];
    let main1 = node(&p, "Main", 1);
    assert_eq!(main1, source_main1 + base);
    let records = &p.station(main1).unwrap().archived;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, source_record.id + base, "its id shifted");
    assert_eq!(
        records[0].lines,
        vec![line_id(&p, "Side")],
        "its line remapped"
    );
    assert_eq!(
        (records[0].name.as_str(), records[0].comment.as_deref()),
        ("S0", Some("side start"))
    );
    let added = p.closures.last().unwrap();
    assert_eq!(
        added.archive(),
        Some((node(&p, "Side", 2), node(&p, "Main", 3))),
        "the closure keeps its archived end"
    );
    let copied: Vec<_> = shown_lines(&p)
        .into_iter()
        .filter(|(name, _)| name == "Main" || name == "Side")
        .collect();
    assert_eq!(
        copied,
        shown_lines(&source),
        "it shows what the source shows"
    );

    assert!(state.undo().success);
    assert_eq!(project(&state).closures, before.closures);
    assert!(project(&state).lines().len() == before.lines().len());
}

/// Merge Projects: the output file carries each source's records and
/// closure archives, and shows what each source shows.
#[test]
fn merged_sources_keep_their_records_and_closure_archives() {
    let dir = TempDir::new().unwrap();
    let source = source_with_archives();
    let mut other = source_with_archives();
    other.created_at = fixed_time(41);
    let output = dir.path().join("merged.stalx");
    let result = AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: vec![
            super::project_add_tests::write_v2(dir.path(), "a.stalx", &source),
            super::project_add_tests::write_v2(dir.path(), "b.stalx", &other),
        ],
        output_path: output.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "{:?}", result.error);
    let merged = super::project_add_tests::read_v2(&output).project;
    assert_valid(&merged);
    let records: usize = merged.stations.values().map(|s| s.archived.len()).sum();
    assert_eq!(records, 2, "one record per source");
    assert_eq!(
        merged
            .closures
            .iter()
            .filter(|c| c.archive().is_some())
            .count(),
        2,
        "one closure archive per source"
    );
    let lines = shown_lines(&merged);
    let expected = shown_lines(&source);
    assert_eq!(lines.len(), 4);
    for (got, want) in lines.iter().zip(expected.iter().chain(expected.iter())) {
        assert_eq!(got.1, want.1, "line {} shows what its source shows", got.0);
    }
}
