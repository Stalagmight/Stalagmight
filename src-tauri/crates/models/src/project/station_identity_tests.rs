//! Station identity on the model: the normalization (`ensure_station_identities`),
//! the creation rules on real edits, Combine Lines' prefixing, the split copy's
//! new id, and the user-edit validation.

use super::*;
use crate::{
    CombineLinesOptions, CombinePick, LineEnd, SplitLineOptions, SplitSide, MAX_STATION_ID,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: Option<&str>, shots: usize) -> LineId {
    project
        .add_line(LineData {
            name: name.map(str::to_string),
            shots: (0..shots).map(|_| shot(90.0, 10.0)).collect(),
            ..Default::default()
        })
        .id
}

/// Names of every station of `id`, in station order (`?` for none).
fn names(project: &Project, id: LineId) -> Vec<String> {
    project
        .line_station_names(id)
        .into_iter()
        .map(|n| n.unwrap_or_else(|| "?".to_string()))
        .collect()
}

/// Station IDs of every station of `id`, in station order.
fn ids(project: &Project, id: LineId) -> Vec<Option<StationId>> {
    let count = project.get_line(id).unwrap().shots.len() as u32;
    (0..=count)
        .map(|i| project.station_id_at(StationRef::new(id, i)))
        .collect()
}

fn some(values: &[StationId]) -> Vec<Option<StationId>> {
    values.iter().copied().map(Some).collect()
}

fn shot_id(project: &Project, id: LineId, position: usize) -> ShotId {
    project.get_line(id).unwrap().shots[position].id
}

/// Two unnamed lines of 2 and 1 shots, normalized.
fn seeded() -> (Project, LineId, LineId) {
    let mut project = Project::new("Ids".to_string(), None);
    let a = add(&mut project, None, 2);
    let b = add(&mut project, None, 1);
    project.ensure_station_identities();
    (project, a, b)
}

#[test]
fn normalization_seeds_names_ids_and_line_names_in_order() {
    let (project, a, b) = seeded();
    assert_eq!(names(&project, a), ["0", "1", "2"]);
    assert_eq!(names(&project, b), ["0", "1"]);
    assert_eq!(ids(&project, a), some(&[0, 1, 2]));
    assert_eq!(ids(&project, b), some(&[3, 4]));
    assert_eq!(project.next_station_id, 5);
    assert_eq!(project.get_line(a).unwrap().name.as_deref(), Some("Line 1"));
    assert_eq!(project.get_line(b).unwrap().name.as_deref(), Some("Line 2"));
}

#[test]
fn a_second_pass_changes_nothing() {
    let (mut project, _, _) = seeded();
    let before = serde_json::to_string(&project).unwrap();
    assert!(project.ensure_station_identities().is_empty());
    assert_eq!(serde_json::to_string(&project).unwrap(), before);
}

/// A file an older build rewrote keeps the names but loses `station_ids`
/// and the counter: every open re-mints the same ids.
#[test]
fn ids_lost_by_an_older_build_re_mint_the_same_way() {
    let (project, _, _) = seeded();
    let mut stripped = project.clone();
    stripped.station_ids.clear();
    stripped.next_station_id = 0;
    stripped.ensure_station_identities();
    assert_eq!(stripped.station_ids, project.station_ids);
    assert_eq!(stripped.next_station_id, project.next_station_id);
}

#[test]
fn an_appended_station_gets_the_next_id_and_name_and_nothing_else_moves() {
    let (mut project, a, b) = seeded();
    project.add_shot(a, shot(0.0, 3.0)).unwrap();
    project.ensure_station_identities();
    assert_eq!(names(&project, a), ["0", "1", "2", "3"]);
    assert_eq!(ids(&project, a), some(&[0, 1, 2, 5]));
    assert_eq!(ids(&project, b), some(&[3, 4]));
    assert_eq!(project.next_station_id, 6);
}

