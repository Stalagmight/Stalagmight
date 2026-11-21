//! Which ignored warnings a file keeps (file-format.md 5.13).
//!
//! An ignore names its shot by uuid. It is kept while the survey has that
//! shot, and also while the file's undo or redo history names it: a step can
//! bring a deleted shot back with its uuid (the undo side of a delete, the
//! redo side of a create or an Add, written as literal rows that carry the
//! uuid as a JSON string), and the shot's ignore must come back with it, as
//! it does within a session (review F-command-parity-2). An entry naming a
//! shot that neither holds is dead, and writers and readers drop it.
//!
//! A shot no history step creates cannot come back by undo or redo: a step
//! that brings back a shot the state before it lacks builds it from literal
//! rows (a copy run copies a shot that state still has), so looking for the
//! uuid as a string in the entries finds every shot that can.

use std::collections::{HashMap, HashSet};

use crate::models::survey::Project;

use super::types::{IgnoredWarning, RawJson};
use super::write::string;

/// `ignores` whose shot the survey or `history` names, in input order.
pub(super) fn reachable<'a, I>(
    project: &Project,
    ignores: I,
    history: &[&[RawJson]],
) -> Vec<&'a IgnoredWarning>
where
    I: IntoIterator<Item = &'a IgnoredWarning>,
{
    let uuids = project.shot_uuids();
    let (live, gone): (Vec<&IgnoredWarning>, Vec<&IgnoredWarning>) =
        ignores.into_iter().partition(|w| uuids.contains(&w.shot));
    if gone.is_empty() {
        return live;
    }
    let named = named_in_history(gone.iter().map(|w| w.shot.as_str()), history);
    let mut kept = live;
    kept.extend(gone.into_iter().filter(|w| named.contains(w.shot.as_str())));
    kept
}

/// Which of `wanted` some entry of `history` holds as a JSON string. One
/// pass over each entry: its string tokens are compared, in the format's
/// escaping (entries are canonical), with the escaped forms of `wanted`.
fn named_in_history<'a>(
    wanted: impl Iterator<Item = &'a str>,
    history: &[&[RawJson]],
) -> HashSet<&'a str> {
    let escaped: HashMap<Vec<u8>, &'a str> = wanted
        .map(|uuid| {
            let mut quoted = Vec::with_capacity(uuid.len() + 2);
            string(&mut quoted, uuid);
            (quoted[1..quoted.len() - 1].to_vec(), uuid)
        })
        .collect();
    let mut found = HashSet::new();
    for entry in history.iter().flat_map(|entries| entries.iter()) {
        if found.len() == escaped.len() {
            break;
        }
        for token in string_tokens(entry.as_str().as_bytes()) {
            if let Some(uuid) = escaped.get(token) {
                found.insert(*uuid);
            }
        }
    }
    found
}

/// The raw (still escaped) contents of every string in the JSON text `bytes`,
/// keys included. `bytes` is well formed, so a quote outside a string opens
/// one and the first unescaped quote after it closes it.
fn string_tokens(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let open = at + bytes.get(at..)?.iter().position(|&b| b == b'"')?;
        let mut i = open + 1;
        while let Some(&b) = bytes.get(i) {
            match b {
                b'\\' => i += 2,
                b'"' => break,
                _ => i += 1,
            }
        }
        let token = bytes.get(open + 1..i)?;
        at = i + 1;
        Some(token)
    })
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: a bad fixture is a broken test"
)]
mod tests {
    use chrono::TimeZone as _;

    use super::super::test_helpers::{generate, SMALL};
    use super::super::types::{DecodeOptions, Extras, History, LocalSync, Mode, PushOptions};
    use super::super::{decode, encode};
    use super::*;

    fn raw(text: &str) -> RawJson {
        RawJson::parse(text).unwrap()
    }

    fn ignore_of(shot: &str) -> IgnoredWarning {
        IgnoredWarning {
            shot: shot.to_string(),
            code: "SHORT_SHOT".to_string(),
            at: chrono::Utc
                .timestamp_opt(1_750_000_000, 0)
                .single()
                .unwrap(),
            comment: None,
        }
    }

    /// A survey whose file holds ignores of three shots it no longer has: one
    /// an undo step brings back, one a redo step does, one nothing does.
    fn file() -> (Project, Extras) {
        let (p, mut extras) = generate(7, SMALL);
        // No link: a push keeps the undo history as the options say.
        extras.speleodb = None;
        extras.local.speleodb = LocalSync::default();
        let entry = |shot: &str| {
            raw(&format!(
                r#"{{"t":"2026-01-01T00:00:00Z","k":"delete_shot","c":{{"lines":[{{"b":{{"s":[{{"u":["{shot}"]}}]}}}}]}}}}"#
            ))
        };
        extras.history = History {
            undo: vec![entry("deleted-shot")],
            redo: vec![entry("created-shot")],
        };
        extras.ignored_warnings = vec![
            ignore_of("created-shot"),
            ignore_of("deleted-shot"),
            ignore_of("gone-shot"),
        ];
        (p, extras)
    }

    fn kept(bytes: &[u8]) -> Vec<String> {
        let decoded = decode(bytes, &DecodeOptions::default()).unwrap();
        let mut shots: Vec<String> = decoded
            .extras
            .ignored_warnings
            .into_iter()
            .map(|w| w.shot)
            .collect();
        shots.retain(|s| s.ends_with("-shot"));
        shots
    }

    /// F-command-parity-2: an ignore of a shot the file's history can bring
    /// back survives the save and the reopen, so undoing the delete after a
    /// reopen does not bring the warning back; a dead one is still dropped.
    #[test]
    fn an_ignore_of_a_shot_the_history_can_bring_back_survives_save_and_reopen() {
        let (p, extras) = file();
        let bytes = encode(&p, &extras, Mode::Normal).unwrap();
        assert_eq!(kept(&bytes), ["created-shot", "deleted-shot"]);
        // The reader keeps them too, and re-encodes the same bytes.
        let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
        assert_eq!(
            encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap(),
            bytes
        );
    }

    /// A push carries no redo: an ignore only a redo step could use goes.
    #[test]
    fn a_push_keeps_only_the_ignores_its_own_history_can_use() {
        let (p, extras) = file();
        let push = Mode::ForPush(PushOptions {
            keep_undo: true,
            ..PushOptions::default()
        });
        let bytes = encode(&p, &extras, push).unwrap();
        assert_eq!(kept(&bytes), ["deleted-shot"]);
        let no_undo = Mode::ForPush(PushOptions::default());
        assert!(kept(&encode(&p, &extras, no_undo).unwrap()).is_empty());
    }

    #[test]
    fn finds_a_uuid_held_as_a_string_and_nothing_else() {
        let entries = [raw(
            r#"{"u":["shot-a","x\"y"],"k":"shot-b-not","shot-c":1}"#,
        )];
        let history: [&[RawJson]; 1] = [&entries];
        let named = named_in_history(
            ["shot-a", "x\"y", "shot-b", "shot-c", "shot-d"].into_iter(),
            &history,
        );
        let mut named: Vec<&str> = named.into_iter().collect();
        named.sort_unstable();
        // A key is a string too; a uuid matches only a whole string.
        assert_eq!(named, ["shot-a", "shot-c", "x\"y"]);
    }

    #[test]
    fn tokens_skip_escaped_quotes() {
        let tokens: Vec<&[u8]> = string_tokens(br#"{"a\"b":"c\\","d":[1,"e"]}"#).collect();
        assert_eq!(
            tokens,
            [&b"a\\\"b"[..], &b"c\\\\"[..], &b"d"[..], &b"e"[..]]
        );
    }
}
