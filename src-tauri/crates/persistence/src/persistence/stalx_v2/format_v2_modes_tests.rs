//! The serializer modes (plan 6.5): push, detached copy, the local-only
//! rewrite and the Recents rename.

use chrono::Utc;

use super::test_helpers::{generate, history, text_of, Rng, SMALL};
use super::types::{LocalSync, SpeleoDbLink};
use super::{
    decode, encode, encode_with_report, probe_bytes, rename_project, rewrite_local, DecodeOptions,
    EncodeError, Extras, Mode, PushOptions, RawJson,
};
use crate::models::UndoMode;

fn linked(seed: u64) -> (crate::models::survey::Project, Extras) {
    let (p, mut extras) = generate(seed, SMALL);
    extras.speleodb = Some(SpeleoDbLink {
        endpoint: "https://speleodb.example".to_string(),
        project_id: "1234".to_string(),
        project_name: "Shared".to_string(),
        undo_mode: Some(UndoMode::Full),
    });
    extras.local.speleodb = LocalSync {
        base_commit: Some("abc".to_string()),
        unpushed_edits: true,
        ..LocalSync::default()
    };
    extras
        .local
        .display
        .insert("grid".to_string(), "true".to_string());
    extras.history = history(&mut Rng(9), 12, 3);
    (p, extras)
}

fn stamp(entry: &RawJson) -> chrono::DateTime<Utc> {
    let v: serde_json::Value = serde_json::from_str(entry.as_str()).unwrap();
    super::write::parse_time(v["t"].as_str().unwrap()).unwrap()
}

#[test]
fn a_push_has_no_local_state_and_no_redo() {
    let (p, extras) = linked(1);
    let options = PushOptions {
        keep_undo: true,
        watermark: None,
        byte_cap: usize::MAX,
    };
    let (bytes, report) = encode_with_report(&p, &extras, Mode::ForPush(options)).unwrap();
    let text = text_of(&bytes);
    assert!(!text.contains("\n\"local\":"));
    assert!(!text.contains("\"redo\""));
    assert!(text.contains("\n\"speleodb\":{"), "the link identity rides");
    assert_eq!(report.history.undo_kept, 12);
    assert_eq!(report.history.redo_dropped, 3);
    let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
    assert!(decoded.extras.local.is_empty());
    assert_eq!(decoded.extras.history.undo, extras.history.undo);

    // Disabled undo mode: no history at all.
    let (bytes, report) =
        encode_with_report(&p, &extras, Mode::ForPush(PushOptions::default())).unwrap();
    assert!(!text_of(&bytes).contains("\"history\""));
    assert!(report.history.undo_dropped);
}

#[test]
fn a_push_cuts_inherited_history_at_the_watermark() {
    let (p, extras) = linked(2);
    let cut_at = stamp(&extras.history.undo[4]);
    let options = PushOptions {
        keep_undo: true,
        watermark: Some(cut_at),
        byte_cap: usize::MAX,
    };
    let (bytes, report) = encode_with_report(&p, &extras, Mode::ForPush(options)).unwrap();
    assert_eq!(report.history.inherited_cut, 5);
    let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
    assert_eq!(
        decoded.extras.history.undo,
        extras.history.undo[5..].to_vec()
    );

    // Out-of-order times: refuse rather than ship inherited entries.
    let mut shuffled = extras.clone();
    shuffled.history.undo.swap(2, 8);
    assert!(matches!(
        encode(&p, &shuffled, Mode::ForPush(options)),
        Err(EncodeError::History { .. })
    ));
    // An Unwind step recorded without a watermark: no history (fail closed).
    let mut unwound = extras;
    unwound.local.speleodb.unwound_from = Some("def".to_string());
    let no_mark = PushOptions {
        watermark: None,
        ..options
    };
    let (_, report) = encode_with_report(&p, &unwound, Mode::ForPush(no_mark)).unwrap();
    assert!(report.history.undo_dropped);
    assert_eq!(report.history.undo_kept, 0);
}

#[test]
fn a_push_keeps_the_newest_entries_that_fit_the_cap() {
    let (p, extras) = linked(3);
    let options = PushOptions {
        keep_undo: true,
        watermark: None,
        byte_cap: usize::MAX,
    };
    let full = encode(&p, &extras, Mode::ForPush(options)).unwrap();
    let entry = extras.history.undo[0].as_str().len() + 2;
    for drop in [1, 5, 11] {
        let cap = full.len() - drop * entry;
        let (bytes, report) = encode_with_report(
            &p,
            &extras,
            Mode::ForPush(PushOptions {
                byte_cap: cap,
                ..options
            }),
        )
        .unwrap();
        assert!(bytes.len() <= cap, "{} > {cap}", bytes.len());
        assert!(report.history.size_cut >= drop);
        let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
        let kept = decoded.extras.history.undo;
        assert_eq!(
            kept,
            extras.history.undo[extras.history.undo.len() - kept.len()..].to_vec()
        );
    }
    // Too small even without history: refused, nothing written.
    assert!(matches!(
        encode(
            &p,
            &extras,
            Mode::ForPush(PushOptions {
                byte_cap: 100,
                ..options
            })
        ),
        Err(EncodeError::TooLarge { .. })
    ));
}

