//! The app under test: one `AppState`, driven through the SAME backend verbs the
//! IPC commands wrap, in the order the frontend issues them (`projectStore`
//! `loadProject` and its auto-acquire, `useOpenRemoteProject`, `useSpeleoActions`,
//! `SpeleoDoneDialog`, the close / quit / account-transition guards,
//! `publishProjectAndReopen`), minus the webview. Logic that lives in the root
//! crate's command layer (`close_project_impl`, the exit hook's bounded release)
//! is replicated here as it is written there.
//!
//! The flows themselves are in [`super::flows`]; this file holds the process
//! (state, event sink, relaunch) and the step outcome the oracle reads.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::target::Account;
use crate::core::events::EventSink;
use crate::core::state::AppState;
use crate::speleodb::SpeleoErrorDto;

/// A project of the run: its server id and the name the list shows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(super) struct Proj {
    pub(super) id: String,
    pub(super) name: String,
}

/// How a step left the project it was editing; the oracle's expectation for the
/// lock depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(super) enum Exit {
    Close,
    Quit,
    /// A load of another project (the detached release of the finalize step).
    Switch,
    Logout,
    /// A sign-in as another account or to another deployment.
    LoginSwitch,
    /// Restart-to-update (no exit hook).
    RestartUpdate,
    /// Unlink / Save copy (the abandoning release).
    Unlink,
}

/// What a step's result asserts about the world, for the oracle to verify.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) enum Claim {
    /// A new edit token is in the working copy.
    Edited(String),
    /// The user confirmed discarding UNSAVED (in-memory) edits.
    DiscardedUnsaved,
    /// The user confirmed discarding saved, un-uploaded local edits too.
    DiscardedLocal,
    /// The process died (crash / restart-to-update): unsaved edits are gone.
    Crashed,
    /// A confirmed upload of pending work: the copy and the tip are one.
    Uploaded { project_id: String },
    /// The app said it handed the lock back.
    Released { project_id: String },
    /// The app left `project_id` while editing it (gate open) as `account`.
    Left {
        project_id: String,
        exit: Exit,
        account: String,
        /// The copy held saved, un-uploaded edits when it was left.
        dirty: bool,
    },
    /// Reload said up to date or fast-forwarded: BASE names the tip.
    BaseIsTip { project_id: String },
    /// A project was published: the world grows.
    Published(Proj),
    /// The step drops the copy's link on purpose (unlink, save copy).
    LinkDropped,
    /// The step deliberately uploaded an older revision (Revert, Unwind push).
    Reverted { project_id: String },
    /// An open answered with the offline cache.
    OfflineOpen { project_id: String },
    /// An upload carried a commit message over 1024 characters.
    LongMessage { project_id: String },
    /// A crash left a hot rollback journal next to this project's cache file.
    Residue { project_id: String },
    /// Take remote returned with the edit gate still open (before the status
    /// refresh that follows it in the frontend).
    EditableAfterTakeRemote { project_id: String },
}

/// One refusal a verb returned: what the UI is handed.
#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct Refusal {
    pub(super) verb: String,
    pub(super) action: String,
    pub(super) status: Option<u16>,
    pub(super) message: String,
    pub(super) newer_schema: bool,
}

/// The result of one step, for the trace and the oracle.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct Outcome {
    pub(super) log: Vec<String>,
    pub(super) claims: Vec<Claim>,
    pub(super) refusals: Vec<Refusal>,
    pub(super) panicked: Option<String>,
}

impl Outcome {
    pub(super) fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    pub(super) fn refused(&mut self, verb: &str, e: &SpeleoErrorDto) {
        self.refusals.push(Refusal {
            verb: verb.to_owned(),
            action: format!("{:?}", e.action),
            status: e.status,
            message: e.message.clone(),
            newer_schema: e.newer_schema.is_some(),
        });
        self.note(format!(
            "{verb} -> Err({:?} {:?}: {})",
            e.action,
            e.status,
            clip(&e.message)
        ));
    }

    pub(super) fn merge(&mut self, other: Outcome) {
        self.log.extend(other.log);
        self.claims.extend(other.claims);
        self.refusals.extend(other.refusals);
        if self.panicked.is_none() {
            self.panicked = other.panicked;
        }
    }
}

pub(super) fn clip(s: &str) -> String {
    s.chars().take(160).collect()
}

/// The app's event sink: `with_app_state` reaches the live process's state, as
/// the Tauri sink does. A dead process (quit, crash, restart) runs nothing: a
/// detached release that had not started is gone with it.
pub(super) struct FuzzSink {
    state: Mutex<Weak<AppState>>,
    dead: AtomicBool,
}

