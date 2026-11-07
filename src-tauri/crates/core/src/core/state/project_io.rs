use super::project_slot::ProjectSlot;
use super::save_sidecars::{SaveMode, SaveRequest};
use super::session_commit::{CommandsHeld, SessionCommit};
use super::AppState;
use crate::core::{CommandError, CommandErrorCode, CommandResult, PersistStack, UndoStack};
use crate::models::{CreationStamp, Project, SectionId};
use crate::persistence::Database;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// What a failed save actually was, once the error chain and the destination
/// have both been consulted. Each variant except `Other` carries a code the
/// frontend keys on to pick the remediation it shows.
#[derive(Clone, Copy)]
enum SaveFailureKind {
    /// The OS refuses a destination that IS there (macOS forgetting a granted
    /// folder, a read-only mount).
    PermissionDenied,
    /// The destination is not reachable: the folder was deleted or renamed, or
    /// its drive is not connected.
    PathNotFound,
    /// The destination path is longer than the OS will open.
    PathTooLong,
    /// Anything else - disk full, corruption, a SQLite error of its own. The
    /// backend message is the best thing to show.
    Other,
}

impl SaveFailureKind {
    /// The failure's code (#576 C4), or `None` when the message stands on its
    /// own (`FAILED` at the boundary).
    fn code(self) -> Option<CommandErrorCode> {
        match self {
            Self::PermissionDenied => Some(CommandErrorCode::FsPermissionDenied),
            Self::PathNotFound => Some(CommandErrorCode::FsPathNotFound),
            Self::PathTooLong => Some(CommandErrorCode::FsPathTooLong),
            Self::Other => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::PermissionDenied => "permission_denied",
            Self::PathNotFound => "path_not_found",
            Self::PathTooLong => "path_too_long",
            Self::Other => "other",
        }
    }
}

/// What the destination directory has to say for itself.
enum DirProbe {
    /// It accepted a file, so it is denying us nothing.
    Writable,
    /// It refused - the corroboration a permission denial needs.
    Denied,
    /// It is not there, or cannot be reached at all.
    Unreachable,
    /// It is there and the probe failed for some other reason (a full disk).
    Other,
}

/// Create the probe file, refusing to write through anything already at that
/// name.
///
/// `create_new` is what makes the probe safe to run in a directory other
/// people can write to: a plain `File::create` FOLLOWS a symlink planted at the
/// name and truncates whatever it points at, and the name is guessable (it
/// carries our pid). `create_new` fails with `AlreadyExists` instead - not a
/// permission denial and not a missing directory, so the caller reads it as the
/// non-committal outcome, which is the honest answer: something else owns that
/// name and this probe learned nothing about the directory.
pub(super) fn create_write_probe(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Ask the destination directory directly, because the error chain would not
/// say: create a file in it and delete it again.
///
/// Only ever called on a save that has ALREADY failed, so the transient probe
/// file costs nothing on the happy path.
fn probe_destination_dir(dir: &Path) -> DirProbe {
    match dir.try_exists() {
        Ok(true) => {}
        Ok(false) => return DirProbe::Unreachable,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return DirProbe::Denied,
        // A directory whose metadata cannot be read for any other reason (a
        // removed stick, a dropped share, a name the OS will not resolve) is
        // not somewhere a file can be written either.
        Err(_) => return DirProbe::Unreachable,
    }
    let probe = dir.join(format!(
        ".stalagmight-write-probe-{}-{}",
        std::process::id(),
        PROBE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    match create_write_probe(&probe) {
        Ok(file) => {
            drop(file);
            if let Err(e) = std::fs::remove_file(&probe) {
                tracing::debug!(
                    path = ?probe,
                    error = %e,
                    "Could not remove the write probe; it is left behind"
                );
            }
            DirProbe::Writable
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => DirProbe::Denied,
        Err(_) => DirProbe::Other,
    }
}

/// Distinguishes concurrent probes in the same process.
static PROBE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Classify a failed save of `target`.
///
/// Three signals, in order:
///
/// 1. the error chain itself, when it is explicit (a typed `PermissionDenied`
///    or `NotFound`, or a message that names one);
/// 2. the destination being longer than SQLite's win32 VFS can open - which its
///    parent directory stat-ing perfectly fine does NOT rule out, because Rust's
///    own std calls go through `\\?\` verbatim paths and SQLite's do not;
/// 3. SQLite's ambiguous `SQLITE_CANTOPEN` message, resolved by probing the
///    destination directory. A directory that accepts a file is denying us
///    nothing, so ONLY a probe that is itself refused makes this a permission
///    denial - the corroboration that stops an over-`MAX_PATH` Windows path
///    being answered with macOS Privacy & Security steps (#429).
fn classify_save_failure(target: &Path, err: &anyhow::Error) -> SaveFailureKind {
    if crate::core::loading::is_permission_denied(err) {
        return SaveFailureKind::PermissionDenied;
    }
    if crate::core::loading::is_path_not_found(err) {
        return SaveFailureKind::PathNotFound;
    }
    if !crate::core::loading::mentions_unopenable_database(err) {
        return SaveFailureKind::Other;
    }
    // The temp file the pipeline opens FIRST is `<name>.stalx.tmp`, four
    // characters longer than the destination, so measure that one.
    if cfg!(windows)
        && crate::core::loading::over_legacy_path_limit(&target.with_extension("stalx.tmp"))
    {
        return SaveFailureKind::PathTooLong;
    }
    let Some(dir) = target.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return SaveFailureKind::Other;
    };
    match probe_destination_dir(dir) {
        DirProbe::Denied => SaveFailureKind::PermissionDenied,
        DirProbe::Unreachable => SaveFailureKind::PathNotFound,
        DirProbe::Writable | DirProbe::Other => SaveFailureKind::Other,
    }
}

#[cfg(test)]
thread_local! {
    /// [#576 RV5-1] Test seam: run once on the saving thread when the temp file is
    /// complete and closed and before it is renamed over the destination, so a
    /// test can land another write in exactly that window.
    pub(super) static AFTER_TEMP_WRITTEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// [#576 FZ-PERSIST] Test seam: run once on the saving thread with the temp
    /// database as soon as it is created and before anything is written into it,
    /// so a fault-injection test can make a later write fail (a disk that fills up
    /// part-way through the save).
    pub(super) static AFTER_TEMP_OPENED: std::cell::RefCell<Option<Box<dyn FnOnce(&Database)>>> =
        const { std::cell::RefCell::new(None) };
    /// [#576 FZ-PERSIST] Test seam: when set, the reopen of the saved file after
    /// the rename reports a failure (taken once), as an unreadable file would.
    pub(super) static FAIL_REOPEN_AFTER_RENAME: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// The refusal for a save to `path` whose folder is not there, or `None`.
///
/// The destination folder has to be there before anything is opened. SQLite
/// answers a deleted folder or an unmapped drive with the same errno-free
/// "unable to open database file" as a genuine denial, so asking first is what
/// lets the user be told which one it is instead of being handed a raw database
/// error (#429). [#576 W27] It also keeps SQLite off a path the kernel cannot
/// walk: a folder that is not there followed by `..` (`missing/../cave.stalx`)
/// fails every file call the save makes, but SQLite resolves `missing/..` by
/// the name alone and opens its database at a name the kernel never reaches.
pub(super) fn unreachable_folder_refusal(path: &Path) -> Option<CommandResult> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())?;
    let reachable = match parent.try_exists() {
        Ok(present) => present,
        // A folder whose metadata cannot be read at all - a removed stick, a
        // dropped share - cannot host the save either. A permission error is the
        // one exception: the folder IS there, and the pipeline classifies that
        // denial with its own guidance.
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
    };
    if reachable {
        return None;
    }
    tracing::warn!(
        path = ?path,
        parent = ?parent,
        purpose = "persist edits safely",
        "Save refused: the destination folder is not reachable"
    );
    Some(CommandResult::refused(CommandError::new(
        CommandErrorCode::FsPathNotFound,
        format!(
            "Cannot save to {}: the folder no longer exists or the drive is not connected",
            path.display()
        ),
    )))
}

/// Build a save-failure `CommandResult`, tagging it with the code that tells
/// the frontend which remediation to show, and logging the full error chain -
/// the one place every failing save passes through, so a failure that only ever
/// happens on a user's machine is still diagnosable from their log (#429).
pub(super) fn save_failure(context: &str, target: &Path, err: &anyhow::Error) -> CommandResult {
    let detail = format!("{context}: {err}");
    let kind = classify_save_failure(target, err);
    tracing::warn!(
        path = ?target,
        stage = context,
        classification = kind.as_str(),
        chain = %crate::core::loading::error_chain(err),
        purpose = "persist edits safely",
        "Save failed"
    );
    match kind.code() {
        Some(code) => CommandResult::refused(CommandError::new(code, detail)),
        None => CommandResult::failure(detail),
    }
}

/// [#576 FZ-PERSIST-fs-3, RVF-b-3] Clear a save's temp name `temp` before the
/// temp database is created there: whatever is at it - a leftover temp, or a
/// link, even one that leads nowhere yet (`exists()` follows a link and says no
/// for a dangling one) - is removed, and `remove_file` removes a link itself,
/// never its target. An entry that cannot be removed (a folder the user cannot
/// change, a link someone else owns in a shared folder) refuses the save with
/// nothing written: the temp database would be created THROUGH a link left
/// there, wherever it points, the write policy having judged only `target`'s
/// name. Worded as that policy words a refusal.
pub(super) fn clear_save_temp(temp: &Path, target: &Path) -> Result<(), Box<CommandResult>> {
    if std::fs::symlink_metadata(temp).is_err() {
        return Ok(());
    }
    match std::fs::remove_file(temp) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            let name_of = |path: &Path| {
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                )
            };
            tracing::warn!(
                path = ?target,
                temp_path = ?temp,
                error = %e,
                purpose = "persist edits safely",
                "Save refused: the entry at the temp name could not be removed"
            );
            Err(Box::new(CommandResult::failure(format!(
                "Refused to change {}: {} beside it, the name it is written under first, \
                 is taken and could not be removed ({e})",
                name_of(target),
                name_of(temp)
            ))))
        }
    }
}

