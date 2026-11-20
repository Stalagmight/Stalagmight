//! Shot identity (#423): a shot's uuid is the project's, project-unique by
//! invariant (`validate` refuses a repeat), and it survives every edit that
//! keeps the shot; Ariane's `<UUID>`/`<UU>` is carried through an import and
//! an export.
//!
//! These tests pin the contract at each place identity could be dropped or
//! forged: the shot commands and the delete/split recipes (run now), the
//! undo, and the import funnel, the exporters and Merge Projects (other
//! packages' doors: ported onto the station-graph API and `#[ignore]`d until
//! those packages land; their reasons say which).
//!
//! What changed with the station graph: a cross-reference is a station id,
//! not a `(line, anchor uuid)` key, so "the anchor still resolves" becomes
//! "the reference is still on its station"; a uuid repeated across lines is
//! no longer legal anywhere, so every door that can meet one re-mints it.
//!
//! No fixture files: every `.tml`/`.tmlu` here is synthetic XML built in-test
//! and written to a temp dir that the test owns.

use std::collections::HashSet;
use std::path::Path;

use super::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::export::survey::{export_tml, export_tml_for_speleodb};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{LineId, ShotData, StationId};
use crate::parsers::tml::{convert_tml_to_import_configs, parse_tml};
use crate::persistence::stalx_v2::{self, Extras, Mode};

// ---------------------------------------------------------------------------
// Synthetic Ariane XML
// ---------------------------------------------------------------------------

/// One `<SurveyData>` row to synthesize: Ariane id, the id it hangs off, and the
/// `<UUID>` the file gives it (`None` writes no element at all).
struct Row {
    id: i64,
    from_id: i64,
    uuid: Option<&'static str>,
}

impl Row {
    fn new(id: i64, from_id: i64, uuid: Option<&'static str>) -> Self {
        Self { id, from_id, uuid }
    }
}

/// A `<SurveyData>` element. `START` rows anchor the chain; every other row is a
/// REAL shot that becomes one of our shots.
fn survey_data_xml(row: &Row, shot_type: &str, line: &str) -> String {
    let uuid = row
        .uuid
        .map(|u| format!("\n    <UUID>{u}</UUID>"))
        .unwrap_or_default();
    let (id, from_id) = (row.id, row.from_id);
    format!(
        r"<SurveyData>
    <ID>{id}</ID>{uuid}
    <FromID>{from_id}</FromID>
    <ClosureToID>-1</ClosureToID>
    <Type>{shot_type}</Type>
    <Section>{line}</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>1.0</Left>
    <Right>2.0</Right>
    <Up>3.0</Up>
    <Down>4.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2026-01-02</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
    )
}

/// A one-line cave file: a START row plus one REAL row per entry in `rows`.
fn cave_file_xml(rows: &[Row]) -> String {
    let start = survey_data_xml(&Row::new(0, -1, None), "START", "Main");
    let body: String = rows
        .iter()
        .map(|r| survey_data_xml(r, "REAL", "Main"))
        .collect::<Vec<_>>()
        .join("\n        ");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>Synthetic</caveName>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
    <firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>
    <Data>
        {start}
        {body}
    </Data>
</CaveFile>"#
    )
}

/// A two-line cave file: `main` hangs off the START, `side` hangs off the
/// last row of `main`. Used to show that a repeat ACROSS lines is legal and
/// must survive untouched.
fn two_line_cave_file_xml(main: &[Row], side: &[Row]) -> String {
    let start = survey_data_xml(&Row::new(0, -1, None), "START", "Main");
    let rows = |rows: &[Row], line: &str| {
        rows.iter()
            .map(|r| survey_data_xml(r, "REAL", line))
            .collect::<Vec<_>>()
            .join("\n        ")
    };
    let (a, b) = (rows(main, "Main"), rows(side, "Side"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>Synthetic</caveName>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
    <firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>
    <Data>
        {start}
        {a}
        {b}
    </Data>
</CaveFile>"#
    )
}

/// Write `xml` next to `dir` under `name` and hand back the path. `.tmlu` is
/// plain XML, so the parser reads it as written.
fn write_tmlu(dir: &Path, name: &str, xml: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, xml).unwrap();
    path
}

