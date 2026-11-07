//! [#215] SpeleoDB "full Project Settings" admin orchestration: collaborator
//! management (per-user and per-team), in-app lock release, and project delete.
//!
//! Split out of `core/state/speleodb.rs` (which is frozen at the file-size ratchet
//! warn line) into a sibling `impl AppState` block. Every method operates on the
//! OPEN linked project - resolving the endpoint/token/`insecure_http` and the
//! persisted link exactly like the #206 metadata methods - and is permission-gated
//! only in the UI; the server's 401/403 is the real enforcement (an ADMIN-only
//! write returns 403 for a non-admin, surfaced verbatim). No lock is required:
//! permissions/locks/delete are all mutex-free (the edit lock governs only the
//! survey `.stalx`). [#576 RVF-c-C1] The calls that CHANGE the server side carry
//! the project they were made for and read their context through
//! `speleodb_admin_write_context`, which refuses another project's.

use super::speleodb::validate_project_id;
use super::speleodb_lock_release::AfterLockVerb;
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{
    Permission, SpeleoCollaborator, SpeleoCollaborators, SpeleoDbError, SpeleoErrorDto,
    SpeleoTeamGrant, SpeleoTeamOption, SpeleoUserSuggestion,
};

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

    /// [#576 RVF-c-C1] [`Self::speleodb_admin_context`] for a call that CHANGES the
    /// open project's server side (its collaborators, its lock, the project itself):
    /// read in one `command_lock` span with the identity check, so a change made for
    /// another project than the open one is refused with `PROJECT_CHANGED` before
    /// anything is sent. The span ends before the request.
    fn speleodb_admin_write_context(
        &self,
        what: &str,
    ) -> Result<(crate::models::SpeleoLink, String, String, bool), SpeleoErrorDto> {
        let _commands = self.hold_commands();
        self.speleodb_refuse_other_project(what)?;
        self.speleodb_admin_context()
    }

    /// [#215] The linked project's collaborator roster: individual users and teams.
    /// GETs the user + team permission lists, resolves each team UUID to a display
    /// name against the caller's `GET /teams/` (best-effort; falls back to the raw
    /// UUID), and flags the signed-in user's own row (`is_self`) so the FE can
    /// disable self-directed actions. Requires READ_ONLY+ (the server 403s below).
    pub fn speleodb_list_collaborators(&self) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) = self.speleodb_admin_context()?;

        let users_raw = self
            .speleodb_handle_auth(
                client::spec::LIST_PERMISSIONS.category,
                client::list_user_permissions(&endpoint, &token, insecure_http, &link.project_id),
            )
            .map_err(|e| dto_for_error(client::spec::LIST_PERMISSIONS.category, &e))?;

        let teams_raw = self
            .speleodb_handle_auth(
                client::spec::LIST_PERMISSIONS.category,
                client::list_team_permissions(&endpoint, &token, insecure_http, &link.project_id),
            )
            .map_err(|e| dto_for_error(client::spec::LIST_PERMISSIONS.category, &e))?;

        // Best-effort team name map: a failure to list teams must not sink the whole
        // roster - each unresolved team just displays as its UUID (still removable).
        let team_names: std::collections::HashMap<String, String> =
            client::list_teams(&endpoint, &token, insecure_http)
                .map(|teams| teams.into_iter().map(|t| (t.id, t.name)).collect())
                .unwrap_or_default();

        // Case-insensitive self-match (SpeleoDB emails are compared case-folded).
        let self_email = self
            .speleodb_session_status()
            .ok()
            .and_then(|s| s.user_email)
            .map(|e| e.to_ascii_lowercase());

        let users = users_raw
            .into_iter()
            .map(|p| {
                let is_self = self_email.as_deref() == Some(p.user.to_ascii_lowercase().as_str());
                SpeleoCollaborator {
                    email: p.user,
                    permission: p.level,
                    is_self,
                }
            })
            .collect();

        let teams = teams_raw
            .into_iter()
            .map(|p| SpeleoTeamGrant {
                team_name: team_names
                    .get(&p.team)
                    .cloned()
                    .unwrap_or_else(|| p.team.clone()),
                team_id: p.team,
                permission: p.level,
            })
            .collect();

        Ok(SpeleoCollaborators { users, teams })
    }

    /// [#215] Grant `email` a permission `level` on the linked project (ADMIN).
    /// Returns the refreshed roster (server truth - a re-POST reactivates a
    /// soft-deleted grant, so re-reading is the honest reflection).
    pub fn speleodb_add_collaborator(
        &self,
        email: String,
        permission: Permission,
    ) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Adding a collaborator")?;
        self.speleodb_handle_auth(
            client::spec::MUTATE_PERMISSION.category,
            client::add_user_permission(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &email,
                permission,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::MUTATE_PERMISSION.category, &e))?;
        self.speleodb_list_collaborators()
    }

    /// [#215] Change an existing collaborator's permission level (ADMIN).
    pub fn speleodb_set_collaborator(
        &self,
        email: String,
        permission: Permission,
    ) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Changing a collaborator's access")?;
        self.speleodb_handle_auth(
            client::spec::MUTATE_PERMISSION.category,
            client::set_user_permission(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &email,
                permission,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::MUTATE_PERMISSION.category, &e))?;
        self.speleodb_list_collaborators()
    }

    /// [#215] Remove a collaborator (soft-delete server-side, ADMIN).
    pub fn speleodb_remove_collaborator(
        &self,
        email: String,
    ) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Removing a collaborator")?;
        self.speleodb_handle_auth(
            client::spec::MUTATE_PERMISSION.category,
            client::remove_user_permission(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &email,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::MUTATE_PERMISSION.category, &e))?;
        self.speleodb_list_collaborators()
    }

    /// [#215] Grant a team a permission `level` on the linked project (ADMIN). The
    /// server rejects `ADMIN` for a team; the FE omits it from the team picker.
    pub fn speleodb_add_team(
        &self,
        team_id: String,
        permission: Permission,
    ) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Adding a team")?;
        self.speleodb_handle_auth(
            client::spec::MUTATE_PERMISSION.category,
            client::add_team_permission(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &team_id,
                permission,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::MUTATE_PERMISSION.category, &e))?;
        self.speleodb_list_collaborators()
    }

    /// [#215] Change a team's permission level (ADMIN).
    pub fn speleodb_set_team(
        &self,
        team_id: String,
        permission: Permission,
    ) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Changing a team's access")?;
        self.speleodb_handle_auth(
            client::spec::MUTATE_PERMISSION.category,
            client::set_team_permission(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &team_id,
                permission,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::MUTATE_PERMISSION.category, &e))?;
        self.speleodb_list_collaborators()
    }

    /// [#215] Revoke a team's access (soft-delete server-side, ADMIN).
    pub fn speleodb_remove_team(
        &self,
        team_id: String,
    ) -> Result<SpeleoCollaborators, SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Removing a team")?;
        self.speleodb_handle_auth(
            client::spec::MUTATE_PERMISSION.category,
            client::remove_team_permission(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &team_id,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::MUTATE_PERMISSION.category, &e))?;
        self.speleodb_list_collaborators()
    }

    /// [#215] Autocomplete users by email/name substring for the add-collaborator
    /// email typeahead. Returns an empty list for a query below the server's 3-char
    /// minimum, so the FE can call on each keystroke without provoking a 400. Needs
    /// only the session token - not an open project - but it is only reachable from
    /// the admin tab, so it resolves the same endpoint/token as the other methods.
    pub fn speleodb_search_users(
        &self,
        query: String,
    ) -> Result<Vec<SpeleoUserSuggestion>, SpeleoErrorDto> {
        let q = query.trim();
        if q.chars().count() < 3 {
            return Ok(Vec::new());
        }
        let (endpoint, token) = self.speleodb_active_token().map_err(admin_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        // `/user/autocomplete/` is authenticated-only with no per-object permission,
        // so (like the whoami chokepoint) a 403 there is the dead-token signal -
        // its row maps through `User`, which makes that ReLogin - the correct action.
        let users = self
            .speleodb_handle_auth(
                client::spec::AUTOCOMPLETE.category,
                client::list_user_autocomplete(&endpoint, &token, insecure_http, q),
            )
            .map_err(|e| dto_for_error(client::spec::AUTOCOMPLETE.category, &e))?;
        Ok(users
            .into_iter()
            .map(|u| SpeleoUserSuggestion {
                email: u.email,
                label: u.label,
            })
            .collect())
    }

    /// [#215] The teams the signed-in user belongs to, for the add-team picker.
    pub fn speleodb_list_teams(&self) -> Result<Vec<SpeleoTeamOption>, SpeleoErrorDto> {
        let (_link, endpoint, token, insecure_http) = self.speleodb_admin_context()?;
        let teams = self
            .speleodb_handle_auth(
                client::spec::LIST_TEAMS.category,
                client::list_teams(&endpoint, &token, insecure_http),
            )
            .map_err(|e| dto_for_error(client::spec::LIST_TEAMS.category, &e))?;
        Ok(teams
            .into_iter()
            .map(|t| SpeleoTeamOption {
                id: t.id,
                name: t.name,
            })
            .collect())
    }

    /// [#215] Release the lock on the OPEN linked project. The server releases the
    /// caller's own lock, or - for an ADMIN - force-releases another user's held
    /// lock (single endpoint; role decided server-side). A recovery action for a
    /// stuck lock: it does NOT push, so the FE warns when the caller holds it. The
    /// FE re-probes link status afterward to reflect the freed lock.
    ///
    /// Dropping our own edit gate is part of the release, not an afterthought. The
    /// gate is what `guard_mutation` reads, and the status poll only ever OPENS it,
    /// so leaving it set here left the shell editable over a lock that no longer
    /// exists - an unflagged lockless edit session, which is precisely the state
    /// `force_editing` exists to make visible. Its two siblings
    /// (`speleodb_release_lock` / `speleodb_release_all_locks`) already do this.
    /// Only when the lock was OURS: an ADMIN force-releasing someone else's lock
    /// never held the gate, and clearing it would drop them out of an unrelated
    /// edit session of their own, and a #213 force-editor never held a lock at all.
    ///
    /// That verdict is reached AFTER the round trip, over the link that is open
    /// then, exactly as `speleodb_release_lock` does it. Sampling it beforehand and
    /// applying it afterwards spanned a blocking network call with nothing held
    /// across it: dismiss the dialog and unlink the same project in that window and
    /// the unlink reopens the gate for what is now a plain local project, which this
    /// completion then closed again. Nothing reopens it from there - status polling
    /// needs a link the project no longer has - so it stayed read-only until it was
    /// closed and reopened.
    pub fn speleodb_release_current_lock(&self) -> Result<(), SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Releasing the lock")?;
        if let Err(e) = self.speleodb_handle_auth(
            client::spec::RELEASE.category,
            client::release_lock(&endpoint, &token, insecure_http, &link.project_id),
        ) {
            // [#576 FZ-SDB-6-D1] A lost reply: the server may have released it anyway,
            // so ask who holds it now. Free, or another account's while we were
            // editing under our own lock: the release landed, and the gate follows
            // below exactly as on a confirmed release. Still ours, the server
            // answered, or the re-read failed: the release is reported as failed,
            // and the gate and claim stay for a retry (or the next poll).
            let was_ours = self.edit_enabled()
                && !self.force_editing()
                && self
                    .speleodb_open_link()
                    .is_ok_and(|open| open.project_id == link.project_id);
            match self.speleodb_reread_after_lock_verb(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &e,
            ) {
                AfterLockVerb::NotOurs(None) => {}
                AfterLockVerb::NotOurs(Some(_)) if was_ours => {}
                _ => return Err(dto_for_error(client::spec::RELEASE.category, &e)),
            }
            tracing::info!(
                project_id = %link.project_id,
                error = %e,
                purpose = "follow a release whose reply was lost (#576 FZ-SDB-6)",
                "speleodb release: the reply was lost, but the lock is no longer ours"
            );
        }
        if self.edit_enabled()
            && !self.force_editing()
            && self
                .speleodb_open_link()
                .is_ok_and(|open| open.project_id == link.project_id)
        {
            self.set_edit_enabled(false);
        }
        Ok(())
    }

    /// [#215] Delete the OPEN linked project on SpeleoDB (soft-delete, ADMIN), then
    /// drop the local link so the app stops treating it as remote. The server delete
    /// does not require releasing the lock first; the subsequent unlink's best-effort
    /// lock release just no-ops against the now-deleted project. The FE closes the
    /// Settings dialog and navigates away on success.
    ///
    /// [#576 NF-AN3-3] A project the server already deleted (a first DELETE whose
    /// reply was lost) answers 404, which the client reports as done - so the copy
    /// is unlinked here instead of staying linked to a project that is gone.
    ///
    /// [#576 RV4-R-S4-D2] The client is handed the copy's server (`link.endpoint`)
    /// beside the session's and refuses before sending anything when they differ:
    /// another server's answer, 404 or 2xx, says nothing about this project. The
    /// refusal returns here before the unlink, so the copy stays linked.
    pub fn speleodb_delete_project(&self) -> Result<(), SpeleoErrorDto> {
        let (link, endpoint, token, insecure_http) =
            self.speleodb_admin_write_context("Deleting the project on SpeleoDB")?;
        self.speleodb_handle_auth(
            client::spec::DELETE_PROJECT.category,
            client::delete_project(
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &link.endpoint,
            ),
        )
        .map_err(|e| dto_for_error(client::spec::DELETE_PROJECT.category, &e))?;
        // Now local-only: drop the persisted link + reopen the edit gate, in place
        // (the project stays open where it is; see `speleodb_unlink_in_place`).
        self.speleodb_unlink_in_place()
    }
}
