//! SDK timestamps must keep the application's UTC and precision contracts.
use super::*;

#[test]
fn project_timestamps_keep_utc_and_chrono_fractional_precision() {
    for (raw, expected) in [
        ("2026-01-01T00:30:00+01:00", "2025-12-31T23:30:00+00:00"),
        ("2026-01-01T23:30:00-02:00", "2026-01-02T01:30:00+00:00"),
        ("2026-01-01T00:00:00Z", "2026-01-01T00:00:00+00:00"),
        ("9999-12-31T23:59:59-01:00", "+10000-01-01T00:59:59+00:00"),
        ("0000-01-01T00:00:00+01:00", "-0001-12-31T23:00:00+00:00"),
        ("2026-01-01T00:00:00.1Z", "2026-01-01T00:00:00.100+00:00"),
        (
            "2026-01-01T00:00:00.1234Z",
            "2026-01-01T00:00:00.123400+00:00",
        ),
        (
            "2026-01-01T00:00:00.123456789Z",
            "2026-01-01T00:00:00.123456789+00:00",
        ),
        (
            "2026-01-01T00:00:00.000000001Z",
            "2026-01-01T00:00:00.000000001+00:00",
        ),
    ] {
        let mut reply = project_json();
        reply["modified_date"] = json!(raw);
        let server = Server::json(reply);
        let project = get_project(&server.base, "t", false, ID).unwrap();
        server.finish();
        assert_eq!(project.modified_date.as_deref(), Some(expected), "{raw}");
        // The detail IPC projection preserves the normalized timestamp.
        let summary = super::super::super::models::SpeleoProjectSummary::from_project(project);
        assert_eq!(summary.modified_date, Some(expected.to_owned()));
    }
}

#[test]
fn revision_timestamps_truncate_to_milliseconds_in_utc_on_both_decode_paths() {
    for legacy in [false, true] {
        let mut commits = vec![
            pre_iso_commit(SHA, "2026-01-01T00:30:00.123456789+01:00"),
            pre_iso_commit(SHA, "2026-01-01T23:30:00.999999999-02:00"),
            pre_iso_commit(SHA, "2026-01-01T00:00:00Z"),
        ];
        if legacy {
            commits.push(pre_iso_commit(SHA, "2026/01/01 00:00"));
        }
        let server = Server::json(json!({"project":project_json(), "commits":commits}));
        let history = get_revisions(&server.base, "t", false, ID).unwrap();
        server.finish();
        let dates: Vec<_> = history
            .commits
            .iter()
            .map(|c| c.authored_date.as_deref())
            .collect();
        let mut expected = vec![
            Some("2025-12-31T23:30:00.123Z"),
            Some("2026-01-02T01:30:00.999Z"),
            Some("2026-01-01T00:00:00.000Z"),
        ];
        if legacy {
            expected.push(Some("2026/01/01 00:00"));
        }
        assert_eq!(dates, expected);
    }
}

#[test]
fn listed_project_timestamps_keep_utc_and_fractional_precision() {
    for (raw, expected) in [
        ("2026-01-01T00:30:00+01:00", "2025-12-31T23:30:00Z"),
        ("9999-12-31T23:59:59-01:00", "+10000-01-01T00:59:59Z"),
        ("2026-01-01T23:30:00.1-02:00", "2026-01-02T01:30:00.100Z"),
        ("2026-01-01T00:00:00.1234Z", "2026-01-01T00:00:00.123400Z"),
        (
            "2026-01-01T00:00:00.123456789Z",
            "2026-01-01T00:00:00.123456789Z",
        ),
    ] {
        let mut reply = project_json();
        reply["modified_date"] = json!(raw);
        let server = Server::json(json!([reply]));
        let projects = list_projects(&server.base, "t", false).unwrap();
        server.finish();
        assert_eq!(
            projects[0].modified_date.as_deref(),
            Some(expected),
            "{raw}"
        );
    }
}

#[test]
fn revision_fallback_keeps_instants_accepted_by_the_previous_sdk() {
    let server = Server::json(json!({"project":project_json(), "commits":[
        pre_iso_commit(SHA, "2026-01-01 00:00:00UTC"),
    ]}));
    let history = get_revisions(&server.base, "t", false, ID).unwrap();
    server.finish();
    assert_eq!(
        history.commits[0].authored_date.as_deref(),
        Some("2026-01-01T00:00:00.000Z")
    );
}

#[test]
fn utc_year_rollover_does_not_turn_successful_mutations_into_errors() {
    fn check(status: u16, operation: impl FnOnce(&str) -> Result<Project, SpeleoDbError>) {
        let mut reply = project_json();
        reply["modified_date"] = json!("9999-12-31T23:59:59-01:00");
        let server = Server::replies(vec![(status, reply.to_string().into_bytes())]);
        let project = operation(&server.base).unwrap();
        assert_eq!(server.finish().len(), 1);
        assert_eq!(
            project.modified_date.as_deref(),
            Some("+10000-01-01T00:59:59+00:00")
        );
    }
    check(201, |base| {
        create_project_with(
            base,
            "t",
            false,
            r#"{"name":"Synthetic cave","description":"SDK test","country":"US","type":"STALAGMIGHT"}"#,
        )
    });
    check(200, |base| acquire_lock(base, "t", false, ID));
    check(200, |base| release_lock(base, "t", false, ID));
    check(200, |base| {
        patch_project(base, "t", false, ID, r#"{"name":"Renamed"}"#)
    });
}

#[test]
fn an_excluded_project_at_the_utc_year_boundary_does_not_hide_the_list() {
    let mut excluded = project_json();
    excluded["type"] = json!("COMPASS");
    excluded["modified_date"] = json!("9999-12-31T23:59:59-01:00");
    let server = Server::json(json!([excluded, project_json()]));
    let projects = list_projects(&server.base, "t", false).unwrap();
    server.finish();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].kind, "stalagmight");
    assert_eq!(
        projects[0].modified_date.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
}

#[test]
fn utc_year_rollover_keeps_revision_topology_on_both_decode_paths() {
    let parent = "1111111111111111111111111111111111111111";
    for legacy in [false, true] {
        let mut tip = pre_iso_commit(SHA, "9999-12-31T23:59:59-01:00");
        tip["parent_ids"] = json!([parent]);
        let root_date = if legacy {
            "2026/01/01 00:00"
        } else {
            "2026-01-01T00:00:00Z"
        };
        let server = Server::json(json!({"project":project_json(), "commits":[
            tip, pre_iso_commit(parent, root_date),
        ]}));
        let history = get_revisions(&server.base, "t", false, ID).unwrap();
        server.finish();
        assert_eq!(
            history.commits[0].authored_date.as_deref(),
            Some("+10000-01-01T00:59:59.000Z")
        );
        assert_eq!(
            super::super::super::sync::tip_of(&history.commits),
            Some(SHA)
        );
    }
}
