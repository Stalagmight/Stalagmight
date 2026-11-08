//! Foreign shot identity: Ariane's `<UUID>`/`<UU>` carried through a round trip
//! (#423).
//!
//! Ariane 26 gives each `<SurveyData>` row a stable id and preserves it across
//! edits. We used to discard it and mint our own, so a file that left Ariane and
//! came back was re-identified shot by shot. These tests pin the contract at
//! each place identity could be dropped: the parse, the import, the transforms
//! that rebuild shots from `ShotData` (reverse, section split, project merge),
//! the undo payload, and the export.
//!
//! No fixture files: every `.tml`/`.tmlu` here is synthetic XML built in-test
//! and written to a temp dir that the test owns.

use std::collections::HashSet;
use std::path::Path;

use super::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::export::tml::{export_tml, export_tml_for_speleodb};
use crate::models::{Project, SectionData, ShotData, StationKey, StationRef};
use crate::parsers::tml::{convert_tml_to_import_configs, parse_tml};
use crate::test_fixtures::{state_with_null_sink, state_with_project};

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
fn survey_data_xml(row: &Row, shot_type: &str, section: &str) -> String {
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
    <Section>{section}</Section>
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

/// A one-section cave file: a START row plus one REAL row per entry in `rows`.
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

/// A two-section cave file: `main` hangs off the START, `side` hangs off the
/// last row of `main`. Used to show that a repeat ACROSS sections is legal and
/// must survive untouched.
fn two_section_cave_file_xml(main: &[Row], side: &[Row]) -> String {
    let start = survey_data_xml(&Row::new(0, -1, None), "START", "Main");
    let rows = |rows: &[Row], section: &str| {
        rows.iter()
            .map(|r| survey_data_xml(r, "REAL", section))
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
/// import configs, execute the import (`execute_tml_import_sections`, with the
/// surface-elevation derivation of #431). The adoption-idempotence test drives
/// the adoption itself rather than this mirror of it (#576 A8).
fn import_ariane_file(state: &AppState, path: &Path) {
    let tml = parse_tml(path).unwrap();
    let (sections, closures, _warnings) = convert_tml_to_import_configs(&tml, false, true).unwrap();
    let surface_elevation =
        crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        surface_elevation,
    );
    assert!(result.success, "import failed: {:?}", result.error);
}

/// Every shot UUID of section `index`, in section order. `None` becomes an empty
/// string so a missing identity fails an equality assertion loudly rather than
/// being skipped.
fn section_uuids(project: &Project, index: usize) -> Vec<String> {
    project.sections[index]
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap_or_default())
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Round trip
// ---------------------------------------------------------------------------

/// The file's `<UUID>` values come back out of the export verbatim, for both
/// extensions. Red at `normalize_key_bytes` before #423 (the element was
/// discarded and every shot got a fresh v4), and red again if the export ever
/// starts re-minting.
#[test]
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

        let state = state_with_project("Synthetic");
        import_ariane_file(&state, &source);

        let project = state.get_project_state().unwrap();
        assert_eq!(
            section_uuids(&project, 0),
            vec![U1.to_string(), U2.to_string(), U3.to_string()],
            "the file's identities must be the imported shots' identities"
        );

        // Round trip back out and in again: the exported document has to carry
        // the same ids, in the same order, under whichever element spelling the
        // extension selects (`<UUID>` for .tml, `<UU>` for .tmlu).
        let exported = dir.path().join(format!("out.{ext}"));
        export_tml(&project, &exported).unwrap();
        let reparsed = parse_tml(&exported).unwrap();
        let out: Vec<String> = reparsed
            .sections
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

/// Only the middle row carries an id. It must land on the middle shot - not the
/// first, not the last - and the uncovered rows must still get distinct,
/// non-empty identities of their own. Red if the carry misaligns by index.
#[test]
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

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3);
    assert_eq!(uuids[1], MID, "the covered shot must keep the file's id");
    assert_ne!(uuids[0], MID, "an uncovered shot must not inherit it");
    assert_ne!(uuids[2], MID, "an uncovered shot must not inherit it");
    assert!(
        !uuids[0].is_empty() && !uuids[2].is_empty(),
        "uncovered shots must still get an identity: {uuids:?}"
    );
    assert_ne!(uuids[0], uuids[2], "fresh mints must be distinct");
}

// ---------------------------------------------------------------------------
// 3. Intra-section duplicate
// ---------------------------------------------------------------------------

/// A source file that repeats an id inside one section is REPAIRED, not
/// rejected and not admitted: the FIRST occurrence keeps the foreign value and
/// the rest are re-minted, which is what `resolve_key` (first match wins) needs.
/// Red without the repair - and the import would not merely warn, it would be
/// rolled back wholesale by the `duplicate_shot_uuid` Critical violation that
/// `finalize_structural_edit` refuses to commit.
#[test]
fn a_repeated_uuid_inside_one_section_is_repaired_keeping_the_first() {
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

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3);
    assert_eq!(uuids[0], DUP, "the FIRST occurrence keeps the foreign id");
    assert_ne!(uuids[1], DUP, "the second must be re-minted");
    assert_ne!(uuids[2], DUP, "the third must be re-minted");
    assert_eq!(
        uuids.iter().collect::<HashSet<_>>().len(),
        3,
        "all three identities must differ: {uuids:?}"
    );
    let dupes: Vec<_> = project
        .check_structural_integrity()
        .into_iter()
        .filter(|v| v.category == "duplicate_shot_uuid")
        .collect();
    assert!(
        dupes.is_empty(),
        "import must not leave a duplicate_shot_uuid violation: {dupes:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Reverse on import
// ---------------------------------------------------------------------------

/// The import-time reverse (#24) moves each shot to the mirrored position.
/// Identity travels inside the `ShotData` being reversed, so it moves with its
/// own shot - the failure a parallel id vector would have had to keep in step by
/// hand. Red if identity ever stops travelling in the payload.
#[test]
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

    let state = state_with_project("Synthetic");
    let tml = parse_tml(&source).unwrap();
    let (mut sections, closures, _warnings) =
        convert_tml_to_import_configs(&tml, false, true).unwrap();
    sections[0].reverse = true;
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        None,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3);
    assert_eq!(uuids[0], LAST, "the file's last id must be first now");
    assert_eq!(uuids[2], FIRST, "the file's first id must be last now");
}

