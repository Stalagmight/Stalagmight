//! What a fuzz run is made of: the steps (an app verb, a second-client action or
//! an interleaving of two app verbs), the faults armed on the proxy, and the
//! deterministic RNG the generator ([`super::gen`]) draws from.
//!
//! A trace is the list of steps as EXECUTED, so a replay or a shrink re-runs the
//! same list against a fresh world. Parameters that depend on the world (project
//! ids, commit shas) are recorded as indices into the world, never as values.

use serde::{Deserialize, Serialize};

/// SplitMix64: small, fast, and identical on every platform, so a seed names the
/// same run everywhere.
#[derive(Clone)]
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`0` when `n == 0`).
    pub(super) fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    pub(super) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// Pick from `(weight, item)` pairs; `None` when every weight is zero.
    pub(super) fn weighted<T: Clone>(&mut self, items: &[(u32, T)]) -> Option<T> {
        let total: u64 = items.iter().map(|(w, _)| u64::from(*w)).sum();
        let mut roll = self.below(total);
        for (w, item) in items {
            let w = u64::from(*w);
            if roll < w {
                return Some(item.clone());
            }
            roll -= w;
        }
        None
    }
}

/// An account of the run, as a step names it. `Owner` is the app's usual account,
/// `Collab` the second account, `Ro` the optional read-only account, `Admin` the
/// superuser (second client only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) enum Acct {
    Owner,
    Collab,
    Ro,
    Admin,
}

/// One step's verb. `p` indexes the run's projects (modulo their count).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum Act {
    // ---- the app (through the fault proxy), as the frontend sequences it ----
    /// Sign in from Settings as `acct` (an account switch when another is signed in).
    Login {
        acct: Acct,
    },
    /// Sign in to ANOTHER SpeleoDB deployment (the in-process stub).
    LoginOther,
    Logout,
    ListProjects,
    /// Open from the SpeleoDB browser; `take_server` answers the un-pushed-local
    /// prompt with "Take the server copy" instead of Cancel.
    Open {
        p: u8,
        take_server: bool,
    },
    /// Open the project's cached `.stalx` through the file-open path.
    OpenFile {
        p: u8,
    },
    /// Create a synthetic local project file and open it.
    NewLocal,
    /// Resume editing (Start Editing).
    StartEditing,
    /// Add a section named by a fresh edit token.
    Edit,
    /// Add a shot to the newest fuzz section.
    EditShot,
    /// The autosave timer: a local save only.
    Autosave,
    /// Save Changes: local save, upload, keep the lock. `long_message` sends a
    /// commit message longer than the server's 1024-character column.
    SaveChanges {
        long_message: bool,
    },
    /// Save and Close: local save, upload and release, then close when clean.
    SaveAndClose,
    /// Close Project (unsaved work: "Discard changes").
    Close,
    /// Quit the app as the window close and the exit hook run it, then relaunch.
    Quit,
    /// Kill the app: no save, no release, then relaunch. `residue` leaves a hot
    /// rollback journal next to the open cache file, as a kill mid-save does.
    Crash {
        residue: bool,
    },
    /// Restart-to-update: the process restarts without the exit hook's release.
    RestartUpdate,
    /// Switch to Read Only Mode.
    ReadOnly,
    /// Reload Project (pending work: the user confirms the discard).
    Reload,
    /// A status refresh (`speleodb_link_status`).
    Refresh,
    /// The divergence dialog's Take remote.
    TakeRemote,
    /// Discard changes and keep editing (the unwound-push dialog).
    DiscardKeep,
    /// History View: `back` commits behind the tip (modulo the history).
    View {
        back: u8,
    },
    /// History Revert: Start Editing, restore, then Done.
    Revert {
        back: u8,
    },
    Unwind,
    Rewind,
    /// The divergence dialog's Save copy: save elsewhere, unlink, open the copy.
    SaveCopy,
    /// Publish the open local project to SpeleoDB and reopen it managed.
    Publish,
    /// A partial config update (`update_config`), as dragging a panel divider does.
    ConfigTouch,
    /// A read of the open project through its mutex (`get_project_state`).
    StateRead,
    // ---- the second client (direct to the server, never through the proxy) ----
    ActorLock {
        who: Acct,
        p: u8,
    },
    ActorRelease {
        who: Acct,
        p: u8,
    },
    /// Push a revision on top of the tip (acquire, upload, release unless
    /// `keep_lock`).
    ActorPush {
        who: Acct,
        p: u8,
        keep_lock: bool,
    },
    /// Push a revision a NEWER Stalagmight saved (one schema version above).
    ActorPushNewer {
        who: Acct,
        p: u8,
    },
    ActorDelete {
        p: u8,
    },
    ActorRename {
        p: u8,
    },
    /// Set the app account's access on a project the OTHER account administers
    /// (0 = remove, 1 = READ_ONLY, 2 = READ_AND_WRITE).
    ActorSetAccess {
        p: u8,
        level: u8,
    },
    // ---- two app verbs at once ----
    /// Run the bare backend verb of `first` on a worker thread with one of its
    /// requests (or replies) held at the proxy, run `second` on another thread,
    /// then let the held request go (or, when `second` ends the process, drop it).
    Interleave {
        first: Box<Act>,
        hold: HoldSpec,
        second: Box<Act>,
    },
}

