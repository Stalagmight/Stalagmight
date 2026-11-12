//! Who has access to a SpeleoDB project, with their display names: the
//! suggestions the survey-credit people inputs offer for a linked project.
//!
//! Two reads, both authenticated and both bounded:
//! - `GET /api/v2/projects/{id}/permissions/user/` lists the users with access,
//!   as `{permissions: [{user: <email>, level, ...}], project}`.
//! - `GET /api/v2/user/autocomplete/?query=<email>` fills in each one's name,
//!   from `[{email, name, label}]` (at most 10 rows, a query of at least 3
//!   characters). Only the row whose email is EXACTLY the query names the
//!   person. A failed or empty lookup leaves the name unknown; it never fails
//!   the list.

use serde_json::Value;

use super::{api_error, client, Endpoint, SpeleoDbError, READ_TIMEOUT};
use speleodb_api::ApiOpError;

/// Most collaborators one listing keeps, and so most name lookups it makes.
pub const MAX_COLLABORATORS: usize = 50;

/// Longest email or name kept from a reply, in chars (the server caps both
/// near 255; anything past this is not a person's label).
const MAX_TEXT_CHARS: usize = 255;

/// The autocomplete endpoint refuses a shorter query.
const MIN_QUERY_CHARS: usize = 3;

/// One user with access to a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCollaborator {
    /// The account email, as the permission list names the user.
    pub email: String,
    /// The permission level (`ADMIN`, `READ_AND_WRITE`, `READ_ONLY`,
    /// `WEB_VIEWER`), verbatim.
    pub level: String,
    /// The display name, when the name lookup found one.
    pub name: Option<String>,
}

fn bounded(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(MAX_TEXT_CHARS).collect())
}

/// `(email, level)` of every row of a permission list reply, deduplicated by
/// email (case-insensitively, first row wins) and bounded to
/// [`MAX_COLLABORATORS`]. Read from the raw JSON so a future enum value in the
/// nested `project` (which the SDK would refuse) cannot hide the list.
fn permission_rows(value: &Value) -> Result<Vec<(String, String)>, SpeleoDbError> {
    let rows = value
        .get("permissions")
        .and_then(Value::as_array)
        .ok_or_else(|| SpeleoDbError::Parse("invalid project permission list".into()))?;
    let mut out: Vec<(String, String)> = Vec::new();
    for row in rows {
        let Some(email) = row.get("user").and_then(Value::as_str).and_then(bounded) else {
            continue;
        };
        if out.iter().any(|(e, _)| e.eq_ignore_ascii_case(&email)) {
            continue;
        }
        let level = row
            .get("level")
            .and_then(Value::as_str)
            .and_then(bounded)
            .unwrap_or_default();
        out.push((email, level));
        if out.len() == MAX_COLLABORATORS {
            break;
        }
    }
    Ok(out)
}

/// The name of the autocomplete row whose email is exactly `email`.
fn exact_name(value: &Value, email: &str) -> Option<String> {
    value.as_array()?.iter().find_map(|row| {
        let row_email = row.get("email").and_then(Value::as_str)?;
        if !row_email.trim().eq_ignore_ascii_case(email) {
            return None;
        }
        row.get("name").and_then(Value::as_str).and_then(bounded)
    })
}

/// The JSON body of an SDK reply, typed or not: a 2xx the SDK could not
/// decode is still read, as `list_projects` does.
fn reply_json<T: serde::Serialize, E: std::fmt::Debug>(
    result: Result<T, ApiOpError<E>>,
    what: &str,
) -> Result<Value, SpeleoDbError> {
    match result {
        Ok(value) => {
            serde_json::to_value(value).map_err(|_| SpeleoDbError::Parse(format!("invalid {what}")))
        }
        Err(ApiOpError::Api(error)) if (200..300).contains(&error.status) => {
            serde_json::from_slice(&error.raw_body)
                .map_err(|_| SpeleoDbError::Parse(format!("invalid {what}")))
        }
        Err(error) => Err(api_error(error)),
    }
}

/// One user's display name by exact email (`GET /user/autocomplete/`), or
/// `None` when the lookup fails, the email is too short to query, or no row
/// matches it exactly. Never an error: a name is a nicety.
pub fn lookup_user_name(base: &str, token: &str, insecure: bool, email: &str) -> Option<String> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT).ok()?;
    lookup(&c, email).ok().flatten()
}

fn lookup(c: &super::SdkClient, email: &str) -> Result<Option<String>, SpeleoDbError> {
    if email.chars().count() < MIN_QUERY_CHARS {
        return Ok(None);
    }
    // Not retried: a suggestion is not worth a second request.
    let value = c.call(Endpoint::UserAutocomplete, false, || async {
        reply_json(
            c.http.v2_user_autocomplete_list(None, email).await,
            "user autocomplete",
        )
    })?;
    Ok(exact_name(&value, email))
}

/// Everyone with access to project `id`, with their names where the name
/// lookup knows them. The permission list failing fails the call; a name
/// lookup failing only leaves that name unknown, and a transport failure stops
/// the remaining lookups (the server is not answering, so each would wait out
/// its own timeout).
pub fn list_project_collaborators(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<Vec<ProjectCollaborator>, SpeleoDbError> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    let rows = c.call(Endpoint::ListPermissions, true, || async {
        let value = reply_json(
            c.http.v2_projects_permissions_user_retrieve(id, None).await,
            "project permission list",
        )?;
        permission_rows(&value)
    })?;
    let mut out = Vec::with_capacity(rows.len());
    let mut lookups_open = true;
    for (email, level) in rows {
        let mut name = None;
        if lookups_open {
            match lookup(&c, &email) {
                Ok(found) => name = found,
                Err(SpeleoDbError::Transport { .. }) => lookups_open = false,
                Err(_) => {}
            }
        }
        out.push(ProjectCollaborator { email, level, name });
    }
    Ok(out)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test assertions")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permission_rows_are_deduplicated_bounded_and_tolerant() {
        let mut permissions: Vec<Value> = vec![
            json!({"user": "a@example.org", "level": "ADMIN"}),
            json!({"user": "A@Example.org", "level": "READ_ONLY"}),
            json!({"user": "", "level": "ADMIN"}),
            json!({"level": "ADMIN"}),
            json!({"user": "b@example.org"}),
        ];
        for i in 0..(MAX_COLLABORATORS + 10) {
            permissions.push(json!({"user": format!("u{i}@example.org"), "level": "WEB_VIEWER"}));
        }
        let rows = permission_rows(&json!({"permissions": permissions, "project": {}})).unwrap();
        assert_eq!(rows.len(), MAX_COLLABORATORS);
        assert_eq!(rows[0], ("a@example.org".to_string(), "ADMIN".to_string()));
        assert_eq!(rows[1], ("b@example.org".to_string(), String::new()));
        assert!(permission_rows(&json!({"project": {}})).is_err());
    }

    #[test]
    fn only_the_exact_email_names_the_person() {
        let rows = json!([
            {"email": "ann.smith@example.org", "name": "Not Her", "label": "x"},
            {"email": "ANN@example.org", "name": " Ann ", "label": "Ann <ann@example.org>"},
        ]);
        assert_eq!(exact_name(&rows, "ann@example.org").as_deref(), Some("Ann"));
        assert_eq!(exact_name(&rows, "bo@example.org"), None);
        assert_eq!(
            exact_name(
                &json!([{"email": "c@example.org", "name": ""}]),
                "c@example.org"
            ),
            None
        );
        assert_eq!(exact_name(&json!({"not": "a list"}), "c@example.org"), None);
    }
}
