//! Authentication publication and rejection are backend ownership boundaries.
use super::speleodb::local_dto;
use super::AppState;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{client, SpeleoDbError, SpeleoErrorDto, SpeleoLoginResult};

impl AppState {
    /// Full local token clear (file + memory) + session reset -
    /// the shared sink for the C5 auto-drop paths. Also drops the transient edit
    /// gate AND the #213 force-edit flag: `guard_mutation` reads only
    /// `edit_enabled`, so leaving it open after the token is gone would let the user
    /// keep editing a remote project that can no longer be pushed. The server lock
    /// is intentionally NOT released here - a 401 may be transient, and surrendering
    /// the single-writer claim on a flaky blip is worse than holding it; a re-login
    /// reconciles. (This is the AppState auto-drop sink; `Session::forget` is left
    /// untouched so the login switch-account path does not spuriously close the
    /// gate.)
    pub(super) fn speleodb_forget(&self) {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.bump_speleodb_session();
        // Drop the transient edit gate for a remote edit session. A dead token
        // surfaces as a 401 on ANY SpeleoDB call (e.g. a background list_projects /
        // refresh), and this is the auto-drop sink for all of them - so an
        // unconditional drop would silently turn an open LOCAL project read-only.
        // Mirror speleodb_logout's remote-gated, fail-SAFE drop exactly (#197
        // review): only a confirmed local-only project (NoRemoteProject) keeps the
        // gate; an undeterminable load (e.g. a poisoned database Mutex) fails safe to
        // read-only rather than leaving a token-less session able to edit.
        if self.edit_enabled() {
            match self.speleodb_open_link() {
                Ok(_) => self.set_edit_enabled(false),
                Err(SpeleoDbError::NoRemoteProject) => {}
                Err(_) => self.set_edit_enabled(false),
            }
        }
        // #213: and the force-edit flag with it. Unconditional, unlike the gate
        // above, because it can only ever be set on a remote force-edit session
        // (`speleodb_force_edit` requires an open remote link) - so there is no local
        // project for it to affect, and a dead token ends that session by definition.
        //
        // Left set, it OUTLIVED the gate it rides on: `speleodb_link_status` reports
        // it on every poll, so the frontend re-seeded `forceEditing: true` right after
        // clearing it, and its big-red force-edit banner then pre-empted the durable
        // "your changes are only on this computer" push-failure banner - hiding the
        // Retry and the typed commit message behind a banner whose only control
        // (Reconnect) 401s again, while `guard_mutation` refused every edit. It
        // self-healed only on close, or on a start_editing / unlink / restore that a
        // dead token cannot reach.
        self.set_force_editing(false);
        if let Ok(mut session) = self.speleodb_session_lock() {
            session.forget();
        }
        if let Err(error) = self.speleodb_clear_saved_token() {
            tracing::warn!(error = %error.message, "Failed to remove rejected SpeleoDB token");
        }
    }

    /// Log in: exchange email+password for a token (or accept a pasted `token`),
    /// store it backend-side, and record the session. The token never crosses
    /// back to the webview - only the account email and instance.
    pub fn speleodb_login(
        &self,
        override_endpoint: Option<String>,
        email: Option<String>,
        password: Option<String>,
        token: Option<String>,
    ) -> Result<SpeleoLoginResult, SpeleoErrorDto> {
        let endpoint = self
            .speleodb_resolve_endpoint(override_endpoint.as_deref())
            .ok_or(SpeleoDbError::NoEndpoint)
            .map_err(local_dto)?;
        // [Track A] #187: read the override and thread it into every client call.
        let insecure_http = self.speleodb_insecure_http();

        let (token, email) =
            if let Some(t) = token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
                let email = client::fetch_user_email(&endpoint, &t, insecure_http)
                    .map_err(|e| dto_for_error(Endpoint::User, &e))?;
                (t, email)
            } else {
                let email = email.map(|e| e.trim().to_owned()).unwrap_or_default();
                let password = password.unwrap_or_default();
                // Reject blank credentials at the boundary rather than POSTing them
                // and relying on the server to 400.
                if email.is_empty() || password.is_empty() {
                    return Err(local_dto(SpeleoDbError::MissingCredentials));
                }
                let token = client::login(&endpoint, insecure_http, &email, &password)
                    .map_err(|e| dto_for_error(Endpoint::Login, &e))?;
                (token, email)
            };

        let _lock_ops = self.speleodb_lock_ops_guard();
        // Match cache-install lock ordering; never acquire project under config.
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Persist before publishing a successful login. A failed write must be
        // visible, rather than silently losing the session on the next launch.
        let mut config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = config.speleodb.clone();
        config.speleodb.endpoint = Some(endpoint.clone());
        config.speleodb.email = Some(email.clone());
        config.speleodb.token.0 = Some(token.clone());
        config.speleodb.preserve_unreadable_file = false;
        if config.save_speleodb().is_err() {
            config.speleodb = previous;
            return Err(local_dto(SpeleoDbError::Io(
                "Could not save SpeleoDB configuration".to_owned(),
            )));
        }
        let mut session = self.speleodb_session_lock().map_err(local_dto)?;
        self.bump_speleodb_session();
        session.token_store.store(&endpoint, &email, &token);
        session.endpoint = Some(endpoint.clone());
        session.email = Some(email.clone());
        session.rehydrate_attempted = true;
        Ok(SpeleoLoginResult {
            user_email: email,
            endpoint,
        })
    }
}
