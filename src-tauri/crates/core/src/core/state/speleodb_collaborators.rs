//! The linked SpeleoDB project's collaborators, offered as survey-credit
//! suggestions (people pills). A read-only nicety: it never gates an edit, and
//! a failure to name someone only leaves their name unknown.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::speleodb::{local_dto, validate_project_id};
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{SpeleoCollaborator, SpeleoDbError, SpeleoErrorDto};

/// How long a project's listing is served from memory before it is fetched
/// again: long enough that typing in a people input never re-asks the server,
/// short enough that a collaborator added on the web shows up within a session.
const CACHE_TTL: Duration = Duration::from_mins(10);

/// Most listings kept in memory at once (one per project actually asked about).
const CACHE_CAPACITY: usize = 32;

/// `(endpoint, account email, project id)`: an account switch or another
/// server never sees a listing it did not fetch.
type CacheKey = (String, String, String);

/// The fetched listings, with when each was fetched.
pub(crate) type CollaboratorCache = HashMap<CacheKey, (Instant, Vec<SpeleoCollaborator>)>;

impl AppState {
    fn cached_collaborators(&self, key: &CacheKey) -> Option<Vec<SpeleoCollaborator>> {
        let guard = self.speleodb_collaborators.lock().ok()?;
        let (at, list) = guard.get(key)?;
        (at.elapsed() < CACHE_TTL).then(|| list.clone())
    }

    fn remember_collaborators(&self, key: CacheKey, list: &[SpeleoCollaborator]) {
        let Ok(mut guard) = self.speleodb_collaborators.lock() else {
            return;
        };
        guard.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if guard.len() >= CACHE_CAPACITY {
            if let Some(oldest) = guard
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest);
            }
        }
        guard.insert(key, (Instant::now(), list.to_vec()));
    }

    /// Forget every cached listing (sign-out, a dropped token).
    pub(super) fn clear_collaborator_cache(&self) {
        if let Ok(mut guard) = self.speleodb_collaborators.lock() {
            guard.clear();
        }
    }
}

impl AppState {
    /// Everyone with access to SpeleoDB project `project_id`, the signed-in user
    /// included, with display names where the server knows them. Served from an
    /// in-memory cache for [`CACHE_TTL`].
    ///
    /// A 403 on the permission list (an account without the right to see who
    /// else has access) is not an error here: the signed-in user alone is still
    /// a useful suggestion.
    #[expect(
        clippy::result_large_err,
        reason = "IPC returns the shared structured SpeleoDB error"
    )]
    pub fn speleodb_project_collaborators(
        &self,
        project_id: String,
    ) -> Result<Vec<SpeleoCollaborator>, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;
        let (endpoint, token, me) = self.speleodb_active_account().map_err(local_dto)?;
        let key = (endpoint.clone(), me.to_lowercase(), project_id.clone());
        if let Some(list) = self.cached_collaborators(&key) {
            return Ok(list);
        }
        let insecure_http = self.speleodb_insecure_http();
        let listed = self.speleodb_handle_auth(
            Endpoint::GetProject,
            client::list_project_collaborators(&endpoint, &token, insecure_http, &project_id),
        );
        let rows = match listed {
            Ok(rows) => rows,
            Err(SpeleoDbError::Status { status: 403, .. }) => Vec::new(),
            Err(e) => return Err(dto_for_error(Endpoint::GetProject, &e)),
        };
        let mut list: Vec<SpeleoCollaborator> = rows
            .into_iter()
            .map(|row| SpeleoCollaborator {
                name: row.name,
                email: row.email,
                level: row.level,
            })
            .collect();
        if !list.iter().any(|c| c.email.eq_ignore_ascii_case(&me)) {
            list.push(SpeleoCollaborator {
                name: client::lookup_user_name(&endpoint, &token, insecure_http, &me),
                email: me,
                level: String::new(),
            });
        }
        self.remember_collaborators(key, &list);
        Ok(list)
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    use super::*;

    const PROJECT: &str = "11111111-1111-1111-1111-111111111111";
    const ME: &str = "me@example.org";

    /// A loopback SpeleoDB answering the permission list with `permissions`
    /// (status, body) and every name lookup from `names`, recording request lines.
    fn server(
        permissions: (u16, &'static str),
        names: fn(&str) -> &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                    }
                }
                let line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let (status, body) = if line.contains("/permissions/user/") {
                    permissions
                } else {
                    (200, names(&line))
                };
                log.lock().expect("log").push(line);
                let response = format!(
                    "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn sign_in(state: &AppState, endpoint: &str) {
        let mut session = state.speleodb_session_lock().expect("session lock");
        session.endpoint = Some(endpoint.to_owned());
        session.email = Some(ME.to_owned());
        session.token_store =
            crate::speleodb::auth::TokenStore::from_in_memory_token("<your_token_here>");
        session.rehydrate_attempted = true;
    }

    fn names(line: &str) -> &'static str {
        if line.contains("query=ann%40example.org") {
            r#"[{"email":"ann@example.org","name":"Ann Smith","label":"Ann Smith"}]"#
        } else if line.contains("query=me%40example.org") {
            r#"[{"email":"me@example.org","name":"Me Myself","label":"Me"}]"#
        } else {
            "[]"
        }
    }

    #[test]
    fn collaborators_include_me_and_are_served_from_the_cache() {
        crate::test_helpers::with_isolated_config(|| {
            let (url, seen) = server(
                (
                    200,
                    r#"{"permissions":[{"user":"ann@example.org","level":"ADMIN"},
                        {"user":"bo@example.org","level":"READ_ONLY"}],"project":{}}"#,
                ),
                names,
            );
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            let got = state
                .speleodb_project_collaborators(PROJECT.to_owned())
                .unwrap();
            let summary: Vec<(Option<&str>, &str, &str)> = got
                .iter()
                .map(|c| (c.name.as_deref(), c.email.as_str(), c.level.as_str()))
                .collect();
            assert_eq!(
                summary,
                [
                    (Some("Ann Smith"), "ann@example.org", "ADMIN"),
                    (None, "bo@example.org", "READ_ONLY"),
                    (Some("Me Myself"), ME, ""),
                ]
            );
            let calls = seen.lock().expect("log").len();
            assert_eq!(calls, 4, "one list, three lookups");
            let again = state
                .speleodb_project_collaborators(PROJECT.to_owned())
                .unwrap();
            assert_eq!(again, got);
            assert_eq!(seen.lock().expect("log").len(), calls, "served from memory");
        });
    }

    #[test]
    fn a_403_on_the_list_still_suggests_me() {
        crate::test_helpers::with_isolated_config(|| {
            let (url, _) = server((403, r#"{"detail":"denied"}"#), names);
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            let got = state
                .speleodb_project_collaborators(PROJECT.to_owned())
                .unwrap();
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].email, ME);
        });
    }

    #[test]
    fn a_bad_project_id_or_no_session_asks_nothing() {
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            state.speleodb_session_lock().expect("session").forget();
            assert!(state
                .speleodb_project_collaborators("../etc".to_owned())
                .is_err());
            assert!(state
                .speleodb_project_collaborators(PROJECT.to_owned())
                .is_err());
        });
    }
}