// ---------------------------------------------------------------------------
// 5. Section split
// ---------------------------------------------------------------------------

/// Splitting a section keeps the foreign ids on BOTH halves (section B is
/// rebuilt from `to_shot_data`), and every station cross-reference still points
/// at the same physical station it did before.
///
/// This is also what pins the split's positional remap as still necessary. A
/// `StationKey` is `(section_id, anchor)`, so a preserved uuid alone cannot
/// follow a shot into the NEW section: drop the remap and the B-half comment's
/// key keeps naming section A, where its anchoring shot no longer exists, and
/// resolves to `None`.
#[test]
fn a_section_split_preserves_the_foreign_ids_and_the_cross_references_of_both_halves() {
    const IDS: [&str; 5] = [
        "aaaaaaaa-0000-4000-8000-000000000001",
        "aaaaaaaa-0000-4000-8000-000000000002",
        "aaaaaaaa-0000-4000-8000-000000000003",
        "aaaaaaaa-0000-4000-8000-000000000004",
        "aaaaaaaa-0000-4000-8000-000000000005",
    ];

    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "split.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(IDS[0])),
            Row::new(2, 1, Some(IDS[1])),
            Row::new(3, 2, Some(IDS[2])),
            Row::new(4, 3, Some(IDS[3])),
            Row::new(5, 4, Some(IDS[4])),
        ]),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let section_id = state.get_project_state().unwrap().sections[0].id;
    // One cross-reference on each side of the coming split: station 1 stays in
    // A, station 4 moves into B.
    for (index, text) in [(1u32, "stays in A"), (4u32, "moves into B")] {
        let r = state.execute(Command::SetStationComment {
            station_ref: StationRef::new(section_id, index),
            comment: Some(text.to_string()),
        });
        assert!(r.success, "{:?}", r.error);
    }

    // Where each stored comment key physically sits before the split.
    let before = state.get_project_state().unwrap();
    let coords_before = |p: &Project, key: &StationKey| {
        let sr = p.resolve_key(key).expect("comment key resolves");
        p.compute_station_coordinates()
            .into_iter()
            .find(|s| s.station_ref == sr)
            .expect("resolved station has coordinates")
            .coordinates
    };
    let anchored: Vec<(StationKey, String, _)> = before
        .station_comments
        .iter()
        .map(|(k, text)| (k.clone(), text.clone(), coords_before(&before, k)))
        .collect();
    assert_eq!(anchored.len(), 2);

    // Split at the middle shot (index 2): A keeps shots 0..2, the shot at 2 is
    // deleted, B receives shots 3..5.
    let mid = before.sections[0].shots[2].id;
    let r = state.execute_delete_shot_and_split(section_id, mid, "A".to_string(), "B".to_string());
    assert!(r.success, "{:?}", r.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(after.sections.len(), 2);
    assert_eq!(
        section_uuids(&after, 0),
        vec![IDS[0].to_string(), IDS[1].to_string()],
        "the A half lost its foreign ids"
    );
    assert_eq!(
        section_uuids(&after, 1),
        vec![IDS[3].to_string(), IDS[4].to_string()],
        "the B half lost its foreign ids - the case the side-channel design gave up on"
    );

    // Every cross-reference still resolves, and to the same physical station.
    assert_eq!(after.station_comments.len(), 2);
    for (_, text, coords) in &anchored {
        let (key, _) = after
            .station_comments
            .iter()
            .find(|(_, t)| *t == text)
            .unwrap_or_else(|| panic!("comment {text:?} disappeared across the split"));
        let sr = after
            .resolve_key(key)
            .unwrap_or_else(|| panic!("comment {text:?} dangles after the split"));
        let now = after
            .compute_station_coordinates()
            .into_iter()
            .find(|s| s.station_ref == sr)
            .expect("resolved station has coordinates")
            .coordinates;
        let moved = (now.x - coords.x).abs() + (now.y - coords.y).abs() + (now.z - coords.z).abs();
        assert!(
            moved < 1e-9,
            "comment {text:?} re-anchored onto a different station: {coords:?} -> {now:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Project merge
// ---------------------------------------------------------------------------

/// Two projects that each imported the same Ariane file hold the same foreign
/// ids in different sections. Uniqueness in this model is per-SECTION, so that
/// is legal and the merge must leave both copies alone. Red on the old
/// project-wide dedupe, which re-minted the second project's whole section.
#[test]
fn merging_two_projects_that_imported_one_file_keeps_both_copies_of_its_ids() {
    const IDS: [&str; 3] = [
        "bbbbbbbb-0000-4000-8000-000000000001",
        "bbbbbbbb-0000-4000-8000-000000000002",
        "bbbbbbbb-0000-4000-8000-000000000003",
    ];

    // What an Ariane import produces: shots whose ShotData already carries the
    // file's ids.
    let imported_project = |name: &str| {
        let mut project = Project::new(name.to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            shots: IDS
                .iter()
                .map(|u| ShotData {
                    uuid: Some((*u).to_string()),
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 10.0,
                    depth_in: 5.0,
                    depth_out: 5.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        project
    };

    let a = imported_project("A");
    let b = imported_project("B");
    assert_eq!(section_uuids(&a, 0), IDS.map(String::from).to_vec());

    let (db_a, _keep_a) = crate::test_helpers::TestDb::with_project(&a).unwrap();
    let (db_b, _keep_b) = crate::test_helpers::TestDb::with_project(&b).unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("merged.stalx");

    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![db_a.path().to_path_buf(), db_b.path().to_path_buf()],
        out.clone(),
        "Merged".to_string(),
        false,
    );
    assert!(result.success, "merge failed: {:?}", result.error);

    // The merge writes the output file rather than opening it, so read it back.
    let merged = crate::persistence::Database::open_existing(&out)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(merged.sections.len(), 2, "both sections must survive");
    let expected = IDS.map(String::from).to_vec();
    assert_eq!(
        section_uuids(&merged, 0),
        expected,
        "the first source's foreign ids must survive the merge"
    );
    assert_eq!(
        section_uuids(&merged, 1),
        expected,
        "the second source's foreign ids must survive too - a cross-section \
         repeat is legal, and re-minting throws away the identity the import kept"
    );
}

// ---------------------------------------------------------------------------
// 7. Non-canonical spellings
// ---------------------------------------------------------------------------

/// `Shot::uuid` is no longer guaranteed to be a v4 we minted, so the two export
/// profiles diverge on purpose. The user-facing file writes whatever the source
/// said, verbatim. The SpeleoDB sidecar re-spells what it accepts into the
/// `8-4-4-4-12` form its `UUID4` field requires, and drops what it does not
/// (a nil uuid is not a v4).
#[test]
fn the_user_export_is_verbatim_while_the_speleodb_sidecar_canonicalises_or_drops() {
    const BRACED: &str = "{cccccccc-0000-4000-8000-000000000001}";
    const HYPHENLESS: &str = "cccccccc000040008000000000000002";
    const NIL: &str = "00000000-0000-0000-0000-000000000000";

    let mut project = Project::new("Spellings".to_string(), None);
    project.add_section(SectionData {
        name: Some("Main".to_string()),
        shots: [BRACED, HYPHENLESS, NIL]
            .iter()
            .map(|u| ShotData {
                uuid: Some((*u).to_string()),
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 5.0,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    assert_eq!(
        section_uuids(&project, 0),
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
    assert!(
        sidecar_xml.contains("<UU>cccccccc-0000-4000-8000-000000000001</UU>"),
        "the sidecar must canonicalise the braced form"
    );
    assert!(
        sidecar_xml.contains("<UU>cccccccc-0000-4000-8000-000000000002</UU>"),
        "the sidecar must canonicalise the hyphenless form"
    );
    assert!(
        !sidecar_xml.contains(BRACED),
        "the braced spelling would fail the server's UUID4 field"
    );
    assert!(
        !sidecar_xml.contains(NIL),
        "a nil uuid is not a v4 and must be omitted from the sidecar"
    );
}

// ---------------------------------------------------------------------------
// 8. Adoption is idempotent
// ---------------------------------------------------------------------------

/// Adopting the same Ariane file twice must give the same shot identities.
/// `speleodb_adopt::adopt_build_into` re-imports `ariane.tml` from scratch, so
/// before the file's `<UUID>` was read back every adoption re-identified every
/// shot - the user-visible churn in the issue.
///
/// [#576 A8] Driven through the adoption's own local half,
/// `adopt_import_and_save` (create, import, save, promote, reopen, stamp), with
/// a null event sink: the create it runs needs a sink, not an `AppHandle`. It
/// used to run a hand-built mirror of the pipeline, kept honest by a tripwire
/// that read `adopt_build_into`'s source for the calls it made - which a change
/// that kept those calls and still dropped the identity would have passed.
///
/// Full `<UUID>` coverage is load-bearing: an uncovered row legitimately mints
/// per adoption, so a mixed file could only be compared over its covered subset.
#[test]
fn adopting_the_same_ariane_file_twice_gives_the_same_shot_uuids() {
    const IDS: [&str; 3] = [
        "eeeeeeee-0000-4000-8000-000000000001",
        "eeeeeeee-0000-4000-8000-000000000002",
        "eeeeeeee-0000-4000-8000-000000000003",
    ];
    const PROJECT_ID: &str = "eeeeeeee-0000-4000-8000-0000000000aa";

    let _cfg = crate::test_helpers::isolated_config();
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

    let adopt_once = |attempt: &str| {
        let dest = dir.path().join(attempt).join("project.stalx");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let state = state_with_null_sink();
        let adopted = state.adopt_import_and_save(
            None,
            &source,
            "ariane.tmlu",
            &dest,
            "CaveA".to_string(),
            |undo_mode| crate::models::SpeleoLink {
                endpoint: "http://speleodb.invalid/".to_string(),
                project_id: PROJECT_ID.to_string(),
                project_name: "CaveA".to_string(),
                base_commit_sha: None,
                has_unpushed_edits: true,
                last_edit_at: Some("2026-07-27T10:00:00Z".to_string()),
                last_verify_at: None,
                undo_mode: Some(undo_mode),
            },
        );
        assert!(
            matches!(adopted, Ok((_, Ok(())))),
            "the adoption's local half adopts and stamps"
        );
        section_uuids(&state.get_project_state().unwrap(), 0)
    };

    let first = adopt_once("first");
    let second = adopt_once("second");
    assert_eq!(
        first,
        IDS.map(String::from).to_vec(),
        "an adopted project must carry the file's own shot ids"
    );
    assert_eq!(
        first, second,
        "re-adopting the same file must not re-identify its shots"
    );
}

// ---------------------------------------------------------------------------
// 9. Undo
// ---------------------------------------------------------------------------

/// Identity now travels in the undo payload, so both undo paths must give back
/// the SAME ids rather than fresh ones: the per-shot restore (which replays a
/// `ShotData` plus the uuid the entry recorded beside it) and the import's
/// snapshot undo/redo.
#[test]
fn undo_restores_the_imported_identities_rather_than_minting_new_ones() {
    const IDS: [&str; 3] = [
        "dddddddd-0000-4000-8000-000000000001",
        "dddddddd-0000-4000-8000-000000000002",
        "dddddddd-0000-4000-8000-000000000003",
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

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);
    let expected = IDS.map(String::from).to_vec();
    assert_eq!(
        section_uuids(&state.get_project_state().unwrap(), 0),
        expected
    );

    // Leg 1: delete a shot and undo it. The restored shot must come back with
    // the file's id, not a new one.
    let (section_id, mid_shot_id) = {
        let p = state.get_project_state().unwrap();
        (p.sections[0].id, p.sections[0].shots[1].id)
    };
    let r = state.execute(Command::DeleteShot {
        section_id,
        shot_id: mid_shot_id,
    });
    assert!(r.success, "{:?}", r.error);
    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(
        section_uuids(&state.get_project_state().unwrap(), 0),
        expected,
        "undoing a shot delete must restore the shot's own identity"
    );

    // Leg 2: undo the import itself, then redo it.
    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);
    assert!(
        state.get_project_state().unwrap().sections.is_empty(),
        "undoing the import must remove the imported section"
    );
    let redone = state.redo();
    assert!(redone.success, "{:?}", redone.error);
    assert_eq!(
        section_uuids(&state.get_project_state().unwrap(), 0),
        expected,
        "redoing the import must bring back the file's identities, not fresh mints"
    );
}

// ---------------------------------------------------------------------------
// 10. The IPC boundary must not let a client name a shot's identity
// ---------------------------------------------------------------------------

/// `ShotData` is the argument type of the `create_shot` command, and since #423
/// it has a `uuid` field. If that field were honoured, a caller could name an
/// EXISTING shot's identity and insert the new shot BEFORE it: both first-wins
/// repairs keep the first OCCURRENCE - now the injected record - and re-mint the
/// original, moving every `AfterShot` anchor onto the injected shot.
///
/// The executor (`AppState::create_shot`, core/state/command_shot.rs) clears
/// it, so the original keeps its identity. The fixture is deliberately an
/// ALREADY-critical project, because
/// `execute`'s structural epilogue rolls back only newly introduced criticals -
/// on a sound project it refuses the create outright (pinned by the companion
/// test), and that would hide the case where nothing objects at all.
#[test]
fn create_shot_cannot_name_an_existing_shots_identity() {
    const IDS: [&str; 3] = [
        "ffffffff-0000-4000-8000-000000000001",
        "ffffffff-0000-4000-8000-000000000002",
        "ffffffff-0000-4000-8000-000000000003",
    ];

    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "boundary.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(IDS[0])),
            Row::new(2, 1, Some(IDS[1])),
            Row::new(3, 2, Some(IDS[2])),
        ]),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    // Make the project ALREADY critical before the attack. `execute`'s
    // structural epilogue only rolls back NEWLY introduced criticals
    // (`finalize_structural_edit(pre_had_critical)`), so this is the state in
    // which the duplicate is admitted and nothing objects until the pre-save
    // repair re-mints the original - the silent shape of the hazard, and the
    // reason the executor clear is load-bearing rather than belt-and-braces.
    let (section_id, victim_shot_id) = {
        let mut p = state.get_project_state().unwrap();
        let ids = (p.sections[0].id, p.sections[0].shots[1].id);
        p.add_section(SectionData {
            name: Some("Corrupt".to_string()),
            shots: vec![ShotData {
                length: f64::NAN,
                ..Default::default()
            }],
            ..Default::default()
        });
        assert!(
            p.has_critical_integrity_violations(),
            "the fixture must start out critical or the epilogue masks the hazard"
        );
        state.set_project_for_tests(p);
        ids
    };

    // The attack: create a shot that claims the victim's identity, placed
    // BEFORE it so it becomes the first occurrence.
    let result = state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            uuid: Some(IDS[1].to_string()),
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 4.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: Some(victim_shot_id),
    });
    assert!(result.success, "{:?}", result.error);

    let mut project = state.get_project_state().unwrap();
    let claimants: Vec<u32> = project.sections[0]
        .shots
        .iter()
        .filter(|s| s.uuid.as_deref() == Some(IDS[1]))
        .map(|s| s.id)
        .collect();
    assert_eq!(
        claimants,
        vec![victim_shot_id],
        "only the original shot may hold its identity - the client named it and \
         was inserted first, so a second claimant is the injection landing"
    );

    // The pre-save repair is where the damage would land, so run it and check
    // the victim - not the injected shot - still owns the identity.
    let reminted = project.ensure_unique_shot_uuids();
    assert_eq!(reminted, 0, "there was never a duplicate to repair");
    let victim = project.sections[0]
        .shots
        .iter()
        .find(|s| s.id == victim_shot_id)
        .expect("the victim shot survives");
    assert_eq!(
        victim.uuid.as_deref(),
        Some(IDS[1]),
        "the original shot must keep the identity its anchors resolve through"
    );
}

/// The same attack through the remaining command doors (pre-PR review, #423).
/// `execute_batch` is a `#[tauri::command]` taking
/// `Vec<Command>` verbatim from the webview; the batch allowlist
/// (`is_batch_allowed`) refuses `CreateShot` today, and this pins that refusal
/// - but `CreateSection` IS batch-allowed, and `state.execute` itself is
/// reachable by any internal caller holding a constructed command. So the
/// identity rule cannot live in one IPC wrapper: the executor
/// (`AppState::create_shot`) and the two `CreateSection` dispatch arms clear
/// it, and each surface is pinned here. Same already-critical fixture as
/// above: a batch takes no integrity snapshot (`finalize_structural_edit`
/// skips `Command::Batch`), so nothing downstream would roll an injection
/// back.
#[test]
fn client_named_identities_are_stripped_at_every_command_door() {
    const IDS: [&str; 2] = [
        "ffffffff-0000-4000-8000-000000000011",
        "ffffffff-0000-4000-8000-000000000012",
    ];

    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "batch-boundary.tmlu",
        &cave_file_xml(&[Row::new(1, 0, Some(IDS[0])), Row::new(2, 1, Some(IDS[1]))]),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let (section_id, victim_shot_id) = {
        let mut p = state.get_project_state().unwrap();
        let ids = (p.sections[0].id, p.sections[0].shots[1].id);
        p.add_section(SectionData {
            name: Some("Corrupt".to_string()),
            shots: vec![ShotData {
                length: f64::NAN,
                ..Default::default()
            }],
            ..Default::default()
        });
        assert!(
            p.has_critical_integrity_violations(),
            "the fixture must start out critical or the epilogue masks the hazard"
        );
        state.set_project_for_tests(p);
        ids
    };

    let smuggling_shot = ShotData {
        uuid: Some(IDS[1].to_string()),
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 4.0,
        depth_in: 5.0,
        depth_out: 5.0,
        ..Default::default()
    };

    // Surface 1: the batch allowlist refuses CreateShot outright. If someone
    // ever allowlists it, the executor clear below still holds.
    let result = state.execute(Command::Batch {
        commands: vec![Command::CreateShot {
            section_id,
            shot: smuggling_shot.clone(),
            after_shot_id: None,
            before_shot_id: Some(victim_shot_id),
        }],
    });
    assert!(
        !result.success,
        "CreateShot inside a batch is refused by the allowlist"
    );

    // Surface 2: `state.execute(Command::CreateShot ...)` directly - the
    // dispatch arm any internal caller lands on. The executor clears the
    // identity, so the victim keeps it and the new shot gets a mint.
    let result = state.execute(Command::CreateShot {
        section_id,
        shot: smuggling_shot.clone(),
        after_shot_id: None,
        before_shot_id: Some(victim_shot_id),
    });
    assert!(result.success, "{:?}", result.error);

    let project = state.get_project_state().unwrap();
    let claimants: Vec<u32> = project.sections[0]
        .shots
        .iter()
        .filter(|s| s.uuid.as_deref() == Some(IDS[1]))
        .map(|s| s.id)
        .collect();
    assert_eq!(
        claimants,
        vec![victim_shot_id],
        "a directly executed CreateShot must not smuggle an identity past the executor clear"
    );

    // Surface 3: CreateSection IS batch-allowed, so the batched section door is
    // live. The strip at the `execute_without_undo` dispatch arm must hold.
    let batched = state.execute(Command::Batch {
        commands: vec![Command::CreateSection {
            name: Some("Batched smuggler".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![smuggling_shot.clone()],
        }],
    });
    assert!(batched.success, "{:?}", batched.error);
    let project = state.get_project_state().unwrap();
    let batched_section = project
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Batched smuggler"))
        .unwrap();
    assert_ne!(
        batched_section.shots[0].uuid.as_deref(),
        Some(IDS[1]),
        "a batched CreateSection must not adopt a client-named identity"
    );

    // The section-shaped door too: `Command::CreateSection` carries a whole
    // `Vec<ShotData>` off IPC. Its executor is shared with the importers (whose
    // identities are legitimate), so the strip lives at the dispatch arms - a
    // client-named identity must not land through it either. Cross-section
    // repeats are legal in the model, so the assertion is on the NEW section:
    // its shot must come back with a fresh mint, not the named identity.
    let created = state.execute(Command::CreateSection {
        name: Some("Smuggler".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![ShotData {
            uuid: Some(IDS[1].to_string()),
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 4.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        }],
    });
    assert!(created.success, "{:?}", created.error);
    let project = state.get_project_state().unwrap();
    let new_section = project.get_section(created.section_id.unwrap()).unwrap();
    assert_ne!(
        new_section.shots[0].uuid.as_deref(),
        Some(IDS[1]),
        "CreateSection must not adopt a client-named identity"
    );
    assert!(
        new_section.shots[0].uuid.is_some(),
        "and it still mints one"
    );
}

/// The same attack on a SOUND project, with a cross-reference anchored on the
/// victim: the anchor must still resolve to the victim's own station and the
/// create must still succeed. Without the executor clear this fails earlier and
/// differently - `duplicate_shot_uuid` is Critical, so `execute`'s epilogue
/// rolls the whole create back with "Blocked: this action would corrupt the
/// survey structure". Recorded as the second line of defence it is, and as
/// evidence that leaving the field honoured would also break a legitimate edit.
#[test]
fn create_shot_cannot_move_an_existing_anchor_onto_a_new_shot() {
    const IDS: [&str; 3] = [
        "ffffffff-1111-4000-8000-000000000001",
        "ffffffff-1111-4000-8000-000000000002",
        "ffffffff-1111-4000-8000-000000000003",
    ];

    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "anchored-boundary.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(IDS[0])),
            Row::new(2, 1, Some(IDS[1])),
            Row::new(3, 2, Some(IDS[2])),
        ]),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    // Anchor a comment on the station AFTER shot index 1, i.e. keyed on IDS[1].
    let (section_id, victim_shot_id) = {
        let p = state.get_project_state().unwrap();
        (p.sections[0].id, p.sections[0].shots[1].id)
    };
    let r = state.execute(Command::SetStationComment {
        station_ref: StationRef::new(section_id, 2),
        comment: Some("anchored on the victim".to_string()),
    });
    assert!(r.success, "{:?}", r.error);
    let anchored_key: StationKey = {
        let p = state.get_project_state().unwrap();
        let (key, _) = p.station_comments.iter().next().unwrap();
        assert_eq!(
            key.anchor,
            crate::models::StationAnchor::AfterShot(IDS[1].to_string()),
            "the comment must be anchored on the victim's own identity"
        );
        key.clone()
    };

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            uuid: Some(IDS[1].to_string()),
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 4.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: Some(victim_shot_id),
    });
    assert!(result.success, "{:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.resolve_key(&anchored_key),
        Some(StationRef::new(section_id, 3)),
        "the anchored comment must still resolve to the victim's own station \
         (now one position later), not to the newly created shot"
    );
}

// ---------------------------------------------------------------------------
// 11. The foreign id is bounded and characterised
// ---------------------------------------------------------------------------

/// A control character in `<UUID>` is not merely odd: the writer escapes only
/// `<>&"'`, so it goes into the export as a raw byte and the document stops
/// being valid XML 1.0. Our own quick-xml reader is lenient enough to read it
/// straight back (measured), so the assertion below is on the emitted BYTES -
/// checking re-import would pass either way and pin nothing. The cost lands on
/// the strict consumers: Ariane and SpeleoDB's converter reject the file.
///
/// An unbounded value is a separate, resource lever: it is cloned into the
/// project-wide uuid index and into every undo snapshot. Both cases fall back to
/// a fresh mint, which costs that shot its foreign identity and nothing else.
#[test]
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

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3);
    assert_eq!(uuids[0], GOOD, "a well-formed id is still adopted verbatim");
    assert!(
        !uuids[1].contains('\u{1}') && !uuids[1].is_empty(),
        "a control-bearing id must be replaced by a mint, not stored: {:?}",
        uuids[1]
    );
    assert!(
        uuids[2].len() <= 128 && uuids[2] != OVERLONG,
        "an over-long id must be replaced by a mint: {} chars",
        uuids[2].len()
    );
    for u in &uuids {
        assert!(
            !u.chars().any(char::is_control),
            "no stored identity may hold a control character: {u:?}"
        );
    }

    // The whole point of the shape check: what we emit is valid XML 1.0. Assert
    // on the BYTES - a raw C0 control in element content is exactly what the
    // spec forbids and what a strict consumer rejects, and our own lenient
    // reader would happily round-trip it, so re-parsing pins nothing.
    let exported = dir.path().join("out.tmlu");
    export_tml(&project, &exported).unwrap();
    let raw = std::fs::read(&exported).unwrap();
    let illegal: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|b| b.is_ascii_control() && !matches!(b, b'\t' | b'\n' | b'\r'))
        .collect();
    assert!(
        illegal.is_empty(),
        "the export must not contain characters XML 1.0 forbids in content: {illegal:?}"
    );
    let out: Vec<String> = parse_tml(&exported)
        .unwrap()
        .sections
        .iter()
        .flat_map(|s| s.shots.iter())
        .filter_map(|s| s.uuid.clone())
        .collect();
    assert!(
        out.contains(&GOOD.to_string()),
        "the good id must survive the round trip: {out:?}"
    );
}

