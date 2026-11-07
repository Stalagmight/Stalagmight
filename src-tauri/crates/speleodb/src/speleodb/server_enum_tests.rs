//! [#576 B16] Server enums the client does not know yet must not fail a whole list.
//!
//! `Permission` and `Visibility` are closed Rust enums parsed from server labels.
//! The project list and the collaborator lists are parsed as ONE value each, so a
//! single project or grant carrying a label this build has never seen used to
//! fail the entire "Open from SpeleoDB" list (or the whole collaborator roster).

use super::models::{Project, TeamPermissionList, UserPermissionList};
use super::Permission;

/// Two projects, one of them with a permission tier and a visibility this build
/// has never heard of (`OWNER`, `UNLISTED`).
const PROJECTS_WITH_A_NEW_TIER: &str = r#"[
    {"id":"11111111-1111-1111-1111-111111111111","name":"CaveA",
     "permission":"READ_ONLY","visibility":"PRIVATE"},
    {"id":"22222222-2222-2222-2222-222222222222","name":"CaveB",
     "permission":"OWNER","visibility":"UNLISTED"}
]"#;

#[test]
fn one_project_with_an_unknown_tier_does_not_fail_the_whole_list() {
    let parsed: Result<Vec<Project>, _> = serde_json::from_str(PROJECTS_WITH_A_NEW_TIER);
    match &parsed {
        Ok(list) => println!(
            "project list with OWNER/UNLISTED -> Ok({} projects)",
            list.len()
        ),
        Err(e) => println!("project list with OWNER/UNLISTED -> Err({e})"),
    }
    let list = parsed.expect("an unknown label must not fail the whole list");
    assert_eq!(list.len(), 2, "both projects are listed");
    let new_tier = list
        .iter()
        .find(|p| p.name == "CaveB")
        .and_then(|p| p.permission)
        .expect("the unknown tier is kept as a value, not dropped");
    // Fail-safe ordering: an unknown tier grants LESS than the lowest known one, so
    // every capability check (`>= ReadOnly`, `>= ReadAndWrite`, `== Admin`) denies.
    assert!(
        new_tier < Permission::WebViewer,
        "an unknown tier ranks below every known tier, got {new_tier:?}"
    );
}

#[test]
fn one_collaborator_with_an_unknown_level_does_not_fail_the_roster() {
    let users: Result<UserPermissionList, _> = serde_json::from_str(
        r#"{"permissions":[{"user":"a@x","level":"ADMIN"},{"user":"b@x","level":"OWNER"}]}"#,
    );
    let teams: Result<TeamPermissionList, _> =
        serde_json::from_str(r#"{"permissions":[{"team":"t1","level":"AUDITOR"}]}"#);
    println!(
        "user roster with OWNER -> {}; team roster with AUDITOR -> {}",
        users.as_ref().map_or_else(
            |e| format!("Err({e})"),
            |l| format!("Ok({})", l.permissions.len())
        ),
        teams.as_ref().map_or_else(
            |e| format!("Err({e})"),
            |l| format!("Ok({})", l.permissions.len())
        ),
    );
    let users = users.expect("an unknown level must not fail the user roster");
    assert_eq!(users.permissions.len(), 2);
    let teams = teams.expect("nor the team roster");
    assert_eq!(teams.permissions.len(), 1);
}

#[test]
fn an_unknown_level_is_never_sent_back_to_the_server() {
    // Nothing is listening on port 9: a request that went out would come back as a
    // transport failure, so a local refusal proves none was made.
    let base = "http://127.0.0.1:9";
    let id = "11111111-1111-1111-1111-111111111111";
    let refused = [
        super::client::add_user_permission(base, "tok", false, id, "a@x", Permission::Unknown),
        super::client::set_user_permission(base, "tok", false, id, "a@x", Permission::Unknown),
        super::client::add_team_permission(base, "tok", false, id, "t1", Permission::Unknown),
        super::client::set_team_permission(base, "tok", false, id, "t1", Permission::Unknown),
    ];
    for result in refused {
        assert!(
            matches!(result, Err(super::SpeleoDbError::Refused { .. })),
            "an UNKNOWN level is refused locally, got {result:?}"
        );
    }
    // And it serializes as the label the frontend shows, never a known tier.
    assert_eq!(
        serde_json::to_string(&Permission::Unknown).expect("serialize"),
        "\"UNKNOWN\""
    );
}
