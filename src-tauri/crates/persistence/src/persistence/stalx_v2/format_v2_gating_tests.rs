//! Version, `requires` and `conventions` gating, and the three tiers of
//! unknown content (plan 6.4).

use crate::models::ShotKind;

use super::test_helpers::{generate, text_of, SMALL};
use super::{
    decode, encode, rewrite_local, DecodeError, DecodeNote, DecodeOptions, Decoded, EncodeError,
    Mode,
};

fn small_text(seed: u64) -> String {
    let (p, extras) = generate(seed, SMALL);
    text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string()
}

fn open(text: &str) -> Result<Decoded, DecodeError> {
    decode(text.as_bytes(), &DecodeOptions::default())
}

/// A file with a shot extras object, a line with a credit, a station with
/// extras, datums, closures, Sections and ignores (seed found by search).
fn rich_text() -> String {
    for seed in 0..200 {
        let text = small_text(seed);
        if [
            "\"sections\":[",
            "\"datums\":[",
            "\"closures\":[",
            "\"ignored_warnings\":[",
            "{\"comment\":",
            "\"credit\":{",
            "\"azimuth\":{",
        ]
        .iter()
        .all(|needle| text.contains(needle))
        {
            return text;
        }
    }
    panic!("no generated file has every record kind");
}

#[test]
fn a_newer_major_version_is_refused_before_the_body_is_read() {
    let text = small_text(1).replacen("\"version\":2", "\"version\":3", 1);
    assert_eq!(
        open(&text).unwrap_err(),
        DecodeError::NewerVersion {
            found: 3,
            supported: 2
        }
    );
    // A newer body need not even be JSON this build can parse.
    let garbage = "{\"format\":\"stalx\",\"version\":17, this is not json";
    assert!(matches!(
        open(garbage).unwrap_err(),
        DecodeError::NewerVersion { found: 17, .. }
    ));
    for old in ["1", "0", "2.0", "-2"] {
        let text = small_text(1).replacen("\"version\":2", &format!("\"version\":{old}"), 1);
        assert!(
            matches!(
                open(&text).unwrap_err(),
                DecodeError::UnsupportedVersion { .. }
            ),
            "{old}"
        );
    }
}

#[test]
fn an_unknown_required_feature_opens_read_only() {
    let text = small_text(2).replacen("\"requires\":[]", "\"requires\":[\"zz-future\"]", 1);
    let decoded = open(&text).unwrap();
    let reason = decoded.read_only().unwrap();
    assert_eq!(reason.unknown_features, vec!["zz-future".to_string()]);
    // Saving the body is refused in every mode...
    for mode in [
        Mode::Normal,
        Mode::DetachedCopy,
        Mode::ForPush(super::PushOptions::default()),
    ] {
        assert!(matches!(
            encode(&decoded.project, &decoded.extras, mode),
            Err(EncodeError::ReadOnly(_))
        ));
    }
    // ...and without the read-only mark, the unknown feature still refuses.
    let mut cleared = decoded.extras.clone();
    cleared.read_only = None;
    assert!(matches!(
        encode(&decoded.project, &cleared, Mode::Normal),
        Err(EncodeError::UnknownFeature { .. })
    ));
    // A local-only rewrite works and keeps `requires`.
    let mut extras = decoded.extras.clone();
    extras
        .local
        .display
        .insert("grid".to_string(), "false".to_string());
    let rewritten = rewrite_local(text.as_bytes(), &extras).unwrap();
    let again = open(text_of(&rewritten)).unwrap();
    assert_eq!(again.extras.requires, vec!["zz-future".to_string()]);
    assert_eq!(again.extras.local.display["grid"], "false");
}

#[test]
fn conventions_are_fixed() {
    let base = small_text(3);
    for (from, to) in [
        ("\"length\":\"m\"", "\"length\":\"ft\""),
        ("\"inclination\":\"+down\"", "\"inclination\":\"+up\""),
        ("\"angles\":\"deg\",", ""),
        (
            "\"temperature\":\"degC\"",
            "\"temperature\":\"degC\",\"pressure\":\"bar\"",
        ),
    ] {
        let text = base.replacen(from, to, 1);
        assert!(
            matches!(open(&text).unwrap_err(), DecodeError::Conventions { .. }),
            "{to}"
        );
    }
    let missing = base.replacen(
        &base[base.find("\"conventions\"").unwrap()..base.find("\"generator\"").unwrap()],
        "",
        1,
    );
    assert!(matches!(
        open(&missing).unwrap_err(),
        DecodeError::Missing { ref field, .. } if field == "conventions"
    ));
}

