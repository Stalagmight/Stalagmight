//! Replacing the local SpeleoDB cache `.stalx` with a freshly-downloaded copy.
//!
//! Three flows do this - the pull (`speleodb_open_project`), the fast-forward
//! (`speleodb_start_editing` / `speleodb_update`), and the revision swap
//! (`speleodb_restore_revision`) - and all three must handle the same hazards
//! around clobbering a file the app may still have open:
//!
//! 1. the per-machine view toggles (`local.display`) of the OLD file have to
//!    survive the replace (a push never carries `local`, #197);
//! 2. the replace itself has to be atomic and durable (`write_atomic`), and
//! 3. a legacy (SQLite) cache file's sidecars have to be out of the way before
//!    the new file takes its name (`speleodb_sidecars`).
//!
//! Every flow first reads the download into memory and checks it
//! (`speleodb_cache_install::PreparedDownload`): a version 2 file is decoded, a
//! legacy one migrated, and the link, the undo policy and the history rule are
//! settled there, before any lock is taken. The pull then installs it through
//! `replace_cache_for_session`, the fast-forward and the revision swap through
//! the prepared reload (`open_project_download`). Each install handles all
//! three hazards, so no call site can forget one. Both run under the
//! session's locks and refuse once a newer open or login has moved it, so a
//! late download can never replace a copy the user has since reopened.
//!
//! It also carries the DATA-SAFETY half: the copy being replaced may hold edits
//! the server has never seen, and those exist in no git anywhere - overwriting
//! them is unrecoverable. So a [`LocalEditsPolicy::Protect`] replace REFUSES
//! ([`SpeleoDbError::WouldDiscardLocalEdits`], which crosses IPC as a typed
//! routing signal for the cancel / take-remote choice), and a caller that
//! genuinely means to discard has to say so by name with
//! [`LocalEditsPolicy::Discard`], so the waiver shows up in a diff as that word.
//! Every caller entitled to it arrives from a frontend that has already had
//! the loss confirmed by name: `speleodb_take_remote` (the divergence prompt's
//! destroy-confirmed "Take remote"), `speleodb_open_project_discarding_local` (the
//! same confirmed choice on a refused open), and `speleodb_restore_revision` under
//! its `discard_local` waiver (a Restore only, never a View - a restore stamps the
//! copy un-pushed itself, so without the waiver Restore was single-use). Anything
//! else passes [`LocalEditsPolicy::Protect`] and refuses instead.

use std::collections::BTreeMap;
use std::path::Path;

use super::open_file::FileState;
use super::AppState;
use crate::models::survey::Project;
use crate::models::SpeleoLink;
use crate::persistence::stalx_v2::{probe_path, Format};
use crate::speleodb::SpeleoDbError;

/// What a cache file says about itself, read without decoding the survey.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum CacheFile {
    /// Nothing is at the path.
    Absent,
    /// A file is there, but its state could not be read: NOT a claim that it
    /// is unlinked or clean.
    Unreadable,
    /// The file's link (none: not a managed copy) and its view toggles.
    Read {
        link: Option<SpeleoLink>,
        display: BTreeMap<String, String>,
    },
}

/// Read the link and the view toggles of the cache file at `path`.
///
/// A version 2 file is read through the header probe (the first bytes only,
/// plan 6.1). A legacy (SQLite) copy, which an older build left in the cache,
/// is read through the frozen 0.6.0 reader and the migration, in memory: its
/// file and journal are copied to a private temp first, so nothing is ever
/// written beside it, and a hot journal is rolled back in the copy, which is
/// exactly the committed state a read-write open would have recovered.
pub(super) fn read_cache_file(path: &Path) -> CacheFile {
    let header = match probe_path(path) {
        Ok(header) => header,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CacheFile::Absent,
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                error = %e,
                purpose = "read a SpeleoDB cache copy's link",
                "speleodb: the cached copy could not be read"
            );
            return CacheFile::Unreadable;
        }
    };
    match header.format {
        Format::V2 if header.complete => {
            let mut file = FileState::default();
            file.extras.speleodb = header.speleodb;
            file.extras.local = header.local.unwrap_or_default();
            let link = file.speleo_link();
            CacheFile::Read {
                link,
                display: std::mem::take(&mut file.extras.local.display),
            }
        }
        Format::Legacy => match crate::persistence::migrate::migrate_path(path) {
            Ok(migrated) => CacheFile::Read {
                link: migrated.extras.speleo_link,
                display: migrated.extras.display_settings,
            },
            Err(e) => {
                tracing::debug!(
                    path = %path.display(),
                    class = e.class(),
                    purpose = "read a legacy SpeleoDB cache copy's link",
                    "speleodb: the legacy cached copy could not be read"
                );
                CacheFile::Unreadable
            }
        },
        Format::V2 | Format::Unknown => CacheFile::Unreadable,
    }
}