/// [#576 NF-W14BE-1] Give every section that has no creation time yet `now`.
///
/// On the MODEL, not only in the file: the `.stalx` `sections.created_at`
/// column is written from `Section::created_at`, so a section stamped here
/// writes this same value on every later save - the time it was first saved,
/// not the time of the latest save. A section that already has one (read back
/// on open, carried by an undo) keeps it.
pub(super) fn stamp_section_creation_times(project: &mut Project, now: DateTime<Utc>) {
    for section in &mut project.sections {
        section
            .created_at
            .get_or_insert_with(|| CreationStamp::at(now));
    }
}

/// The undo-less heals a save applies to the LIVE project, in one place.
///
/// Every one of them mutates the open project with **no undo entry of its own**,
/// which is what makes them interesting: a `ProjectSnapshotPair` captured before
/// a save holds the pre-heal project in both halves, so undoing (or redoing) an
/// unrelated entry recorded before the save would otherwise revert the heal and
/// make the project at a given undo depth depend on the route taken to reach it
/// (issue #413, finding 11). [`reapply_heals_after_restore`] is the other half
/// of that fix and documents, heal by heal, which of these a restore re-applies.
///
/// Called AFTER the unresolvable-cross-reference gate in [`AppState::save_project`],
/// deliberately: two of these (the non-finite sanitize above all) are destructive,
/// and a REFUSED save must leave the open project byte-for-byte unchanged.
fn apply_save_heals(project: &mut Project) {
    // Mint UUIDs for any shot still missing one (pre-v6 .stalx
    // files loaded today come back with NULL uuid columns). The UUID
    // is the station identity every cross-reference anchors to
    // (`StationKey::AfterShot`), and it is what keeps the `.tml`
    // SpeleoDB renders byte-stable across re-exports, so it has to be
    // settled BEFORE the SQLite write rather than minted per-consumer.
    project.ensure_shot_uuids();
    // Re-mint any within-section duplicate shot UUID before refs are resolved
    // to positional form for persistence. resolve_key snaps AfterShot(uuid) to
    // the FIRST matching shot, so a ref anchored on a later duplicate would be
    // written to the wrong station - duplicate_shot_uuid is a Critical violation
    // that the unresolvable-cross-reference save gate does NOT catch (the
    // ref still resolves, just to the wrong shot). Re-minting is lossless and
    // non-destructive: the kept (first) occurrence keeps its UUID so refs
    // anchored on it stay faithful, and it clears the Critical so the heal isn't
    // re-detected on the next load. (Mirrors the load-path dedup in
    // redrive_offsets_after_load; non-destructive, unlike dropping a dangling
    // ref, so unlike those it is safe to auto-heal on save rather than block.)
    project.ensure_unique_shot_uuids();

    // Capture the least-squares gauge pair for any section that lacks one (#24), now that
    // every shot is guaranteed a UUID. Recording the station the section is ALREADY using
    // means capturing never moves anybody's coordinates; from here on the gauge is named by
    // a pair of shots rather than re-derived from an index that a later append can shift.
    project.capture_missing_gauge_anchors();

    // Stamp each section's creation time on its first save (#576 NF-W14BE-1).
    stamp_section_creation_times(project, Utc::now());

    // Drop section_offsets for sections that no longer exist. An orphan entry
    // (a section removed via a path that bypassed cleanup) is dead data, and the
    // SQLite section_offsets row carries a FOREIGN KEY to sections(id) - writing
    // it aborts the ENTIRE save with a generic FK error rather than routing the
    // user to Repair. Lossless (matches repair_structural_integrity), so heal on
    // save. (orphan_section_offset is Critical but the save gate only sees
    // station cross-refs, not section_offsets.)
    project.drop_orphan_section_offsets();

    // Sanitize non-finite (NaN/inf) section offsets before persisting, mirroring
    // the load-boundary guard and the shot sanitize below. section_offsets has no
    // other finite guard, and a non-finite offset would either be coerced to NULL
    // by SQLite (a NaN, aborting the save) or persist (an inf) and poison every
    // world coordinate for that component on the next load.
    //
    // Through `write_section_offset`, not by rewriting the tuple in place: a
    // fully-sanitized offset is (0, 0), which means "not displaced", and this
    // map spells that as an ABSENT key. Writing the zero row instead persisted
    // an inert entry the same project reached by any other route would not have
    // - the exact shape `write_section_offset` exists to prevent (#413 finding
    // 2) - and the row then had to be loaded, cascaded over and re-saved.
    let non_finite: Vec<SectionId> = project
        .section_offsets()
        .iter()
        .filter(|(_, offset)| !offset.0.is_finite() || !offset.1.is_finite())
        .map(|(id, _)| *id)
        .collect();
    for id in non_finite {
        let offset = project.section_offset(id);
        let finite = (
            if offset.0.is_finite() { offset.0 } else { 0.0 },
            if offset.1.is_finite() { offset.1 } else { 0.0 },
        );
        project.set_section_offset(id, finite);
    }

    // Sanitize non-finite (NaN/inf) shot geometry before binding. A NaN cannot
    // be stored in a `REAL NOT NULL` column (SQLite coerces it to NULL → the
    // INSERT aborts the whole save), and an inf would persist and only be
    // neutralized on the next load. The load path already sanitizes, so mirror
    // it here so a save can neither abort nor write unusable geometry.
    // (non_finite_geometry is a detect-only Critical that Repair can't auto-fix,
    // so blocking the save would strand the user; neutralizing matches the load
    // round-trip - the value was already unusable.)
    for section in &mut project.sections {
        for shot in &mut section.shots {
            shot.sanitize_non_finite();
        }
    }

    // [#576 B35(h)] ...and station offsets, the one geometry map that had no
    // finite guard on either side: an infinite component used to be written to
    // the file as it was, to poison every open after it. Component-wise, as the
    // load boundary does: the finite components are a real move.
    Project::sanitize_station_offsets(&mut project.station_offsets);
}