/// The full Ariane import as the app runs it: parse the file, convert it to
/// import configs, execute the import (the TML-specific entry point
/// `speleodb_pull::adopt_build_into` uses, with the surface elevation #431).
fn import_ariane_file(state: &AppState, path: &Path) {
    let tml = parse_tml(path).unwrap();
    let (lines, closures, _warnings) = convert_tml_to_import_configs(&tml, false, true).unwrap();
    let surface_elevation =
        crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    let result = state.execute_tml_import_sections(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        surface_elevation,
    );
    assert!(result.success, "import failed: {:?}", result.error);
}

fn state_with_project() -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::CreateProject {
        name: "Synthetic".to_string(),
        location: None,
    });
    assert!(r.success, "{:?}", r.error);
    state
}

/// Every shot uuid of line `index` (list order), in line order.
fn line_uuids(project: &Project, index: usize) -> Vec<String> {
    project.lines()[index]
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect()
}

/// A level 10 m shot east, with `uuid`.
fn shot_u(uuid: &str) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 5.0,
        depth_out: 5.0,
        ..ShotData::default()
    }
}

/// A one-line survey named `Main` whose shots carry `uuids` (what an Ariane
/// import makes of a covered file), placed at the origin.
fn survey_with_uuids(uuids: &[&str]) -> (Project, LineId) {
    let mut p = Project::new("Synthetic", None);
    let line = p
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            uuids.iter().map(|u| shot_u(u)).collect(),
        )
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    (p, line)
}

fn state_with(project: Project) -> AppState {
    let violations = project.validate();
    assert!(violations.is_empty(), "invalid test survey: {violations:?}");
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn st(p: &Project, line: LineId, index: usize) -> StationId {
    p.line(line).unwrap().station(index).unwrap()
}

fn set_comment(p: &mut Project, station: StationId, text: &str) {
    p.stations.get_mut(&station).unwrap().comment = Some(text.to_string());
}

/// Every uuid the project holds is held once (the invariant).
fn assert_unique(p: &Project) {
    let all: Vec<&str> = p
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.as_str()))
        .collect();
    assert_eq!(
        all.iter().collect::<HashSet<_>>().len(),
        all.len(),
        "a shot uuid is held twice: {all:?}"
    );
    assert!(p.validate().is_empty(), "{:?}", p.validate());
}

// ---------------------------------------------------------------------------
// 1. Round trip
// ---------------------------------------------------------------------------