/// The exact open project a SpeleoDB transition was decided over: the
/// session generation and the project revision, which every change of the
/// project slot bumps (an edit, an undo, an open, a close). A transition that
/// replaces the open project carries it from the moment its consent was taken
/// to its commit, and refuses with [`SpeleoDbError::SessionMoved`] when either
/// moved, so an edit made while the download ran is never thrown away by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OpenProjectToken {
    pub(super) generation: u64,
    pub(super) revision: u64,
}

impl AppState {
    /// The token of the project open now. The caller holds the `project`
    /// guard, so the two counters describe one world.
    pub(super) fn open_project_token_locked(&self) -> OpenProjectToken {
        OpenProjectToken {
            generation: self.speleodb_session_generation(),
            revision: self.current_revision(),
        }
    }

    /// Whether `token` still describes the project open now. The caller holds
    /// the `project` guard.
    pub(super) fn open_project_token_holds_locked(&self, token: OpenProjectToken) -> bool {
        self.open_project_token_locked() == token
    }

    /// The view toggles of the copy about to be replaced at `dest`, for the
    /// new copy to keep (#197). `open` is the project in the slot (the caller
    /// holds the `project` guard): when `dest` is its file, the toggles in
    /// memory are the authority (they are durable at once, but a legacy or a
    /// never-saved copy holds them in memory only). Otherwise the file is read;
    /// an absent or unreadable one has none, which is a first open's defaults.
    pub(super) fn outgoing_display(
        &self,
        open: Option<&Project>,
        dest: &Path,
    ) -> BTreeMap<String, String> {
        let is_open = open
            .and_then(|p| p.file_path.as_deref())
            .is_some_and(|path| super::project_io::same_file(path, dest));
        if is_open {
            return self.with_file(|file| file.display_settings().clone());
        }
        file_display(dest)
    }
}

/// The view toggles the cache file at `dest` holds; none when it is absent or
/// unreadable (a first open's defaults).
pub(super) fn file_display(dest: &Path) -> BTreeMap<String, String> {
    match read_cache_file(dest) {
        CacheFile::Read { display, .. } => display,
        CacheFile::Absent | CacheFile::Unreadable => BTreeMap::new(),
    }
}

/// Whether a re-pull may destroy un-pushed local edits at its destination.
///
/// For the orchestration paths that serve BOTH kinds of caller with the same
/// mechanics - `speleodb_fast_forward`, the shared body behind
/// `speleodb_open_project` / `speleodb_open_project_discarding_local`, and
/// `speleodb_restore_revision`'s View-vs-Restore split - so the intent is named at
/// every call site instead of riding on a bare `bool`, and so a diff that adds a
/// waiver says `Discard` in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalEditsPolicy {
    /// Refuse the replace when the destination holds un-pushed edits. The default
    /// for every path the user did not explicitly point at their own work.
    Protect,
    /// Discard them: the user confirmed it (take-remote's destroy-confirmation, or
    /// the "take remote and discard local" branch of the open-project prompt).
    Discard,
}

/// [#303] What to do with the undo history riding IN the downloaded `.stalx`.
///
/// A Full-mode push ships its undo stack, so a server copy can arrive carrying
/// the last pusher's edit history. Inheriting that IMPLICITLY is the #194 b3 hazard:
/// press Ctrl-Z on a freshly-pulled project and you replay a stranger's command, with
/// its prior-state payload, against a tree you have not looked at yet.
///
/// So the default is [`Self::Drop`] on every path that replaces the cache file, and the
/// history is only ever handed back through the ONE explicit, lock-held route that exists
/// to ask for it: the Unwind (`speleodb_restore_revision` with `keep_undo_history`, which
/// no UI offers since #668 and whose preconditions the backend enforces itself).
/// `undo_history_keep_is_unwind_only` pins that there is exactly one `Keep` site.
///
/// Applied to the download in memory (`PreparedDownload`), before the cache file is
/// written, so the history never reaches the disk for a later open to pick up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InheritedUndo {
    /// The installed copy holds no undo or redo history. Every implicit adopt path.
    Drop,
    /// Keep the downloaded undo history: the caller is the Unwind, and that history
    /// is what it came for.
    Keep,
}

