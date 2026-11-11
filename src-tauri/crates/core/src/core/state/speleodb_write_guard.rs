//! Fresh server ownership checks for remote writes. Local offline editing uses
//! the acquired session gate; uploads and metadata writes require connectivity.
use super::speleodb::{local_dto, lock_held_by_me};
use super::AppState;
use crate::models::SpeleoLink;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{client, Permission, SpeleoDbError, SpeleoErrorDto};

impl AppState {
    #[cfg(test)]
    pub(super) fn speleodb_require_owned_lock(
        &self,
        link: &SpeleoLink,
    ) -> Result<(String, String), SpeleoErrorDto> {
        let _lock_ops = self.speleodb_lock_ops_guard();
        self.speleodb_require_owned_lock_locked(link)
    }

    /// The caller holds lock-ops through the mutation as well as this probe.
    pub(super) fn speleodb_require_owned_lock_locked(
        &self,
        link: &SpeleoLink,
    ) -> Result<(String, String), SpeleoErrorDto> {
        let (generation, open) = self.speleodb_session_snapshot()?;
        if open.project_id != link.project_id
            || crate::speleodb::auth::normalize_endpoint(&open.endpoint)
                != crate::speleodb::auth::normalize_endpoint(&link.endpoint)
        {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }
        if !self.edit_enabled() {
            return Err(local_dto(SpeleoDbError::Io(
                "acquire the editing lock before changing this project".to_owned(),
            )));
        }
        let (endpoint, token, email) = self.speleodb_active_account().map_err(local_dto)?;
        if crate::speleodb::auth::normalize_endpoint(&endpoint)
            != crate::speleodb::auth::normalize_endpoint(&link.endpoint)
        {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }
        let project = self
            .speleodb_handle_auth(
                Endpoint::GetProject,
                client::get_project(
                    &endpoint,
                    &token,
                    self.speleodb_insecure_http(),
                    &link.project_id,
                ),
            )
            .map_err(|error| {
                if matches!(
                    &error,
                    SpeleoDbError::Status {
                        status: 401 | 403,
                        ..
                    }
                ) {
                    self.speleodb_disable_editing_if_current(generation);
                }
                dto_for_error(Endpoint::GetProject, &error)
            })?;
        // Session boundaries take this mutex. The final ownership check and a
        // possible gate close must be atomic against a replacement project.
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (now_endpoint, now_token) = self.speleodb_active_token().map_err(local_dto)?;
        if self.speleodb_session_generation() != generation
            || now_endpoint != endpoint
            || now_token != token
            || !self.edit_enabled()
        {
            return Err(local_dto(SpeleoDbError::Io(
                "the project or account changed; retry in the current session".to_owned(),
            )));
        }
        let holder = project
            .active_mutex
            .as_ref()
            .map(|mutex| mutex.user.as_str());
        if !project
            .permission
            .is_some_and(|permission| permission >= Permission::ReadAndWrite)
            || !lock_held_by_me(holder, Some(email.as_str()))
        {
            self.speleodb_shut_edit_gate();
            return Err(local_dto(SpeleoDbError::Io(
                "write permission or the editing lock was lost; resume editing before uploading"
                    .to_owned(),
            )));
        }
        // Callers must use these verified credentials, rather than reading a
        // replacement account after this probe has completed.
        Ok((endpoint, token))
    }

    /// A stale status response must never close a successor session's gate.
    pub(super) fn speleodb_disable_editing_if_current(&self, generation: u64) {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.speleodb_session_generation() == generation {
            self.speleodb_shut_edit_gate();
        }
    }
}
