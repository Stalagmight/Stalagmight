//! Release the open linked project's write lock through its authenticated session.

use super::speleodb::validate_project_id;
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

    /// Explicit recovery may release another holder's lock for an administrator.
    /// Automatic close/quit sets `own_only`, which checks current ownership first.
    pub fn speleodb_release_current_lock(
        &self,
        own_only: bool,
        allow_unpushed: bool,
        expected: Option<crate::speleodb::SpeleoReleaseContext>,
    ) -> Result<(), SpeleoErrorDto> {
        let _lock_ops = self.speleodb_lock_ops_guard();
        let generation = self.speleodb_session_generation();
        let (link, endpoint, token, insecure_http) = self.speleodb_admin_context()?;
        if let Some(expected) = &expected {
            let (_, _, email) = self.speleodb_active_account().map_err(admin_dto)?;
            if expected.session_generation != generation.to_string()
                || expected.project_id != link.project_id
                || crate::speleodb::auth::normalize_endpoint(&expected.endpoint)
                    != crate::speleodb::auth::normalize_endpoint(&endpoint)
                || !super::speleodb::lock_held_by_me(Some(&expected.user_email), Some(&email))
            {
                return Err(admin_dto(SpeleoDbError::SessionMoved));
            }
        }
        if link.has_unpushed_edits && (!allow_unpushed || expected.is_none()) {
            return Err(admin_dto(SpeleoDbError::Parse(
                "Unpushed edits require explicit release confirmation for this project and account"
                    .into(),
            )));
        }
        if crate::speleodb::auth::normalize_endpoint(&endpoint)
            != crate::speleodb::auth::normalize_endpoint(&link.endpoint)
        {
            return Err(admin_dto(SpeleoDbError::SessionMoved));
        }
        let should_release = if own_only {
            let (_, _, email) = self.speleodb_active_account().map_err(admin_dto)?;
            let holder = self
                .speleodb_lock_holder_checked(&endpoint, &token, insecure_http, &link.project_id)
                .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
            super::speleodb::lock_held_by_me(holder.as_deref(), Some(&email))
        } else {
            true
        };
        if generation != self.speleodb_session_generation() {
            return Err(admin_dto(SpeleoDbError::SessionMoved));
        }
        if should_release {
            self.speleodb_handle_auth(
                Endpoint::Release,
                client::release_lock(&endpoint, &token, insecure_http, &link.project_id),
            )
            .map_err(|e| dto_for_error(Endpoint::Release, &e))?;
        }
        if self.edit_enabled()
            && !self.force_editing()
            && self.speleodb_open_link().is_ok_and(|open| {
                open.project_id == link.project_id && open.endpoint == link.endpoint
            })
        {
            self.set_edit_enabled(false);
        }
        self.speleodb_forget_lock_acquired_for(&link.project_id);
        Ok(())
    }
}
