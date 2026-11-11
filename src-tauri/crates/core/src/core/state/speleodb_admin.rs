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
        let _lock_ops = self.speleodb_lock_ops_guard();
        self.speleodb_check_expected_context(expected.as_ref())
            .map_err(admin_dto)?;
        let generation = self.speleodb_session_generation();
        let (link, endpoint, token, insecure_http) = self.speleodb_admin_context()?;
        if link.has_unpushed_edits {
            return Err(admin_dto(SpeleoDbError::Io(
                "Save your changes using Save Changes before switching to read-only mode.".into(),
            )));
        }
        {
            let _commands = self
                .command_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.speleodb_require_clean_project(project.as_ref())
                .map_err(admin_dto)?;
        }
        let (_, _, email) = self.speleodb_active_account().map_err(admin_dto)?;
        let holder = self
            .speleodb_lock_holder_checked(&endpoint, &token, insecure_http, &link.project_id)
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
        // Clean check and gate transition under ONE `command_lock` + `project`
        // acquisition: mutations and session boundaries cannot slip between them.
        // Taking the claim out IS shutting the gate (`speleodb_edit_gate`), and a
        // failed POST can put back exactly what was there.
        let (gate_was_open, claim) = {
            let _commands = self
                .command_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if generation != self.speleodb_session_generation()
                || expected.as_ref().is_none_or(|expected| {
                    expected.session_generation != generation.to_string()
                        || project.as_ref().is_none_or(|project| {
                            project.modified_at.to_rfc3339() != expected.project_modified_at
                        })
                })
            {
                return Err(admin_dto(SpeleoDbError::SessionMoved));
            }
            self.speleodb_require_clean_project(project.as_ref())
                .map_err(admin_dto)?;
            let open = self.edit_enabled();
            let claim = self.speleodb_forget_lock_acquired_for(&link.project_id);
            (open, claim)
        };
        if super::speleodb::lock_held_by_me(holder.as_deref(), Some(&email)) {
            if let Err(e) = client::release_lock(&endpoint, &token, insecure_http, &link.project_id)
            {
                // Still ours: give the same session its gate back, exactly as it was
                // before this call. A boundary that landed during the POST owns the
                // gate now, so leave it alone - but the claim goes back either way:
                // it is the only local pointer to a mutex the server still holds,
                // and with the session gone it is what the janitor retries from.
                let _project = self
                    .project
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let authored = claim.as_ref().is_some_and(LockClaim::is_authored);
                self.speleodb_restore_lock_claim(claim);
                if gate_was_open && generation == self.speleodb_session_generation() {
                    // Re-bound through the one way in, as the same kind of claim
                    // it was; never recorded as a flow's fresh acquisition.
                    let _ = self.speleodb_bind_edit_claim(
                        &link.project_id,
                        &link.endpoint,
                        authored,
                        false,
                    );
                }
                return Err(dto_for_error(Endpoint::Release, &e));
            }
        }
        Ok(())
    }

    /// Caller holds project and excludes command mutations.
    pub(super) fn speleodb_require_clean_project(
        &self,
        project: Option<&crate::models::Project>,
    ) -> Result<(), SpeleoDbError> {
        let database = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let db = database.as_ref().ok_or(SpeleoDbError::NoRemoteProject)?;
        let saved = db
            .load_project()
            .map_err(|e| SpeleoDbError::Io(e.to_string()))?;
        let link = db
            .load_speleo_link()
            .map_err(|e| SpeleoDbError::Io(e.to_string()))?;
        if project.is_none_or(|p| p.modified_at != saved.modified_at)
            || link.is_none_or(|link| link.has_unpushed_edits)
        {
            return Err(SpeleoDbError::Io(
                "Save your changes using Save Changes before switching to read-only mode.".into(),
            ));
        }
        Ok(())
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