#[test]
fn required_members_are_required() {
    let base = small_text(4);
    let without = |member: &str| {
        let start = base.find(&format!("\n\"{member}\":")).unwrap() + 1;
        let end = start + base[start..].find(",\n").unwrap() + 2;
        format!("{}{}", &base[..start], &base[end..])
    };
    assert!(matches!(
        open(&without("project")).unwrap_err(),
        DecodeError::Missing { ref field, .. } if field == "project"
    ));
    let no_end = base.replacen(",\n\"end\":true}", "}", 1);
    assert_eq!(open(&no_end).unwrap_err(), DecodeError::MissingEnd);
    let false_end = base.replacen("\"end\":true", "\"end\":false", 1);
    assert_eq!(open(&false_end).unwrap_err(), DecodeError::MissingEnd);
    let no_solver = base.replacen(",\"solver\":1", "", 1);
    assert!(matches!(
        open(&no_solver).unwrap_err(),
        DecodeError::Missing { ref field, .. } if field == "solver"
    ));
}

#[test]
fn another_solver_is_noted_not_refused() {
    let text = small_text(5).replacen("\"solver\":1", "\"solver\":2", 1);
    let decoded = open(&text).unwrap();
    assert!(decoded
        .notes
        .contains(&DecodeNote::OtherSolver { found: 2 }));
    assert!(decoded.read_only().is_none());
}

/// Tier 1: an unknown key or value in a structural record opens read-only;
/// never a refusal.
#[test]
fn unknown_structural_content_opens_read_only() {
    let base = rich_text();
    let cases: Vec<(&str, String)> = vec![
        (
            "line header",
            base.replacen(",\"start\":", ",\"zz\":[1,{\"a\":2}],\"start\":", 1),
        ),
        (
            "shot extras",
            base.replacen("{\"kind\":", "{\"zz\":true,\"kind\":", 1),
        ),
        (
            "station extras",
            base.replacen("{\"comment\":", "{\"zz\":1,\"comment\":", 1),
        ),
        (
            "datum",
            base.replacen("{\"station\":", "{\"zz\":null,\"station\":", 1),
        ),
        (
            "closure",
            base.replacen("\"strategy\":", "\"zz\":0,\"strategy\":", 1),
        ),
        (
            "section",
            replace_in(&base, "\"sections\":[\n{", "\"sections\":[\n{\"zz\":1,"),
        ),
        (
            "counters",
            base.replacen("\"counters\":{", "\"counters\":{\"zz\":4,", 1),
        ),
        (
            "credit",
            base.replacen("\"credit\":{\"", "\"credit\":{\"zz\":1,\"", 1),
        ),
        (
            "azimuth",
            base.replacen("\"azimuth\":{", "\"azimuth\":{\"zz\":1,", 1),
        ),
        (
            "ignore",
            replace_in(
                &base,
                "\"ignored_warnings\":[\n{",
                "\"ignored_warnings\":[\n{\"zz\":1,",
            ),
        ),
        (
            "unknown kind",
            base.replacen("\"kind\":\"jump\"", "\"kind\":\"teleport\"", 1),
        ),
        ("unknown strategy", replace_strategy(&base)),
        (
            "unknown direction",
            base.replacen("\"direction\":\"in\"", "\"direction\":\"sideways\"", 1),
        ),
        (
            "unknown north",
            base.replacen("\"north\":\"", "\"north\":\"z", 1),
        ),
    ];
    for (what, text) in cases {
        assert_ne!(text, base, "{what}: the test edit did not apply");
        let decoded = match open(&text) {
            Ok(d) => d,
            Err(e) => panic!("{what}: refused instead of read-only: {e}"),
        };
        let reason = decoded
            .read_only()
            .unwrap_or_else(|| panic!("{what}: not read-only"));
        assert!(reason.unknown_content_total >= 1, "{what}");
        assert!(!reason.unknown_content.is_empty(), "{what}");
        assert!(matches!(
            encode(&decoded.project, &decoded.extras, Mode::Normal),
            Err(EncodeError::ReadOnly(_))
        ));
    }
}

fn replace_in(text: &str, from: &str, to: &str) -> String {
    text.replacen(from, to, 1)
}

fn replace_strategy(text: &str) -> String {
    for token in [
        "batch",
        "move",
        "tie",
        "least_squares_weighted",
        "proportional_by_length",
        "equal_distribution",
    ] {
        let from = format!("\"strategy\":\"{token}\"");
        if text.contains(&from) {
            return text.replacen(&from, "\"strategy\":\"magnetic_snap\"", 1);
        }
    }
    text.to_string()
}

#[test]
fn an_unknown_shot_kind_reads_fail_closed_as_reference() {
    let base = rich_text();
    let text = base.replacen("\"kind\":\"jump\"", "\"kind\":\"teleport\"", 1);
    let decoded = open(&text).unwrap();
    let original = open(&base).unwrap();
    let changed: Vec<(ShotKind, ShotKind)> = original
        .project
        .lines()
        .iter()
        .flat_map(|l| &l.shots)
        .zip(decoded.project.lines().iter().flat_map(|l| &l.shots))
        .filter(|(a, b)| a.kind != b.kind)
        .map(|(a, b)| (a.kind, b.kind))
        .collect();
    assert_eq!(changed, vec![(ShotKind::Jump, ShotKind::Reference)]);
}