/// A NUL is the case that would diverge silently rather than loudly: SQLite
/// TEXT read-back can truncate at it, so a saved identity and a reloaded one
/// would differ with nothing to show for it. The shape check keeps NUL out of
/// the model entirely, so save and reload agree exactly.
#[test]
fn a_nul_bearing_uuid_cannot_diverge_across_save_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    // A literal NUL and a literal U+0001 in the element text, not entities.
    let xml = cave_file_xml(&[Row::new(1, 0, Some("aa\u{0}bb")), Row::new(2, 1, None)]);
    let source = write_tmlu(dir.path(), "nul.tmlu", &xml);

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);
    let saved = state.get_project_state().unwrap();
    let before = section_uuids(&saved, 0);
    assert!(
        before.iter().all(|u| !u.contains('\u{0}') && !u.is_empty()),
        "a NUL-bearing id must never reach the model: {before:?}"
    );

    let (db, _keep) = crate::test_helpers::TestDb::with_project(&saved).unwrap();
    let reloaded = crate::persistence::Database::open_existing(db.path())
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(
        section_uuids(&reloaded, 0),
        before,
        "identity must not change across save and reload"
    );
}

// ---------------------------------------------------------------------------
// 12. Splitting one shot into two is a duplicate path
// ---------------------------------------------------------------------------

