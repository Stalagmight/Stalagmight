//! Foreign copies of SpeleoDB-managed projects.
//!
//! The invariant this module enforces: a `.stalx` carrying a `speleo_link` row
//! is usable in exactly one place on this machine - the pull cache,
//! `<cache>/speleodb/<project_id>/project.stalx`. Any other file that carries
//! one is *foreign*: opening it is refused (`do_load_project_full` calls
//! [`AppState::foreign_link`] before it opens the database), and the frontend
//! turns the refusal into a prompt with the two real ways forward - open the
//! managed copy from SpeleoDB, or strip this file's link and open it as a
//! plain local project.
//!
//! Foreign copies exist because Save As used to carry the `speleo_link` row
//! onto whatever destination the user picked. That carry is gone (an explicit
//! destination on a linked project now writes an UNLINKED copy - see
//! `save_project`), but files written by older builds, restored from backups,
//! or received from another machine still hold one, and every one of them is a
//! second local copy of a project whose one true home is the cache. Two copies
//! of the same remote project is exactly the shape the SpeleoDB locking
//! protocol cannot express (`docs/plans/2026-07-30_speleodb-lock-session-races.md`,
//! F8): the lock bookkeeping is keyed by project id alone, so a second copy's
//! session is indistinguishable from the first's.
//!
//! The probe is deliberately read-only (`Database::open_for_metadata`): a
//! refusal must not write a byte to - or migrate, or mmap - a file the user
//! may not want touched.

use std::path::{Path, PathBuf};

use super::speleodb::local_dto;
use super::AppState;
use crate::core::loading::SpeleoManagedFile;
use crate::persistence::Database;
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

/// What the probe established about the spelling the load was handed.
///
/// Three answers rather than two, because "this IS the canonical copy" and "this
/// is the canonical copy under a name that a save would destroy" are different
/// facts - see [`LinkedSpelling::Canonical`].
#[derive(Debug)]
pub(super) enum LinkedSpelling {
    /// Open the path exactly as it was spelled: an unmanaged project, or the
    /// cache copy reached by a spelling that keeps resolving to it on every
    /// access (a `..` segment, a symlinked parent directory, a Windows case
    /// variant).
    AsSpelled,
    /// The cache copy reached through a symlink AS THE FINAL COMPONENT: open the
    /// carried canonical cache path instead of the spelling that was asked for.
    Canonical(PathBuf),
    /// A second local copy of a managed project: refuse the open, and hand the
    /// frontend this payload for its prompt.
    Foreign(SpeleoManagedFile),
}