/// The file's `<UUID>` values come back out of the export verbatim, for both
/// extensions.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn ariane_shot_uuids_survive_import_then_export_verbatim() {
    const U1: &str = "11111111-1111-4111-8111-111111111111";
    const U2: &str = "22222222-2222-4222-8222-222222222222";
    const U3: &str = "33333333-3333-4333-8333-333333333333";

    for ext in ["tmlu", "tml"] {
        let dir = tempfile::tempdir().unwrap();
        let source = write_tmlu(
            dir.path(),
            "source.tmlu",
            &cave_file_xml(&[
                Row::new(1, 0, Some(U1)),
                Row::new(2, 1, Some(U2)),
                Row::new(3, 2, Some(U3)),
            ]),
        );
        let state = state_with_project();
        import_ariane_file(&state, &source);
        let project = project(&state);
        assert_eq!(
            line_uuids(&project, 0),
            vec![U1.to_string(), U2.to_string(), U3.to_string()],
            "the file's identities must be the imported shots' identities"
        );
        let exported = dir.path().join(format!("out.{ext}"));
        export_tml(&project, &exported).unwrap();
        let reparsed = parse_tml(&exported).unwrap();
        let out: Vec<String> = reparsed
            .lines
            .iter()
            .flat_map(|s| s.shots.iter())
            .filter_map(|s| s.uuid.clone())
            .collect();
        for expected in [U1, U2, U3] {
            assert!(
                out.contains(&expected.to_string()),
                ".{ext} round trip lost {expected}; got {out:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Partial coverage
// ---------------------------------------------------------------------------

#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn a_single_covered_shot_keeps_its_id_and_the_others_get_fresh_ones() {
    const MID: &str = "44444444-4444-4444-8444-444444444444";
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "partial.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, None),
            Row::new(2, 1, Some(MID)),
            Row::new(3, 2, None),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let uuids = line_uuids(&project(&state), 0);
    assert_eq!(uuids.len(), 3);
    assert_eq!(uuids[1], MID, "the covered shot must keep the file's id");
    assert_ne!(uuids[0], MID);
    assert_ne!(uuids[2], MID);
    assert!(!uuids[0].is_empty() && !uuids[2].is_empty());
    assert_ne!(uuids[0], uuids[2], "fresh mints must be distinct");
}

// ---------------------------------------------------------------------------
// 3. Intra-line duplicate
// ---------------------------------------------------------------------------

/// A source file that repeats an id inside one line is REPAIRED, not
/// rejected: the FIRST occurrence keeps the foreign value and the rest are
/// re-minted, so the model's uniqueness invariant holds.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn a_repeated_uuid_inside_one_line_is_repaired_keeping_the_first() {
    const DUP: &str = "55555555-5555-4555-8555-555555555555";
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "dup.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(DUP)),
            Row::new(2, 1, Some(DUP)),
            Row::new(3, 2, Some(DUP)),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let project = project(&state);
    let uuids = line_uuids(&project, 0);
    assert_eq!(uuids[0], DUP, "the FIRST occurrence keeps the foreign id");
    assert_ne!(uuids[1], DUP);
    assert_ne!(uuids[2], DUP);
    assert_unique(&project);
}

// ---------------------------------------------------------------------------
// 4. Reverse on import
// ---------------------------------------------------------------------------

/// The import-time reverse (#24) moves each shot to the mirrored position;
/// identity travels inside the `ShotData` being reversed.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn the_import_reverse_carries_each_id_with_its_own_shot() {
    const FIRST: &str = "66666666-6666-4666-8666-666666666666";
    const LAST: &str = "77777777-7777-4777-8777-777777777777";
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "reverse.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(FIRST)),
            Row::new(2, 1, None),
            Row::new(3, 2, Some(LAST)),
        ]),
    );
    let state = state_with_project();
    let tml = parse_tml(&source).unwrap();
    let (mut lines, closures, _warnings) =
        convert_tml_to_import_configs(&tml, false, true).unwrap();
    lines[0].reverse = true;
    let result = state.execute_tml_import_sections(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        None,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let uuids = line_uuids(&project(&state), 0);
    assert_eq!(uuids[0], LAST, "the file's last id must be first now");
    assert_eq!(uuids[2], FIRST, "the file's first id must be last now");
}

// ---------------------------------------------------------------------------
// 5. Line split
// ---------------------------------------------------------------------------