impl Act {
    /// The second client's steps (they never go through the proxy).
    pub(super) fn is_actor(&self) -> bool {
        matches!(
            self,
            Act::ActorLock { .. }
                | Act::ActorRelease { .. }
                | Act::ActorPush { .. }
                | Act::ActorPushNewer { .. }
                | Act::ActorDelete { .. }
                | Act::ActorRename { .. }
                | Act::ActorSetAccess { .. }
        )
    }

    /// The name `STALAGMIGHT_SPELEODB_FUZZ_SKIP_ACTS` matches: the variant name.
    pub(super) fn name(&self) -> String {
        match serde_json::to_value(self).unwrap_or_default() {
            serde_json::Value::String(s) => s,
            serde_json::Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
            _ => String::new(),
        }
    }
}

/// Which request of an interleaving's first verb the proxy holds, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct HoldSpec {
    pub(super) method: String,
    /// A substring of the request path.
    pub(super) path: String,
    /// `true`: forward, then hold the reply; `false`: hold before forwarding.
    pub(super) reply: bool,
}

/// A fault the proxy applies to the matching request(s) of one app step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Fault {
    /// Answer this status; the server never sees the request.
    Status(u16),
    /// The server applies the request; the client gets this status instead.
    StatusAfter(u16),
    LatencyMs(u64),
    /// Drop the connection mid-upload, before the server sees the request.
    DropBefore,
    /// The server applies the request; the reply is lost.
    DropAfter,
    /// Cut the response body short (mid-download drop).
    Truncate,
    /// Answer half the body with a consistent length (a malformed payload).
    Corrupt,
    /// Answer the body with its tail scrambled, same length.
    Scramble,
    /// Hold the reply past the client's timeout, then let the server apply it.
    HangMs(u64),
    /// A connection-level failure (the server looks unreachable).
    Refuse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FaultSpec {
    pub(super) fault: Fault,
    pub(super) method: Option<String>,
    /// A substring of the request path (never a project id: the world differs per run).
    pub(super) path: String,
    pub(super) times: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Step {
    pub(super) act: Act,
    pub(super) fault: Option<FaultSpec>,
    /// Further faults armed after `fault`, in this order (the first armed rule that
    /// matches a request fires), for a directed step whose act needs two of its
    /// requests to fail. The generator never draws one; step lists saved without it
    /// read as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) then: Vec<FaultSpec>,
}

impl Step {
    pub(super) fn plain(act: Act) -> Self {
        Self {
            act,
            fault: None,
            then: Vec::new(),
        }
    }
}