/// Whether `path` may be opened, and under which spelling.
///
/// `root` is the pull-cache root (`speleodb_local_state::cache_root()`), passed
/// explicitly so this is unit-testable against a temp tree without latching the
/// process-global app dirs.
///
/// Degradations, each deliberate:
///
/// - **No link row**: not refused - an unmanaged project.
/// - **The read-only probe cannot answer**: ask again with a READ-WRITE open
///   before concluding anything (see [`link_row_of`]). "Unreadable" was treated
///   as "cannot become a second usable copy" and waved through, on the reasoning
///   that the full open a few lines later would report `CORRUPT`/`DATABASE`
///   anyway. That reasoning does not hold, and this tree documents why in three
///   other places: a hot rollback journal (a kill, a power loss) fails a
///   `SQLITE_OPEN_READ_ONLY` connection with `SQLITE_READONLY_ROLLBACK` while a
///   read-write open ROLLS IT BACK AND SUCCEEDS. So the one probe that decides
///   whether a file is a second managed copy answered "no idea, carry on"
///   precisely when the open it was guarding was about to succeed - and a
///   foreign copy became fully usable, with a session and lock claims that are
///   indistinguishable from the canonical copy's by project id alone.
/// - **Unreadable to BOTH opens**: not refused. Nothing can read a link row out
///   of it, and the full open right after fails exactly as this probe did, so
///   the load reports `CORRUPT`/`DATABASE` and routes to the corruption-recovery
///   UI - strictly better than a "managed by SpeleoDB (project unknown)" prompt
///   for a file that does not open at all.
/// - **Link row with an invalid project id**: refused, with
///   [`SpeleoManagedFile::can_open_remote`] false. An id that fails the same
///   UUID validation the edit verbs use must never be joined into a cache path
///   (route-traversal guard), so the canonical slot cannot be named and the
///   file cannot be proven to be it; the strip action is still offered so the
///   file is not a dead end.
/// - **`root == None`**: not refused. With no cache directory there is no
///   canonical slot to conflict with and no pull can run; in production the
///   app dirs resolve during `setup`, so this branch is tests / pre-setup only.
/// - **A symlink whose target IS the cache copy**: not refused, but the load is
///   redirected to the real cache path ([`LinkedSpelling::Canonical`]). It is
///   the same file to READ - `is_the_cached_copy` follows the link and rightly
///   says yes - and not the same file to WRITE: every save writes a temp sibling
///   and renames it over the destination, and that rename replaces the SYMLINK
///   ENTRY with a regular file. Opening the link by name would therefore leave
///   the cached copy stale at the first implicit save while a brand new linked
///   `.stalx` appeared wherever the link lived, which is two local copies of one
///   remote project - the exact shape this module exists to prevent. Redirecting
///   is better than refusing here: the file behind the link genuinely is the
///   canonical copy, and refusing it would offer "open the managed copy from
///   SpeleoDB" for a copy the user already has.
///
///   Only the FINAL component is treated this way. A symlinked parent DIRECTORY
///   is harmless and stays as spelled, because the rename resolves through it
///   into the real directory and lands on the real entry.
pub(super) fn foreign_link_in(root: Option<&Path>, path: &Path) -> LinkedSpelling {
    let Some(link) = link_row_of(path) else {
        return LinkedSpelling::AsSpelled;
    };
    let valid_id = super::speleodb::validate_project_id(&link.project_id).is_ok();
    let Some(root) = root else {
        return LinkedSpelling::AsSpelled;
    };
    if valid_id && super::speleodb_local_state::is_the_cached_copy(root, &link.project_id, path) {
        return if final_component_is_a_symlink(path) {
            LinkedSpelling::Canonical(super::speleodb_local_state::cache_path_in(
                root,
                &link.project_id,
            ))
        } else {
            LinkedSpelling::AsSpelled
        };
    }
    LinkedSpelling::Foreign(SpeleoManagedFile {
        path: path.display().to_string(),
        project_id: link.project_id,
        project_name: link.project_name,
        endpoint: link.endpoint,
        can_open_remote: valid_id,
    })
}

/// The `speleo_link` row of the `.stalx` at `path`, read the cheapest way that
/// can answer.
///
/// Read-only FIRST, because that is the answer for every ordinary file and it
/// neither migrates nor mmaps a file this probe may be about to refuse. But a
/// read-only connection cannot recover a hot rollback journal - it fails with
/// `SQLITE_READONLY_ROLLBACK` where a read-write open rolls the journal back and
/// succeeds - so a read-only miss is NOT evidence that the file is unopenable.
/// Escalating is what keeps this probe's verdict aligned with the open it
/// guards; without it the guard fell open in exactly the state where the file
/// was about to become a fully usable second managed copy.
///
/// The escalation goes through [`Database::open_for_metadata_recovering`], NOT
/// `open_existing`, and that distinction is load-bearing. The only write this
/// probe may make is the journal rollback itself: unavoidable (it is the only
/// way to see what the file committed) and identical to what the next ordinary
/// open would do anyway. `open_existing` would go on to run
/// `ensure_schema_version`, whose v15 migration drops `collab_state` and the
/// persisted undo/redo history - so a file this probe is about to REFUSE would
/// have been migrated and stripped of its history before the user ever saw the
/// prompt, with "Cancel" unable to put it back.
///
/// `None` covers both "no link row" (an unmanaged project) and "neither open
/// could read one" - the caller treats them the same, and the full open
/// immediately after reports the unreadable case far better than this probe
/// could (see [`foreign_link_in`]'s degradation list). Both handles are dropped
/// before returning, which matters on Windows: SQLite holds the file without
/// `FILE_SHARE_DELETE` and the real open follows immediately.
fn link_row_of(path: &Path) -> Option<crate::models::SpeleoLink> {
    match Database::open_for_metadata(path).and_then(|db| db.load_speleo_link()) {
        Ok(link) => link,
        Err(read_only_error) => Database::open_for_metadata_recovering(path)
            .and_then(|db| db.load_speleo_link())
            .inspect_err(|e| {
                tracing::warn!(
                    path = %path.display(),
                    read_only_error = %read_only_error,
                    error = %e,
                    purpose = "keep one local copy per SpeleoDB project",
                    "Neither open could read a speleo_link row; the load reports the \
                     open failure itself"
                );
            })
            .ok()
            .flatten(),
    }
}

