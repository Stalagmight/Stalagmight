//! The survey digest (`local.speleodb.base_survey`): what it covers, what it
//! leaves out, and that it is bound to the BASE revision it was taken for.

use super::test_helpers::{generate, history, text_of, Rng, SMALL};
use super::types::{History, LocalSync, SpeleoDbLink};
use super::{
    decode, encode, probe_bytes, rewrite_local, survey_digest, DecodeOptions, EncodeError, Extras,
    Mode, PushOptions, RawJson,
};
use crate::models::survey::Project;
use crate::models::UndoMode;

const BASE: &str = "3a7bd3e2360a3d29eea436fcfb7e44c735d117c4";

/// A linked survey with a dirty sync state, display settings and history.
fn linked(seed: u64) -> (Project, Extras) {
    let (p, mut extras) = generate(seed, SMALL);
    extras.speleodb = Some(SpeleoDbLink {
        endpoint: "https://speleodb.example".to_string(),
        project_id: "1234".to_string(),
        project_name: "Shared".to_string(),
        undo_mode: Some(UndoMode::Full),
    });
    extras.local.speleodb = LocalSync {
        base_commit: Some(BASE.to_string()),
        unpushed_edits: true,
        ..LocalSync::default()
    };
    extras
        .local
        .display
        .insert("grid".to_string(), "true".to_string());
    extras.history = history(&mut Rng(9), 4, 2);
    (p, extras)
}

fn digest_of(p: &Project, extras: &Extras) -> String {
    survey_digest(&encode(p, extras, Mode::Normal).unwrap(), BASE).unwrap()
}

#[test]
fn what_is_not_survey_content_leaves_the_digest_as_it_is() {
    for seed in 0..8 {
        let (p, extras) = linked(seed);
        let digest = digest_of(&p, &extras);
        assert!(
            digest.starts_with("sha256:") && digest.len() == 7 + 64,
            "{digest}"
        );

        // Per-machine state, the history, which build wrote it, the edit time
        // and the link's endpoint and name are not the survey.
        let mut other = extras.clone();
        other.local.speleodb.unpushed_edits = false;
        other.local.speleodb.base_survey = Some(digest.clone());
        other.local.display.clear();
        other.history = History::default();
        other.generator = "Stalagmight 9.9.9".to_string();
        if let Some(link) = other.speleodb.as_mut() {
            link.endpoint = "https://elsewhere.example".to_string();
            link.project_name = "Renamed on the server".to_string();
        }
        let mut touched = p.clone();
        touched.modified_at += chrono::Duration::seconds(90);
        assert_eq!(digest_of(&touched, &other), digest, "seed {seed}");

        // A local-only rewrite copies the survey verbatim.
        let bytes = encode(&p, &extras, Mode::Normal).unwrap();
        let rewritten = rewrite_local(&bytes, &other).unwrap();
        assert_eq!(survey_digest(&rewritten, BASE).unwrap(), digest);

        // And a decode and encode again writes the same survey.
        let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
        assert_eq!(digest_of(&decoded.project, &decoded.extras), digest);
    }
}

#[test]
fn an_upload_and_the_saved_file_of_one_survey_have_one_digest() {
    // Without history, the push snapshot (no `local`, no history) carries the
    // same survey members as the file the push saved first: a digest taken of
    // what went up matches the copy that stays.
    for seed in 0..8 {
        let (p, mut extras) = linked(seed);
        extras.history = History::default();
        let pushed = encode(
            &p,
            &extras,
            Mode::ForPush(PushOptions {
                keep_undo: true,
                watermark: None,
                byte_cap: usize::MAX,
            }),
        )
        .unwrap();
        assert!(!text_of(&pushed).contains("\n\"local\":"));
        assert_eq!(
            survey_digest(&pushed, BASE).unwrap(),
            digest_of(&p, &extras),
            "seed {seed}"
        );
    }
}