impl EventSink for FuzzSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        if self.dead.load(Ordering::SeqCst) {
            return true;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .upgrade();
        if let Some(state) = state {
            f(&state);
        }
        true
    }
}

/// The frontend's own state the flows consult.
#[derive(Default)]
pub(super) struct Front {
    /// `isDirty`: an edit not yet saved to the file.
    pub(super) unsaved: bool,
    pub(super) edits: usize,
    pub(super) copies: usize,
    pub(super) locals: usize,
    pub(super) publishes: usize,
    pub(super) messages: usize,
}

/// The accounts the app can sign in as.
#[derive(Clone)]
pub(super) struct Accounts {
    pub(super) owner: Account,
    pub(super) collab: Account,
    pub(super) ro: Option<Account>,
}

/// The app process. `restart` replaces the `AppState` as a relaunch would; the
/// cache and config folders survive it, as they do on disk.
pub(super) struct App {
    state: Mutex<Arc<AppState>>,
    sink: Mutex<Arc<FuzzSink>>,
    pub(super) proxy_url: String,
    pub(super) other_url: String,
    pub(super) accounts: Accounts,
    pub(super) copies_dir: PathBuf,
    pub(super) cache_root: PathBuf,
    pub(super) front: Mutex<Front>,
    pub(super) tag: String,
    /// The run's config resolution, adopted by every worker thread so a sign-in
    /// made there persists where a relaunch reads it.
    pub(super) config: crate::test_helpers::ConfigHandoff,
}

fn boot() -> (Arc<AppState>, Arc<FuzzSink>) {
    let state =
        Arc::new(AppState::new().unwrap_or_else(|e| panic!("FZ-SDB: AppState::new: {e:#}")));
    let sink = Arc::new(FuzzSink {
        state: Mutex::new(Arc::downgrade(&state)),
        dead: AtomicBool::new(false),
    });
    state.set_event_sink(Arc::clone(&sink) as Arc<dyn EventSink>);
    (state, sink)
}

impl App {
    pub(super) fn new(
        proxy_url: String,
        other_url: String,
        accounts: Accounts,
        copies_dir: PathBuf,
        cache_root: PathBuf,
        tag: String,
    ) -> Self {
        let (state, sink) = boot();
        Self {
            state: Mutex::new(state),
            sink: Mutex::new(sink),
            proxy_url,
            other_url,
            accounts,
            copies_dir,
            cache_root,
            front: Mutex::new(Front::default()),
            tag,
            config: crate::test_helpers::ConfigHandoff::capture(),
        }
    }

    /// The live process's state.
    pub(super) fn st(&self) -> Arc<AppState> {
        Arc::clone(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(super) fn front(&self) -> std::sync::MutexGuard<'_, Front> {
        self.front.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The process ends here: nothing it had not started yet runs.
    pub(super) fn kill(&self) {
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .dead
            .store(true, Ordering::SeqCst);
    }

    /// Relaunch: a fresh `AppState` over the same folders. The saved token
    /// (speleodb.conf) signs it back in on first use, as on a real relaunch.
    pub(super) fn restart(&self) {
        self.kill();
        let (state, sink) = boot();
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = sink;
        let mut front = self.front();
        front.unsaved = false;
    }

    /// The open project's file path, as the backend holds it.
    pub(super) fn open_path(&self) -> Option<PathBuf> {
        self.st().project.lock().ok()?.as_ref()?.file_path.clone()
    }

    /// The open project's persisted link.
    pub(super) fn open_link(&self) -> Option<crate::models::SpeleoLink> {
        self.st().speleodb_open_link().ok()
    }

    pub(super) fn editing(&self) -> bool {
        self.st().edit_enabled() && self.open_link().is_some()
    }

    pub(super) fn session(&self) -> crate::speleodb::SpeleoSessionStatus {
        self.st()
            .speleodb_session_status()
            .unwrap_or(crate::speleodb::SpeleoSessionStatus {
                logged_in: false,
                user_email: None,
                endpoint: None,
            })
    }

    /// The signed-in account's email, lowercased.
    pub(super) fn account(&self) -> Option<String> {
        self.session().user_email.map(|e| e.to_ascii_lowercase())
    }

    /// The cache file of `id`.
    pub(super) fn cache_file(&self, id: &str) -> PathBuf {
        self.cache_root.join(id).join("project.stalx")
    }

    /// Run one step's flow, catching a panic as the outcome it is.
    pub(super) fn run(&self, act: &super::model::Act, projects: &[Proj]) -> Outcome {
        let mut out = Outcome::default();
        let answer = catch_unwind(AssertUnwindSafe(|| {
            super::flows::dispatch(self, act, projects, &mut out);
        }));
        if let Err(panic) = answer {
            out.panicked = Some(panic_text(panic.as_ref()));
        }
        out
    }

    /// The claim a step that leaves the open project owes, read BEFORE it acts:
    /// `None` when no edit session is open.
    pub(super) fn leaving(&self, exit: Exit) -> Option<Claim> {
        if !self.st().edit_enabled() {
            return None;
        }
        let link = self.open_link()?;
        Some(Claim::Left {
            project_id: link.project_id,
            exit,
            account: self.account().unwrap_or_default(),
            dirty: link.has_unpushed_edits,
        })
    }
}

pub(super) fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "non-string panic".to_owned())
}

