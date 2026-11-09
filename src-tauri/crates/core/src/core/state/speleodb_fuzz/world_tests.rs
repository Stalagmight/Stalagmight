//! Offline tests of the world's pure rules: the tip, the existence rule, the
//! stale-token retry, the newer-schema stamp and the wire-to-error mapping.
//! Nothing here touches a network.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use super::super::data::{self, Fp};
use super::super::wire::Reply;
use super::{
    append_line, exists_rule, level_label, ok_reply, retry_after_relogin, stale_token,
    topological_tip, write_upload, ServerView, Who,
};
use crate::models::NewerSchema;
use crate::persistence::Database;
use crate::speleodb::models::RevisionCommit;
use crate::speleodb::{Permission, SpeleoDbError, TransportCause};

fn commit(id: &str, parents: &[&str], date: Option<&str>) -> RevisionCommit {
    RevisionCommit {
        id: id.to_owned(),
        parent_ids: parents.iter().map(|p| (*p).to_owned()).collect(),
        author_email: "owner@fuzz.invalid".to_owned(),
        authored_date: date.map(str::to_owned),
        message: None,
    }
}

#[test]
fn the_tip_is_the_commit_nobody_names_as_a_parent() {
    let at = Some("2026-11-09T10:00:00Z");
    let linear = [
        commit("c3", &["c2"], at),
        commit("c2", &["c1"], at),
        commit("c1", &[], at),
    ];
    assert_eq!(topological_tip(&linear).as_deref(), Some("c3"));
    let shuffled = [
        commit("c2", &["c1"], at),
        commit("c1", &[], at),
        commit("c3", &["c2"], at),
    ];
    assert_eq!(topological_tip(&shuffled).as_deref(), Some("c3"));
    // The parent listed nowhere (a truncated history) does not matter.
    assert_eq!(
        topological_tip(&[commit("c9", &["gone"], at)]).as_deref(),
        Some("c9")
    );
    assert_eq!(topological_tip(&[]), None);
    let cycle = [commit("a", &["b"], at), commit("b", &["a"], at)];
    assert_eq!(topological_tip(&cycle), None);
}

#[test]
fn several_heads_pick_the_newest_instant_then_the_first_listed() {
    // 10:00+02:00 is 08:00Z, older than 09:00Z although it sorts after it as text.
    let heads = [
        commit("east", &["root"], Some("2026-11-09T10:00:00+02:00")),
        commit("utc", &["root"], Some("2026-11-09T09:00:00Z")),
        commit("root", &[], Some("2026-11-09T07:00:00Z")),
    ];
    assert_eq!(topological_tip(&heads).as_deref(), Some("utc"));
    let tie = [
        commit("first", &[], Some("2026-11-09T09:00:00Z")),
        commit("second", &[], Some("2026-11-09T09:00:00+00:00")),
    ];
    assert_eq!(topological_tip(&tie).as_deref(), Some("first"));
    let undated = [
        commit("nodate", &[], None),
        commit("garbled", &[], Some("yesterday")),
        commit("dated", &[], Some("2020-01-01T00:00:00Z")),
    ];
    assert_eq!(topological_tip(&undated).as_deref(), Some("dated"));
    let all_undated = [commit("x", &[], None), commit("y", &[], Some("?"))];
    assert_eq!(topological_tip(&all_undated).as_deref(), Some("x"));
}

