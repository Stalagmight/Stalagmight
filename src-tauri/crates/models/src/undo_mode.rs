use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Whether a SpeleoDB-linked project's undo history rides to the server (#303).
///
/// A linked project is a binary `.stalx` committed to a git repo, so what its undo
/// stack holds at push time is what every commit stores forever. The two answers
/// are a project-level policy, not a per-machine preference:
///
/// - [`Self::Full`] - the hybrid stack. The project keeps its full edit history
///   since the last commit (no `undo_history_limit` / `undo_persist_limit` window,
///   bounded instead by a byte budget) and that history rides in the pushed
///   snapshot, so undo depth chains backwards commit by commit.
/// - [`Self::Disabled`] - undo works locally, under the ordinary fixed window, and
///   is STRIPPED from the pushed snapshot. For projects big enough that shipping
///   whole-project undo snapshots in every commit is not worth the repo growth.
///
/// A legacy NULL project value is initialized once from the global default. The
/// explicit project value is authoritative after that. Local-only projects ignore
/// this entirely - they have no commit to chain through, so the fixed window is the
/// only policy that means anything there.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/")]
pub enum UndoMode {
    /// Full depth since the last commit, pushed with it.
    #[default]
    Full,
    /// Undo stays local; the pushed snapshot carries none of it.
    Disabled,
    /// Catch-all for a value this build does not know; normalized to `Full`.
    ///
    /// Same `#[serde(other)]` idiom as `ThemeMode`: a
    /// hand-edited config, or one written by a NEWER build, must not stop the file
    /// loading. It is normalized rather than honoured because the safe default for
    /// an unreadable policy is the documented one, and because every consumer asks
    /// [`Self::resolved`] rather than matching this variant.
    #[serde(other, rename = "unknown")]
    Unknown,
}

impl UndoMode {
    /// The mode with the unknown sentinel collapsed to the default. Every consumer
    /// reads this, so `Unknown` can never reach a branch that decides what gets
    /// pushed.
    #[must_use]
    pub fn resolved(self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Full | Self::Unknown => Self::Full,
        }
    }

    /// True when a pushed snapshot may carry the undo history.
    #[must_use]
    pub fn pushes_history(self) -> bool {
        matches!(self.resolved(), Self::Full)
    }

    /// The `speleo_link.undo_mode` column spelling (schema v16). `None` in the
    /// column is legacy/uninitialized and is materialized on open.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self.resolved() {
            Self::Disabled => "disabled",
            // `Unknown` is resolved away above, so this arm is Full only.
            Self::Full | Self::Unknown => "full",
        }
    }

    /// Parse the column back. An unrecognized stored value fails closed to
    /// `Disabled`; NULL is handled separately as legacy initialization.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "disabled" => Self::Disabled,
            "full" => Self::Full,
            _ => Self::Disabled,
        }
    }
}

/// The effective undo policy for the OPEN project - what every trim / strip /
/// clear site asks instead of re-deriving it from the link and the config.
///
/// `linked` is deliberately separate from `mode`: the hybrid rules apply only to a
/// SpeleoDB-linked project (q4), so a local project reads `linked: false` and keeps
/// the fixed window whatever `mode` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndoPolicy {
    /// The open project has a `speleo_link` row.
    pub linked: bool,
    /// The resolved mode (explicit project value, or one-time legacy fallback).
    pub mode: UndoMode,
}

impl UndoPolicy {
    /// A local-only project: fixed window, nothing pushed.
    #[must_use]
    pub fn local() -> Self {
        Self {
            linked: false,
            mode: UndoMode::Full,
        }
    }

    /// True when the count limits (`undo_history_limit` / `undo_persist_limit`) are
    /// bypassed in favour of the byte budget - Full mode on a linked project only.
    #[must_use]
    pub fn unlimited_depth(self) -> bool {
        self.linked && self.mode.pushes_history()
    }

    /// True when a push snapshot of this project may keep its `undo_stack`.
    #[must_use]
    pub fn pushes_history(self) -> bool {
        self.linked && self.mode.pushes_history()
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::*;

    #[test]
    fn unknown_resolves_to_full() {
        assert_eq!(UndoMode::Unknown.resolved(), UndoMode::Full);
        assert!(UndoMode::Unknown.pushes_history());
        assert_eq!(UndoMode::Unknown.as_db_str(), "full");
    }

    #[test]
    fn db_string_round_trips_and_tolerates_junk() {
        for mode in [UndoMode::Full, UndoMode::Disabled] {
            assert_eq!(UndoMode::from_db_str(mode.as_db_str()), mode);
        }
        // Case / whitespace tolerance, and a malformed stored value that must not
        // turn uncertainty into permission to publish history.
        assert_eq!(UndoMode::from_db_str(" DISABLED "), UndoMode::Disabled);
        assert_eq!(UndoMode::from_db_str("wat"), UndoMode::Disabled);
        assert!(!UndoMode::from_db_str("wat").pushes_history());
    }

    #[test]
    fn serde_uses_snake_case_and_the_other_sentinel() {
        let json = serde_json::to_string(&UndoMode::Disabled).expect("serialize");
        assert_eq!(json, "\"disabled\"");
        let parsed: UndoMode = serde_json::from_str("\"nonsense\"").expect("unknown sentinel");
        assert_eq!(parsed, UndoMode::Unknown);
    }

    #[test]
    fn a_local_project_keeps_the_fixed_window_whatever_the_mode_says() {
        // q4: the hybrid rules are gated on the LINK, not on the mode. A local
        // project has no commit to chain through, so Full must not unbound it.
        let local = UndoPolicy::local();
        assert!(!local.unlimited_depth());
        assert!(!local.pushes_history());

        let local_full = UndoPolicy {
            linked: false,
            mode: UndoMode::Full,
        };
        assert!(!local_full.unlimited_depth());
        assert!(!local_full.pushes_history());
    }

    #[test]
    fn linked_full_is_the_only_unlimited_shape() {
        let full = UndoPolicy {
            linked: true,
            mode: UndoMode::Full,
        };
        assert!(full.unlimited_depth());
        assert!(full.pushes_history());

        let disabled = UndoPolicy {
            linked: true,
            mode: UndoMode::Disabled,
        };
        assert!(
            !disabled.unlimited_depth(),
            "Disabled keeps the count window"
        );
        assert!(!disabled.pushes_history());
    }
}