/// Splitting a line keeps every shot's uuid on whichever half holds it, and
/// every station reference stays on its station, where it was.
#[test]
fn a_line_split_keeps_the_ids_on_both_halves_and_the_references_on_their_stations() {
    const IDS: [&str; 5] = [
        "aaaaaaaa-0000-4000-8000-000000000001",
        "aaaaaaaa-0000-4000-8000-000000000002",
        "aaaaaaaa-0000-4000-8000-000000000003",
        "aaaaaaaa-0000-4000-8000-000000000004",
        "aaaaaaaa-0000-4000-8000-000000000005",
    ];
    let (mut p, line) = survey_with_uuids(&IDS);
    // One reference on each side of the coming split.
    let (stays, moves) = (st(&p, line, 1), st(&p, line, 4));
    set_comment(&mut p, stays, "stays in A");
    set_comment(&mut p, moves, "moves into B");
    let state = state_with(p);
    let before = project(&state);
    let mid = before.line(line).unwrap().shots[2].id;

    let r = state.execute_delete_shot_and_split(line, mid, "A".to_string(), "B".to_string());
    assert!(r.success, "{:?}", r.error);
    let after = project(&state);
    let new_line = r.line_id.unwrap();
    assert_eq!(
        line_uuids(&after, 0),
        vec![IDS[0].to_string(), IDS[1].to_string()]
    );
    let b: Vec<String> = after
        .line(new_line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect();
    assert_eq!(b, vec![IDS[3].to_string(), IDS[4].to_string()]);
    for (station, text) in [(stays, "stays in A"), (moves, "moves into B")] {
        assert_eq!(
            after.station(station).unwrap().comment.as_deref(),
            Some(text)
        );
        let (was, now) = (
            before.raw_positions().xy(station).unwrap(),
            after.raw_positions().xy(station).unwrap(),
        );
        assert!(
            (was[0] - now[0]).abs() + (was[1] - now[1]).abs() < 1e-9,
            "{text:?} moved: {was:?} -> {now:?}"
        );
    }
    assert_eq!(
        after
            .topology()
            .slot_on(moves, new_line)
            .map(|s| s.station_index),
        Some(1)
    );
    assert_unique(&after);
}

// ---------------------------------------------------------------------------
// 6. Project merge
// ---------------------------------------------------------------------------

/// Two projects that hold the same uuids: the merge keeps the FIRST source's
/// copy verbatim and re-mints the second's, so the merged project holds each
/// once.
#[test]
#[ignore = "C8: Merge Projects (execute_merge_projects) is ported by the add/merge package"]
fn merging_two_projects_with_the_same_ids_keeps_the_first_copy() {
    const IDS: [&str; 3] = [
        "bbbbbbbb-0000-4000-8000-000000000001",
        "bbbbbbbb-0000-4000-8000-000000000002",
        "bbbbbbbb-0000-4000-8000-000000000003",
    ];
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str| {
        let (mut p, _) = survey_with_uuids(&IDS);
        p.name = name.to_string();
        let path = dir.path().join(format!("{name}.stalx"));
        std::fs::write(
            &path,
            stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap(),
        )
        .unwrap();
        path
    };
    let (a, b) = (write("A"), write("B"));
    let out = dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![a, b],
        output_path: out.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);
    let merged = stalx_v2::decode(&std::fs::read(&out).unwrap(), &Default::default())
        .unwrap()
        .project;
    assert_eq!(merged.lines().len(), 2, "both lines must survive");
    assert_eq!(line_uuids(&merged, 0), IDS.map(String::from).to_vec());
    assert!(line_uuids(&merged, 1)
        .iter()
        .all(|u| !IDS.contains(&u.as_str())));
    assert_unique(&merged);
}

// ---------------------------------------------------------------------------
// 7. Non-canonical spellings
// ---------------------------------------------------------------------------

/// The user-facing export writes whatever the source said, verbatim; the
/// SpeleoDB sidecar re-spells what it accepts into the `8-4-4-4-12` form its
/// `UUID4` field requires, and drops what it does not (a nil uuid is not a
/// v4).
#[test]
fn the_user_export_is_verbatim_while_the_speleodb_sidecar_canonicalises_or_drops() {
    const BRACED: &str = "{cccccccc-0000-4000-8000-000000000001}";
    const HYPHENLESS: &str = "cccccccc000040008000000000000002";
    const NIL: &str = "00000000-0000-0000-0000-000000000000";
    let (project, _) = survey_with_uuids(&[BRACED, HYPHENLESS, NIL]);
    assert_eq!(
        line_uuids(&project, 0),
        vec![BRACED.to_string(), HYPHENLESS.to_string(), NIL.to_string()],
        "the model stores what it was given, un-coerced"
    );

    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user.tmlu");
    export_tml(&project, &user).unwrap();
    let user_xml = std::fs::read_to_string(&user).unwrap();
    for raw in [BRACED, HYPHENLESS, NIL] {
        assert!(
            user_xml.contains(&format!("<UU>{raw}</UU>")),
            "the user export must not rewrite {raw}"
        );
    }
    let sidecar = dir.path().join("sidecar.tmlu");
    export_tml_for_speleodb(&project, &sidecar).unwrap();
    let sidecar_xml = std::fs::read_to_string(&sidecar).unwrap();
    assert!(sidecar_xml.contains("<UU>cccccccc-0000-4000-8000-000000000001</UU>"));
    assert!(sidecar_xml.contains("<UU>cccccccc-0000-4000-8000-000000000002</UU>"));
    assert!(!sidecar_xml.contains(BRACED));
    assert!(!sidecar_xml.contains(NIL), "a nil uuid is not a v4");
}

// ---------------------------------------------------------------------------
// 8. Adoption is idempotent
// ---------------------------------------------------------------------------