#[test]
fn a_project_exists_while_the_superuser_sees_it_and_a_run_account_lists_it() {
    let id = "0B7E8F4C-2F43-4C55-9A43-6F1D2B3C4D5E";
    let has = |ids: &[&str]| Some(ids.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>());
    let listed = has(&["0b7e8f4c-2f43-4c55-9a43-6f1d2b3c4d5e"]);
    let other = has(&["11111111-2222-3333-4444-555555555555"]);
    let empty = has(&[]);
    // Soft-deleted: the superuser keeps a view, no run account lists it.
    assert!(!exists_rule(true, &[other.clone(), empty.clone()], id));
    assert!(exists_rule(true, &[other.clone(), listed.clone()], id));
    assert!(exists_rule(true, &[listed.clone(), empty.clone()], id));
    // An unreadable list says nothing, so it counts as holding the project.
    assert!(exists_rule(true, &[None, empty.clone()], id));
    assert!(exists_rule(true, &[other, None], id));
    // Without the superuser's GET it does not exist, whatever the lists say.
    for lists in [
        [listed.clone(), listed],
        [None, None],
        [empty.clone(), empty],
    ] {
        assert!(!exists_rule(false, &lists, id));
    }
}

fn status(code: u16) -> SpeleoDbError {
    SpeleoDbError::Status {
        status: code,
        message: format!("HTTP {code}"),
    }
}

/// Drive [`retry_after_relogin`] with an operation that answers `answers` in
/// turn; returns the result, the tokens the operation saw and how many times
/// the re-login ran.
fn drive(
    whoami: bool,
    answers: Vec<Result<u8, SpeleoDbError>>,
    relogin_ok: bool,
) -> (Result<u8, SpeleoDbError>, Vec<String>, usize) {
    let answers = RefCell::new(answers.into_iter());
    let seen = RefCell::new(Vec::new());
    let relogins = Cell::new(0);
    let result = retry_after_relogin(
        "old",
        whoami,
        |token| {
            seen.borrow_mut().push(token.to_owned());
            answers
                .borrow_mut()
                .next()
                .expect("the operation ran more often than scripted")
        },
        || {
            relogins.set(relogins.get() + 1);
            if relogin_ok {
                Ok("fresh".to_owned())
            } else {
                Err(status(400))
            }
        },
    );
    (result, seen.into_inner(), relogins.get())
}

#[test]
fn a_stale_token_is_refreshed_once_and_the_call_retried_once() {
    let (result, seen, relogins) = drive(false, vec![Err(status(401)), Ok(7)], true);
    assert_eq!(
        (result.ok(), seen, relogins),
        (Some(7), vec!["old".to_owned(), "fresh".to_owned()], 1)
    );

    // A second 401 is returned as is: one re-login, two calls, no loop.
    let (result, seen, relogins) = drive(false, vec![Err(status(401)), Err(status(401))], true);
    assert!(matches!(
        result,
        Err(SpeleoDbError::Status { status: 401, .. })
    ));
    assert_eq!((seen.len(), relogins), (2, 1));

    // A 403 is stale only on whoami.
    let (result, seen, relogins) = drive(true, vec![Err(status(403)), Ok(1)], true);
    assert_eq!((result.ok(), seen.len(), relogins), (Some(1), 2, 1));
    let (result, seen, relogins) = drive(false, vec![Err(status(403))], true);
    assert!(matches!(
        result,
        Err(SpeleoDbError::Status { status: 403, .. })
    ));
    assert_eq!((seen.len(), relogins), (1, 0));

    // Anything else is never retried.
    for err in [
        status(404),
        status(409),
        status(500),
        SpeleoDbError::Transport {
            cause: TransportCause::Unknown,
            detail: "reset".to_owned(),
        },
    ] {
        assert!(!stale_token(&err, true), "{err:?}");
        let (result, seen, relogins) = drive(true, vec![Err(err)], true);
        assert!(result.is_err());
        assert_eq!((seen.len(), relogins), (1, 0));
    }

    // A failed re-login ends it with the re-login's error, without a retry.
    let (result, seen, relogins) = drive(false, vec![Err(status(401))], false);
    assert!(matches!(
        result,
        Err(SpeleoDbError::Status { status: 400, .. })
    ));
    assert_eq!((seen.len(), relogins), (1, 1));
}