#[test]
fn repeated_mid_inserts_letter_after_the_previous_station() {
    let (mut project, a, _) = seeded();
    // Insert after the shot ending at "1"... twice, each time right after
    // station "1": `1 - 1b - 1a - 2`.
    // The app normalizes after every command; so does this test.
    let first = shot_id(&project, a, 0);
    project
        .insert_shot_after(a, first, shot(0.0, 1.0), None, None)
        .unwrap();
    project.ensure_station_identities();
    project
        .insert_shot_after(a, first, shot(0.0, 1.0), None, None)
        .unwrap();
    project.ensure_station_identities();
    assert_eq!(names(&project, a), ["0", "1", "1b", "1a", "2"]);
    assert_eq!(ids(&project, a), some(&[0, 1, 6, 5, 2]));
}

#[test]
fn insert_station_on_a_shot_takes_the_from_station_plus_a_letter() {
    let (mut project, a, _) = seeded();
    let second = shot_id(&project, a, 1);
    project.split_shot_at_distance(a, second, 4.0).unwrap();
    assert_eq!(names(&project, a), ["0", "1", "1a", "2"]);
}

#[test]
fn a_prepended_station_takes_the_next_number_of_the_first_family() {
    let mut project = Project::new("Prepend".to_string(), None);
    let a = add(&mut project, None, 6);
    project.ensure_station_identities();
    let first = shot_id(&project, a, 0);
    project
        .insert_shot_before(a, first, shot(0.0, 2.0), None, None)
        .unwrap();
    assert_eq!(names(&project, a), ["7", "0", "1", "2", "3", "4", "5", "6"]);

    // A lettered line keeps its letter.
    let b = add(&mut project, None, 3);
    project.ensure_station_identities();
    for i in 0..=3 {
        let key = project.key_for_ref(StationRef::new(b, i)).unwrap();
        project.station_names.insert(key, format!("A{i}"));
    }
    let first = shot_id(&project, b, 0);
    project
        .insert_shot_before(b, first, shot(0.0, 2.0), None, None)
        .unwrap();
    assert_eq!(names(&project, b), ["A4", "A0", "A1", "A2", "A3"]);
}

#[test]
fn a_repeated_name_and_a_repeated_id_heal_on_the_later_station() {
    let (mut project, a, _) = seeded();
    let k2 = project.key_for_ref(StationRef::new(a, 2)).unwrap();
    project.station_names.insert(k2.clone(), "1".to_string());
    project.station_ids.insert(k2, 0);
    let heal = project.ensure_station_identities();
    assert_eq!(names(&project, a), ["0", "1", "1a"]);
    assert_eq!(ids(&project, a), some(&[0, 1, 5]));
    assert_eq!(heal.names_deduped, 1);
    assert_eq!(heal.ids_allocated, 1);
}

#[test]
fn an_id_whose_station_is_gone_is_dropped() {
    let (mut project, a, _) = seeded();
    project.station_ids.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        99,
    );
    project.ensure_station_identities();
    assert_eq!(project.station_ids.len(), 5);
    assert!(!project.station_ids.values().any(|id| *id == 99));
    // The counter never goes back below an id it handed out... but an id
    // never handed out by this project (a crafted 99) does not raise it.
    assert_eq!(project.next_station_id, 5);
}

#[test]
fn deleting_and_reversing_keep_every_surviving_station_on_its_id() {
    let (mut project, a, _) = seeded();
    let middle = shot_id(&project, a, 1);
    project.remove_shot_with_rewire(a, middle).unwrap();
    project.ensure_station_identities();
    // Shot 1 (station 1 -> 2) removed station 2.
    assert_eq!(names(&project, a), ["0", "1"]);
    assert_eq!(ids(&project, a), some(&[0, 1]));

    project.add_shot(a, shot(0.0, 3.0)).unwrap();
    project.ensure_station_identities();
    assert!(project.reverse_line(a, false));
    assert_eq!(names(&project, a), ["2", "1", "0"]);
    assert_eq!(ids(&project, a), some(&[5, 1, 0]));
}

#[test]
fn line_names_are_seeded_and_deduplicated() {
    let mut project = Project::new("Lines".to_string(), None);
    let a = add(&mut project, Some("Main"), 1);
    let b = add(&mut project, Some("Main"), 1);
    let c = add(&mut project, None, 1);
    let d = add(&mut project, Some("Line 1"), 1);
    project.ensure_station_identities();
    let name = |id| project.get_line(id).unwrap().name.clone().unwrap();
    assert_eq!(name(a), "Main");
    assert_eq!(name(b), "Main (2)");
    assert_eq!(name(c), "Line 2");
    assert_eq!(name(d), "Line 1");
}