/// Tier 2: unknown keys of `project`, `settings`, `settings.validation`,
/// `speleodb` and `summary`, and unknown top-level members, are kept
/// verbatim (canonical whitespace) and written back after the known keys.
#[test]
fn unknown_header_content_is_preserved_on_resave() {
    let base = (0..50)
        .map(small_text)
        .find(|t| t.contains("\"speleodb\":{\"endpoint\""))
        .unwrap();
    let text = base
        .replacen(
            "\"project\":{",
            "\"project\":{\"zz_p\":{\"b\": [1, 2.50]},",
            1,
        )
        .replacen(
            "\"settings\":{\"validation\":{",
            "\"settings\":{\"zz_s\":\"x\",\"validation\":{\"zz_v\":7,",
            1,
        )
        .replacen(
            "\"speleodb\":{\"endpoint\"",
            "\"speleodb\":{\"zz_l\":[],\"endpoint\"",
            1,
        )
        .replacen("\"summary\":{", "\"summary\":{\"zz_m\":\"\\u0041\",", 1)
        .replacen(
            "\n\"stations\":[",
            "\n\"zz_top\" : { \"k\" : -0 },\n\"stations\":[",
            1,
        )
        .replacen("\n\"end\":true", "\n\"zz_tail\":1E3,\n\"end\":true", 1);
    let decoded = open(&text).unwrap();
    assert!(decoded.read_only().is_none(), "tier 2 is not read-only");
    let preserved = &decoded.extras.preserved;
    assert_eq!(preserved.project[0].0, "zz_p");
    assert_eq!(preserved.project[0].1.as_str(), "{\"b\":[1,2.50]}");
    assert_eq!(preserved.settings[0].0, "zz_s");
    assert_eq!(preserved.validation[0].1.as_str(), "7");
    assert_eq!(preserved.speleodb[0].0, "zz_l");
    assert_eq!(preserved.summary[0].1.as_str(), "\"A\"");
    let top: Vec<(&str, &str)> = preserved
        .top
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(top, vec![("zz_top", "{\"k\":-0}"), ("zz_tail", "1E3")]);

    let resaved = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
    let resaved_text = text_of(&resaved);
    assert!(
        resaved_text.contains(",\"zz_p\":{\"b\":[1,2.50]}}"),
        "after the known keys"
    );
    assert!(resaved_text.contains(",\"zz_v\":7}"));
    assert!(resaved_text.contains("\n\"zz_top\":{\"k\":-0},\n\"zz_tail\":1E3,\n"));
    // A fixpoint from there on.
    let again = open(resaved_text).unwrap();
    assert_eq!(
        encode(&again.project, &again.extras, Mode::Normal).unwrap(),
        resaved
    );
}

/// Tier 3: `local` is lenient (unknown or unreadable keys are dropped with a
/// note), an unreadable `history` is dropped whole.
#[test]
fn local_and_history_are_lenient() {
    let base = (0..50)
        .map(small_text)
        .find(|t| t.contains("\"local\":{\"display\":{") && t.contains("\"history\":{\"undo\":["))
        .unwrap();
    let text = base.replacen(
        "\"local\":{\"display\":{",
        "\"local\":{\"zz\":1,\"display\":{\"bad\":5,",
        1,
    );
    let decoded = open(&text).unwrap();
    assert!(decoded.read_only().is_none());
    let dropped: Vec<&DecodeNote> = decoded
        .notes
        .iter()
        .filter(|n| matches!(n, DecodeNote::LocalDropped { .. }))
        .collect();
    assert_eq!(dropped.len(), 2, "{:?}", decoded.notes);
    assert!(!decoded.extras.local.display.contains_key("bad"));

    let not_object = base
        .replacen("\"local\":{", "\"local\":[{", 1)
        .replacen("}},\n\"", "}}],\n\"", 1);
    if let Ok(decoded) = open(&not_object) {
        assert!(decoded.extras.local.is_empty());
    }

    let bad_history = base.replacen(
        "\"history\":{\"undo\":[",
        "\"history\":{\"undo\":7,\"x\":[",
        1,
    );
    let decoded = open(&bad_history).unwrap();
    assert!(decoded.extras.history.is_empty());
    assert!(decoded
        .notes
        .iter()
        .any(|n| matches!(n, DecodeNote::HistoryDropped { .. })));

    let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
    let deep_history = base.replacen(
        "\"history\":{\"undo\":[\n",
        &format!("\"history\":{{\"undo\":[\n{deep},\n"),
        1,
    );
    let decoded = open(&deep_history).unwrap();
    assert!(decoded.extras.history.is_empty());
}

#[test]
fn too_many_history_entries_drop_the_history_not_the_file() {
    let (p, mut extras) = generate(8, SMALL);
    extras.history = super::test_helpers::history(&mut super::test_helpers::Rng(3), 30, 0);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let mut options = DecodeOptions::default();
    options.limits.history_entries = 10;
    let decoded = decode(&bytes, &options).unwrap();
    assert!(decoded.extras.history.is_empty());
    assert!(decoded
        .notes
        .iter()
        .any(|n| matches!(n, DecodeNote::HistoryDropped { .. })));
}