/// Whether the LAST component of `path` is itself a symlink.
///
/// `symlink_metadata` follows every parent component but NOT the final one,
/// which is exactly the distinction that matters to the save pipeline: a rename
/// resolves parent directory links and lands on the real entry, so a symlinked
/// parent is safe; the final component is the entry the rename REPLACES.
///
/// A path that cannot be stat'ed answers false: the probe already read a link
/// row out of it, so an error here is a race or a permission quirk, and the open
/// a few lines later reports it far better than a canonical-storage refusal
/// would.
fn final_component_is_a_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_symlink())
}

impl AppState {
    /// [`foreign_link_in`] against the production cache root. See
    /// [`LinkedSpelling`]: the open is refused, redirected to the canonical
    /// cache path, or left alone.
    pub(super) fn foreign_link(&self, path: &Path) -> LinkedSpelling {
        foreign_link_in(super::speleodb_local_state::cache_root().as_deref(), path)
    }

    /// Turn the foreign managed copy at `path` into a plain unmanaged project by
    /// deleting its `speleo_link` row. The counterpart for the OPEN project is
    /// `speleodb_unlink`, which also releases the held write lock and moves the
    /// edit gates; `path` here is a file this session does not have open, so
    /// there is no lock and no gate to move.
    ///
    /// `open_existing` rather than the metadata probe: this DOES mutate the
    /// file, so it should refuse one that is not a Stalagmight project and run
    /// the same integrity check + migration a real open would - a corrupt file
    /// is reported instead of half-edited. The handle drops before returning,
    /// which matters on Windows: the frontend reopens this very file right
    /// afterwards, and SQLite holds it without FILE_SHARE_DELETE.
    ///
    /// The row delete itself is idempotent (`DELETE FROM speleo_link`), so a
    /// second invocation - or one against a file whose link is already gone -
    /// succeeds harmlessly.
    ///
    /// The refusal and the delete share ONE `project` acquisition, for the same
    /// reason `delete_project`'s check and teardown do. Asking
    /// `open_project_path` (which releases the lock) and only then deleting is a
    /// TOCTOU against load finalization: a load of THIS path finishing in the
    /// gap makes it the open project - gates already derived as remote - and the
    /// row then vanishes from underneath a live remote session, leaving a gate
    /// and possibly an acquired server mutex with no link left to release or
    /// push through. Every installer of a file-backed project finalizes under
    /// this same lock, so holding it here orders this command strictly before
    /// such a load (we refuse nothing, and the load then owns a file with its
    /// link already gone - an ordinary local project) or strictly after it (we
    /// see it open and refuse).
    ///
    /// `expected` is the link the REFUSAL reported, and it is re-checked against
    /// the file under the handle that does the delete. The prompt can sit on
    /// screen indefinitely, and a path is not a file: the `.stalx` there can be
    /// replaced, renamed over or restored from a backup in between. Stripping
    /// whatever now occupies the path would silently unmanage a project the user
    /// never saw a prompt about - possibly the canonical cache copy of something
    /// else, which is the one file this module exists to keep managed.
    pub fn speleodb_remove_link_at(
        &self,
        path: PathBuf,
        expected: SpeleoManagedFile,
    ) -> Result<(), SpeleoErrorDto> {
        let open = self.project.lock();
        // Read through the guard rather than `open_project_path`, which takes
        // the same lock and would deadlock here.
        // Resolved ONCE, and everything below - the checks and the write - is
        // about the file this names. Re-asking the original path after opening
        // it is a TOCTOU with symlinks in it: `open_existing` follows the link
        // as it was, so replacing the link afterwards makes the checks describe
        // a file the open handle is not writing to. Canonicalizing first leaves
        // one resolution for both, and the checks can no longer be pointed at a
        // different file than the delete.
        let path = std::fs::canonicalize(&path).map_err(|e| {
            local_dto(SpeleoDbError::Io(format!(
                "cannot resolve {}: {e}",
                path.display()
            )))
        })?;
        if open
            .as_ref()
            .and_then(|project| project.file_path.as_deref())
            .is_some_and(|open| super::project_io::same_file(open, &path))
        {
            return Err(local_dto(SpeleoDbError::refused(
                "that file is the project currently open; detach it from SpeleoDB instead"
                    .to_owned(),
            )));
        }
        let db = Database::open_existing(&path)
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        // Read and compare through the SAME handle that is about to write, so
        // nothing can slip between the check and the delete.
        //
        // A link naming a DIFFERENT project is the case this exists for, and the
        // only one that refuses. NO link row at all is still a success: the
        // desired end state is already there, nothing is unmanaged by saying so,
        // and the idempotency this command documents (a second confirm, a file
        // whose link is already gone) survives.
        let found = db
            .load_speleo_link()
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        if let Some(link) = &found {
            let same_project = link.project_id == expected.project_id
                && crate::speleodb::auth::normalize_endpoint(&link.endpoint)
                    == crate::speleodb::auth::normalize_endpoint(&expected.endpoint);
            if !same_project {
                tracing::warn!(
                    path = %path.display(),
                    prompted_for = %expected.project_id,
                    found = %link.project_id,
                    purpose = "keep one local copy per SpeleoDB project",
                    "Refusing to strip a link: the file at this path is a different \
                     managed project than the prompt was raised for"
                );
                return Err(local_dto(SpeleoDbError::refused(
                    "that file is a different SpeleoDB project than the one this \
                     prompt was about; reopen it to see what it is now"
                        .to_owned(),
                )));
            }
            // Right project, and STILL the wrong file if this resolves to the
            // canonical copy - a path whose final component was replaced by a
            // link to the cache copy passes the identity check above on every
            // field (same project, same deployment) while the file underneath is
            // the one this module exists to keep managed. Asked of the RESOLVED
            // path, which is also the path the handle above was opened at, so
            // the answer describes the file that is about to be written.
            // `foreign_link_in` recognises the same spelling at open time
            // (`LinkedSpelling::Canonical`). No cache root means no canonical
            // slot to protect, which is the module's standing degradation rule.
            if super::speleodb_local_state::cache_root().is_some_and(|root| {
                super::speleodb_local_state::is_the_cached_copy(&root, &link.project_id, &path)
            }) {
                tracing::warn!(
                    path = %path.display(),
                    project_id = %link.project_id,
                    purpose = "keep one local copy per SpeleoDB project",
                    "Refusing to strip a link: this path now resolves to the \
                     project's canonical cache copy"
                );
                return Err(local_dto(SpeleoDbError::refused(
                    "that path now leads to this project's managed copy, which \
                     cannot be unlinked here; reopen it to see what it is now"
                        .to_owned(),
                )));
            }
        }
        db.save_speleo_link(None)
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        drop(open);
        tracing::info!(
            path = %path.display(),
            purpose = "keep one local copy per SpeleoDB project",
            "Removed the SpeleoDB link from a foreign copy"
        );
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::*;
    use crate::models::{Project, SpeleoLink};

    const ID: &str = "11111111-1111-1111-1111-111111111111";
    const OTHER_ID: &str = "22222222-2222-2222-2222-222222222222";

    /// Write a linked `.stalx` at `path` whose `speleo_link` names `project_id`.
    fn seed_linked(path: &Path, project_id: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut db = Database::open(path).expect("open db");
        db.save_project(&Project::new("Linked".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x/".to_owned(),
            project_id: project_id.to_owned(),
            project_name: "Linked".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            // [#303] NULL exercises the legacy initialization path.
            undo_mode: None,
        }))
        .expect("save link");
    }

