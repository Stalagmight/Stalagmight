//! Backend-only token cache. Persistence belongs to speleodb/speleodb.conf.

/// Token contents are omitted from debug output and IPC.
#[derive(Clone, Default)]
pub struct AuthToken(pub Option<String>);

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthToken([redacted])")
    }
}

/// Stable identity for a server and account.
pub fn account_key(endpoint: &str, email: &str) -> String {
    format!("{}|{email}", normalize_endpoint(endpoint))
}

pub fn normalize_endpoint(endpoint: &str) -> String {
    // The same cleaning every entry point applies, so a pasted spelling with
    // invisible characters around it is the same server identity as the clean one.
    let cleaned = super::clean_endpoint_input(endpoint).unwrap_or_default();
    let trimmed = cleaned.as_str();
    if let Ok(uri) = trimmed.parse::<http::Uri>() {
        if let Some(host) = uri.host() {
            let scheme = uri.scheme_str().unwrap_or("https").to_ascii_lowercase();
            let host = host.to_ascii_lowercase();
            // Drop the port when it is the scheme default so `:443`/`:80` and the
            // implicit form collapse together.
            let default_port = match scheme.as_str() {
                "http" => Some(80),
                "https" => Some(443),
                _ => None,
            };
            return match uri.port_u16() {
                Some(port) if Some(port) != default_port => format!("{scheme}://{host}:{port}"),
                _ => format!("{scheme}://{host}"),
            };
        }
    }
    // Bare host / unparseable: keep the spike's lightweight normalization.
    trimmed.trim_end_matches('/').to_ascii_lowercase()
}

#[derive(Debug, Default)]
pub struct TokenStore {
    token: AuthToken,
}

impl TokenStore {
    pub fn load(&mut self, _endpoint: &str, _email: &str) -> Option<String> {
        self.token.0.clone()
    }

    pub fn store(&mut self, _endpoint: &str, _email: &str, token: &str) {
        self.token.0 = Some(token.to_owned());
    }

    pub fn clear(&mut self) {
        self.token.0 = None;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_in_memory_token(token: &str) -> Self {
        Self {
            token: AuthToken(Some(token.to_owned())),
        }
    }
}
