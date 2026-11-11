//! SpeleoDB settings. Persisted with the backend-only token in speleodb.conf.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SpeleoDbConfig {
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Backend-only token. Excluded from IPC and from ordinary config serialization.
    #[serde(skip)]
    #[ts(skip)]
    pub token: super::auth::AuthToken,
    /// A failed auth-file load must survive unrelated preference saves.
    #[serde(skip)]
    #[ts(skip)]
    pub preserve_unreadable_file: bool,
    /// Allow an `http://` endpoint outside the automatic exceptions (`localhost`,
    /// `127.0.0.1`, `192.168.x.x`; see `speleodb::tls`). Off by default. Relaxes
    /// ONLY the http scheme rule in `speleodb::tls::validate_endpoint` - NEVER TLS cert
    /// verification (self-signed / internal CAs stay rejected). A development-build
    /// option: kept in the schema so existing configs load (in every build) and so
    /// a development build can offer it; a release build offers no control for it
    /// and the policy treats it as `false` (`tls::INSECURE_HTTP_HONOURED`).
    #[serde(default)]
    pub insecure_http: bool,
    /// [#303] Default undo policy for SpeleoDB-linked projects, under any
    /// per-project `speleo_link.undo_mode` override. Default
    /// [`UndoMode::Full`] - the hybrid stack the issue exists to deliver.
    ///
    /// This is the GLOBAL half of the two-level setting: it seeds projects that
    /// carry no override, so switching it changes every such project at once. It
    /// says nothing about local-only projects, which always keep the fixed window.
    #[serde(default)]
    pub undo_mode: crate::models::UndoMode,
}

impl Default for SpeleoDbConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            email: None,
            token: super::auth::AuthToken::default(),
            preserve_unreadable_file: false,
            insecure_http: false,
            undo_mode: crate::models::UndoMode::Full,
        }
    }
}