    /// The refusal payload a prompt for `project_id` at `path` would carry -
    /// what the strip command re-checks the file against.
    fn prompted_for(path: &Path, project_id: &str) -> SpeleoManagedFile {
        SpeleoManagedFile {
            path: path.display().to_string(),
            project_id: project_id.to_owned(),
            project_name: "Linked".to_owned(),
            endpoint: "http://x/".to_owned(),
            can_open_remote: true,
        }
    }

    /// The refusal payload, or `None` when the probe accepted the file (under
    /// either spelling).
    fn refusal(spelling: LinkedSpelling) -> Option<SpeleoManagedFile> {
        match spelling {
            LinkedSpelling::Foreign(managed) => Some(managed),
            LinkedSpelling::AsSpelled | LinkedSpelling::Canonical(_) => None,
        }
    }

    /// Whether the probe accepted the file under the spelling it was given.
    fn accepted_as_spelled(spelling: &LinkedSpelling) -> bool {
        matches!(spelling, LinkedSpelling::AsSpelled)
    }

    /// Backdate the schema version recorded in `path`, so that a migration run
    /// against it would be observable.
    fn set_user_version(path: &Path, version: u32) {
        let conn = rusqlite::Connection::open(path).expect("open to backdate the schema");
        conn.pragma_update(None, "user_version", version)
            .expect("set user_version");
    }