/// A crafted file of thousands of lines sharing one name, and thousands with
/// none: each repeat and each seed resumes the search where the last stopped
/// (the search from ` (2)` / `Line 1` per line was quadratic).
#[test]
fn many_repeated_and_unnamed_lines_are_named_in_one_pass() {
    const N: usize = 10_000;
    let mut project = Project::new("Lines".to_string(), None);
    let repeated: Vec<LineId> = (0..N).map(|_| add(&mut project, Some("Main"), 0)).collect();
    let unnamed: Vec<LineId> = (0..N).map(|_| add(&mut project, None, 0)).collect();
    let held = add(&mut project, Some("Main (3)"), 0);
    let started = std::time::Instant::now();
    assert_eq!(project.ensure_line_names(), 2 * N - 1);
    let elapsed = started.elapsed();
    let name = |id| project.get_line(id).unwrap().name.clone().unwrap();
    assert_eq!(name(repeated[0]), "Main");
    assert_eq!(name(repeated[1]), "Main (2)");
    assert_eq!(
        name(repeated[2]),
        "Main (4)",
        "Main (3) is held further down"
    );
    assert_eq!(name(repeated[N - 1]), format!("Main ({})", N + 1));
    assert_eq!(name(held), "Main (3)");
    assert_eq!(name(unnamed[0]), "Line 1");
    assert_eq!(name(unnamed[N - 1]), format!("Line {N}"));
    let all: HashSet<String> = project
        .lines
        .iter()
        .filter_map(|l| l.name.clone())
        .collect();
    assert_eq!(all.len(), project.lines.len(), "every name unique");
    assert_eq!(project.ensure_line_names(), 0, "a fixpoint");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "naming took {elapsed:?}"
    );
}