/// Adopting the same Ariane file twice must give the same shot identities
/// (`speleodb_pull::adopt_build_into` re-imports `ariane.tml` from scratch).
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn adopting_the_same_ariane_file_twice_gives_the_same_shot_uuids() {
    const IDS: [&str; 3] = [
        "eeeeeeee-0000-4000-8000-000000000001",
        "eeeeeeee-0000-4000-8000-000000000002",
        "eeeeeeee-0000-4000-8000-000000000003",
    ];
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "ariane.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(IDS[0])),
            Row::new(2, 1, Some(IDS[1])),
            Row::new(3, 2, Some(IDS[2])),
        ]),
    );
    let adopt_once = || {
        let state = state_with_project();
        import_ariane_file(&state, &source);
        line_uuids(&project(&state), 0)
    };
    let first = adopt_once();
    assert_eq!(first, IDS.map(String::from).to_vec());
    assert_eq!(first, adopt_once(), "re-adopting must not re-identify");
}

// ---------------------------------------------------------------------------
// 9. Undo
// ---------------------------------------------------------------------------

/// Undo gives a deleted shot its own identity back, not a fresh one, in the
/// session and after a save and reopen (the persisted entry carries no
/// session ids, but it carries the uuids).
#[test]
fn undoing_a_shot_delete_restores_the_shots_own_identity() {
    const IDS: [&str; 3] = [
        "dddddddd-0000-4000-8000-000000000001",
        "dddddddd-0000-4000-8000-000000000002",
        "dddddddd-0000-4000-8000-000000000003",
    ];
    let (p, line) = survey_with_uuids(&IDS);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("undo.stalx");
    std::fs::write(
        &path,
        stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::OpenProject { path: path.clone() })
            .success
    );
    let expected = IDS.map(String::from).to_vec();
    let mid = project(&state).line(line).unwrap().shots[1].id;

    assert!(
        state
            .execute(Command::DeleteShot {
                line_id: line,
                shot_id: mid
            })
            .success
    );
    assert!(state.undo().success);
    assert_eq!(line_uuids(&project(&state), 0), expected, "in the session");
    assert!(state.redo().success);
    assert!(state.execute(Command::SaveProject { path: None }).success);

    let reopened = AppState::new().unwrap();
    assert!(reopened.execute(Command::OpenProject { path }).success);
    assert_eq!(reopened.get_undo_stack().len(), 1);
    assert!(reopened.undo().success, "the persisted entry steps");
    assert_eq!(
        line_uuids(&project(&reopened), 0),
        expected,
        "after a reopen"
    );
}

/// The import's own undo and redo bring the file's identities back, not
/// fresh mints.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn undoing_and_redoing_an_import_keeps_the_files_identities() {
    const IDS: [&str; 3] = [
        "dddddddd-1111-4000-8000-000000000001",
        "dddddddd-1111-4000-8000-000000000002",
        "dddddddd-1111-4000-8000-000000000003",
    ];
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "undo.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(IDS[0])),
            Row::new(2, 1, Some(IDS[1])),
            Row::new(3, 2, Some(IDS[2])),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let expected = IDS.map(String::from).to_vec();
    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty(), "the import is undone");
    assert!(state.redo().success);
    assert_eq!(line_uuids(&project(&state), 0), expected);
}

// ---------------------------------------------------------------------------
// 10. A client cannot name a shot's identity
// ---------------------------------------------------------------------------

/// `ShotData` is the argument type of the `create_shot` command and carries
/// a `uuid`. The executor clears it, so a client naming an EXISTING shot's
/// identity (inserted before it, to take its place) gets a fresh mint, and
/// the original keeps the identity every reference to it was made with.
#[test]
fn create_shot_cannot_name_an_existing_shots_identity() {
    const IDS: [&str; 3] = [
        "ffffffff-0000-4000-8000-000000000001",
        "ffffffff-0000-4000-8000-000000000002",
        "ffffffff-0000-4000-8000-000000000003",
    ];
    let (p, line) = survey_with_uuids(&IDS);
    let victim = p.line(line).unwrap().shots[1].id;
    let state = state_with(p);

    let result = state.execute(Command::CreateShot {
        line_id: line,
        shot: ShotData {
            length: 4.0,
            ..shot_u(IDS[1])
        },
        after_shot_id: None,
        before_shot_id: Some(victim),
    });
    assert!(result.success, "{:?}", result.error);
    let after = project(&state);
    let claimants: Vec<u32> = after
        .line(line)
        .unwrap()
        .shots
        .iter()
        .filter(|s| s.uuid == IDS[1])
        .map(|s| s.id)
        .collect();
    assert_eq!(
        claimants,
        vec![victim],
        "only the original holds its identity"
    );
    assert_unique(&after);
}