/// `compute_split` is the one place a shot is cloned into two, and its
/// `uuid: None` on the head is the load-bearing instance of the rule on
/// [`ShotData::uuid`]: the TAIL keeps the parent's identity (which is what lets
/// every existing anchor keep resolving with no remap), the head is new.
///
/// Red if the head copies the parent's uuid: the section then holds the id
/// twice with the head FIRST, and the first-wins repair re-mints the tail -
/// moving every anchor onto the head.
#[test]
fn splitting_a_shot_at_distance_leaves_the_identity_on_the_tail_only() {
    const IDS: [&str; 2] = [
        "cafecafe-0000-4000-8000-000000000001",
        "cafecafe-0000-4000-8000-000000000002",
    ];

    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "split-shot.tmlu",
        &cave_file_xml(&[Row::new(1, 0, Some(IDS[0])), Row::new(2, 1, Some(IDS[1]))]),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let (section_id, target) = {
        let p = state.get_project_state().unwrap();
        (p.sections[0].id, p.sections[0].shots[0].id)
    };
    // Anchor a comment on the station after the shot being split.
    let r = state.execute(Command::SetStationComment {
        station_ref: StationRef::new(section_id, 1),
        comment: Some("after the split shot".to_string()),
    });
    assert!(r.success, "{:?}", r.error);

    let r = state.execute_split_shot_at_distance(section_id, target, 4.0);
    assert!(r.success, "{:?}", r.error);

    let mut project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3, "the split adds one shot: {uuids:?}");
    assert_eq!(
        uuids[1], IDS[0],
        "the TAIL child must keep the parent's identity"
    );
    assert_ne!(
        uuids[0], IDS[0],
        "the HEAD child is a new shot and must not copy the parent's identity"
    );
    assert!(!uuids[0].is_empty(), "the head must still get an identity");
    assert_eq!(uuids[2], IDS[1], "the untouched shot keeps its own id");
    assert_eq!(
        project.ensure_unique_shot_uuids(),
        0,
        "the split must not leave a duplicate for the repair to find"
    );

    // The anchored comment must still name the tail, so it lands on the station
    // that is now one position later.
    let (key, _) = {
        let p = state.get_project_state().unwrap();
        let (k, t) = p.station_comments.iter().next().unwrap();
        (k.clone(), t.clone())
    };
    assert_eq!(
        key.anchor,
        crate::models::StationAnchor::AfterShot(IDS[0].to_string())
    );
    assert_eq!(
        state.get_project_state().unwrap().resolve_key(&key),
        Some(StationRef::new(section_id, 2)),
        "the comment must follow the tail child, not snap onto the head"
    );
}

