use super::AppState;
use crate::core::{CommandResult, PersistStack, UndoStack};
use crate::models::{Project, SectionId};
use crate::persistence::Database;
use std::path::{Path, PathBuf};

/// What a failed save actually was, once the error chain and the destination
/// have both been consulted. Each variant except `Other` carries a marker the
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
    /// The marker prefixed onto the `CommandResult` error, or `None` when the
    /// message stands on its own.
    fn code(self) -> Option<&'static str> {
        match self {
            Self::PermissionDenied => Some(crate::core::loading::PERMISSION_DENIED_CODE),
            Self::PathNotFound => Some(crate::core::loading::PATH_NOT_FOUND_CODE),
            Self::PathTooLong => Some(crate::core::loading::PATH_TOO_LONG_CODE),
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

/// Build a save-failure `CommandResult`, tagging it with the marker that tells
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
        Some(code) => CommandResult::failure(format!("{code}: {detail}")),
        None => CommandResult::failure(detail),
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

    // Drop section_offsets for sections that no longer exist. An orphan entry
    // (a section removed via a path that bypassed cleanup) is dead data, and the
    // SQLite section_offsets row carries a FOREIGN KEY to sections(id) - writing
    // it aborts the ENTIRE save with a generic FK error rather than routing the
    // user to Repair. Lossless (matches repair_structural_integrity), so heal on
    // save. (orphan_section_offset is Critical but the save gate only sees
    // station cross-refs, not section_offsets.)
    let existing_sections: std::collections::HashSet<SectionId> =
        project.sections.iter().map(|s| s.id).collect();
    project
        .section_offsets
        .retain(|sid, _| existing_sections.contains(sid));

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
        .section_offsets
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
        Project::write_section_offset(&mut project.section_offsets, id, finite);
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
    for section in &mut restored.sections {
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

    pub(super) fn save_project(&self, path: Option<PathBuf>) -> CommandResult {
        self.save_project_at_generation(path, None)
    }

    pub(super) fn save_project_at_generation(
        &self,
        path: Option<PathBuf>,
        expected_generation: Option<u64>,
    ) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        if expected_generation
            .is_some_and(|generation| self.speleodb_session_generation() != generation)
        {
            return CommandResult::failure("The project session changed before saving".to_string());
        }
        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

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
            return CommandResult::failure(format!(
                "Can't save: the project has {count} unresolved cross-reference{} \
                 (a station they point to was deleted). Use \"Repair\" in the \
                 integrity banner, then save.",
                if count == 1 { "" } else { "s" }
            ));
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

        // Determine save path and ensure .stalx extension
        let is_implicit_save = path.is_none();
        let save_path = path.or_else(|| project.file_path.clone());
        let mut save_path = match save_path {
            Some(p) => {
                // Ensure .stalx extension
                if p.extension().is_none_or(|ext| ext != "stalx") {
                    p.with_extension("stalx")
                } else {
                    p
                }
            }
            None => return CommandResult::failure("No save path specified".to_string()),
        };

        // On the first implicit save (path omitted), avoid overwriting an existing file.
        // Deferred new projects can reserve a canonical path; if it now exists, auto-rename.
        let has_open_database = self.database.lock().map(|db| db.is_some()).unwrap_or(false);
        if is_implicit_save && !has_open_database {
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

        // The destination folder has to be there before anything is opened.
        // SQLite answers a deleted folder or an unmapped drive with the same
        // errno-free "unable to open database file" as a genuine denial, so
        // asking first is what lets the user be told which one it is instead of
        // being handed a raw database error (#429).
        if let Some(parent) = save_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let reachable = match parent.try_exists() {
                Ok(present) => present,
                // A folder whose metadata cannot be read at all - a removed
                // stick, a dropped share - cannot host the save either. A
                // permission error is the one exception: the folder IS there,
                // and the pipeline below classifies that denial with its own
                // guidance.
                Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
            };
            if !reachable {
                tracing::warn!(
                    path = ?save_path,
                    parent = ?parent,
                    purpose = "persist edits safely",
                    "Save refused: the destination folder is not reachable"
                );
                return CommandResult::failure(format!(
                    "{}: Cannot save to {}: the folder no longer exists or the drive is not \
                     connected",
                    crate::core::loading::PATH_NOT_FOUND_CODE,
                    save_path.display()
                ));
            }
        }

        // Create a temporary file path in the same directory (for atomic rename)
        let temp_path = save_path.with_extension("stalx.tmp");
        tracing::info!(
            final_path = ?save_path,
            temp_path = ?temp_path,
            purpose = "persist edits safely",
            "Starting safe save operation"
        );

        // Load display settings from current database or in-memory cache
        let display_settings = {
            let db_guard = self.database.lock().ok();
            let from_db = db_guard
                .as_ref()
                .and_then(|db| db.as_ref())
                .and_then(|db| db.load_display_settings().ok())
                .unwrap_or_default();

            // If DB had settings, use those; otherwise use the in-memory cache
            if from_db.is_empty() {
                self.display_settings_cache
                    .lock()
                    .map(|cache| cache.clone())
                    .unwrap_or_default()
            } else {
                from_db
            }
        };

        // Resolved (ignored) warnings live in their own table, not in the Project
        // model, so save_project() won't carry them. Snapshot them from the
        // current database and re-apply to the fresh save file below, otherwise
        // every save would drop the user's "Ignore" acknowledgements (issue #57).
        //
        // [#517] The read is `load_resolved_warnings_verbatim`, not the load
        // path's `load_resolved_warnings`: it takes the table in whatever shape
        // the file has (a pre-v8 file with no `shot_uuid`, e.g. one whose
        // migration chain a forged `PRAGMA user_version` skipped) instead of
        // failing on the one missing column. What it still cannot read is
        // refused below rather than guessed at.
        let carried_ignores = {
            // Recover a poisoned lock rather than silently dropping every ignore.
            let db_guard = self.database.lock().unwrap_or_else(|e| e.into_inner());
            match db_guard.as_ref() {
                Some(db) => db.load_resolved_warnings_verbatim(),
                None => Ok(Vec::new()),
            }
        }
        // ...plus the ignores an "Add .stalx" carried in for sections this file
        // has never held (#674): THIS save is the first that can write them.
        // They ride the same remap and section filter as every other row.
        .map(|mut rows| {
            rows.extend(self.pending_ignores());
            rows
        });
        let resolved_warnings: Vec<_> = match carried_ignores {
            Ok(rows) => {
                // Drop rows whose section no longer exists. resolved_warnings has
                // a FOREIGN KEY on section_id and the carry-over re-inserts in a
                // single transaction - one orphaned row (e.g. a section deleted
                // after a warning on it was ignored; delete_section is in-memory
                // only and never prunes this table) would abort the whole
                // transaction and silently drop EVERY still-valid ignore.
                let section_ids: std::collections::HashSet<_> =
                    project.sections.iter().map(|s| s.id).collect();
                // Re-derive each UUID-keyed row's stored (section_id, shot_id)
                // from the live shot's CURRENT position so two distinct live
                // shots never share a (section_id, shot_id) in the fresh save
                // file (issue #57). save_project writes shot_id =
                // section_id*FEATURE_ID_STRIDE + sequence_order, so we derive
                // the same value from the in-memory project; load regenerates
                // ids with this identical formula. Without this, a renumber that
                // lands a different shot on a stale UUID-keyed row's old position
                // would let INSERT OR REPLACE (against UNIQUE(section_id, shot_id,
                // warning_code)) silently delete the older shot's still-valid
                // ignore.
                // Key by (section_id, uuid), not uuid alone: shot UUIDs are unique
                // across the live project, but a STORED row may name an id that
                // two sections held before the load renamed one copy, and only
                // its section says which shot it meant (issue #57 / Codex).
                let live_positions: std::collections::HashMap<(SectionId, &str), (SectionId, u32)> =
                    project
                        .sections
                        .iter()
                        .flat_map(|s| {
                            s.shots.iter().enumerate().filter_map(move |(seq, sh)| {
                                sh.uuid.as_deref().map(|u| {
                                    (
                                        (s.id, u),
                                        (
                                            s.id,
                                            s.id * crate::models::FEATURE_ID_STRIDE + seq as u32,
                                        ),
                                    )
                                })
                            })
                        })
                        .collect();
                // Index live shots by their CURRENT (section_id, shot_id) → (uuid,
                // saved position). A position-keyed (NULL-uuid) row written before
                // its shot's UUID was persisted (resolve_warning on a pre-v6 or
                // not-yet-saved shot) is upgraded here: THIS save persists every
                // shot's UUID, so we re-key such a row to the shot's saved position
                // AND its now-persisted UUID. Without this, its stale in-memory
                // shot_id could point at a different shot (or none) after an
                // insert/delete on the next load (issue #57 / Codex).
                let live_by_id: std::collections::HashMap<
                    (SectionId, u32),
                    (&str, (SectionId, u32)),
                > = project
                    .sections
                    .iter()
                    .flat_map(|s| {
                        s.shots.iter().enumerate().filter_map(move |(seq, sh)| {
                            sh.uuid.as_deref().map(|u| {
                                (
                                    (s.id, sh.id),
                                    (
                                        u,
                                        (
                                            s.id,
                                            s.id * crate::models::FEATURE_ID_STRIDE + seq as u32,
                                        ),
                                    ),
                                )
                            })
                        })
                    })
                    .collect();
                let homes = super::warnings::ShotHomes::of(project);
                // Remap each UUID-keyed row to its shot's live position; DROP a
                // UUID-keyed row whose shot was deleted (UUID no longer live) - it
                // can never match a live shot by UUID on reload, so it is dead.
                // Keeping it with its stale id would let it collide (under
                // UNIQUE(section_id, shot_id, warning_code) + INSERT OR REPLACE)
                // with a live shot that a renumber moved onto that position, and
                // REPLACE could drop either row depending on insert order - i.e.
                // silently lose the live shot's still-valid ignore. Dropping dead
                // rows here also stops them accumulating across saves. NULL-uuid
                // (legacy, position-keyed) rows are kept and matched by position
                // (the load-time backfill upgrades the ones that map to a shot).
                rows.into_iter()
                    .filter_map(|mut r| {
                        match r.shot_uuid.as_deref() {
                            Some(uuid) => {
                                // Look up by (section_id, uuid). A row stored against
                                // an id the load renamed (a cross-section repeat in a
                                // file saved before shot UUIDs were project-wide
                                // unique) names its shot through the derived id, and
                                // is re-keyed to it here: this save persists it.
                                let live = crate::models::live_shot_uuid_for_stored(
                                    r.section_id,
                                    uuid,
                                    |id| live_positions.contains_key(&(r.section_id, id)),
                                );
                                // A shot a split or combine moved to another
                                // section keeps its uuid: re-key the row to its
                                // new section rather than drop it.
                                let (home, live_uuid) = match live {
                                    Some(live) => (r.section_id, live),
                                    None => (homes.moved_to(r.section_id, uuid)?, uuid.to_string()),
                                };
                                let &(sec, shot) =
                                    live_positions.get(&(home, live_uuid.as_str()))?;
                                r.shot_uuid = Some(live_uuid);
                                r.section_id = sec;
                                r.shot_id = shot;
                                Some(r)
                            }
                            None => {
                                // Upgrade a position-keyed row to its shot's saved
                                // position + now-persisted UUID when its (section_id,
                                // shot_id) still maps to a live shot. A row that maps
                                // to NO live shot is DEAD (its shot was deleted) - DROP
                                // it: carrying it forward would let the load-time
                                // position backfill re-attach it to whatever shot now
                                // occupies that saved position, suppressing an
                                // unrelated shot's warning (issue #57 / Codex).
                                let &(uuid, (sec, shot)) =
                                    live_by_id.get(&(r.section_id, r.shot_id))?;
                                r.shot_uuid = Some(uuid.to_string());
                                r.section_id = sec;
                                r.shot_id = shot;
                                Some(r)
                            }
                        }
                    })
                    // Then drop any (legacy) row whose section no longer exists so
                    // the FK on section_id holds for every carried-over row.
                    .filter(|r| section_ids.contains(&r.section_id))
                    .collect()
            }
            // [#517] REFUSE the save. This snapshot is not a nicety the save
            // can do without: the pipeline builds a FRESH `.stalx.tmp` and
            // renames it over the destination, so the file's ignore list is
            // rebuilt from exactly these rows. Continuing with `Vec::new()`
            // wrote a file with zero ignores and destroyed the old one in the
            // same stroke - every "Ignore" the user ever recorded, gone, on a
            // save reported as successful.
            //
            // Refusing loses nothing: no temp file exists yet, the destination
            // is untouched, and the rows stay in the file that still holds
            // them. It is also the only channel that reaches the user - a
            // `CommandResult.warnings` entry on a SUCCESSFUL save is read by
            // nothing on the frontend (`projectStore.saveProject` inspects the
            // list only for `DATABASE_NOT_REOPENED`), whereas a failure is
            // routed to the error dialog / autosave banner by
            // `surfaceSaveFailure`.
            //
            // The verbatim read above already absorbs every legacy SHAPE, and
            // coerces every hostile VALUE, so what remains here is STRUCTURAL:
            // a missing key column, an unreadable `sqlite_master`, a step error
            // from a corrupt page. A single nonsense row must never land here -
            // that would make one poisoned cell in a file the user merely opened
            // into a project that can never be saved again. The project's survey
            // data is exportable meanwhile, so the user is not walled in.
            //
            // The message carries `IGNORES_UNREADABLE` because an unmarked
            // failure is routed to Review & Repair whenever the integrity check
            // is also unhappy, and Review & Repair cannot fix this - see
            // `saveErrors.ts` / `surfaceSaveFailure`. The quoted cause is clamped:
            // SQLite error text embeds the attacker's own identifiers.
            Err(e) => {
                tracing::error!(
                    error = %e,
                    purpose = "carry resolved warnings across save",
                    "Refused to save: the resolved-warnings table could not be read, and the \
                     save would rebuild the file without the user's ignores"
                );
                return CommandResult::failure(format!(
                    "{}: Can't save: this project's list of ignored warnings could not be read \
                     ({}). Saving would rewrite the file without them, so nothing was written - \
                     the file on disk is unchanged.",
                    crate::core::loading::IGNORES_UNREADABLE_CODE,
                    crate::core::loading::clamp_quoted_error(&e.to_string()),
                ));
            }
        };

        // [Track B] (#191, D-B13c) The SpeleoDB hand-off link (speleo_link) lives
        // in its own table, not the Project model, so save_project() won't carry
        // it. The save pipeline writes a FRESH temp .stalx and atomically renames
        // it over the destination, so without this snapshot+re-apply a local save
        // would DROP the link entirely - silently un-remoting the project and
        // clearing has_unpushed_edits (exactly the "local save NEVER clears the
        // dirty bit" invariant). Snapshot it here and re-apply to the fresh file
        // below. base_commit_sha + the dirty bit are carried verbatim.
        let speleo_link = {
            let db_guard = self.database.lock().unwrap_or_else(|e| e.into_inner());
            db_guard
                .as_ref()
                .and_then(|db| db.load_speleo_link().ok().flatten())
        };

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
        let copy_only = speleo_link.is_some() && !is_implicit_save;
        if copy_only {
            // Refuse the managed file itself as the copy's destination: writing an
            // unlinked copy over it would strip the managed project's link in
            // place - the one outcome this whole arrangement exists to prevent.
            // Both spellings of "itself" are checked: the open file, and the
            // project's canonical cache slot (they are normally the same file, but
            // `same_file` cannot bridge them when one side does not resolve).
            let names_the_managed_file = project
                .file_path
                .as_ref()
                .is_some_and(|open| same_file(open, &save_path))
                || speleo_link.as_ref().is_some_and(|link| {
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
        // [#303] The Unwind chain pointer needs the same treatment, and for a sharper
        // reason than "it would be lost". Losing it is documented as benign - the next
        // Unwind restarts from BASE - but the pointer is also what EXCUSES the dirty bit
        // a walk leaves behind. Dropped here, a copy the user only unwound reads as
        // holding un-pushed work nobody warned them about, so the confirmation and the
        // backend guard disagree: the dialog resolved the flag before an autosave, the
        // guard re-derived it after, and the Unwind failed on press with "this copy holds
        // un-pushed edits that the undo history no longer covers". A scripted run never
        // sees it because nothing saves in between; a person pausing between two Unwinds
        // hits it every time.
        // A detached copy deliberately sheds both markers, so it does not need to read
        // them. For a managed save, keep absence distinct from a read error: if either
        // marker cannot be preserved, the selected fail-closed policy is to save the
        // survey with no undo history rather than publish unclassified inherited rows.
        let marker_snapshot = if copy_only {
            Ok((None, None))
        } else {
            let db_guard = self.database.lock().unwrap_or_else(|e| e.into_inner());
            match db_guard.as_ref() {
                Some(db) => db.load_speleo_undo_chain().and_then(|chain| {
                    db.load_speleo_undo_watermark()
                        .map(|watermark| (chain, watermark))
                }),
                None => Ok((None, None)),
            }
        };
        let (speleo_undo_chain, speleo_undo_watermark, mut marker_preservation_failed) =
            match marker_snapshot {
                Ok((chain, watermark)) => (chain, watermark, false),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        purpose = "save without unclassified inherited undo history (#303)",
                        "Could not read the undo-chain metadata; the saved file will drop Undo and Redo history"
                    );
                    (None, None, true)
                }
            };

        // Remove any leftover temp file from previous failed saves
        if temp_path.exists() {
            let _ = std::fs::remove_file(&temp_path);
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

        // Copy display settings to the new database
        for (key, value) in &display_settings {
            if let Err(e) = db.save_display_setting(key, value) {
                tracing::warn!(
                    key = %key,
                    error = %e,
                    purpose = "preserve display settings on save",
                    "Failed to copy display setting"
                );
            }
        }

        // Carry over resolved (ignored) warnings so they survive the save.
        if !resolved_warnings.is_empty() {
            if let Err(e) = db.save_resolved_warnings_bulk(&resolved_warnings) {
                tracing::warn!(
                    error = %e,
                    purpose = "preserve resolved warnings on save",
                    "Failed to copy resolved warnings"
                );
            }
        }

        // [Track B] (#191, D-B13c) Carry the SpeleoDB hand-off link forward so a
        // local save never un-remotes the project or clears the dirty bit. The
        // dirty bit is set-only here - only a later server push clears it.
        // EXCEPT on a copy save, where dropping the row is the whole point: the
        // copy is a plain unmanaged project.
        // [#303] The chain position and the inherited-history watermark join the link
        // under the SAME guard, rather than each carrying their own: all three describe
        // this machine's relationship to the SERVER, so a copy that has deliberately shed
        // its link must shed them too. A copy that kept the watermark and was later linked
        // would have its own first push cut the user's history as "inherited". Grouped
        // because the per-site version of this rule has already been missed once.
        if !copy_only {
            if let Some(link) = &speleo_link {
                if let Err(e) = db.save_speleo_link(Some(link)) {
                    tracing::warn!(
                        error = %e,
                        purpose = "preserve SpeleoDB hand-off link on save",
                        "Failed to copy speleo_link onto the fresh save file"
                    );
                }
            }
            // [#303] ...and the Unwind chain position with it. Only written when one exists:
            // `None` is already the fresh file's state, and writing it unconditionally would
            // mean every save of every project touched this table for nothing.
            if let Some(sha) = &speleo_undo_chain {
                if let Err(e) = db.save_speleo_undo_chain(Some(sha)) {
                    marker_preservation_failed = true;
                    tracing::warn!(
                        error = %e,
                        purpose = "preserve the Unwind chain position on save (#303)",
                        "Failed to copy speleo_undo_chain onto the fresh save file; Undo and \
                         Redo history will be removed from the saved file"
                    );
                }
            }

            // [#303] Likewise the inherited-history watermark.
            if let Some(ts) = &speleo_undo_watermark {
                if let Err(e) = db.save_speleo_undo_watermark(Some(ts)) {
                    marker_preservation_failed = true;
                    tracing::warn!(
                        error = %e,
                        purpose = "preserve the inherited-history watermark on save (#303)",
                        "Failed to copy speleo_undo_watermark onto the fresh save file; Undo and \
                         Redo history will be removed from the saved file"
                    );
                }
            }
        }

        // Persist history while the file is still staged. A partial undo/redo rewrite
        // must never be renamed into place: try the pair atomically, and if it fails,
        // atomically clear both stacks and continue with a visible warning.
        let mut history_dropped = match self.persist_or_clear_undo_stacks_into(&db) {
            Ok(dropped) => dropped,
            Err(e) => {
                drop(db);
                let _ = std::fs::remove_file(&temp_path);
                return save_failure("Failed to finalize undo history", &save_path, &e);
            }
        };
        if marker_preservation_failed {
            if let Err(e) = db.clear_history_stacks() {
                drop(db);
                let _ = std::fs::remove_file(&temp_path);
                return save_failure("Failed to remove unclassified undo history", &save_path, &e);
            }
            history_dropped = true;
            // Keep the fresh file coherent where possible. History is already empty, so
            // failure here cannot expose inherited rows and must not turn the selected
            // continue-saving policy back into a refused save.
            if let Err(e) = db.save_speleo_undo_chain(None) {
                tracing::warn!(error = %e, "Could not clear the saved undo-chain pointer");
            }
            if let Err(e) = db.save_speleo_undo_watermark(None) {
                tracing::warn!(error = %e, "Could not clear the saved undo watermark");
            }
        }

        // Close the temp database before renaming
        drop(db);

        if copy_only {
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
            drop(proj_guard);
            self.note_recent_project(&save_path);
            tracing::info!(
                path = ?save_path,
                purpose = "write a detached unlinked copy",
                "Saved an unlinked copy of a SpeleoDB-managed project"
            );
            let mut result =
                CommandResult::success().with_unlinked_copy_path(save_path.display().to_string());
            Self::warn_history_dropped(&mut result, history_dropped);
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
        let closed_open_db = self
            .database
            .lock()
            .map(|mut db| db.take().is_some())
            .unwrap_or(false);

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
                        if let Ok(mut database) = self.database.lock() {
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

        // The `.stalx` is now written, so the initial save is no longer deferred.
        // Set this BEFORE the reopen below: that path returns early on failure, and
        // the project is persisted regardless of whether the handle reopens.
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);
        // The file now holds every pending ignore (#674) - still under the
        // project guard the carry-over read them under, so none can slip in
        // between. The detached-copy branch above returns before this: the
        // session's own file did not receive them.
        self.clear_pending_ignores();

        // The installed file deliberately has no history because its chain metadata
        // could not be classified safely. Drop the live mirror at the same commit point:
        // otherwise a later save can write these same unclassified rows straight back
        // into the now-valid database and undo the fail-closed fallback.
        if marker_preservation_failed {
            let mut undo_stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            undo_stack.clear();
            undo_stack.set_inherited_through(None);
        }

        // Reopen the database at the final path
        let db = match Database::open(&save_path) {
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
                project.file_path = Some(save_path.clone());
                if let Ok(mut database) = self.database.lock() {
                    *database = None;
                }
                drop(proj_guard);
                // Distinct code (not the generic "SYSTEM") so the frontend can keep
                // databaseOpen=false and leave DB-backed actions (e.g. Ignore)
                // disabled until the project is reopened (issue #57 / Codex).
                let mut result = CommandResult::success();
                Self::warn_history_dropped(&mut result, history_dropped);
                result.warnings.push(crate::core::ValidationWarning {
                    code: "DATABASE_NOT_REOPENED".to_string(),
                    message: "File saved, but database could not be reopened. Some features may not work until the project is reopened.".to_string(),
                    severity: crate::core::WarningSeverity::Warning,
                });
                return result;
            }
        };

        // Update file path after successful save
        project.file_path = Some(save_path.clone());

        // Publish the reopened handle before a replacement project can take the slot.
        if let Ok(mut database) = self.database.lock() {
            *database = Some(db);
        }
        drop(proj_guard);

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
        Self::warn_history_dropped(&mut result, history_dropped);
        result
    }

    fn warn_history_dropped(result: &mut CommandResult, dropped: bool) {
        if dropped {
            result.warnings.push(crate::core::ValidationWarning {
                code: "UNDO_HISTORY_DROPPED".to_owned(),
                message: "The project was saved, but its Undo and Redo history could not be preserved and was removed.".to_owned(),
                severity: crate::core::WarningSeverity::Warning,
            });
        }
    }

    pub(super) fn persist_or_clear_undo_stacks_into(&self, db: &Database) -> anyhow::Result<bool> {
        let undo_stack = self
            .undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (undo_rows, redo_rows) = self.history_rows_to_persist(db, &undo_stack);
        match db.replace_history_stacks_with_timestamps(undo_rows, redo_rows) {
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
        // projects, Disabled mode - keeps the `undo_persist_limit` window unchanged.
        //
        // The link is read from the LOCAL `db` handle rather than through
        // `undo_policy()`: `self.database` still points at the pre-save handle when the
        // normal save calls this, so asking the AppState would consult the wrong file.
        let (undo_rows, redo_rows) = self.history_rows_to_persist(db, undo_stack);
        match db.replace_history_stacks_with_timestamps(undo_rows, redo_rows) {
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
        Vec<(String, String, Option<String>)>,
        Vec<(String, String, Option<String>)>,
    ) {
        let count_limit = {
            let policy = self.undo_policy_for(db.load_speleo_link().ok().flatten().as_ref());
            if policy.unlimited_depth() {
                None
            } else {
                Some(
                    self.config
                        .lock()
                        .map(|c| c.general.undo_persist_limit)
                        .unwrap_or(20),
                )
            }
        };

        // Every save writes the WHOLE (windowed) stack, because the pipeline above
        // built a fresh temp file and renamed it over the target: the reopened `db`
        // has no undo tables to add to. See `UndoStack::rows_to_persist`.
        let undo_rows = undo_stack.rows_to_persist(PersistStack::Undo, count_limit);

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
        let redo_rows = undo_stack.rows_to_persist(PersistStack::Redo, count_limit);
        (undo_rows, redo_rows)
    }

    /// Put `path` at the head of the recent-projects list and persist it to
    /// config. Takes no state locks other than `recent_projects` + `config`;
    /// callers must NOT hold `project` (the save paths drop it first).
    fn note_recent_project(&self, path: &Path) {
        if let Ok(mut recent) = self.recent_projects.lock() {
            recent.retain(|p| p != path);
            recent.insert(0, path.to_path_buf());
            if let Ok(mut config) = self.config.lock() {
                recent.truncate(config.general.recent_projects_max);
                config.general.recent_projects.clone_from(&recent);
                if let Err(e) = config.save() {
                    tracing::warn!(
                        error = %e,
                        purpose = "persist recent projects list",
                        "Failed to save config with recent projects"
                    );
                }
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
    /// function is the pre-rename close that `close_project_if_open_at` relies on,
    /// and it used to report `true` while still holding the live SQLite handle and
    /// its 256 MB mmap, so the caller went on to rename a new file over it - the
    /// exact SIGBUS / `MoveFileEx`-denied hazard the guard exists to prevent.
    pub(super) fn close_project(&self) -> CommandResult {
        tracing::debug!("close_project: acquiring project lock");
        let mut proj = self.project.lock().unwrap_or_else(|e| e.into_inner());
        self.close_open_project_locked(&mut proj)
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
    /// The caller owns the poison recovery for the `project` lock (both of this
    /// function's callers use `PoisonError::into_inner`, for the reason spelled out
    /// on `close_project`); the locks taken here recover it themselves.
    pub(super) fn close_open_project_locked(&self, project: &mut Option<Project>) -> CommandResult {
        tracing::debug!("close_project: dropping project");
        *project = None;
        // A session boundary: the caller holds the `project` guard (it passed it
        // in), which is the bump rule (`speleodb_session`). Covers close, delete
        // and the project switch, which all route through here.
        self.bump_speleodb_session();
        // An add's pending ignores (#674) die with the project they belong to.
        self.clear_pending_ignores();
        // So does the record of renamed shot ids: it describes this project only.
        self.load_renamed_shots
            .store(false, std::sync::atomic::Ordering::Relaxed);
        // [#473] The discarded-history banner describes the OPEN project; with none
        // open there is nothing for it to describe. Cleared here as well as on every
        // load commit, so closing a project takes its banner with it rather than
        // leaving it up over the welcome screen.
        self.set_history_discarded(false);
        tracing::debug!("close_project: project dropped");

        tracing::debug!("close_project: acquiring database lock");
        {
            let mut db = self.database.lock().unwrap_or_else(|e| e.into_inner());
            tracing::debug!("close_project: dropping database");
            *db = None;
            tracing::debug!("close_project: database dropped");
        }

        tracing::debug!("close_project: acquiring undo_stack lock");
        {
            let mut undo_stack = self.undo_stack.lock().unwrap_or_else(|e| e.into_inner());
            tracing::debug!("close_project: clearing undo stack");
            undo_stack.clear();
            // [#303] Back onto the configured entry window: the next project to open is
            // local until a link says otherwise.
            undo_stack.use_count_limit();
            tracing::debug!("close_project: undo stack cleared");
        }

        // Clear display settings cache
        self.clear_display_settings_cache();
        self.invalidate_adjusted_cache();
        // [Track B] (#191) Nothing is open: back to unmanaged (see
        // `speleodb_edit_gate`); the closed session's claim stays a pointer only.
        self.speleodb_set_open_gate(None);

        tracing::info!(purpose = "release project resources", "Project closed");
        tracing::debug!("close_project: returning success");
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
        let guard = self.project.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().and_then(|p| p.file_path.clone())
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
    pub(super) fn close_project_if_open_at(&self, dest: &Path) -> bool {
        let Some(open_path) = self.open_project_path() else {
            return false;
        };
        if !same_file(&open_path, dest) {
            return false;
        }
        tracing::info!(
            path = ?dest,
            purpose = "release the live handle before replacing the file",
            "Closing the open project because its .stalx is about to be replaced"
        );
        self.close_project();
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
