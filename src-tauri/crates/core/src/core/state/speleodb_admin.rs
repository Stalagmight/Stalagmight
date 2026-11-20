//! Release the open linked project's write lock through its authenticated session.

use super::speleodb::validate_project_id;
use super::speleodb_session::LockClaim;
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

/// Map a local (non-endpoint) [`SpeleoDbError`] - a lock-resolution / not-linked
/// failure raised before any HTTP call - into the IPC error DTO. Mirrors
/// `speleodb::local_dto` (private to that module): the endpoint is irrelevant for a
/// pre-request error, so `User` is a neutral choice for the typed-action mapping.
fn admin_dto(err: SpeleoDbError) -> SpeleoErrorDto {
    dto_for_error(Endpoint::User, &err)
}

impl AppState {
    /// Resolve the `(link, endpoint, token, insecure_http)` tuple every admin call
    /// needs: the open project's persisted link plus the active session, or an IPC
    /// error DTO when there is no remote project / no usable token.
    fn speleodb_admin_context(
        &self,
    ) -> Result<(crate::models::SpeleoLink, String, String, bool), SpeleoErrorDto> {
        let link = self.speleodb_open_link().map_err(admin_dto)?;
        // Route-traversal guard on the persisted project_id before it reaches any
        // `projects/{id}/...` URL (the same check `speleodb_validated_link` runs for
        // the edit verbs; reused so this entry point can't forget it).
        validate_project_id(&link.project_id).map_err(admin_dto)?;
        let (endpoint, token) = self.speleodb_active_token().map_err(admin_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        Ok((link, endpoint, token, insecure_http))
    }

    /// Inspect a linked SQLite file before replacing the open project. Reads only
    /// local metadata and the existing project endpoint; never acquires a lock.
    pub fn speleodb_project_access_at(
        &self,
        path: String,
        local_only: bool,
    ) -> Result<Option<crate::speleodb::SpeleoFileAccess>, SpeleoErrorDto> {
        use crate::speleodb::{SpeleoAccessVerification, SpeleoFileAccess, SpeleoProjectSummary};
        let path = std::path::Path::new(&path);
        let Ok(db) = crate::persistence::Database::open_for_metadata(path) else {
            return Ok(None); // The normal loader reports invalid/missing files.
        };
        let Some(link) = db
            .load_speleo_link()
            .map_err(|e| admin_dto(SpeleoDbError::Io(e.to_string())))?
        else {
            return Ok(None);
        };
        validate_project_id(&link.project_id).map_err(admin_dto)?;
        // Unknown access (offline, signed out, another deployment) opens read-only.
        let fallback = SpeleoProjectSummary {
            modified_date: None,
            id: link.project_id.clone(),
            name: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            permission: None,
            lock_holder: None,
            type_tag: "STALAGMIGHT".to_owned(),
            latest_commit_sha: None,
        };
        let release_context =
            self.speleodb_session_snapshot()
                .ok()
                .and_then(|(generation, open)| {
                    if open.project_id != link.project_id || open.endpoint != link.endpoint {
                        return None;
                    }
                    let (endpoint, _, user_email) = self.speleodb_active_account().ok()?;
                    if crate::speleodb::auth::normalize_endpoint(&endpoint)
                        != crate::speleodb::auth::normalize_endpoint(&link.endpoint)
                    {
                        return None;
                    }
                    Some(crate::speleodb::SpeleoReleaseContext {
                        project_id: open.project_id,
                        endpoint,
                        user_email,
                        session_generation: generation.to_string(),
                        project_modified_at: self
                            .project
                            .lock()
                            .ok()?
                            .as_ref()?
                            .modified_at
                            .to_rfc3339(),
                    })
                });
        let unavailable = || SpeleoFileAccess {
            project: fallback.clone(),
            endpoint: link.endpoint.clone(),
            verification: SpeleoAccessVerification::Unavailable,
            release_context: release_context.clone(),
        };
        if local_only {
            return Ok(Some(unavailable()));
        }
        let Ok((endpoint, token)) = self.speleodb_active_token() else {
            return Ok(Some(unavailable()));
        };
        if crate::speleodb::auth::normalize_endpoint(&endpoint)
            != crate::speleodb::auth::normalize_endpoint(&link.endpoint)
        {
            return Ok(Some(unavailable()));
        }
        Ok(Some(
            match client::get_project_probe(
                &endpoint,
                &token,
                self.speleodb_insecure_http(),
                &link.project_id,
            ) {
                Ok(project) => SpeleoFileAccess {
                    project: SpeleoProjectSummary::from_project(project),
                    endpoint: link.endpoint,
                    verification: SpeleoAccessVerification::Verified,
                    release_context: release_context.clone(),
                },
                Err(_) => unavailable(),
            },
        ))
    }

    /// Switch the current project to read-only. No discard or upload is implicit.
    ///
    /// No guard but the lock-ops span is held across either network call. The holder
    /// probe runs with nothing else held. The clean check and the gate closing then
    /// happen together under `command_lock` + `project`, so no mutation or session
    /// boundary lands between them, and both guards drop before the release POST.
    /// That was the freeze: holding them across the POST made every edit, undo, save
    /// and `get_project_state` - synchronous commands, so on the main thread - wait
    /// up to the release timeout. With the gate already shut, an edit attempted
    /// during the POST is refused as read-only instead of waiting. If the POST
    /// fails, a gate this call closed is reopened for the same session, so a failed
    /// switch still leaves the project editable under the lock it still holds.
    pub fn speleodb_release_current_lock(
        &self,
        expected: Option<crate::speleodb::SpeleoReleaseContext>,
    ) -> Result<(), SpeleoErrorDto> {
        let _ = (&expected,);
        crate::core::stub::not_ported("speleodb_admin::speleodb_release_current_lock")
    }

    /// Caller holds project and excludes command mutations.
    pub(super) fn speleodb_require_clean_project(
        &self,
        project: Option<&crate::models::survey::Project>,
    ) -> Result<(), SpeleoDbError> {
        let _ = (&project,);
        crate::core::stub::not_ported("speleodb_admin::speleodb_require_clean_project")
    }

    /// Bind UI consent to the open project and authenticated backend session.
    pub(super) fn speleodb_check_expected_context(
        &self,
        expected: Option<&crate::speleodb::SpeleoReleaseContext>,
    ) -> Result<(), SpeleoDbError> {
        let Some(expected) = expected else {
            return Err(SpeleoDbError::SessionMoved);
        };
        let (generation, link) = self
            .speleodb_session_snapshot()
            .map_err(|_| SpeleoDbError::SessionMoved)?;
        let (endpoint, _, email) = self.speleodb_active_account()?;
        let project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if project
            .as_ref()
            .is_none_or(|p| p.modified_at.to_rfc3339() != expected.project_modified_at)
            || expected.session_generation != generation.to_string()
            || expected.project_id != link.project_id
            || crate::speleodb::auth::normalize_endpoint(&expected.endpoint)
                != crate::speleodb::auth::normalize_endpoint(&endpoint)
            || crate::speleodb::auth::normalize_endpoint(&link.endpoint)
                != crate::speleodb::auth::normalize_endpoint(&endpoint)
            || !super::speleodb::lock_held_by_me(Some(&expected.user_email), Some(&email))
        {
            return Err(SpeleoDbError::SessionMoved);
        }
        Ok(())
    }
}