// ---------------------------------------------------------------------------
// 13. Adversarial element shapes
// ---------------------------------------------------------------------------

/// An empty or whitespace-only element is "no identity", not an identity that
/// happens to be blank - an empty stored uuid reads as a real shared key to
/// anything that consumes it (see `export::tml::shot_export_uuid`).
#[test]
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

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3);
    for (i, u) in uuids.iter().take(2).enumerate() {
        assert!(
            !u.is_empty() && !u.trim().is_empty(),
            "blank element {i} must yield a minted identity, not a blank one: {u:?}"
        );
    }
    assert_eq!(uuids[0..2].iter().collect::<HashSet<_>>().len(), 2);
    assert_eq!(uuids[2], "dadadada-0000-4000-8000-000000000001");
    assert!(project
        .check_structural_integrity()
        .iter()
        .all(|v| v.category != "duplicate_shot_uuid"));
}

/// A repeat across TWO sections of one file is legal by design - `resolve_key`
/// is section-local, so it cannot mis-resolve - and must be left alone. Red if
/// the per-section repair is ever widened back to project scope.
#[test]
fn the_same_uuid_in_two_sections_of_one_file_is_left_alone() {
    const SHARED: &str = "bebebebe-0000-4000-8000-000000000001";

    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "cross-section.tmlu",
        &two_section_cave_file_xml(
            &[Row::new(1, 0, Some(SHARED)), Row::new(2, 1, None)],
            &[Row::new(3, 2, Some(SHARED)), Row::new(4, 3, None)],
        ),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        2,
        "the file must import as 2 sections"
    );
    let held: Vec<String> = (0..2)
        .map(|i| section_uuids(&project, i)[0].clone())
        .collect();
    assert_eq!(
        held,
        vec![SHARED.to_string(), SHARED.to_string()],
        "a cross-section repeat is legal and must survive in BOTH sections"
    );
    assert!(
        project
            .check_structural_integrity()
            .iter()
            .all(|v| v.category != "duplicate_shot_uuid"),
        "a cross-section repeat is not a violation"
    );
}