    /// The schema version recorded in `path`.
    fn user_version(path: &Path) -> u32 {
        let conn = rusqlite::Connection::open(path).expect("open to read the schema version");
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read user_version")
    }

    /// The rollback journal SQLite keeps beside `path` mid-transaction.
    fn journal_of(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push("-journal");
        PathBuf::from(name)
    }

    /// Copy `src` to a new path WITH a live, HOT rollback journal - what a kill
    /// or a power loss mid-write leaves behind.
    ///
    /// Dirtying a page is not enough. SQLite writes the journal header's magic
    /// as ZEROES until it is about to modify the database file itself, exactly
    /// so that a crash before that point leaves a journal nothing needs to roll
    /// back. The magic goes in when the page cache SPILLS, which is what a tiny
    /// `cache_size` plus a few hundred rows forces here. Only then does a copy
    /// of the pair reproduce the state the fix is about: the database and its
    /// journal are copied while both are live, and the original is rolled back
    /// afterwards, so the copy is left holding a hot journal and nobody's lock.
    fn copy_with_a_hot_journal(src: &Path, dest: &Path) {
        let conn = rusqlite::Connection::open(src).expect("open the source directly");
        conn.execute_batch("PRAGMA cache_size = 2; BEGIN IMMEDIATE; CREATE TABLE dirty (x);")
            .expect("open a write transaction");
        {
            let mut insert = conn
                .prepare("INSERT INTO dirty VALUES (?1)")
                .expect("prepare");
            let filler = "x".repeat(400);
            for _ in 0..500 {
                insert
                    .execute([&filler])
                    .expect("fill until the cache spills");
            }
        }
        std::fs::copy(src, dest).expect("copy the database mid-transaction");
        std::fs::copy(journal_of(src), journal_of(dest)).expect("copy its live journal");
        conn.execute_batch("ROLLBACK;")
            .expect("roll the original back");
    }

    #[test]
    fn a_linked_stalx_outside_the_cache_is_refused() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("cache");
        let foreign = temp.path().join("elsewhere").join("copy.stalx");
        seed_linked(&foreign, ID);