#[test]
fn a_detached_copy_sheds_the_link() {
    let (p, mut extras) = linked(4);
    extras.local.speleodb.inherited_through = Some(stamp(&extras.history.undo[2]));
    let (bytes, report) = encode_with_report(&p, &extras, Mode::DetachedCopy).unwrap();
    let text = text_of(&bytes);
    assert!(!text.contains("\n\"speleodb\":"));
    assert!(!text.contains("\"base\""));
    assert_eq!(report.history.inherited_cut, 3);
    let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
    assert!(decoded.extras.speleodb.is_none());
    assert!(decoded.extras.local.speleodb.is_empty());
    assert_eq!(decoded.extras.local.display, extras.local.display);
    assert_eq!(
        decoded.extras.history.undo,
        extras.history.undo[3..].to_vec()
    );
    assert_eq!(decoded.extras.history.redo, extras.history.redo);
    assert_eq!(probe_bytes(&bytes).summary.unwrap().speleodb, None);
}

/// The local-only rewrite gives exactly the bytes a full save with the new
/// extras would (same generator), without decoding the body.
#[test]
fn the_local_only_rewrite_equals_a_full_save() {
    for seed in 0..30 {
        let (p, before) = generate(seed, SMALL);
        let original = encode(&p, &before, Mode::Normal).unwrap();
        let (_, other) = generate(seed + 1000, SMALL);
        let mut after = before.clone();
        after.speleodb = other.speleodb.clone();
        after.preserved.speleodb.clear();
        after.local = other.local.clone();
        after.history = other.history.clone();
        let rewritten = rewrite_local(&original, &after).unwrap();
        let full = encode(&p, &after, Mode::Normal).unwrap();
        assert_eq!(text_of(&rewritten), text_of(&full), "seed {seed}");
    }
}

#[test]
fn the_local_only_rewrite_refuses_what_is_not_a_complete_v2_file() {
    let (p, extras) = generate(5, SMALL);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    assert!(rewrite_local(&bytes[..bytes.len() / 2], &extras).is_err());
    assert!(rewrite_local(b"SQLite format 3\0....", &extras).is_err());
    let newer = text_of(&bytes).replacen("\"version\":2", "\"version\":3", 1);
    assert!(rewrite_local(newer.as_bytes(), &extras).is_err());
    let no_end = text_of(&bytes).replacen(",\n\"end\":true}", "}", 1);
    assert!(rewrite_local(no_end.as_bytes(), &extras).is_err());
}

#[test]
fn renaming_a_closed_file_changes_two_lines() {
    let (p, extras) = generate(6, SMALL);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let renamed = rename_project(&bytes, "New \"name\"").unwrap();
    let decoded = decode(&renamed, &DecodeOptions::default()).unwrap();
    assert_eq!(decoded.project.name, "New \"name\"");
    assert_eq!(probe_bytes(&renamed).summary.unwrap().name, "New \"name\"");
    let mut expected = p;
    expected.name = "New \"name\"".to_string();
    assert_eq!(encode(&expected, &extras, Mode::Normal).unwrap(), renamed);
    let changed = text_of(&bytes)
        .lines()
        .zip(text_of(&renamed).lines())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed, 2);
    // Refused on an unknown required feature.
    let future = text_of(&bytes).replacen("\"requires\":[]", "\"requires\":[\"zz\"]", 1);
    assert!(matches!(
        rename_project(future.as_bytes(), "x"),
        Err(EncodeError::UnknownFeature { .. })
    ));
}

#[test]
fn an_invalid_model_is_never_written() {
    let (mut p, extras) = generate(7, SMALL);
    p.placements.clear();
    p.datums.clear();
    assert!(matches!(
        encode(&p, &extras, Mode::Normal),
        Err(EncodeError::Invalid { .. })
    ));
    let (mut p, extras) = generate(7, SMALL);
    if let Some(line) = p.lines().first().map(|l| l.id) {
        if let Some(shot) = p.line_mut(line).unwrap().shots.first_mut() {
            shot.length = f64::NAN;
            assert!(matches!(
                encode(&p, &extras, Mode::Normal),
                Err(EncodeError::Invalid { .. })
            ));
        }
    }
}
