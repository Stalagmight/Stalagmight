//! Authentication publication and rejection are backend ownership boundaries.
use super::speleodb::local_dto;
use super::AppState;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{client, SpeleoDbError, SpeleoErrorDto, SpeleoLoginResult};

impl AppState {
    pub(super) fn speleodb_forget(&self) {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Ends a SpeleoDB edit session by construction (`speleodb_edit_gate`): the
        // bump leaves any claim's binding stale and the forgotten account matches no
        // claim, so the gate of an open linked project shuts - while an open LOCAL
        // project, which a dead token surfacing as a 401 on ANY SpeleoDB call (e.g. a
        // background list_projects / refresh) reaches too, stays editable without
        // this having to ask which one is open. The claims themselves survive: each
        // is still the only pointer to a mutex the account may hold, for after the
        // next sign-in.
        self.bump_speleodb_session();
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
    ///
    /// `insecure_http` is the caller's own choice for this login - Settings'
    /// Connect in a development build passes the box on screen, which may differ
    /// from the saved one - and `None` means the saved value. It is what the
    /// credentials are sent under, so a public `http://` endpoint is refused unless
    /// THIS choice allows it (and only a development build lets it: the transport
    /// policy ignores it in a release build). On success it is persisted together
    /// with the endpoint and token: the session and the connection it was opened
    /// over are applied as one. A failed login persists nothing.
    pub fn speleodb_login(
        &self,
        override_endpoint: Option<String>,
        insecure_http: Option<bool>,
        email: Option<String>,
        password: Option<String>,
        token: Option<String>,
    ) -> Result<SpeleoLoginResult, SpeleoErrorDto> {
        let entry_generation = {
            let _project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.speleodb_session_generation()
        };
        let endpoint = self
            .speleodb_resolve_endpoint(override_endpoint.as_deref())
            .ok_or(SpeleoDbError::NoEndpoint)
            .map_err(local_dto)?;
        // [Track A] #187: thread the override into every client call.
        let insecure_http = insecure_http.unwrap_or_else(|| self.speleodb_insecure_http());
        // Refuse an address the transport policy would refuse before anything else
        // reads as a sign-in attempt.
        crate::speleodb::validate_endpoint(&endpoint, insecure_http).map_err(local_dto)?;
        // A connected session is bound to its server: switching servers is a
        // Disconnect (with its own sign-out flow) and a fresh Connect, never a
        // sign-in that silently replaces the session underneath open work.
        let current = self.speleodb_session_status()?;
        if current.logged_in
            && current.endpoint.as_deref().is_some_and(|old| {
                crate::speleodb::auth::normalize_endpoint(old)
                    != crate::speleodb::auth::normalize_endpoint(&endpoint)
            })
        {
            return Err(local_dto(SpeleoDbError::ServerChangeWhileConnected));
        }

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
        if self.speleodb_session_generation() != entry_generation {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }
        // Persist before publishing a successful login. A failed write must be
        // visible, rather than silently losing the session on the next launch.
        let mut config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = config.speleodb.clone();
        config.speleodb.endpoint = Some(endpoint.clone());
        config.speleodb.insecure_http = insecure_http;
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
        // A new identity/token must verify ownership before editing resumes: the
        // bump leaves any claim's edit binding stale, which shuts a linked
        // project's gate (`speleodb_edit_gate`) until Start Editing binds again.
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

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::AppState;

    /// A loopback server answering every request with the SDK-completed `body`.
    fn answering_stub(body: &'static str) -> String {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                }
                let body = crate::core::state::tests_support::sdk_fixture(body);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn saved_insecure_http(state: &AppState) -> bool {
        state.config.lock().expect("config").speleodb.insecure_http
    }

    fn set_saved_insecure_http(state: &AppState, value: bool) {
        state.config.lock().expect("config").speleodb.insecure_http = value;
    }

    /// The refusal message of a password login, or `None` when it went through.
    fn password_login_error(
        state: &AppState,
        endpoint: &str,
        insecure_http: Option<bool>,
    ) -> Option<String> {
        state
            .speleodb_login(
                Some(endpoint.to_owned()),
                insecure_http,
                Some("synthetic@example.com".to_owned()),
                Some("synthetic-password".to_owned()),
                None,
            )
            .err()
            .map(|error| error.message)
    }

    #[test]
    fn log_in_now_refuses_public_http_when_the_on_screen_box_is_unticked() {
        // Saved: insecure HTTP allowed. On screen: unticked. The credentials must
        // not go out over plaintext to a public host on the strength of the saved
        // value, and nothing about the refused login may be saved.
        let state = AppState::new().expect("state");
        set_saved_insecure_http(&state, true);
        let error = password_login_error(&state, "http://8.8.8.8/", Some(false))
            .expect("public http is refused under the on-screen choice");
        assert!(error.contains("HTTPS is required"), "{error}");
        assert!(saved_insecure_http(&state), "a failed login saves nothing");
        assert_ne!(
            state
                .config
                .lock()
                .expect("config")
                .speleodb
                .endpoint
                .as_deref(),
            Some("http://8.8.8.8/"),
        );
    }

    #[test]
    fn a_login_without_a_choice_refuses_public_http_under_the_saved_default() {
        let state = AppState::new().expect("state");
        set_saved_insecure_http(&state, false);
        let error = password_login_error(&state, "http://8.8.8.8/", None)
            .expect("public http is refused by default");
        assert!(error.contains("HTTPS is required"), "{error}");
    }

    #[test]
    fn a_successful_login_saves_the_on_screen_choice_with_the_session() {
        // One server for both rounds: the second round's state restores the first
        // round's saved session, and a connected session is bound to its server.
        let base = answering_stub(r#"{"email":"synthetic@example.com"}"#);
        crate::test_helpers::with_isolated_config(|| {
            for on_screen in [true, false] {
                login_saving_the_choice(&base, on_screen);
            }
        });
    }

    fn login_saving_the_choice(base: &str, on_screen: bool) {
        let state = AppState::new().expect("state");
        set_saved_insecure_http(&state, !on_screen);
        state
            .speleodb_login(
                Some(base.to_owned()),
                Some(on_screen),
                None,
                None,
                Some("<your_token_here>".to_owned()),
            )
            .expect("login");
        let saved = state.config.lock().expect("config").speleodb.clone();
        assert_eq!(saved.insecure_http, on_screen);
        assert_eq!(saved.endpoint.as_deref(), Some(base));
        let reloaded = crate::config::Config::load().expect("reload");
        assert_eq!(
            reloaded.speleodb.insecure_http, on_screen,
            "persisted, not only held in memory"
        );
    }
}