        let managed = refusal(foreign_link_in(Some(&root), &foreign))
            .expect("a foreign linked file is refused");
        assert_eq!(managed.path, foreign.display().to_string());
        assert_eq!(managed.project_id, ID);
        assert_eq!(managed.project_name, "Linked");
        assert_eq!(managed.endpoint, "http://x/");
        assert!(managed.can_open_remote, "a valid id can be opened remotely");
    }

    /// The read-only probe is not the last word on whether a file is a second
    /// managed copy, because it fails in a state where the file is about to open
    /// perfectly well: a hot rollback journal defeats `SQLITE_OPEN_READ_ONLY`
    /// (`SQLITE_READONLY_ROLLBACK`) while the read-write open a few lines later
    /// rolls it back and succeeds. Reading that miss as "cannot become a second
    /// usable copy" waved a foreign linked file straight through the one guard
    /// that exists to stop it.
    #[test]
    fn a_foreign_copy_is_refused_even_when_only_a_read_write_open_can_read_it() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("cache");
        let foreign = temp.path().join("elsewhere").join("copy.stalx");
        seed_linked(&foreign, ID);

        // An OLD file, so a migration would be visible: `ensure_schema_version`
        // would raise this to the current version (and, at v15, drop the
        // persisted undo/redo history with it).
        set_user_version(&foreign, 1);
        let hot = temp.path().join("elsewhere").join("hot.stalx");
        copy_with_a_hot_journal(&foreign, &hot);

        assert!(
            Database::open_for_metadata(&hot)
                .and_then(|db| db.load_speleo_link())
                .is_err(),
            "precondition: a read-only connection cannot roll back a hot journal"
        );

        let managed = refusal(foreign_link_in(Some(&root), &hot))
            .expect("an unreadable-to-read-only foreign copy is still a foreign copy");
        assert_eq!(managed.project_id, ID);
        assert_eq!(
            user_version(&hot),
            1,
            "and the refused file was NOT migrated: the probe may recover the \
             journal, because that is the only way to read what the file \
             committed and the next open would do it anyway, but it must not \
             rewrite a schema - the user has not even answered the prompt yet, \
             and Cancel cannot put a v15 migration's dropped history back"
        );

        // What the full open WOULD have done to it, which is why the escalation
        // does not use one. Both recover the journal; only this one migrates.
        let migrated = temp.path().join("elsewhere").join("hot2.stalx");
        copy_with_a_hot_journal(&foreign, &migrated);
        drop(Database::open_existing(&migrated).expect("recovers the journal as well"));
        assert_ne!(
            user_version(&migrated),
            1,
            "precondition for the assertion above: `open_existing` migrates, so \
             the probe reaching for it would have rewritten a refused file"
        );
    }

    /// ...but a file NOTHING can read is still left to the open that follows,
    /// which reports CORRUPT/DATABASE and routes to the recovery UI - better
    /// than a "managed by SpeleoDB (project unknown)" prompt for a file that
    /// does not open at all.
    #[test]
    fn a_file_neither_open_can_read_is_left_to_the_load_to_report() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("cache");
        let garbage = temp.path().join("not-a-database.stalx");
        std::fs::write(&garbage, b"this is not sqlite").expect("write garbage");

        assert!(
            accepted_as_spelled(&foreign_link_in(Some(&root), &garbage)),
            "nothing can read a link row out of it, and the open right after \
             reports the real problem"
        );
    }

    #[test]
    fn the_canonical_cache_copy_is_not_refused() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        let cached = super::super::speleodb_local_state::cache_path_in(root, ID);
        seed_linked(&cached, ID);

        assert!(
            accepted_as_spelled(&foreign_link_in(Some(root), &cached)),
            "the cache copy is the canonical home and opens normally"
        );
    }

    #[test]
    fn an_alias_spelling_of_the_canonical_copy_is_not_refused() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        let cached = super::super::speleodb_local_state::cache_path_in(root, ID);
        seed_linked(&cached, ID);

        // The same file reached through a `..` segment: literal comparison says
        // "different path", `same_file` canonicalization says "same file".
        let dir = cached.parent().expect("parent");
        let aliased = dir.join("..").join(ID).join("project.stalx");
        assert!(
            accepted_as_spelled(&foreign_link_in(Some(root), &aliased)),
            "an alias spelling of the cache path is still the cache copy"
        );

        #[cfg(unix)]
        {
            // The cache root reached through a symlink is still the same file -
            // and is left AS SPELLED, unlike a symlinked final component: a
            // rename resolves the directory link and still lands on the real
            // entry in the real directory, so nothing here needs rewriting.
            let sym_root = temp.path().join("sym-root");
            std::os::unix::fs::symlink(root, &sym_root).expect("symlink");
            let through_symlink = super::super::speleodb_local_state::cache_path_in(&sym_root, ID);
            assert!(
                accepted_as_spelled(&foreign_link_in(Some(root), &through_symlink)),
                "the cache copy through a symlinked root is still the cache copy"
            );
        }
    }

    /// A symlink to the cache copy is the cache copy to READ, and the probe used
    /// to stop there and accept the link's own name. Saving through that name is
    /// what breaks: the save writes a temp sibling and renames it over the
    /// destination, so the rename replaces the SYMLINK ENTRY with a regular file.
    /// The real cached copy is then stale forever and a second linked `.stalx`
    /// exists wherever the link lived - two local copies of one remote project,
    /// which is precisely what this module exists to prevent.
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_the_cache_copy_opens_the_real_cache_path_instead() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("cache");
        let cached = super::super::speleodb_local_state::cache_path_in(&root, ID);
        seed_linked(&cached, ID);

        let alias = temp.path().join("my-project.stalx");
        std::os::unix::fs::symlink(&cached, &alias).expect("symlink");

        let spelling = foreign_link_in(Some(&root), &alias);
        assert!(
            matches!(&spelling, LinkedSpelling::Canonical(real) if real == &cached),
            "the load must be redirected to the real cache path, not the link: {spelling:?}"
        );
    }

    /// The same rewrite when the CACHE SLOT ITSELF is the symlink (the user
    /// pointed it at a file of their own). The canonical path is the slot, so the
    /// answer is the path we were given - and the first save materialises the real
    /// file in the slot, exactly as a pull's atomic replace would. The old target
    /// is then an ordinary foreign copy, refused by this same probe.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_cache_slot_still_resolves_to_the_canonical_path() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("cache");
        let cached = super::super::speleodb_local_state::cache_path_in(&root, ID);
        let elsewhere = temp.path().join("elsewhere").join("real.stalx");
        seed_linked(&elsewhere, ID);
        std::fs::create_dir_all(cached.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&elsewhere, &cached).expect("symlink");

        let spelling = foreign_link_in(Some(&root), &cached);
        assert!(
            matches!(&spelling, LinkedSpelling::Canonical(real) if real == &cached),
            "the canonical slot is the canonical path even when it is a link: {spelling:?}"
        );
    }

    /// The redirect is for the CACHE COPY only: a link that points at some other
    /// linked `.stalx` is still a second local copy, and is still refused - under
    /// the name the user asked for, so the prompt names the file they picked.
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_foreign_linked_file_is_still_refused() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("cache");
        let foreign = temp.path().join("elsewhere").join("copy.stalx");
        seed_linked(&foreign, ID);

        let alias = temp.path().join("my-project.stalx");
        std::os::unix::fs::symlink(&foreign, &alias).expect("symlink");

        let managed = refusal(foreign_link_in(Some(&root), &alias))
            .expect("a symlink to a foreign copy is still a second local copy");
        assert_eq!(
            managed.path,
            alias.display().to_string(),
            "the refusal names the file the user asked for"
        );
        assert_eq!(managed.project_id, ID);
    }

    #[test]
    fn a_copy_in_the_wrong_project_folder_is_refused() {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        // Sitting at OTHER_ID's slot while its link claims ID: inside the cache
        // tree, but NOT the canonical home of the project it names.
        let misfiled = super::super::speleodb_local_state::cache_path_in(root, OTHER_ID);
        seed_linked(&misfiled, ID);

        let managed = refusal(foreign_link_in(Some(root), &misfiled))
            .expect("a copy in another project's cache slot is refused");
        assert_eq!(managed.project_id, ID);
    }

    #[test]
    fn an_unlinked_stalx_is_not_refused() {
        let temp = tempfile::TempDir::new().expect("temp");
        let plain = temp.path().join("plain.stalx");
        let mut db = Database::open(&plain).expect("open db");
        db.save_project(&Project::new("Plain".to_owned(), None))
            .expect("save project");
        drop(db);

        assert!(
            accepted_as_spelled(&foreign_link_in(Some(temp.path()), &plain)),
            "an unmanaged project opens normally from anywhere"
        );
    }

    #[test]
    fn an_unreadable_file_is_refused_by_nothing_so_the_load_can_report_it() {
        let temp = tempfile::TempDir::new().expect("temp");
        let junk = temp.path().join("junk.stalx");
        std::fs::write(&junk, b"not a database at all").expect("write junk");

        // The read really does fail on this input (pinned by
        // `metadata_open_still_propagates_a_genuinely_unreadable_file` in
        // persistence/sqlite/tests/speleo_link_tests.rs) - and the probe still does not refuse:
        // the full open right after it reports CORRUPT through the existing
        // recovery UI, which is the better surface for this file.
        assert!(
            accepted_as_spelled(&foreign_link_in(Some(temp.path()), &junk)),
            "an unreadable file is the corruption path's problem, not this probe's"
        );
    }

    #[test]
    fn a_non_uuid_project_id_is_refused_with_the_remote_action_withheld() {
        let temp = tempfile::TempDir::new().expect("temp");
        let foreign = temp.path().join("bad-id.stalx");
        seed_linked(&foreign, "../../../etc/passwd");

        let managed = refusal(foreign_link_in(Some(temp.path()), &foreign))
            .expect("a linked file with an unusable id is still refused");
        assert!(
            !managed.can_open_remote,
            "an id that fails validation must never become a cache path or a route"
        );
        assert_eq!(managed.project_id, "../../../etc/passwd");
    }

    #[test]
    fn no_cache_root_refuses_nothing() {
        let temp = tempfile::TempDir::new().expect("temp");
        let foreign = temp.path().join("copy.stalx");
        seed_linked(&foreign, ID);

        assert!(
            accepted_as_spelled(&foreign_link_in(None, &foreign)),
            "with no cache directory there is no canonical slot to conflict with"
        );
    }

    #[test]
    fn remove_link_at_clears_the_row_and_leaves_the_project_readable() {
        let temp = tempfile::TempDir::new().expect("temp");
        let foreign = temp.path().join("copy.stalx");
        seed_linked(&foreign, ID);
        let state = AppState::new().expect("state");

        state
            .speleodb_remove_link_at(foreign.clone(), prompted_for(&foreign, ID))
            .expect("removing the link from a closed foreign copy succeeds");

        let db = Database::open_for_metadata(&foreign).expect("reopen");
        assert_eq!(
            db.load_speleo_link().expect("read link"),
            None,
            "the link row is gone"
        );
        drop(db);
        let db = Database::open_existing(&foreign).expect("full open still works");
        assert_eq!(
            db.load_project().expect("project still loads").name,
            "Linked",
            "the project data is untouched"
        );

        // Idempotent: a second removal (nothing left to delete) still succeeds.
        state
            .speleodb_remove_link_at(foreign.clone(), prompted_for(&foreign, ID))
            .expect("a second removal is a harmless no-op");
    }

    #[test]
    fn remove_link_at_refuses_a_file_that_is_no_longer_the_one_the_prompt_named() {
        // Round 11: the prompt can sit on screen indefinitely, and a path is not
        // a file. If the `.stalx` there is replaced meanwhile - restored from a
        // backup, renamed over, or simply a different managed project - then
        // confirming the prompt would strip a link the user was never asked
        // about, up to and including another project's canonical cache copy.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        let path = temp.path().join("copy.stalx");
        seed_linked(&path, ID);
        let prompted = prompted_for(&path, ID);

        // Same path, DIFFERENT project now.
        seed_linked(&path, OTHER_ID);
        state
            .speleodb_remove_link_at(path.clone(), prompted.clone())
            .expect_err("the file at that path is not the one the prompt was about");
        assert!(
            Database::open_for_metadata(&path)
                .and_then(|db| db.load_speleo_link())
                .expect("readable")
                .is_some(),
            "and its link row is untouched"
        );

        // The file the prompt WAS about still strips.
        seed_linked(&path, ID);
        state
            .speleodb_remove_link_at(path.clone(), prompted)
            .expect("the expected file strips as before");
        assert!(
            Database::open_for_metadata(&path)
                .and_then(|db| db.load_speleo_link())
                .expect("readable")
                .is_none(),
            "its link row is gone"
        );
    }

    /// Round 12: the identity re-check passes on every field when the path is
    /// replaced by a symlink to the SAME project's canonical cache copy - same
    /// project, same deployment - while the handle underneath is the one file
    /// this module exists to keep managed. `same_file` follows symlinks, so
    /// stripping there would unmanage the canonical copy itself.
    ///
    /// Needs the production cache root to be resolvable, which is what
    /// `init_for_test` latches; the module's other tests deliberately avoid it
    /// (it is a process-global `OnceLock`), so this one asks the predicate the
    /// production code asks and skips when no root is available.
    #[cfg(unix)]
    #[test]
    fn remove_link_at_refuses_a_path_that_now_resolves_to_the_canonical_copy() {
        let Some(root) = super::super::speleodb_local_state::cache_root() else {
            // No app dirs in this process: there is no canonical slot to
            // protect, which is the same degradation the production path takes.
            return;
        };
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        let cached = super::super::speleodb_local_state::cache_path_in(&root, ID);
        seed_linked(&cached, ID);

        // The prompt was raised for a foreign copy; the path is then replaced by
        // a link to the canonical one.
        let foreign = temp.path().join("copy.stalx");
        std::os::unix::fs::symlink(&cached, &foreign).expect("symlink");

        state
            .speleodb_remove_link_at(foreign.clone(), prompted_for(&foreign, ID))
            .expect_err("the canonical copy must not be unlinked through an alias");
        assert!(
            Database::open_for_metadata(&cached)
                .and_then(|db| db.load_speleo_link())
                .expect("readable")
                .is_some(),
            "the canonical copy keeps its link"
        );
        let _ = std::fs::remove_file(&cached);
    }

    #[test]
    fn remove_link_at_refuses_the_open_project() {
        let temp = tempfile::TempDir::new().expect("temp");
        let open_path = temp.path().join("open.stalx");
        seed_linked(&open_path, ID);
        let state = AppState::new().expect("state");
        assert!(
            state.open_project(open_path.clone()).success,
            "fixture opens"
        );

        state
            .speleodb_remove_link_at(open_path.clone(), prompted_for(&open_path, ID))
            .expect_err("the open project must detach via speleodb_unlink instead");

        let db = Database::open_for_metadata(&open_path).expect("reopen");
        assert!(
            db.load_speleo_link().expect("read link").is_some(),
            "the refusal must not have touched the row"
        );
    }

    #[test]
    fn remove_link_at_reports_a_file_that_is_not_a_project() {
        let temp = tempfile::TempDir::new().expect("temp");
        let junk = temp.path().join("junk.stalx");
        std::fs::write(&junk, b"not a database at all").expect("write junk");
        let state = AppState::new().expect("state");

        state
            .speleodb_remove_link_at(junk.clone(), prompted_for(&junk, ID))
            .expect_err("a file that is not a Stalagmight project is reported, not half-edited");
    }
}