pub(super) fn short(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The rollback-journal magic SQLite writes once a journal is synced.
const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
/// The journal sector size the header declares.
const JOURNAL_SECTOR: usize = 512;

/// SQLite's page checksum (`pager_cksum`): the nonce plus every 200th byte,
/// counted down from 200 bytes before the end of the page.
fn journal_checksum(nonce: u32, page: &[u8]) -> u32 {
    let mut sum = nonce;
    let mut i = page.len().saturating_sub(200);
    while i > 0 {
        sum = sum.wrapping_add(u32::from(page.get(i).copied().unwrap_or(0)));
        i = i.saturating_sub(200);
    }
    sum
}

/// Plant a hot rollback journal next to `db`, as a process killed mid-commit
/// leaves one: SQLite synced the journal (magic, record count, the original
/// size) and was writing the database when it died. The journal holds the
/// current images of the first pages, so a replay onto this same file changes
/// nothing, while a replay onto a file installed in its place mixes this file's
/// pages into it.
pub(super) fn plant_hot_journal(db: &Path) -> Result<(), String> {
    let bytes = std::fs::read(db).map_err(|e| format!("read: {e}"))?;
    let page_size = match bytes.get(16..18) {
        Some([1, 0]) => 65_536,
        Some(&[hi, lo]) => usize::from(u16::from_be_bytes([hi, lo])),
        _ => return Err("not a SQLite file".to_owned()),
    };
    if page_size < 512 || bytes.len() < page_size {
        return Err(format!("unexpected page size {page_size}"));
    }
    let pages = bytes.len() / page_size;
    let records = pages.min(6);
    let nonce: u32 = 0x5eed_0f00;
    let be = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes();
    let mut journal = Vec::with_capacity(JOURNAL_SECTOR + records * (page_size + 8));
    journal.extend_from_slice(&JOURNAL_MAGIC);
    journal.extend_from_slice(&be(records));
    journal.extend_from_slice(&nonce.to_be_bytes());
    journal.extend_from_slice(&be(pages));
    journal.extend_from_slice(&be(JOURNAL_SECTOR));
    journal.extend_from_slice(&be(page_size));
    journal.resize(JOURNAL_SECTOR, 0);
    for n in 0..records {
        let page = bytes
            .get(n * page_size..(n + 1) * page_size)
            .ok_or("short file")?;
        journal.extend_from_slice(&be(n + 1));
        journal.extend_from_slice(page);
        journal.extend_from_slice(&journal_checksum(nonce, page).to_be_bytes());
    }
    let path = PathBuf::from(format!("{}-journal", db.display()));
    std::fs::write(&path, journal).map_err(|e| format!("plant: {e}"))
}

#[test]
fn a_planted_journal_is_hot_and_harmless_to_its_own_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("project.stalx");
    super::data::write_stalx(&path, &[("A".to_owned(), 3), ("B".to_owned(), 2)]).expect("seed");
    let before = super::data::fingerprint_file(&path).expect("read");
    plant_hot_journal(&path).expect("plant");
    let journal = PathBuf::from(format!("{}-journal", path.display()));
    assert!(journal.exists());
    // The next read-write open replays it (it is hot) and consumes it.
    let conn = rusqlite::Connection::open(&path).expect("open");
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("check");
    drop(conn);
    assert_eq!(ok, "ok");
    assert!(
        !journal.exists(),
        "a hot journal is rolled back and removed"
    );
    assert_eq!(
        super::data::fingerprint_file(&path).expect("reread"),
        before
    );
}