/// Bring a project restored from an undo/redo snapshot up to the standard a
/// saved project is held to, so a history move cannot revert a save's heal
/// (issue #413, finding 11).
///
/// `live` is the project being replaced - what the heals have already been
/// applied to.
///
/// The full save-time set is [`apply_save_heals`]; this is the subset a RESTORE
/// may re-apply, decided per heal rather than wholesale, because a restore that
/// changes more than the save changed is a new route-dependence rather than a
/// fix:
///
/// * `ensure_shot_uuids` - RE-APPLIED. Pure fill-in: it only touches a shot with
///   no uuid, and it cannot alter one that has it. (A no-op in practice today -
///   `Section::new` mints, and the load path mints - which is exactly why it is
///   safe to keep as a guarantee.)
/// * `capture_missing_gauge_anchors` - RE-APPLIED AS A CARRY-FORWARD, not as a
///   fresh capture. The anchors `live` holds are copied onto any restored
///   section that has none AND CAN RESOLVE THEM, so the pair a save recorded
///   survives the undo. It is deliberately NOT the unconditional capture:
///   minting anchors for a project that has never been saved would have a
///   restore invent state no save authorised, and the restored project would
///   then differ from the state it is supposed to be restoring. Neither
///   direction can OVERWRITE an anchor - both the app's capture and this
///   carry-forward skip a section that already has one - so the pair a section
///   is pinned to never moves.
/// * `stamp_section_creation_times` - RE-APPLIED AS A CARRY-FORWARD, for the
///   same reason (#576 NF-W14BE-1): a restored section with no creation time
///   takes the one its live counterpart (same id) holds, so a save recorded
///   before the undo still writes the time the first save stamped. Never over a
///   time the restored section has, and never a fresh stamp - that is the
///   save's to make.
///
/// # The half of finding 11 this does NOT cover, and what does
///
/// KEPT, not superseded, by `AppState::refresh_snapshot_pair_counterpart`
/// (`undo.rs`) - the two cover different halves of a history move and the split
/// is exact:
///
/// * THIS covers the half being RESTORED. It reaches into a payload recorded
///   before a heal and brings it up to date, which nothing else can do: the
///   payload is the only record of that state, so if the restore did not repair
///   it the heal would simply be reverted.
/// * THE REFRESH covers the half being LEFT BEHIND, by re-recording it from the
///   live project. That is what makes the reverse move exact - and it is the only
///   thing that can, because this carry-forward is a LIVE-SECTION LOOKUP and a
///   snapshot pair whose halves disagree about whether a section EXISTS (a
///   `Batch` holding a `CreateSection`, `execute_delete_shot_range_split`'s
///   re-created tail) has no live counterpart at the moment the section comes
///   back (#413, finding 11's re-creation residue).
///
/// Neither subsumes the other, and this one is also on the path of every
/// `ProjectSnapshot`, rollback and load-time restore, where there is no pair to
/// refresh at all.
/// * `ensure_unique_shot_uuids` - NOT re-applied. It RE-MINTS an existing uuid,
///   which is not fill-in, and the pre-repair state of a `duplicate_shot_uuid`
///   Repair is a snapshot that intentionally holds duplicates: undoing that
///   Repair has to restore them exactly (`restore_snapshot_into` says the same
///   thing about its legacy-canonicalization block, and
///   `undo_of_duplicate_uuid_repair_restores_the_duplicates` pins it).
/// * the orphan `section_offsets` retain - NOT re-applied. It DROPS state rather
///   than filling it in, and an orphan key is inert until the next save: every
///   consumer looks offsets up by an id in `sections`, and the save runs its own
///   retain before the FK-bearing write. Restoring it faithfully keeps undo an
///   exact inverse.
/// * the non-finite sanitize - NOT re-applied. It is destructive, which is why
///   the save runs it only after the save gate and why a refused save
///   deliberately leaves it undone. An undo must not rewrite the user's values;
///   the load and save boundaries neutralize them where it matters.
pub(super) fn reapply_heals_after_restore(restored: &mut Project, live: Option<&Project>) {
    restored.ensure_shot_uuids();
    let Some(live) = live else {
        return;
    };
    // [#576 NF-W18-1] The project's creation time as the live project holds it
    // (its file's text) when the snapshot names the same moment: one persisted
    // before that text was kept holds it as `DateTime<Utc>` serialized it, which
    // the next save would otherwise write.
    if restored.created_at.utc() == live.created_at.utc() {
        restored.created_at.clone_from(&live.created_at);
    }
    for section in &mut restored.sections {
        if section.created_at.is_none() {
            section.created_at = live
                .get_section(section.id)
                .and_then(|s| s.created_at.clone());
        }
        if section.gauge_shot_uuid.is_some() {
            continue;
        }
        let Some(healed) = live
            .sections
            .iter()
            .find(|candidate| candidate.id == section.id)
        else {
            continue;
        };
        let Some(gauge_shot) = healed.gauge_shot_uuid.as_deref() else {
            continue;
        };
        // Matched by id, but the pair names shots BY UUID - and the section this
        // is being copied onto is a DIFFERENT state of that id, which need not
        // hold them. `DeleteShot` records a snapshot pair; save while the undo
        // has the deleted shot back, and the pair a save captures names a shot
        // the `after` half never had.
        //
        // A pair that does not resolve is not a weaker pin, it is no pin at all,
        // and it is worse than none: `Project::gauge_index` silently falls back
        // to the index rule, and `capture_missing_gauge_anchors` skips every
        // section whose `gauge_shot_uuid.is_some()` - so the section stays on the
        // fallback, and the gauge walks onto a new far station the next time the
        // section is extended, which is the whole defect the pair exists to
        // prevent (#24). Leaving the anchors as the snapshot had them (unset) is
        // what lets the next save capture a pair the section can actually
        // resolve.
        let holds = |uuid: &str| {
            section
                .shots
                .iter()
                .any(|s| s.uuid.as_deref() == Some(uuid))
        };
        // The neighbour is legitimately absent for a single-shot section, so
        // "unset" passes and only a RECORDED one has to resolve.
        if !holds(gauge_shot) || !healed.gauge_neighbour_uuid.as_deref().is_none_or(holds) {
            continue;
        }
        section.gauge_shot_uuid = healed.gauge_shot_uuid.clone();
        section.gauge_neighbour_uuid = healed.gauge_neighbour_uuid.clone();
    }
}