/// The same attack through the other doors: the batch allowlist refuses
/// `CreateShot` outright, and a directly executed one is cleared.
#[test]
fn client_named_identities_are_stripped_at_the_shot_doors() {
    const IDS: [&str; 2] = [
        "ffffffff-0000-4000-8000-000000000011",
        "ffffffff-0000-4000-8000-000000000012",
    ];
    let (p, line) = survey_with_uuids(&IDS);
    let victim = p.line(line).unwrap().shots[1].id;
    let state = state_with(p);
    let smuggling = ShotData {
        length: 4.0,
        ..shot_u(IDS[1])
    };

    let result = state.execute(Command::Batch {
        commands: vec![Command::CreateShot {
            line_id: line,
            shot: smuggling.clone(),
            after_shot_id: None,
            before_shot_id: Some(victim),
        }],
        label: None,
    });
    assert!(!result.success, "CreateShot inside a batch is refused");
    assert_eq!(state.get_undo_stack().len(), 0);

    let result = state.execute(Command::CreateShot {
        line_id: line,
        shot: smuggling,
        after_shot_id: None,
        before_shot_id: Some(victim),
    });
    assert!(result.success, "{:?}", result.error);
    let after = project(&state);
    let holders = after
        .line(line)
        .unwrap()
        .shots
        .iter()
        .filter(|s| s.uuid == IDS[1])
        .count();
    assert_eq!(holders, 1);
    assert_unique(&after);
}

/// `CreateLine` carries a whole `Vec<ShotData>` off IPC, alone and in a
/// batch; neither may adopt a client-named identity.
#[test]
#[ignore = "C3: CreateLine is ported by the lines package (command_line.rs)"]
fn create_line_takes_no_client_named_identity() {
    const ID: &str = "ffffffff-0000-4000-8000-000000000021";
    let (p, _) = survey_with_uuids(&[ID]);
    let state = state_with(p);
    let line_with = |name: &str| Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot_u(ID)],
        cave_section_id: None,
    };
    let batched = state.execute(Command::Batch {
        commands: vec![line_with("Batched smuggler")],
        label: None,
    });
    assert!(batched.success, "{:?}", batched.error);
    let created = state.execute(line_with("Smuggler"));
    assert!(created.success, "{:?}", created.error);
    let after = project(&state);
    for line in after.lines().iter().skip(1) {
        assert_ne!(
            line.shots[0].uuid, ID,
            "{:?} adopted a client identity",
            line.name
        );
        assert!(!line.shots[0].uuid.is_empty());
    }
    assert_unique(&after);
}

/// The same attack on a sound project with a reference on the station the
/// victim shot ends at: the reference stays on that station (now one
/// position later), never moving to the new shot's station.
#[test]
fn create_shot_cannot_move_a_reference_onto_a_new_shot() {
    const IDS: [&str; 3] = [
        "ffffffff-1111-4000-8000-000000000001",
        "ffffffff-1111-4000-8000-000000000002",
        "ffffffff-1111-4000-8000-000000000003",
    ];
    let (mut p, line) = survey_with_uuids(&IDS);
    let victim = p.line(line).unwrap().shots[1].id;
    let anchored = st(&p, line, 2);
    set_comment(&mut p, anchored, "anchored on the victim");
    let state = state_with(p);

    let result = state.execute(Command::CreateShot {
        line_id: line,
        shot: ShotData {
            length: 4.0,
            ..shot_u(IDS[1])
        },
        after_shot_id: None,
        before_shot_id: Some(victim),
    });
    assert!(result.success, "{:?}", result.error);
    let after = project(&state);
    assert_eq!(
        st(&after, line, 3),
        anchored,
        "the victim's end is one further on"
    );
    assert_eq!(
        after.station(anchored).unwrap().comment.as_deref(),
        Some("anchored on the victim")
    );
    assert_eq!(after.station(st(&after, line, 2)).unwrap().comment, None);
}