impl AppState {
    /// Refuse when the copy of `project_id` at `dest` still holds edits the server
    /// has never seen. The gate in front of every download-replace, and the reason
    /// re-opening a project from the SpeleoDB list can no longer silently destroy a
    /// week of unpushed survey.
    ///
    /// Reads through [`AppState::local_copy_state`], so the OPEN project's live
    /// in-memory link counts - the case a naive on-disk-only check gets wrong, and
    /// the one where the user is most likely to be actively working.
    ///
    /// Shared with the pull's early check, so known local edits can refuse the
    /// download before any network work starts.
    pub(super) fn guard_local_edits(
        &self,
        project_id: &str,
        dest: &Path,
    ) -> Result<(), SpeleoDbError> {
        // `None` = the file is there but unreadable, so we CANNOT rule out un-pushed
        // edits. A guard that stands in front of an overwrite has to fail closed on
        // that: for a legacy copy the likeliest cause is a hot journal after a kill or
        // power loss, and for any copy a file that is damaged or cannot be opened.
        // Refusing is recoverable in a way overwriting is not: the frontend's prompt
        // offers cancel (nothing is touched) or an explicit take-remote.
        let Some(local) = self.local_copy_state(project_id, dest) else {
            tracing::warn!(
                project_id = %project_id,
                path = %dest.display(),
                purpose = "fail closed when the local copy cannot be read",
                "speleodb: refusing to replace a local copy whose state could not be determined"
            );
            return Err(SpeleoDbError::WouldDiscardLocalEdits {
                project_id: project_id.to_owned(),
                local_path: dest.to_string_lossy().into_owned(),
                last_edit_at: None,
            });
        };
        if !local.has_unpushed_edits {
            return Ok(());
        }
        tracing::info!(
            project_id = %project_id,
            path = %dest.display(),
            is_open = local.is_open,
            purpose = "avoid silently destroying un-pushed local work on a re-pull",
            "speleodb: refusing to replace a local copy that has un-pushed edits"
        );
        Err(SpeleoDbError::WouldDiscardLocalEdits {
            project_id: project_id.to_owned(),
            local_path: dest.to_string_lossy().into_owned(),
            last_edit_at: local.last_edit_at,
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::*;
    use crate::core::state::stalx_fixtures::{
        display_at, fixture_link, history_counts, link_on_disk, name_at, seed_with_history,
        set_display, write_v2, FIXTURE_ID,
    };
    use crate::core::state::tests_support::speleodb_prod_matches;
    use crate::persistence::stalx_v2::DecodeOptions;

    const ID: &str = FIXTURE_ID;

    /// [#303] Spec item 15's structural half, and the reason `InheritedUndo` is a named
    /// enum rather than a bool: KEEPING a downloaded copy's undo history is a waiver of
    /// the #194 b3 guard, so there must be exactly ONE place in the whole orchestration
    /// that asks for it - the Unwind, which exists to fetch that history back under an
    /// explicit, lock-held action whose preconditions the backend checks.
    ///
    /// A diff that adds a second `Keep` site fails here and has to justify itself.
    #[test]
    fn undo_history_keep_is_unwind_only() {
        let keeps = speleodb_prod_matches("InheritedUndo::Keep", &["speleodb_cache.rs"]);
        assert_eq!(
            keeps, 1,
            "exactly one production site may keep a downloaded copy's undo history (the \
             Unwind); found {keeps}. Every other adopt path inherits a stranger's \
             history one Ctrl-Z away from the user (#194 b3)."
        );
    }

    /// A downloaded version 2 file standing in for the server's copy.
    fn server_stalx() -> tempfile::NamedTempFile {
        let native = tempfile::NamedTempFile::new().expect("native file");
        let mut link = fixture_link("https://server.example", false);
        link.base_commit_sha = Some("0000000".to_owned());
        write_v2(native.path(), "Server Copy", Some(&link));
        native
    }

    /// Install `source` over `dest` through the session-bound replace, under the
    /// generation that is current right now (nothing moves it in these tests).
    fn install(
        state: &AppState,
        source: &Path,
        dest: &Path,
        policy: LocalEditsPolicy,
        undo: InheritedUndo,
    ) -> Result<(), SpeleoDbError> {
        let prepared = state.prepare_download(
            source,
            fixture_link("http://x/", false),
            undo,
            &DecodeOptions::default(),
        )?;
        state.replace_cache_for_session(
            prepared,
            dest,
            ID,
            policy,
            state.speleodb_session_generation(),
        )
    }

    #[test]
    fn only_a_named_discard_gets_a_replace_past_un_pushed_edits() {
        // The behaviour the policy exists for, pinned from both sides: `Protect`
        // refuses over un-pushed work and leaves the file alone, and `Discard` is the
        // ONLY thing that gets through. A caller that wants the destruction has to
        // name it.
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        write_v2(&dest, "Local", Some(&fixture_link("http://x/", true)));
        let before = std::fs::read(&dest).expect("read dest");
        let state = AppState::new().expect("state");
        let native = server_stalx();

        let err = install(
            &state,
            native.path(),
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect_err("un-pushed edits must refuse the protected replace");
        assert!(
            matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "and refuse it with the typed routing signal, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("read dest"),
            before,
            "a refusal must leave the local copy byte-identical"
        );

        install(
            &state,
            native.path(),
            &dest,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
        .expect("the confirmed-discard waiver replaces the copy");
        assert_eq!(
            name_at(&dest),
            "Server Copy",
            "and only that waiver actually overwrites it"
        );
    }

    /// [#303] Spec item 15's behavioural half: a replace that carries
    /// `InheritedUndo::Drop` leaves the installed file with NO history and no chain
    /// metadata, and one that carries `Keep` keeps the downloaded undo history.
    ///
    /// Asserted on the FILE rather than on the in-memory stack, because that is where it
    /// has to be true: the load path reads the file's history into the live stack, so a
    /// clear that only touched memory would leave the history on disk for the next open.
    #[test]
    fn a_dropped_replace_empties_the_downloaded_history() {
        let temp = tempfile::TempDir::new().expect("temp");
        let native = temp.path().join("download.stalx");
        seed_with_history(
            &native,
            "Pushed",
            &fixture_link("https://server.example", false),
            2,
            1,
        );
        assert_eq!(history_counts(&native), (2, 1), "precondition: history");
        let state = AppState::new().expect("state");

        let kept = temp.path().join("kept.stalx");
        install(
            &state,
            &native,
            &kept,
            LocalEditsPolicy::Discard,
            InheritedUndo::Keep,
        )
        .expect("replace with Keep");
        assert_eq!(
            history_counts(&kept),
            (2, 0),
            "Keep leaves the downloaded undo history in place - what the Unwind needs \
             (a push never carries redo, and neither does an install)"
        );

        let dropped = temp.path().join("dropped.stalx");
        install(
            &state,
            &native,
            &dropped,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
        .expect("replace with Drop");
        assert_eq!(
            history_counts(&dropped),
            (0, 0),
            "Drop empties the history, so no adopt path can inherit it (#194 b3)"
        );
        for path in [&kept, &dropped] {
            let link = link_on_disk(path).expect("linked");
            assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
            let sync = crate::core::state::stalx_fixtures::decoded(path)
                .extras
                .local
                .speleodb;
            assert_eq!(
                (sync.unwound_from, sync.inherited_through),
                (None, None),
                "the other machine's chain position and watermark never ride along"
            );
        }
    }

    #[test]
    fn a_refused_download_leaves_the_existing_cache_untouched() {
        let temp = tempfile::TempDir::new().expect("temp");
        let native = temp.path().join("download.stalx");
        std::fs::write(&native, b"{\"format\":\"stalx\",\"version\":2,").expect("truncated");
        let dest = temp.path().join("project.stalx");
        std::fs::write(&dest, b"original cache").expect("seed destination");
        let before = std::fs::read(&dest).expect("read destination");
        let state = AppState::new().expect("state");

        let err = install(
            &state,
            &native,
            &dest,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
        .expect_err("a truncated download must not be adopted");
        assert!(
            err.to_string()
                .contains("the downloaded project file was refused"),
            "{err}"
        );
        assert_eq!(std::fs::read(&dest).expect("read destination"), before);
        let mut remaining: Vec<_> = std::fs::read_dir(temp.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                std::ffi::OsString::from("download.stalx"),
                std::ffi::OsString::from("project.stalx")
            ],
            "a refused download leaves no staging file behind"
        );
    }

    #[test]
    fn a_clean_copy_is_replaced_without_any_waiver() {
        // The other half: `Protect` is not "always refuse". With nothing un-pushed
        // there is no work to lose, so an ordinary re-pull goes straight through.
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        write_v2(&dest, "Local", Some(&fixture_link("http://x/", false)));
        let state = AppState::new().expect("state");
        let native = server_stalx();

        install(
            &state,
            native.path(),
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect("a clean local copy needs no waiver");
        assert_eq!(name_at(&dest), "Server Copy", "the server copy landed");
    }

    /// The per-machine view toggles of the replaced copy survive the replace; the
    /// server copy never carries any of its own (#197).
    #[test]
    fn local_view_toggles_survive_a_download_replace() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        write_v2(&dest, "Local", Some(&fixture_link("http://x/", false)));
        set_display(&dest, "show_station_names", "true");
        let native = temp.path().join("download.stalx");
        write_v2(
            &native,
            "Server Copy",
            Some(&fixture_link("http://x/", false)),
        );
        set_display(&native, "show_station_names", "false");
        set_display(&native, "pushers_view", "1");
        let state = AppState::new().expect("state");

        install(
            &state,
            &native,
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect("install");
        let display = display_at(&dest);
        assert_eq!(
            display.get("show_station_names").map(String::as_str),
            Some("true")
        );
        assert!(
            !display.contains_key("pushers_view"),
            "the downloaded file's own view state is not adopted"
        );
        let fresh = temp.path().join("fresh.stalx");
        install(
            &state,
            &native,
            &fresh,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect("install");
        assert!(
            display_at(&fresh).is_empty(),
            "a first install has defaults"
        );
    }

    /// [#649] A server `.stalx` a newer Stalagmight wrote (a higher major format
    /// version) never replaces the local copy: this build could not open it.
    #[test]
    fn a_download_from_a_newer_format_never_replaces_the_cache() {
        let temp = tempfile::TempDir::new().expect("temp");
        let source = temp.path().join("download.stalx");
        std::fs::write(
            &source,
            b"{\"format\":\"stalx\",\"version\":3,\"requires\":[],\"end\":true}\n",
        )
        .expect("a newer file");
        let dest = temp.path().join("project.stalx");
        write_v2(&dest, "Local", Some(&fixture_link("http://x/", false)));
        let before = std::fs::read(&dest).expect("destination");
        let state = AppState::new().expect("state");
        let err = install(
            &state,
            &source,
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect_err("newer format refused");
        let SpeleoDbError::NewerSchema(newer) = &err else {
            panic!("wrong refusal: {err:?}");
        };
        assert_eq!(newer.file, "project.stalx");
        assert_eq!(newer.found, 3);
        assert_eq!(newer.supported, 2);
        assert_eq!(std::fs::read(&dest).expect("destination"), before);
        let mut names = std::fs::read_dir(temp.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                std::ffi::OsString::from("download.stalx"),
                std::ffi::OsString::from("project.stalx")
            ]
        );
    }

    #[test]
    fn the_cache_reader_tells_absent_unreadable_and_read_apart() {
        let temp = tempfile::TempDir::new().expect("temp");
        assert_eq!(
            read_cache_file(&temp.path().join("missing.stalx")),
            CacheFile::Absent
        );
        let junk = temp.path().join("junk.stalx");
        std::fs::write(&junk, b"not a project").expect("junk");
        assert_eq!(read_cache_file(&junk), CacheFile::Unreadable);
        let legacy = temp.path().join("legacy.stalx");
        std::fs::write(&legacy, b"SQLite format 3\0 but nothing after it").expect("legacy");
        assert_eq!(
            read_cache_file(&legacy),
            CacheFile::Unreadable,
            "a legacy copy the frozen reader refuses is unreadable, never clean"
        );
        let linked = temp.path().join("linked.stalx");
        write_v2(&linked, "Linked", Some(&fixture_link("http://x/", true)));
        set_display(&linked, "view", "kept");
        let CacheFile::Read { link, display } = read_cache_file(&linked) else {
            panic!("a version 2 copy reads");
        };
        let link = link.expect("linked");
        assert!(link.has_unpushed_edits);
        assert_eq!(link.project_id, ID);
        assert_eq!(display.get("view").map(String::as_str), Some("kept"));
    }
}
