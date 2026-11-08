//! [#576 W89] The admin verbs never act on another SpeleoDB server than the one the
//! open copy is linked to.
//!
//! Every admin verb sends the SESSION's token to the session's server and names the
//! LINK's project: Release lock (an ADMIN's force-release of a colleague's lock
//! included), the collaborator and team changes, the project metadata and the
//! web-viewer (exclude-GeoJSON) switch. So a copy linked to server X, open while
//! signed in to server Y, must not reach Y with X's project id: it is refused before
//! anything is sent, as Delete already was (#576 RV4-R-S4-D2) and Start editing is
//! (W88). The same server spelled another way - the case of the scheme, a trailing
//! slash - is still the same server, and every verb reaches it as before.
//!
//! Driven against a loopback routing stub. All data is synthetic, built in code.

use super::speleodb::{
    authed_session_at, editing_fixture, link_open_copy_to, remote_readonly_fixture, routing_stub,
};
use super::AppState;
use crate::models::CommandErrorCode;
use crate::speleodb::{Permission, SpeleoErrorDto, SpeleoProjectMetadataPatch};

/// The server the copy is linked to in the mismatch case (never reachable).
const LINKED_TO: &str = "http://x/";

/// `endpoint` spelled another way: the scheme upper-cased, a trailing slash added.
fn spelled_another_way(endpoint: &str) -> String {
    format!(
        "{}/",
        endpoint.trim_end_matches('/').replacen("http", "HTTP", 1)
    )
}

/// What SpeleoDB answers the admin calls with, enough for each verb to complete: the
/// two permission lists, the caller's teams, an accepted permission write, and the
/// project record for Release lock, the metadata PATCH and the metadata GET.
fn admin_route(line: &str) -> (u16, &'static str) {
    if line.contains("/permissions/user/") || line.contains("/permissions/team/") {
        (200, r#"{"permissions":[]}"#)
    } else if line.starts_with("GET /api/v2/teams/") {
        (200, "[]")
    } else if line.contains("/permission/") {
        (200, "{}")
    } else {
        (
            200,
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote"}"#,
        )
    }
}

/// An admin verb, its answer dropped.
type Verb = fn(&AppState) -> Result<(), SpeleoErrorDto>;

/// One verb's run: its answer (code and message on failure), the requests the
/// signed-in server saw, and the edit gate afterwards.
struct Run {
    result: Result<(), (CommandErrorCode, String)>,
    sent: Vec<String>,
    gate: bool,
}

/// Run `verb` over the remote fixture, signed in to a fresh stub and with the copy
/// linked to `link(stub)`. `editing` starts it mid-edit-session (gate open, this
/// session's claim recorded), as a lock holder's own Release lock is reached;
/// otherwise the gate is shut, as an ADMIN releasing a colleague's lock is.
fn run(verb: Verb, editing: bool, link: fn(&str) -> String) -> Run {
    let (server, _listener, seen) = routing_stub(admin_route);
    let (_temp, state) = if editing {
        editing_fixture(&server)
    } else {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&server);
        (temp, state)
    };
    link_open_copy_to(&state, &link(&server));
    let result = verb(&state).map_err(|e| (e.code, e.message));
    let sent = seen.lock().map(|lines| lines.clone()).unwrap_or_default();
    Run {
        result,
        sent,
        gate: state.edit_enabled(),
    }
}