// ---------------------------------------------------------------------------
// 11. The foreign id is bounded and characterised
// ---------------------------------------------------------------------------

/// A control character or an over-long value in `<UUID>` falls back to a
/// fresh mint, and the export stays valid XML 1.0 (asserted on the bytes).
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn an_unstorable_foreign_uuid_falls_back_to_a_mint_and_the_export_stays_well_formed() {
    const GOOD: &str = "abcdabcd-0000-4000-8000-000000000001";
    // `&#1;` unescapes to a real U+0001.
    const CONTROL: &str = "&#1;abcdabcd-0000-4000-8000-000000000002";
    // 200 characters - past the 128 cap.
    const OVERLONG: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "unstorable.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(GOOD)),
            Row::new(2, 1, Some(CONTROL)),
            Row::new(3, 2, Some(OVERLONG)),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let project = project(&state);
    let uuids = line_uuids(&project, 0);
    assert_eq!(uuids[0], GOOD);
    assert!(!uuids[1].contains('\u{1}') && !uuids[1].is_empty());
    assert!(uuids[2].len() <= 128 && uuids[2] != OVERLONG);
    assert!(uuids.iter().all(|u| !u.chars().any(char::is_control)));

    let exported = dir.path().join("out.tmlu");
    export_tml(&project, &exported).unwrap();
    let raw = std::fs::read(&exported).unwrap();
    let illegal: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|b| b.is_ascii_control() && !matches!(b, b'\t' | b'\n' | b'\r'))
        .collect();
    assert!(illegal.is_empty(), "{illegal:?}");
}

/// A NUL never reaches the model, so save and reload agree exactly.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn a_nul_bearing_uuid_cannot_diverge_across_save_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let xml = cave_file_xml(&[Row::new(1, 0, Some("aa\u{0}bb")), Row::new(2, 1, None)]);
    let source = write_tmlu(dir.path(), "nul.tmlu", &xml);
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let saved = project(&state);
    let before = line_uuids(&saved, 0);
    assert!(before.iter().all(|u| !u.contains('\u{0}') && !u.is_empty()));
    let bytes = stalx_v2::encode(&saved, &Extras::default(), Mode::Normal).unwrap();
    let reloaded = stalx_v2::decode(&bytes, &Default::default())
        .unwrap()
        .project;
    assert_eq!(line_uuids(&reloaded, 0), before);
}

// ---------------------------------------------------------------------------
// 12. Splitting one shot into two
// ---------------------------------------------------------------------------

/// Insert Station clones one shot into two: the TAIL keeps the parent's
/// identity, the head is new; a reference on the shot's end station stays on
/// it.
#[test]
fn splitting_a_shot_at_distance_leaves_the_identity_on_the_tail_only() {
    const IDS: [&str; 2] = [
        "cafecafe-0000-4000-8000-000000000001",
        "cafecafe-0000-4000-8000-000000000002",
    ];
    let (mut p, line) = survey_with_uuids(&IDS);
    let target = p.line(line).unwrap().shots[0].id;
    let end = st(&p, line, 1);
    set_comment(&mut p, end, "after the split shot");
    let state = state_with(p);

    let r = state.execute_split_shot_at_distance(line, target, 4.0);
    assert!(r.success, "{:?}", r.error);
    let after = project(&state);
    let uuids = line_uuids(&after, 0);
    assert_eq!(uuids.len(), 3, "the split adds one shot: {uuids:?}");
    assert_eq!(uuids[1], IDS[0], "the TAIL keeps the parent's identity");
    assert_ne!(uuids[0], IDS[0], "the HEAD is a new shot");
    assert!(!uuids[0].is_empty());
    assert_eq!(uuids[2], IDS[1]);
    assert_unique(&after);
    assert_eq!(
        st(&after, line, 2),
        end,
        "the end station is one further on"
    );
    assert_eq!(
        after.station(end).unwrap().comment.as_deref(),
        Some("after the split shot")
    );
}

// ---------------------------------------------------------------------------
// 13. Adversarial element shapes
// ---------------------------------------------------------------------------