impl AppState {
    /// True when the open project's initial save is still deferred - it was created
    /// with no save path and has never been written to disk, so it has no `.stalx`
    /// file and no Recent Projects entry yet. Used to durably persist a brand-new
    /// project on its first import (#92), at the command every import path funnels
    /// through, so the fix survives changes to the import UI. Backed by an explicit
    /// flag (set at deferred creation, cleared on save/open/load) rather than
    /// inferred from the DB handle or file existence, both of which are ambiguous.
    pub fn initial_save_deferred(&self) -> bool {
        self.initial_save_deferred
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The refusals a save makes BEFORE it touches the live project or any file:
    /// a project a newer build saved (#576 B6-D1), stored settings the open could
    /// not read (#576 B15), and unresolvable cross-references. Called under the
    /// `project` lock, like the checks it reads.
    ///
    /// [#576 RV2-R-6] Its own method so EVERY in-place write of the model applies
    /// the same rules as the save: the SpeleoDB name fold persists the whole model
    /// beside the save path, and ran before the flush that would have refused - so
    /// it wrote the in-memory defaults over unread settings.
    pub(super) fn model_write_refusal(&self, project: &Project) -> Option<CommandResult> {
        // [#576 B6-D1] A project a NEWER build saved is never written by this one -
        // not in place (Ctrl+S, autosave, the pre-push flush) and not elsewhere
        // either: a Save As would write this build's schema of it, the same silent
        // downgrade under a different name. Checked first, under the project lock
        // its flag was committed under, so a refused save leaves the live project
        // exactly as loaded (the heals below mutate it).
        if let Some(reason) = self.newer_file_refusal() {
            tracing::warn!(
                purpose = "never downgrade a project saved by a newer version",
                "Refused to save a project saved by a newer version of Stalagmight"
            );
            return Some(CommandResult::refused(CommandError::new(
                CommandErrorCode::NewerFileReadOnly,
                format!("Saving is disabled: {reason}"),
            )));
        }

        // [#576 B15] Stored per-project settings the open could not read are held as
        // DEFAULTS in the model, and the core save writes them back: any save, in
        // place or elsewhere, would put the defaults over the stored values. Refused
        // under the side-table policy for the same reason, and before the heals for
        // the reason above.
        if let Some(refusal) =
            super::save_sidecars::unread_settings_refusal(&self.unreadable_project_settings())
        {
            tracing::warn!(
                purpose = "never write defaults over unread project settings",
                "Refused to save: stored project settings could not be read when the project \
                 was opened"
            );
            return Some(refusal);
        }

        // Refuse to persist a project with unresolvable cross-references (a station an
        // endpoint anchors on was deleted) BEFORE the heals below mutate the live
        // project. The heals (UUID mint/dedup, orphan-offset drop, and the DESTRUCTIVE
        // non-finite sanitize) do NOT affect this predicate - it depends only on
        // section existence and anchor resolution - so checking first is equivalent
        // for the block decision but guarantees a REFUSED save leaves the open project
        // byte-for-byte unchanged (the non-finite sanitize rewrites shot values to
        // finite defaults with no undo entry, so mutating on a refused save would
        // silently lose the user's data).
        //
        // This applies to AUTO-SAVE as well as manual saves: AUTO-SAVE MUST NEVER
        // PERSIST A BROKEN PROJECT. When the project is broken the save is refused (no
        // file written) and the caller surfaces that auto-save did not run (the
        // auto-save indicator + the integrity banner); the user restores integrity via
        // Repair, after which saves resume. This is the single chokepoint that
        // guarantees only sound projects reach disk.
        if project.has_unresolvable_cross_references() {
            let count = project
                .check_structural_integrity()
                .iter()
                .filter(|v| {
                    matches!(
                        v.category.as_str(),
                        "dangling_merge_ref"
                            | "dangling_closure_ref"
                            | "dangling_zero_datum_ref"
                            | "orphan_station_offset"
                            | "orphan_station_comment"
                    )
                })
                .count();
            tracing::error!(
                count,
                purpose = "block persisting unresolvable cross-references",
                "Refused to save project with dangling cross-references"
            );
            return Some(CommandResult::failure(format!(
                "Can't save: the project has {count} unresolved cross-reference{} \
                 (a station they point to was deleted). Use \"Repair\" in the \
                 integrity banner, then save.",
                if count == 1 { "" } else { "s" }
            )));
        }
        None
    }

    pub(super) fn save_project(&self, path: Option<PathBuf>) -> CommandResult {
        let mut proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        if let Some(refusal) = self.model_write_refusal(project) {
            return refusal;
        }

        // The undo-less heals, all of them, in `apply_save_heals` - which is
        // also what a snapshot restore consults so an undo of an entry recorded
        // BEFORE this save cannot revert them (#413, finding 11).
        apply_save_heals(project);

        // Don't let geometric corruption persist silently: a release-mode warning
        // when drifted merges are about to be written. (The frontend re-checks
        // after save and offers a repair via the integrity banner.)
        let misaligned = project.check_merge_alignment();
        if !misaligned.is_empty() {
            tracing::warn!(
                count = misaligned.len(),
                purpose = "flag misaligned merges before persisting",
                "Saving a project with drifted station merges"
            );
            for violation in &misaligned {
                tracing::warn!(category = %violation.category, "{}", violation.message);
            }
        }

        // Determine save path and ensure .stalx extension. `CaveA.STALX` already
        // has it: rewriting that to `CaveA.stalx` names a different file on a
        // case-sensitive filesystem, one no overwrite prompt was shown for (B33).
        let is_implicit_save = path.is_none();
        let save_path = path.or_else(|| project.file_path.clone());
        let mut save_path = match save_path {
            Some(p) => crate::persistence::with_stalx_extension(p),
            None => return CommandResult::failure("No save path specified".to_string()),
        };

        // A missing database handle does not mean "first save": a failed reopen
        // leaves one behind too (`DATABASE_NOT_REOPENED` below), on a project whose
        // file exists and is the only place its ignores, display settings and
        // SpeleoDB link live. Every side-table read below comes from this handle.
        if let Some(refusal) = self.restore_missing_database(project, &save_path) {
            return refusal;
        }

        // On the first implicit save (path omitted), avoid overwriting an existing file.
        // Deferred new projects can reserve a canonical path; if it now exists, auto-rename.
        // Decided by the explicit flag: inferring it from the missing handle sent every
        // autosave after a failed reopen to `<stem>_1.stalx` (#576 B35(c)).
        let deferred = self.initial_save_deferred();
        if is_implicit_save && deferred {
            let candidate_path = Self::next_available_project_path(&save_path);
            if candidate_path != save_path {
                tracing::warn!(
                    requested_path = ?save_path,
                    selected_path = ?candidate_path,
                    purpose = "avoid overwriting existing project on first implicit save",
                    "Implicit first save path already exists; using auto-renamed path"
                );
                save_path = candidate_path;
            }
        }

        if let Some(refused) = unreachable_folder_refusal(&save_path) {
            return refused;
        }

        // Create a temporary file path in the same directory (for atomic rename)
        let temp_path = save_path.with_extension("stalx.tmp");
        tracing::info!(
            final_path = ?save_path,
            temp_path = ?temp_path,
            purpose = "persist edits safely",
            "Starting safe save operation"
        );

        // [#576 A3] The side tables the core save does not write - display settings,
        // ignored warnings, the SpeleoDB link, the undo-chain markers, the undo
        // history - are carried by the registry in `save_sidecars`, each under its
        // failure policy. Snapshotted from the open file before any temp file
        // exists, so a refusal here has written nothing; the same pass resolves the
        // mode, which the link decides.
        let rewrites_open_file = project
            .file_path
            .as_ref()
            .is_some_and(|open| same_file(open, &save_path));
        let carried = match self.snapshot_sidecars(
            project,
            SaveRequest {
                destination: &save_path,
                deferred,
                implicit: is_implicit_save,
                rewrites_open_file,
            },
        ) {
            Ok(carried) => carried,
            Err(refusal) => return *refusal,
        };
        let mode = carried.mode();

        // Save a Copy: an EXPLICIT destination on a SpeleoDB-managed project does
        // not relocate the session - it writes a detached copy WITHOUT the
        // `speleo_link` row and leaves the session on the managed file. A managed
        // project is usable at exactly one path (its cache slot - see
        // `speleodb_foreign`), so carrying the link to an arbitrary destination
        // would mint the second linked copy that module exists to refuse. The
        // predicate is safe for every in-tree caller: `speleodb_publish` and the
        // Ariane adopt both save to the cache path while the project is STILL
        // UNLINKED (the link is stamped after - pinned by
        // `publish_relocates_into_the_cache_before_it_stamps_the_link`), and
        // `speleodb_upload`'s pre-push flush is an implicit save.
        if mode == SaveMode::DetachedCopy {
            // Refuse the managed file itself as the copy's destination: writing an
            // unlinked copy over it would strip the managed project's link in
            // place - the one outcome this whole arrangement exists to prevent.
            // Both spellings of "itself" are checked: the open file, and the
            // project's canonical cache slot (they are normally the same file, but
            // `same_file` cannot bridge them when one side does not resolve).
            let names_the_managed_file = rewrites_open_file
                || carried.speleo_link().is_some_and(|link| {
                    super::speleodb::validate_project_id(&link.project_id).is_ok()
                        && super::speleodb_local_state::cache_root().is_some_and(|root| {
                            super::speleodb_local_state::is_the_cached_copy(
                                &root,
                                &link.project_id,
                                &save_path,
                            )
                        })
                });
            if names_the_managed_file {
                return CommandResult::failure(format!(
                    "\"{}\" is the managed copy of \"{}\" - Save a Copy needs a \
                     different destination. Use Save to save the project itself.",
                    save_path.display(),
                    project.name
                ));
            }
        }

        // Remove any leftover temp file from previous failed saves - and any link
        // at that name, even one that leads nowhere yet: `exists()` follows a link
        // and says no for a dangling one, and the temp database would then be
        // created THROUGH it, wherever it points (#576 FZ-PERSIST-fs-3). The write
        // policy judged the project's own name, not this one. One that cannot be
        // removed refuses the save (#576 RVF-b-3).
        if let Err(refused) = clear_save_temp(&temp_path, &save_path) {
            return *refused;
        }

        // Save to temporary file first
        let mut db = match Database::open(&temp_path) {
            Ok(db) => db,
            Err(e) => {
                // Clean up temp file on failure
                let _ = std::fs::remove_file(&temp_path);
                return save_failure("Failed to create save file", &save_path, &e);
            }
        };

        #[cfg(test)]
        if let Some(hook) = AFTER_TEMP_OPENED.with(|hook| hook.borrow_mut().take()) {
            hook(&db);
        }

        if let Err(e) = db.save_project(project) {
            // Clean up temp file on failure
            drop(db);
            let _ = std::fs::remove_file(&temp_path);
            tracing::error!(
                error = %e,
                purpose = "persist edits safely",
                "Save failed; temp file cleaned up"
            );
            return save_failure("Failed to save project", &save_path, &e);
        }

        // Every side table, written while the file is still staged, so a partial
        // rewrite is never renamed into place: a table the policy will not save
        // without stops the save here (the temp is discarded, the destination is
        // untouched), and a cosmetic one comes back as a warning. The undo and redo
        // history is persisted synchronously as the last of them.
        let warnings = match self.write_sidecars(&db, carried) {
            Ok(warnings) => warnings,
            Err(failure) => {
                drop(db);
                let _ = std::fs::remove_file(&temp_path);
                return save_failure(&failure.context, &save_path, &failure.error);
            }
        };

        // Close the temp database before renaming
        drop(db);

        #[cfg(test)]
        if let Some(hook) = AFTER_TEMP_WRITTEN.with(|hook| hook.borrow_mut().take()) {
            hook();
        }

        if mode == SaveMode::DetachedCopy {
            // The destination is provably NOT the live file (refused above), so
            // the live connection stays open, the session keeps its file_path and
            // database handle, and no cache is invalidated. Rename the temp into
            // place, list the copy, and report it via `unlinked_copy_path` -
            // deliberately NOT `saved_path`, which the frontend treats as "the
            // open project now lives here".
            if let Err(e) = std::fs::rename(&temp_path, &save_path) {
                let _ = std::fs::remove_file(&temp_path);
                let e = anyhow::Error::from(e);
                tracing::error!(
                    error = %e,
                    purpose = "write a detached unlinked copy",
                    "Failed to rename temp file to the copy destination"
                );
                return save_failure("Failed to finalize save", &save_path, &e);
            }
            // [#576 FZ-FZ-INT-R2] As below: the copy replaced whatever was there.
            self.note_save_renamed();
            drop(proj_guard);
            self.note_recent_project(&save_path);
            tracing::info!(
                path = ?save_path,
                purpose = "write a detached unlinked copy",
                "Saved an unlinked copy of a SpeleoDB-managed project"
            );
            let mut result =
                CommandResult::success().with_unlinked_copy_path(save_path.display().to_string());
            result.warnings.extend(warnings);
            return result;
        }

        // Close our own persistent connection to the destination BEFORE the
        // rename. Windows refuses to rename a file over a destination that
        // still has an open handle: SQLite's Connection opens the file without
        // FILE_SHARE_DELETE, so MoveFileEx(REPLACE_EXISTING) fails with
        // "Access is denied" (os error 5). Unix renames over an open file fine
        // (the live handle keeps pointing at the old inode), which is why this
        // only bit Windows. We reopen below - at save_path on success (existing
        // logic), or at the prior path here if the rename fails - so the
        // session keeps a working connection either way.
        let closed_open_db = self.database.lock().take().is_some();

        // Atomically rename temp file to final destination
        // This is safe: if rename fails, original file is untouched
        if let Err(e) = std::fs::rename(&temp_path, &save_path) {
            let _ = std::fs::remove_file(&temp_path);
            // The destination is untouched. If we just closed our live
            // connection, reopen the previously-open project so subsequent
            // saves, undo persistence, and the implicit-save overwrite guard
            // keep working instead of operating with no open database.
            if closed_open_db {
                if let Some(prev_path) = project.file_path.clone() {
                    if let Ok(reopened) = Database::open(&prev_path) {
                        {
                            let mut database = self.database.lock();
                            *database = Some(reopened);
                        }
                    }
                }
            }
            let e = anyhow::Error::from(e);
            tracing::error!(
                error = %e,
                purpose = "persist edits safely",
                "Failed to rename temp file to final destination"
            );
            return save_failure("Failed to finalize save", &save_path, &e);
        }
        // [#576 FZ-FZ-INT-R2] A file was renamed over `save_path`: a load of it in
        // flight read the file this replaced, and its commit refuses on this.
        self.note_save_renamed();

        // The `.stalx` is now written, so the initial save is no longer deferred.
        // Set this BEFORE the reopen below: that path returns early on failure, and
        // the project is persisted regardless of whether the handle reopens.
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);

        // Reopen the database at the final path
        let reopened = Database::open(&save_path);
        #[cfg(test)]
        let reopened = if FAIL_REOPEN_AFTER_RENAME.with(std::cell::Cell::take) {
            reopened.and(Err(anyhow::anyhow!("injected reopen failure")))
        } else {
            reopened
        };
        let db = match reopened {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    purpose = "reopen saved project for continued editing",
                    "Failed to reopen saved database (file was saved successfully)"
                );
                // File was saved but can't be reopened - subsequent operations
                // (undo persistence, display settings) will fail. Clear the stale
                // database handle so callers don't use it.
                self.note_open_project_moved(project.file_path.as_deref(), &save_path);
                project.file_path = Some(save_path.clone());
                drop(proj_guard);
                {
                    let mut database = self.database.lock();
                    *database = None;
                }
                // Distinct code (not the generic "SYSTEM") so the frontend can keep
                // databaseOpen=false and leave DB-backed actions (e.g. Ignore)
                // disabled until the project is reopened (issue #57 / Codex).
                let mut result = CommandResult::success();
                result.warnings.extend(warnings);
                result.warnings.push(crate::core::ValidationWarning {
                    code: super::save_sidecars::DATABASE_NOT_REOPENED_CODE.to_string(),
                    message: "File saved, but database could not be reopened. Some features may not work until the project is reopened.".to_string(),
                    severity: crate::core::WarningSeverity::Warning,
                });
                return result;
            }
        };

        // Update file path after successful save (a Save As moves the open
        // project: #576 RV10b-4).
        self.note_open_project_moved(project.file_path.as_deref(), &save_path);
        project.file_path = Some(save_path.clone());

        // Update database handle
        drop(proj_guard);
        {
            let mut database = self.database.lock();
            *database = Some(db);
        }

        // Clear in-memory cache since settings are now persisted to DB
        self.clear_display_settings_cache();
        self.invalidate_adjusted_cache();

        // Add to recent projects and persist to config
        self.note_recent_project(&save_path);

        let save_path_str = save_path.display().to_string();
        tracing::info!(
            path = ?save_path,
            purpose = "persist edits safely",
            "Project saved successfully"
        );
        let mut result = CommandResult::success().with_path(save_path_str);
        result.warnings.extend(warnings);
        result
    }

    /// Reinstall the open project's database handle when it is missing on a project
    /// whose file exists (#576 B35(c)). Returns the refusal `save_project` must hand
    /// back when the file cannot be reopened and this save would replace it.
    ///
    /// The handle is legitimately absent only for a deferred new project (no file
    /// yet) or one never saved (no `file_path`); both are left alone. Otherwise it is
    /// missing because a reopen failed - after the rename (`DATABASE_NOT_REOPENED`),
    /// or after a failed rename when the previous file would not reopen either - and
    /// the save's side-table snapshots would read "nothing" from it.
    ///
    /// Display settings changed while the handle was gone went to the in-memory
    /// cache, which the save reads only when the file has none; they are written
    /// into the reopened file, as they would have been with the handle in place.
    ///
    /// A Save As to a DIFFERENT file goes ahead without the handle: the open file,
    /// and the side tables in it, stay untouched on disk, and it is the user's way
    /// out when the open file cannot be reopened.
    fn restore_missing_database(
        &self,
        project: &Project,
        save_path: &Path,
    ) -> Option<CommandResult> {
        if self.initial_save_deferred() {
            return None;
        }
        let open_path = project.file_path.as_ref()?;
        let mut db_guard = self.database.lock();
        if db_guard.is_some() {
            return None;
        }
        match Database::open_existing(open_path) {
            Ok(db) => {
                let cached = self.display_settings_cache.lock().clone();
                for (key, value) in &cached {
                    if let Err(e) = db.save_display_setting(key, value) {
                        tracing::warn!(
                            key = %key,
                            error = %e,
                            purpose = "preserve display settings on save",
                            "Failed to write a cached display setting into the reopened database"
                        );
                    }
                }
                tracing::info!(
                    path = ?open_path,
                    purpose = "read the project's side tables before saving",
                    "Reopened the project database the session had lost"
                );
                *db_guard = Some(db);
                None
            }
            Err(e) if !same_file(open_path, save_path) => {
                tracing::warn!(
                    error = %e,
                    path = ?open_path,
                    destination = ?save_path,
                    purpose = "save to another file without the unreadable open file",
                    "Could not reopen the project database; saving to the other file without its side tables"
                );
                None
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    path = ?open_path,
                    purpose = "refuse to rewrite a project without its side tables",
                    "Refused to save: the project database could not be reopened"
                );
                Some(CommandResult::failure(format!(
                    "Can't save: the project file could not be reopened ({}). Saving would \
                     rewrite it without its ignored warnings, display settings and SpeleoDB \
                     link, so nothing was written - the file on disk is unchanged. Use Save \
                     As to save your changes to another file.",
                    crate::core::loading::clamp_quoted_error(&e.to_string()),
                )))
            }
        }
    }

    pub(super) fn persist_or_clear_undo_stacks_into(&self, db: &Database) -> anyhow::Result<bool> {
        // [#576 RV5-PI-2] After a crash under `command_lock`, `project` or
        // `undo_stack` the history may be out of step with the survey: undo and
        // redo refuse it for this session (`refuse_after_crash`). Persisting it
        // would hand it to the restarted session, which has nothing poisoned and
        // would replay it. The survey is saved with an EMPTY history instead, and
        // the save says so (`UNDO_HISTORY_DROPPED`), like a history that could not
        // be written.
        if self.refuse_after_crash("Saving the Undo history").is_err() {
            db.clear_history_stacks()?;
            return Ok(true);
        }
        let undo_stack = self.undo_stack.lock();
        let (undo_rows, redo_rows) = self.history_rows_to_persist(db, &undo_stack);
        match super::save_sidecars::replace_history_rows(db, undo_rows, redo_rows) {
            Ok(()) => Ok(false),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "save project without partial undo history",
                    "Could not persist the complete Undo and Redo history; saving with both stacks empty"
                );
                db.clear_history_stacks()?;
                Ok(true)
            }
        }
    }

    /// Persist an already-locked stack. The confirmed-push path uses this while it
    /// holds the live database and undo guards, so no late edit can land between
    /// pruning memory and rewriting SQLite.
    pub(super) fn persist_locked_undo_stacks_into(
        &self,
        db: &Database,
        undo_stack: &UndoStack,
    ) -> bool {
        // [#303] A Full-mode SpeleoDB-linked project persists its WHOLE history (bounded
        // by the byte budget, not by a count) because that history is what rides in the
        // next commit and gives undo its cross-commit reach. Everything else - local
        // projects, Disabled mode - keeps the `undo_persist_limit` window, byte-capped
        // by `UNDO_BYTE_BUDGET_BYTES` (#576 B20).
        //
        // The link is read from the LOCAL `db` handle rather than through
        // `undo_policy()`: `self.database` still points at the pre-save handle when the
        // normal save calls this, so asking the AppState would consult the wrong file.
        let (undo_rows, redo_rows) = self.history_rows_to_persist(db, undo_stack);
        match db.replace_history_stacks_stored(&undo_rows, &redo_rows) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "preserve undo and redo history atomically",
                    "Failed to persist undo and redo stacks"
                );
                false
            }
        }
    }

    fn history_rows_to_persist(
        &self,
        db: &Database,
        undo_stack: &UndoStack,
    ) -> (
        Vec<crate::persistence::StoredHistoryRow>,
        Vec<crate::persistence::StoredHistoryRow>,
    ) {
        let count_limit = {
            let policy = self.undo_policy_for(db.load_speleo_link().ok().flatten().as_ref());
            if policy.unlimited_depth() {
                None
            } else {
                Some(self.config.lock().general.undo_persist_limit)
            }
        };

        // Every save writes the WHOLE (windowed) stack, because the pipeline above
        // built a fresh temp file and renamed it over the target: the reopened `db`
        // has no undo tables to add to. See `UndoStack::rows_to_persist`. [#576 B20]
        // It does not have to COMPRESS them all: each entry's gzipped rows are kept
        // after the first save, so only entries new since then are compressed.
        let undo_rows = undo_stack.stored_rows_to_persist(PersistStack::Undo, count_limit);

        // [#303] Redo is persisted in EVERY mode, including Full, where that means a
        // save can write up to 2x `UNDO_BYTE_BUDGET_BYTES` - the budget is applied to
        // the two stacks independently.
        //
        // That doubling is deliberate and was re-decided after review, so do not
        // "fix" it by skipping redo in Full mode: that change was made and reversed.
        // The reasoning is that 64 MiB is a CEILING, not growth - each stack is
        // bounded by the budget, and a confirmed push clears the undo history - and
        // that persisting redo is what keeps a redo branch alive across a close and
        // reopen. Dropping it bought a smaller save on exactly the projects whose
        // saves are largest, and cost redo-after-reopen on all of them, which is the
        // worse end of the trade.
        //
        // None of the redo half reaches the server either way: `redo_stack` is
        // unconditionally in `PUSH_SNAPSHOT_LOCAL_TABLES`, so this is local-only cost
        // for a local-only feature.
        let redo_rows = undo_stack.stored_rows_to_persist(PersistStack::Redo, count_limit);
        (undo_rows, redo_rows)
    }

    /// Put `path` at the head of the recent-projects list and persist it to
    /// config. Takes no state locks other than `recent_projects` + `config`;
    /// callers must NOT hold `project` (the save paths drop it first).
    fn note_recent_project(&self, path: &Path) {
        {
            let mut recent = self.recent_projects.lock();
            recent.retain(|p| p != path);
            recent.insert(0, path.to_path_buf());
            // Under the config write lock (#576 RV4-R-M2).
            if let Err(e) = self.persist_recent_projects(&mut recent, true) {
                tracing::warn!(
                    error = %e,
                    purpose = "persist recent projects list",
                    "Failed to save config with recent projects"
                );
            }
        }
    }

    /// Drop the open project's resources.
    ///
    /// Every lock here is RECOVERED from poisoning rather than skipped. A poisoned
    /// mutex means some earlier call panicked while holding it; the guarded values
    /// are `Option<Project>` / `Option<Database>` / the undo stack, and setting
    /// them to `None`/empty is precisely the recovery - there is no half-written
    /// state to protect. Skipping the drop, by contrast, is never safe: this
    /// function's teardown (`close_open_project_locked`) is the pre-replace close
    /// that `close_project_if_open_at_held` relies on (#576 RV9a-8), and it used to
    /// report `true` while still holding the live SQLite handle and
    /// its 256 MB mmap, so the caller went on to rename a new file over it - the
    /// exact SIGBUS / `MoveFileEx`-denied hazard the guard exists to prevent.
    pub(super) fn close_project(&self) -> CommandResult {
        // [#576 A1] `command_lock` first: a close landing inside an edit or an undo
        // would leave that command's entry on the emptied session's stacks.
        let commands = self.hold_commands();
        self.close_project_held(&commands)
    }

    /// [`close_project`](Self::close_project) inside a `command_lock` span the caller
    /// already holds [#576 NF-W4C-3] - the Ariane adoption's, whose close before the
    /// promote and whose failure-path closes are steps of that one span.
    pub(super) fn close_project_held(&self, commands: &CommandsHeld<'_>) -> CommandResult {
        tracing::debug!("close_project: acquiring project lock");
        let mut proj = self.project.lock();
        self.close_open_project_locked(commands, &mut proj)
    }

    /// [`close_project`](Self::close_project)'s teardown, with the `project` guard
    /// supplied by the caller so that a caller which must first DECIDE whether to
    /// close can decide and close under ONE acquisition.
    ///
    /// `delete_project` is that caller: it closes only when the `.stalx` it is
    /// about to hand to the OS delete is the file backing the open project, and
    /// doing the check through `open_project_path` (which releases the lock) and
    /// only then calling `close_project` (which re-acquires it) is a TOCTOU - a
    /// load that finished installing a DIFFERENT project in that gap had ITS
    /// database handle and undo stack torn down while the delete went on to remove
    /// the original file.
    ///
    /// Holding `project` across the `database` and `undo_stack` acquisitions below
    /// is the ordering the rest of the state layer already uses - `save_project`
    /// holds its `project` guard while it takes both - and no path acquires either
    /// of those BEFORE `project`, so it cannot deadlock. Calling
    /// [`close_project`](Self::close_project) with the guard held, by contrast,
    /// would self-deadlock on its own first acquisition.
    ///
    /// The caller holds the `project` guard, taken through `RecoveringMutex::lock`
    /// like every lock (#576 A11), so a poisoned `project` cell is already
    /// recovered by the time it gets here; the locks taken here recover theirs
    /// themselves.
    pub(super) fn close_open_project_locked(
        &self,
        commands: &CommandsHeld<'_>,
        project: &mut ProjectSlot,
    ) -> CommandResult {
        // [#576 A5] The same commit every load and create goes through, installing the
        // empty session: every per-project cell back to its no-project value - the undo
        // stack's depth bound AND its inherited-history watermark included (#576
        // NF-AN3-6) - and the session generation bumped under the caller's guard (the
        // `speleodb_session` rule). Covers close, delete and the project switch, which
        // all route through here.
        self.commit_open_session_into(commands, project, SessionCommit::closed());
        // [#576 SDB-POLL-D1] An acquire whose reply was lost is settled by a poll of
        // the project it was sent for; the close ends that session.
        self.speleodb_drop_unconfirmed_acquire();
        tracing::info!(purpose = "release project resources", "Project closed");
        CommandResult::success()
    }

    /// The path of the `.stalx` currently open, when one is open and it has been
    /// saved somewhere (a never-saved project has no `file_path`).
    ///
    /// Recovers a poisoned lock rather than reporting "nothing open": every caller
    /// uses this to decide whether the live handle / live link belongs to a file it
    /// is about to touch, and answering "no" on a poisoned mutex is the unsafe
    /// direction in both.
    pub(super) fn open_project_path(&self) -> Option<PathBuf> {
        let guard = self.project.lock();
        guard.as_ref().and_then(|p| p.file_path.clone())
    }

    /// [`Self::close_project_if_open_at_held`] under a `command_lock` span of its own.
    ///
    /// [#576 RV6a-5] Test builds only: the one production caller, the cache
    /// replace, closes inside its own `command_lock` span
    /// ([`Self::close_project_if_open_at_held`]).
    #[cfg(test)]
    pub(super) fn close_project_if_open_at(&self, dest: &Path) -> bool {
        let commands = self.hold_commands();
        self.close_project_if_open_at_held(&commands, dest)
    }

    /// Close the open project IF AND ONLY IF it is the file at `dest`; returns
    /// whether it was closed.
    ///
    /// Guards every path that REPLACES a `.stalx` on disk out from under the app
    /// (the SpeleoDB cache download-replace: pull / fast-forward / revision swap).
    /// Two reasons the live handle has to go first:
    ///
    /// - `Database::open_existing` enables a 256 MB mmap over the file. Truncating
    ///   or replacing a file that SQLite has mmap'd can fault the mapped pages
    ///   (SIGBUS) and hands the connection a torn view of a different database.
    /// - On Windows, SQLite opens the file WITHOUT `FILE_SHARE_DELETE`, so
    ///   `MoveFileEx(REPLACE_EXISTING)` over a live handle fails with "Access is
    ///   denied" - the same hazard the save pipeline's pre-rename close covers.
    ///
    /// Closing is deliberately conditional: these callers replace a file that is
    /// USUALLY, but not always, the open project (a pull can target a cached
    /// project the user does not currently have open), and dropping an unrelated
    /// open project would throw away its undo/redo history for nothing.
    ///
    /// [#576 A1] Decided and closed under ONE `project` acquisition inside
    /// `command_lock`, like `delete_project`'s check-and-teardown: checking through
    /// `open_project_path` and closing through `close_project` re-took the lock in
    /// between, so a load finishing in that gap had ITS session torn down instead.
    ///
    /// Inside a `command_lock` span the caller already holds - the cache replace's,
    /// which reads the display settings to carry over in the same span (#576 RV6a-5).
    pub(super) fn close_project_if_open_at_held(
        &self,
        commands: &CommandsHeld<'_>,
        dest: &Path,
    ) -> bool {
        let mut open = self.project.lock();
        let is_dest = open
            .as_ref()
            .and_then(|p| p.file_path.as_deref())
            .is_some_and(|open_path| same_file(open_path, dest));
        if !is_dest {
            return false;
        }
        tracing::info!(
            path = ?dest,
            purpose = "release the live handle before replacing the file",
            "Closing the open project because its .stalx is about to be replaced"
        );
        self.close_open_project_locked(commands, &mut open);
        true
    }
}

/// Whether two paths denote the same file on disk.
///
/// Canonicalization resolves `.`/`..`, symlinks, and (on Windows) case/short-name
/// differences, so two spellings of the open project's path still compare equal.
/// When either side cannot be canonicalized - most commonly because `dest` does
/// not exist yet (a first pull into an empty cache) - it cannot be the file a live
/// connection is holding unless it is LITERALLY the same path, so fall back to a
/// plain comparison instead of guessing.
///
/// Shared with `speleodb_local_state`, where the same question ("is the open
/// project THIS file?") decides whether the live in-memory link may vouch for a
/// copy that could not be read.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}