/// Every verb of a family: refused over a copy linked to another server, with
/// nothing sent; and, over the same server spelled another way, sent and answered.
fn assert_family(family: &str, editing: bool, verbs: &[(&str, Verb)]) {
    // Every verb runs and prints before any assertion, so one run shows them all.
    let runs: Vec<(&str, Run, Run)> = verbs
        .iter()
        .map(|(name, verb)| {
            let other = run(*verb, editing, |_| LINKED_TO.to_owned());
            let same = run(*verb, editing, spelled_another_way);
            println!(
                "[W89 item 1, {family} / {name}] linked to {LINKED_TO}, signed in \
                 elsewhere: {:?}, requests to the signed-in server {:?}, gate {}. Same \
                 server spelled another way: {:?}, requests {:?}, gate {}",
                other.result, other.sent, other.gate, same.result, same.sent, same.gate
            );
            (*name, other, same)
        })
        .collect();
    for (name, other, same) in runs {
        assert!(
            other.sent.is_empty(),
            "{family} / {name}: nothing reaches the signed-in server for a copy of \
             another server ({:?})",
            other.sent
        );
        let (code, message) = other
            .result
            .expect_err("a copy linked to another server is refused");
        assert_eq!(code, CommandErrorCode::SpeleodbFatal, "{family} / {name}");
        assert!(
            message.contains(LINKED_TO) && message.contains("sign in to that server"),
            "{family} / {name}: the refusal names the copy's server and says what to \
             do: {message}"
        );
        assert!(
            same.result.is_ok(),
            "{family} / {name}: the same server spelled another way still works: {:?}",
            same.result
        );
        assert!(
            !same.sent.is_empty(),
            "{family} / {name}: and its request reaches that server"
        );
    }
}

// The holder's own release (gate open, claim recorded) and an ADMIN's release of a
// colleague's lock (gate shut) are the same verb and the same request.
#[test]
fn releasing_ones_own_lock_never_reaches_another_server_than_the_copys() {
    assert_family(
        "release lock, own",
        true,
        &[("release", AppState::speleodb_release_current_lock)],
    );
}

#[test]
fn an_admin_releasing_a_colleagues_lock_never_reaches_another_server_than_the_copys() {
    assert_family(
        "release lock, a colleague's",
        false,
        &[("release", AppState::speleodb_release_current_lock)],
    );
}

#[test]
fn collaborator_changes_never_reach_another_server_than_the_copys() {
    assert_family(
        "collaborators",
        false,
        &[
            ("list", |s| s.speleodb_list_collaborators().map(drop)),
            ("add", |s| {
                s.speleodb_add_collaborator("c@x.invalid".to_owned(), Permission::ReadOnly)
                    .map(drop)
            }),
            ("set", |s| {
                s.speleodb_set_collaborator("c@x.invalid".to_owned(), Permission::ReadAndWrite)
                    .map(drop)
            }),
            ("remove", |s| {
                s.speleodb_remove_collaborator("c@x.invalid".to_owned())
                    .map(drop)
            }),
        ],
    );
}

#[test]
fn team_changes_never_reach_another_server_than_the_copys() {
    assert_family(
        "teams",
        false,
        &[
            ("list teams", |s| s.speleodb_list_teams().map(drop)),
            ("add", |s| {
                s.speleodb_add_team("team-1".to_owned(), Permission::ReadOnly)
                    .map(drop)
            }),
            ("set", |s| {
                s.speleodb_set_team("team-1".to_owned(), Permission::ReadAndWrite)
                    .map(drop)
            }),
            ("remove", |s| {
                s.speleodb_remove_team("team-1".to_owned()).map(drop)
            }),
        ],
    );
}

#[test]
fn project_metadata_never_reaches_another_server_than_the_copys() {
    assert_family(
        "project metadata",
        false,
        &[
            ("get", |s| s.speleodb_get_project_metadata().map(drop)),
            ("patch", |s| {
                s.speleodb_patch_project(SpeleoProjectMetadataPatch {
                    name: None,
                    description: Some("A synthetic description".to_owned()),
                    country: None,
                    visibility: None,
                    coordinates_edited: false,
                    latitude: None,
                    longitude: None,
                })
                .map(drop)
            }),
        ],
    );
}

#[test]
fn the_web_viewer_switch_never_reaches_another_server_than_the_copys() {
    assert_family(
        "exclude-GeoJSON",
        false,
        &[
            ("enable rendering", |s| {
                s.speleodb_set_exclude_geojson(false)
            }),
            ("disable rendering", |s| {
                s.speleodb_set_exclude_geojson(true)
            }),
        ],
    );
}