#[test]
fn every_survey_change_changes_the_digest() {
    let (p, extras) = linked(3);
    let digest = digest_of(&p, &extras);
    let changed = |what: &str, p: &Project, extras: &Extras| {
        assert_ne!(digest_of(p, extras), digest, "{what}");
    };

    let mut shot = p.clone();
    let line = shot
        .lines_mut()
        .iter_mut()
        .find(|l| !l.shots.is_empty())
        .unwrap();
    line.shots[0].length += 0.5;
    changed("a shot length", &shot, &extras);

    let mut named = p.clone();
    named.name.push_str(" (renamed)");
    changed("the project name", &named, &extras);

    let mut created = p.clone();
    created.created_at -= chrono::Duration::days(1);
    changed("the creation time", &created, &extras);

    let mut settings = p.clone();
    settings.settings.validation.warn_shot_length_max += 1.0;
    changed("a validation threshold", &settings, &extras);

    let mut counters = p.clone();
    counters.counters.next_section_id += 1;
    changed("a counter", &counters, &extras);

    let mut mode = extras.clone();
    if let Some(link) = mode.speleodb.as_mut() {
        link.undo_mode = Some(UndoMode::Disabled);
    }
    changed("the project's undo mode, which rides uploads", &p, &mode);

    let mut unknown = extras;
    unknown
        .preserved
        .top
        .push(("x_newer".to_string(), RawJson::parse("[1,2]").unwrap()));
    changed("a newer build's survey member", &p, &unknown);
}

#[test]
fn the_digest_is_bound_to_its_base_revision() {
    let (p, extras) = linked(5);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let here = survey_digest(&bytes, BASE).unwrap();
    assert_ne!(
        survey_digest(&bytes, "0123456789abcdef0123456789abcdef01234567").unwrap(),
        here,
        "a digest kept over another BASE never matches"
    );
    assert_eq!(survey_digest(&bytes, BASE).unwrap(), here, "deterministic");
}

#[test]
fn unknown_members_count_by_key_not_by_position() {
    let (p, extras) = linked(6);
    let member = |key: &str, value: &str| (key.to_string(), RawJson::parse(value).unwrap());
    let mut ab = extras.clone();
    ab.preserved.top = vec![member("x_a", "1"), member("x_b", "{\"k\":2}")];
    let mut ba = extras;
    ba.preserved.top = vec![member("x_b", "{\"k\":2}"), member("x_a", "1")];
    let first = encode(&p, &ab, Mode::Normal).unwrap();
    let second = encode(&p, &ba, Mode::Normal).unwrap();
    assert_ne!(first, second, "the files differ in member order");
    assert_eq!(
        survey_digest(&first, BASE).unwrap(),
        survey_digest(&second, BASE).unwrap()
    );
}

#[test]
fn only_a_complete_version_2_file_has_a_digest() {
    let (p, extras) = linked(7);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let truncated = &bytes[..bytes.len() / 2];
    assert!(matches!(
        survey_digest(truncated, BASE),
        Err(EncodeError::Original(_))
    ));
    assert!(survey_digest(b"SQLite format 3\0rest of a legacy file", BASE).is_err());
    assert!(survey_digest(b"not a project", BASE).is_err());
}

#[test]
fn the_base_digest_round_trips_in_the_sync_state_and_the_header() {
    let (p, mut extras) = linked(8);
    extras.local.speleodb.base_survey = Some(digest_of(&p, &extras));
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let text = text_of(&bytes);
    assert!(
        text.contains(&format!(
            "\"speleodb\":{{\"base\":\"{BASE}\",\"base_survey\":\"sha256:"
        )),
        "written right after the base it belongs to"
    );
    let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
    assert_eq!(decoded.extras.local.speleodb, extras.local.speleodb);
    let header = probe_bytes(&bytes);
    assert_eq!(
        header.local.map(|l| l.speleodb.base_survey),
        Some(extras.local.speleodb.base_survey.clone())
    );
    // Never in an upload or a detached copy: it is per-machine sync state.
    for mode in [Mode::ForPush(PushOptions::default()), Mode::DetachedCopy] {
        let out = encode(&p, &extras, mode).unwrap();
        assert!(!text_of(&out).contains("base_survey"), "{mode:?}");
    }
}
