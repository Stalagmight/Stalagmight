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
    /// [Track C] (#187/D-B10/D-B19): allow an `http://` endpoint to a PUBLIC host.
    /// Off by default. Relaxes ONLY the public-http scheme block in
    /// `speleodb::tls::enforce` (Track A #187) - NEVER TLS cert verification
    /// (self-signed / internal CAs stay rejected). Pre-wired for #205 Settings;
    /// D-B19's scheme reject lives in `tls.rs`, not here.
    #[serde(default)]
    pub insecure_http: bool,
    /// [#213] Opt-in: allow "force-edit" - editing a remote project WITHOUT holding
    /// the server write lock (surveying offline / when someone else holds it), behind
    /// a big-red unmergeable-`.stalx` warning. Off by default; #205 Settings owns the
    /// warned toggle. When off, a remote project stays strictly read-only until the
    /// lock is acquired (the only escape being "Save a local copy" / detach).
    #[serde(default)]
    pub allow_force_edit: bool,
    /// How often (seconds) the action shell re-checks the linked project's remote
    /// state (lock holder / remote-ahead / conflict) for its status icon. Default
    /// 60; **clamped up to `MIN_POLL_INTERVAL_SECONDS` on load** so a hand-edited
    /// config below the floor can't spam the server.
    #[serde(default = "default_poll_interval_seconds")]
    pub poll_interval_seconds: u32,
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

/// Minimum interval for remote-state polling.
pub const MIN_POLL_INTERVAL_SECONDS: u32 = 30;

fn default_poll_interval_seconds() -> u32 {
    60
}

impl Default for SpeleoDbConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            email: None,
            token: super::auth::AuthToken::default(),
            preserve_unreadable_file: false,
            insecure_http: false,
            allow_force_edit: false,
            poll_interval_seconds: default_poll_interval_seconds(),
            undo_mode: crate::models::UndoMode::Full,
        }
    }
}

impl SpeleoDbConfig {
    pub fn normalize(&mut self) {
        if self.poll_interval_seconds < MIN_POLL_INTERVAL_SECONDS {
            tracing::warn!(
                requested = self.poll_interval_seconds,
                floor = MIN_POLL_INTERVAL_SECONDS,
                "SpeleoDB poll_interval_seconds below floor; clamping up"
            );
            self.poll_interval_seconds = MIN_POLL_INTERVAL_SECONDS;
        }
    }
}