#[test]
fn a_newer_schema_upload_is_refused_by_this_build_and_a_plain_one_reads_back() {
    let dir = tempfile::tempdir().expect("tmp");
    let sections = vec![("A".to_owned(), 3), ("B".to_owned(), 2)];
    let project = data::project_with(&sections);

    let plain = dir.path().join("plain.stalx");
    write_upload(&plain, &project, false).expect("write plain");
    assert_eq!(
        data::fingerprint_file(&plain).expect("a plain upload reads back"),
        sections.iter().cloned().collect::<Fp>()
    );

    let newer = dir.path().join("newer.stalx");
    write_upload(&newer, &project, true).expect("write newer");
    let err = Database::open_readonly(&newer)
        .err()
        .expect("this build refuses a newer file");
    let refusal = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<NewerSchema>())
        .unwrap_or_else(|| panic!("not a NewerSchema refusal: {err:#}"));
    assert_eq!(refusal.found, refusal.supported + 1, "{refusal:?}");
    let raw = rusqlite::Connection::open(&newer).expect("raw open");
    let (stamp, column): (i64, i64) = (
        raw.query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version"),
        raw.query_row("SELECT schema_version FROM project", [], |row| row.get(0))
            .expect("schema_version"),
    );
    assert_eq!(
        (stamp, column),
        (i64::from(refusal.found), i64::from(refusal.found))
    );
    assert!(data::fingerprint_file(&newer).is_err());

    // Rewriting the same path replaces the stamped file.
    write_upload(&newer, &project, false).expect("rewrite plain");
    assert!(data::fingerprint_file(&newer).is_ok());
}

#[test]
fn wire_replies_map_onto_the_client_error_type() {
    let reply = |status, body: &[u8]| {
        Ok(Reply {
            status,
            body: body.to_vec(),
        })
    };
    assert_eq!(ok_reply(reply(201, b"{}")).expect("2xx").status, 201);
    let long = vec![b'x'; 500];
    match ok_reply(reply(400, &long)) {
        Err(SpeleoDbError::Status { status, message }) => {
            assert_eq!((status, message.len()), (400, 200));
        }
        other => panic!("{:?}", other.map(|r| r.status)),
    }
    match ok_reply(reply(404, br#"{"detail":"Not found."}"#)) {
        Err(SpeleoDbError::Status { status, message }) => {
            assert_eq!(
                (status, message.as_str()),
                (404, r#"{"detail":"Not found."}"#)
            );
        }
        other => panic!("{:?}", other.map(|r| r.status)),
    }
    assert!(matches!(
        ok_reply(Err("connect: refused".to_owned())),
        Err(SpeleoDbError::Transport { .. })
    ));
}

#[test]
fn permission_labels_match_the_wire_spelling() {
    for level in [
        Permission::WebViewer,
        Permission::ReadOnly,
        Permission::ReadAndWrite,
        Permission::Admin,
    ] {
        assert_eq!(
            serde_json::to_value(level).expect("serialize"),
            serde_json::Value::from(level_label(level))
        );
    }
}

#[test]
fn a_view_serializes_with_named_accounts_and_trace_lines_append() {
    let view = ServerView {
        exists: true,
        perms: BTreeMap::from([(Who::Owner, Some(Permission::Admin)), (Who::Ro, None)]),
        ..ServerView::default()
    };
    let value = serde_json::to_value(&view).expect("serialize");
    assert_eq!(
        value.get("perms"),
        Some(&serde_json::json!({ "Owner": "ADMIN", "Ro": null }))
    );

    let dir = tempfile::tempdir().expect("tmp");
    let trace = dir.path().join("trace.jsonl");
    append_line(&trace, &serde_json::json!({ "step": 1 }));
    append_line(&trace, &value);
    let text = std::fs::read_to_string(&trace).expect("read the trace");
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("one JSON value per line"))
        .collect();
    assert_eq!(lines, vec![serde_json::json!({ "step": 1 }), value]);
}