#[test]
fn split_at_a_station_gives_the_copy_a_new_id_and_the_same_name() {
    let mut project = Project::new("Split".to_string(), None);
    let a = add(&mut project, Some("Main"), 3);
    project.ensure_station_identities();
    let options = SplitLineOptions {
        name_a: "Main".to_string(),
        name_b: "Main B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    };
    let b = project.split_line_at_station(a, 2, &options).unwrap();
    project.ensure_station_identities();
    assert_eq!(names(&project, a), ["0", "1", "2"]);
    assert_eq!(names(&project, b), ["2", "3"]);
    assert_eq!(ids(&project, a), some(&[0, 1, 2]));
    assert_eq!(
        ids(&project, b),
        some(&[4, 3]),
        "the copy of station 2 is new"
    );
}

/// Two lines meeting end to start: `K` (stations 0..=3) and `O` (0..=2),
/// merged at K/3 = O/0.
fn combinable(kept_names: &[&str], other_names: &[&str]) -> (Project, LineId, LineId) {
    let mut project = Project::new("Combine".to_string(), None);
    let k = add(&mut project, Some("K"), kept_names.len() - 1);
    let o = add(&mut project, Some("O"), other_names.len() - 1);
    project.ensure_station_identities();
    for (line, list) in [(k, kept_names), (o, other_names)] {
        for (i, name) in list.iter().enumerate() {
            let key = project
                .key_for_ref(StationRef::new(line, i as u32))
                .unwrap();
            project.station_names.insert(key, (*name).to_string());
        }
    }
    let end = kept_names.len() as u32 - 1;
    let now = project
        .compute_station_raw_coords(StationRef::new(k, end))
        .unwrap();
    let start = project
        .compute_station_raw_coords(StationRef::new(o, 0))
        .unwrap();
    project.adjust_line_offset(o, now.x - start.x, now.y - start.y);
    project
        .merge_stations(StationRef::new(k, end), StationRef::new(o, 0))
        .unwrap();
    (project, k, o)
}

fn combine_options() -> CombineLinesOptions {
    CombineLinesOptions {
        name: "K".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Both,
        join: Some(crate::CombineJoinEnds {
            kept: LineEnd::End,
            other: LineEnd::Start,
        }),
    }
}

#[test]
fn combining_colliding_lines_letters_both_and_keeps_the_kept_join_name() {
    let (mut project, k, o) = combinable(&["0", "1", "2", "3"], &["0", "1", "2"]);
    let join_id = project.station_id_at(StationRef::new(k, 3)).unwrap();
    project.combine_lines(k, o, &combine_options()).unwrap();
    project.ensure_station_identities();
    assert_eq!(names(&project, k), ["A0", "A1", "A2", "A3", "B1", "B2"]);
    assert_eq!(
        project.station_id_at(StationRef::new(k, 3)),
        Some(join_id),
        "the joined station keeps the kept station's id"
    );

    // A later combine with the lettered line takes the next free letter.
    let mut project2 = project.clone();
    let p = add(&mut project2, Some("P"), 1);
    project2.ensure_station_identities();
    let end = project2.get_line(k).unwrap().shots.len() as u32;
    let at = project2
        .compute_station_raw_coords(StationRef::new(k, end))
        .unwrap();
    let start = project2
        .compute_station_raw_coords(StationRef::new(p, 0))
        .unwrap();
    project2.adjust_line_offset(p, at.x - start.x, at.y - start.y);
    project2
        .merge_stations(StationRef::new(k, end), StationRef::new(p, 0))
        .unwrap();
    project2.combine_lines(k, p, &combine_options()).unwrap();
    project2.ensure_station_identities();
    assert_eq!(
        names(&project2, k),
        ["A0", "A1", "A2", "A3", "B1", "B2", "C1"]
    );
}

#[test]
fn combining_lines_that_do_not_collide_keeps_every_name() {
    let (mut project, k, o) = combinable(&["0", "1", "2", "3"], &["3", "4", "5"]);
    project.combine_lines(k, o, &combine_options()).unwrap();
    project.ensure_station_identities();
    assert_eq!(names(&project, k), ["0", "1", "2", "3", "4", "5"]);
}

#[test]
fn a_user_station_name_is_trimmed_and_must_be_unique_in_its_line() {
    let (mut project, a, b) = seeded();
    let station = StationRef::new(a, 1);
    assert_eq!(
        project
            .set_station_name(station, "  sump  ")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    assert_eq!(project.station_name_at(station), Some("sump"));
    assert_eq!(
        project.set_station_name(station, "   ").unwrap_err(),
        "A station name cannot be empty"
    );
    assert_eq!(
        project
            .set_station_name(StationRef::new(a, 2), "sump")
            .unwrap_err(),
        "Line \"Line 1\" already has a station named \"sump\""
    );
    // Another line may hold the same name.
    project
        .set_station_name(StationRef::new(b, 0), "sump")
        .unwrap();
    // Renaming a station to its own name is fine.
    project.set_station_name(station, "sump").unwrap();
}

#[test]
fn a_user_line_name_must_be_unique_and_non_empty() {
    let (project, a, b) = seeded();
    assert_eq!(
        project.validate_line_name(" Line 2 ", Some(a)).unwrap_err(),
        "A line named \"Line 2\" already exists"
    );
    assert_eq!(
        project.validate_line_name("", Some(a)).unwrap_err(),
        "A line name cannot be empty"
    );
    assert_eq!(
        project.validate_line_name("Line 2", Some(b)).unwrap(),
        "Line 2"
    );
    assert_eq!(project.validate_line_name(" New ", None).unwrap(), "New");
}

#[test]
fn labels_name_the_line_and_the_stations() {
    let (project, a, _) = seeded();
    assert_eq!(
        project.station_label(StationRef::new(a, 2)).as_deref(),
        Some("Line 1 / 2")
    );
    let shot = shot_id(&project, a, 1);
    assert_eq!(
        project.shot_label(a, shot).as_deref(),
        Some("Line 1: 1 \u{2192} 2")
    );
    assert_eq!(project.station_label(StationRef::new(a, 9)), None);
}

#[test]
fn snapshots_carry_ids_and_the_counter() {
    let (project, _, _) = seeded();
    let json = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back.station_ids, project.station_ids);
    assert_eq!(back.next_station_id, project.next_station_id);
    // An older snapshot without the fields reads as empty.
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("station_ids");
    object.remove("next_station_id");
    let old: Project = serde_json::from_value(value).unwrap();
    assert!(old.station_ids.is_empty());
    assert_eq!(old.next_station_id, 0);
}

/// The epilogue runs after every structural command (metadata-only commands
/// skip it), so its no-change pass must stay cheap on the largest surveys
/// (~89k stations): one shot-uuid index and a scan, no allocation per
/// station. Prints the timing; asserts only a generous bound.
#[test]
#[ignore = "timing probe; run with --ignored --nocapture"]
#[expect(clippy::print_stderr, reason = "the probe prints what it measures")]
fn the_no_change_pass_is_cheap_on_a_large_project() {
    let mut project = Project::new("Large".to_string(), None);
    for _ in 0..14_000 {
        add(&mut project, None, 5);
    }
    project.ensure_station_identities();
    let start = std::time::Instant::now();
    for _ in 0..10 {
        assert!(project.ensure_station_identities().is_empty());
    }
    let per_pass = start.elapsed() / 10;
    eprintln!("no-change pass over 84k stations: {per_pass:?}");
    assert!(per_pass < std::time::Duration::from_millis(500));
}

/// A crafted or corrupt file can hold ids and a counter far past anything a
/// real project allocates. Ids at or above `MAX_STATION_ID` are invalid and
/// re-minted, a counter past it starts again above the valid ids, and the
/// result is settled: no duplicate ids, no re-settle on every later pass.
#[test]
fn huge_ids_and_counters_are_re_minted_and_stay_settled() {
    let (mut project, a, b) = seeded();
    let key = |project: &Project, line: LineId, i: u32| {
        project.key_for_ref(StationRef::new(line, i)).unwrap()
    };
    let (k1, k2) = (key(&project, a, 1), key(&project, a, 2));
    project.station_ids.insert(k1, u32::MAX);
    project.station_ids.insert(k2, MAX_STATION_ID);
    project.next_station_id = u32::MAX;
    let heal = project.ensure_station_identities();
    assert_eq!(heal.ids_allocated, 2, "{heal:?}");
    let all: Vec<StationId> = [ids(&project, a), ids(&project, b)]
        .concat()
        .into_iter()
        .map(Option::unwrap)
        .collect();
    assert!(all.iter().all(|id| *id < MAX_STATION_ID), "{all:?}");
    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(unique.len(), all.len(), "{all:?}");
    assert_eq!(ids(&project, a)[0], Some(0), "valid ids keep their value");
    assert!(project.next_station_id < MAX_STATION_ID);
    assert!(project.ensure_station_identities().is_empty(), "settled");
}

/// When minting would run past the bound, every station is renumbered
/// compactly instead of handing out an id twice.
#[test]
fn an_allocation_that_would_overflow_renumbers_compactly() {
    let (mut project, a, b) = seeded();
    project.next_station_id = MAX_STATION_ID - 1;
    let key = project.key_for_ref(StationRef::new(a, 0)).unwrap();
    project.station_ids.insert(key, MAX_STATION_ID - 2);
    let c = add(&mut project, None, 2);
    project.ensure_station_identities();
    let all: Vec<_> = [ids(&project, a), ids(&project, b), ids(&project, c)].concat();
    assert_eq!(all, some(&[0, 1, 2, 3, 4, 5, 6, 7]));
    assert_eq!(project.next_station_id, 8);
    assert!(project.ensure_station_identities().is_empty(), "settled");

    // An Add that would push the incoming ids past the bound lands there too.
    let mut incoming = Project::new("Incoming".to_string(), None);
    add(&mut incoming, None, 1);
    incoming.ensure_station_identities();
    incoming.offset_station_ids(u32::MAX - 1);
    assert!(incoming
        .station_ids
        .values()
        .all(|id| *id == MAX_STATION_ID));
    incoming.ensure_station_identities();
    assert!(incoming.station_ids.values().all(|id| *id < MAX_STATION_ID));
    assert!(incoming.ensure_station_identities().is_empty());
}