/// Entity-escaped text is one value after unescaping and must be stored whole.
/// A CDATA-split value is the shape the field reader cannot represent - the
/// `Text` handler overwrites per chunk, which predates #423 but only became
/// identity-bearing with it. Whatever it yields, the invariant that matters
/// holds: the stored id is a single non-empty value with no control characters,
/// and the export re-parses.
#[test]
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

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 2);
    assert_eq!(
        uuids[0], "AA&BB",
        "an entity-escaped id must be unescaped once and stored whole"
    );
    for u in &uuids {
        assert!(!u.is_empty() && !u.chars().any(char::is_control), "{u:?}");
    }
    assert_eq!(uuids.iter().collect::<HashSet<_>>().len(), 2);

    // The escaping has to survive a round trip, or the export is unreadable.
    let exported = dir.path().join("escaped-out.tmlu");
    export_tml(&project, &exported).unwrap();
    let reparsed = parse_tml(&exported).expect("the export must be well-formed XML");
    let out: Vec<String> = reparsed
        .sections
        .iter()
        .flat_map(|s| s.shots.iter())
        .filter_map(|s| s.uuid.clone())
        .collect();
    assert!(
        out.contains(&"AA&BB".to_string()),
        "the ampersand must survive re-escaping: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// 14. The export-character gate is the XML rule, not just the control range
// ---------------------------------------------------------------------------

/// U+FFFF is excluded by XML 1.0's `Char` production but is NOT a control
/// character, so a gate written as `char::is_control` adopted it, stored it, and
/// let `BytesText::new` (which escapes only `<>&"'`) write it raw into element
/// content - producing the ill-formed document the gate exists to prevent.
///
/// The asymmetry is what makes this worth a hard check rather than a shrug: a
/// rejected id costs ONE shot its foreign identity, an accepted bad byte costs
/// the WHOLE file, because a strict consumer rejects the document rather than
/// the row.
///
/// Asserted on the emitted BYTES: our own quick-xml reader is lenient enough to
/// read such a document straight back, so a re-parse would pin nothing.
#[test]
fn a_uuid_outside_the_xml_char_set_falls_back_to_a_mint() {
    const GOOD: &str = "beefbeef-0000-4000-8000-000000000001";
    // Neither of these is a control character; both are outside `Char`.
    const NONCHAR_FFFF: &str = "a\u{FFFF}b";
    const NONCHAR_FFFE: &str = "a\u{FFFE}b";

    // The gate itself is unit-tested next to it, in `parsers::tml`
    // (`foreign_shot_uuid_rejects_characters_xml_forbids`); this is the
    // end-to-end consequence.
    let dir = tempfile::tempdir().unwrap();
    let source = write_tmlu(
        dir.path(),
        "nonchar.tmlu",
        &cave_file_xml(&[
            Row::new(1, 0, Some(GOOD)),
            Row::new(2, 1, Some(NONCHAR_FFFF)),
            Row::new(3, 2, Some(NONCHAR_FFFE)),
        ]),
    );

    let state = state_with_project("Synthetic");
    import_ariane_file(&state, &source);

    let project = state.get_project_state().unwrap();
    let uuids = section_uuids(&project, 0);
    assert_eq!(uuids.len(), 3);
    assert_eq!(uuids[0], GOOD);
    for (i, u) in uuids.iter().enumerate().skip(1) {
        assert!(
            !u.contains('\u{FFFF}') && !u.contains('\u{FFFE}') && !u.is_empty(),
            "shot {i} adopted a non-Char identity: {u:?}"
        );
    }

    let exported = dir.path().join("nonchar-out.tmlu");
    export_tml(&project, &exported).unwrap();
    let raw = std::fs::read(&exported).unwrap();
    let text = String::from_utf8(raw).expect("the export must be valid UTF-8");
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
    assert!(
        forbidden.is_empty(),
        "the export must contain only characters XML 1.0 allows: {forbidden:x?}"
    );
}

// ---------------------------------------------------------------------------
// 15. A legacy undo entry must not restore a shot with no identity
// ---------------------------------------------------------------------------

/// An undo entry serialised before #423 carries neither a `ShotData::uuid` nor a
/// `ShotInfo`-side `uuid`. `Shot::with_uuid` resolves `data.uuid.or(explicit)`
/// and never mints, so replaying one used to restore a shot with `uuid: None`:
/// absent from `shot_uuid_index`, unnameable by any `StationKey::AfterShot`, and
/// left that way until some later save happened to mint one.
///
/// `restore_shot` goes through `Shot::uuid_or_mint`, so the restored shot always
/// has an identity and a cross-reference can be anchored on it immediately.
#[test]
fn a_legacy_undo_entry_with_no_recorded_identity_restores_a_usable_shot() {
    let state = state_with_project("Synthetic");
    let section_id = state
        .execute(Command::CreateSection {
            name: Some("Legacy".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 10.0,
                    depth_in: 5.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 11.0,
                    depth_in: 5.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
            ],
        })
        .section_id
        .unwrap();

    // The pre-#423 payload shape: no uuid on the ShotData, and none recorded
    // beside it. Pushed straight onto the stack, exactly as a persisted entry
    // written by an older build deserialises.
    {
        let mut stack = state.undo_stack.lock();
        stack.push_with_data(
            Command::DeleteShot {
                section_id,
                shot_id: 0,
            },
            crate::core::undo::UndoData::ShotSnapshot {
                section_id,
                shot_id: 9_999,
                shot_data: ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 45.0,
                    length: 7.0,
                    depth_in: 5.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
                position: 1,
                uuid: None,
            },
        );
    }

    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);

    let project = state.get_project_state().unwrap();
    let restored = project
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .find(|s| s.id == 9_999)
        .expect("the legacy entry restored its shot");
    let uuid = restored
        .uuid
        .clone()
        .expect("a restored shot must have an identity, or no anchor can name it");
    assert!(!uuid.is_empty());

    // The point of having one: an AfterShot anchor on it resolves.
    let key = project
        .key_for_ref(StationRef::new(section_id, 2))
        .expect("the station after the restored shot must be nameable");
    assert_eq!(
        key.anchor,
        crate::models::StationAnchor::AfterShot(uuid),
        "the anchor must be keyed on the restored shot's own identity"
    );
    assert_eq!(
        project.resolve_key(&key),
        Some(StationRef::new(section_id, 2)),
        "and it must resolve back to the same station"
    );
}