/// An empty or whitespace-only element is "no identity": a mint.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn an_empty_or_whitespace_uuid_element_means_no_identity() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "blank.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some("")),
            Row::new(2, 1, Some("   ")),
            Row::new(3, 2, Some("dadadada-0000-4000-8000-000000000001")),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let project = project(&state);
    let uuids = line_uuids(&project, 0);
    for u in uuids.iter().take(2) {
        assert!(!u.trim().is_empty(), "a blank element yields a mint: {u:?}");
    }
    assert_eq!(uuids[2], "dadadada-0000-4000-8000-000000000001");
    assert_unique(&project);
}

/// A repeat across TWO lines of one file: the first line keeps the file's
/// id and the second gets a replacement, as the uniqueness invariant needs.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn the_same_uuid_in_two_lines_of_one_file_stays_in_the_first_line() {
    const SHARED: &str = "bebebebe-0000-4000-8000-000000000001";
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "cross-line.tmlu",
        &two_line_cave_file_xml(
            &[Row::new(1, 0, Some(SHARED)), Row::new(2, 1, None)],
            &[Row::new(3, 2, Some(SHARED)), Row::new(4, 3, None)],
        ),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let project = project(&state);
    assert_eq!(project.lines().len(), 2);
    assert_eq!(line_uuids(&project, 0)[0], SHARED);
    assert_ne!(line_uuids(&project, 1)[0], SHARED);
    assert_unique(&project);
}

/// Entity-escaped text is one value after unescaping and is stored whole;
/// the export re-escapes it.
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn an_entity_escaped_uuid_is_stored_whole_and_a_split_one_stays_exportable() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "escaped.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some("AA&amp;BB")),
            Row::new(2, 1, Some("AA<![CDATA[&]]>BB")),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let project = project(&state);
    let uuids = line_uuids(&project, 0);
    assert_eq!(uuids[0], "AA&BB");
    for u in &uuids {
        assert!(!u.is_empty() && !u.chars().any(char::is_control), "{u:?}");
    }
    assert_unique(&project);
    let exported = dir.path().join("escaped-out.tmlu");
    export_tml(&project, &exported).unwrap();
    let reparsed = parse_tml(&exported).expect("the export must be well-formed XML");
    let out: Vec<String> = reparsed
        .lines
        .iter()
        .flat_map(|s| s.shots.iter())
        .filter_map(|s| s.uuid.clone())
        .collect();
    assert!(out.contains(&"AA&BB".to_string()), "{out:?}");
}

// ---------------------------------------------------------------------------
// 14. The export-character gate is the XML rule, not just the control range
// ---------------------------------------------------------------------------

/// U+FFFF and U+FFFE are outside XML 1.0's `Char` but not control
/// characters: such an id falls back to a mint, and the export holds only
/// characters XML allows (asserted on the bytes).
#[test]
#[ignore = "C7: the import funnel (execute_import_lines_inner) is ported by the imports package"]
fn a_uuid_outside_the_xml_char_set_falls_back_to_a_mint() {
    const GOOD: &str = "beefbeef-0000-4000-8000-000000000001";
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "nonchar.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(GOOD)),
            Row::new(2, 1, Some("a\u{FFFF}b")),
            Row::new(3, 2, Some("a\u{FFFE}b")),
        ]),
    );
    let state = state_with_project();
    import_ariane_file(&state, &source);
    let project = project(&state);
    let uuids = line_uuids(&project, 0);
    assert_eq!(uuids[0], GOOD);
    for u in uuids.iter().skip(1) {
        assert!(!u.contains('\u{FFFF}') && !u.contains('\u{FFFE}') && !u.is_empty());
    }
    let exported = dir.path().join("nonchar-out.tmlu");
    export_tml(&project, &exported).unwrap();
    let text = String::from_utf8(std::fs::read(&exported).unwrap()).unwrap();
    let forbidden: Vec<u32> = text
        .chars()
        .filter(|c| {
            !matches!(c,
                '\u{9}' | '\u{A}' | '\u{D}'
                | '\u{20}'..='\u{D7FF}'
                | '\u{E000}'..='\u{FFFD}'
                | '\u{10000}'..='\u{10FFFF}')
        })
        .map(u32::from)
        .collect();
    assert!(forbidden.is_empty(), "{forbidden:x?}");
}