/// The redo twin of the test above, for the one insert arm that bypasses the
/// `section.rs` helpers: a legacy `CreatedShotSnapshot` (both anchors `None`,
/// no recorded uuid) redone as a TRUE APPEND takes the raw
/// `section.shots.insert` path in `core/state/undo.rs`, which used to hand
/// `Shot::with_uuid` a bare `None` - recreating the identity-less shot every
/// other insert path mints for.
#[test]
fn a_legacy_created_shot_redo_append_mints_an_identity() {
    let state = state_with_project("Synthetic");
    let section_id = state
        .execute(Command::CreateSection {
            name: Some("LegacyRedoAppend".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 10.0,
                    depth_in: 5.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 11.0,
                    depth_in: 5.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
            ],
        })
        .section_id
        .unwrap();

    // A pre-#423 persisted redo entry: no anchors, no uuid anywhere, and a
    // position at/past the tail so the replay is a true append.
    let entry = crate::core::undo::UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CreateShot {
            section_id,
            shot: ShotData {
                azimuth_in: 45.0,
                azimuth_out: 45.0,
                length: 7.0,
                depth_in: 5.0,
                depth_out: 5.0,
                ..Default::default()
            },
            after_shot_id: None,
            before_shot_id: None,
        },
        undo_data: crate::core::undo::UndoData::CreatedShotSnapshot {
            section_id,
            shot_id: 9_999,
            shot_data: ShotData {
                azimuth_in: 45.0,
                azimuth_out: 45.0,
                length: 7.0,
                depth_in: 5.0,
                depth_out: 5.0,
                ..Default::default()
            },
            position: 2,
            station_shift_from: 3,
            after_shot_id: None,
            before_shot_id: None,
            // A legacy persisted entry predates every uuid-anchor field and the
            // captured-offsets map, so all of them deserialize to their defaults.
            after_shot_uuid: None,
            before_shot_uuid: None,
            uuid: None,
            previous_offsets: std::collections::HashMap::new(),
            legacy_comment_rows: Vec::new(),
        },
        timestamp: None,
    };
    state.undo_stack.lock().push_redo(entry);

    let redone = state.redo();
    assert!(redone.success, "{:?}", redone.error);

    let project = state.get_project_state().unwrap();
    let restored = project
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .find(|s| s.id == 9_999)
        .expect("the legacy redo appended its shot");
    let uuid = restored
        .uuid
        .clone()
        .expect("an appended shot must have an identity, or no anchor can name it");
    assert!(!uuid.is_empty());

    // The point of having one: an AfterShot anchor on it resolves.
    let key = project
        .key_for_ref(StationRef::new(section_id, 3))
        .expect("the station after the appended shot must be nameable");
    assert_eq!(
        key.anchor,
        crate::models::StationAnchor::AfterShot(uuid),
        "the anchor must be keyed on the appended shot's own identity"
    );
    assert_eq!(
        project.resolve_key(&key),
        Some(StationRef::new(section_id, 3)),
        "and it must resolve back to the same station"
    );
}
