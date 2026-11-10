//! "Add .stalx to current project" (#674): append another project file's
//! survey into the OPEN project, as one undoable command.
//!
//! The copy itself is [`append_project`], the routine "Merge projects" uses.
//! What this file adds around it is what only an add needs: reading the source
//! without touching it, placing a georeferenced source by its datum, carrying
//! the source's dedup hashes and ignored warnings, refusing corruption without
//! a whole-project snapshot, and the receipt.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::project_append::{append_project, AppendOutcome};
use super::project_io::same_file;
use super::AppState;
use crate::core::command::{sanitize_display_text, ImportReceipt, MAX_FILE_NAME_LEN};
use crate::core::{
    extend_unique_hashes, AddedContent, CarriedIgnore, Command, CommandResult, UndoData,
};
use crate::models::{
    gps_to_local, warning_is_ignorable, ClassifyScope, GpsCoordinates, Project, SectionId,
    SpeleoLink, FEATURE_ID_STRIDE, MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID,
};
use crate::persistence::{Database, ResolvedWarning};

/// The import-dedup entry an add records for its source, in
/// `Project::imported_file_hashes` beside the content hashes the importers
/// record there. Prefixed so it can never collide with a hex SHA-256.
///
/// Keyed on the source PROJECT's identity (`created_at`, stable across every
/// edit and save of that project), not on the file's bytes: a `.stalx` is
/// rewritten on every save - `modified_at`, the undo history - so a byte hash
/// would miss exactly the common case of adding the same project again after it
/// was opened and saved. A Save As copy keeps the identity, and holds the same
/// survey, so it warns too.
const STALX_IDENTITY_PREFIX: &str = "stalx-project:";

fn stalx_identity(project: &Project) -> String {
    format!("{STALX_IDENTITY_PREFIX}{}", project.created_at.to_rfc3339())
}

/// The source as the add reads it.
struct AddSource {
    project: Project,
    /// Its `resolved_warnings` rows; `None` when the table could not be read.
    ignores: Option<Vec<ResolvedWarning>>,
    link: Option<SpeleoLink>,
}

/// Read `path` WITHOUT touching it, normalized the way opening it would be.
///
/// Through a private copy: opening an older file migrates it in place, and the
/// source - possibly a SpeleoDB-managed cache copy whose bytes are compared
/// against its server commit - must stay exactly as it was. The source's undo
/// history is never read.
fn load_add_source(path: &Path, label: &str) -> Result<AddSource, String> {
    let dir = tempfile::tempdir()
        .map_err(|e| format!("Failed to prepare a private copy of '{label}': {e}"))?;
    let copy = dir.path().join("source.stalx");
    copy_regular_file(path, &copy, label)?;
    let db = Database::open_existing(&copy)
        .map_err(|e| format!("Failed to open project '{label}': {e}"))?;
    let mut project = db
        .load_project()
        .map_err(|e| format!("Failed to load project '{label}': {e}"))?;
    let ignores = db.load_resolved_warnings_verbatim().ok();
    // An unreadable link table on a file whose project DID load is treated as
    // no link: the copy carries survey data only either way, so the link only
    // ever decides one receipt line.
    let link = db.load_speleo_link().ok().flatten();
    // The v12->v13 migration that just ran on the copy seeded every legacy
    // reference shot as Jump and left this flag for the open path to finish.
    // Read before the copy is dropped: it goes with the temp database, so an
    // add that skipped it would leave those shots swimmable for good.
    let kind_classification_pending = db.is_kind_classification_pending();
    drop(db);
    refuse_non_finite_placement(&project, label)?;

    // What an open does to a freshly read project, in the same order
    // (`do_load_project_full`): every shot gets a uuid (the identity ignores
    // and cross-references are re-keyed onto), legacy closures get ids, the
    // derived offsets are re-solved rather than trusted, and a pending #27
    // Jump/Reference backfill is run on the source's own graph. Nothing is
    // written back: the copy is discarded and the source stays untouched.
    project.ensure_shot_uuids();
    project.assign_closure_ids();
    project.redrive_offsets_after_load();
    if kind_classification_pending {
        project.classify_virtual_shots(&ClassifyScope::WholeProject);
    }
    Ok(AddSource {
        project,
        ignores,
        link,
    })
}

/// Refuse a source whose placement data cannot be trusted: the file reader
/// builds zero-datum GPS without `GpsCoordinates::new`'s validation, and reads
/// offsets as stored. A NaN or infinite value would pass every tolerance
/// comparison downstream (NaN is never greater than a tolerance) and be written
/// into the open project as section offsets; an out-of-range datum would place
/// the copy somewhere meaningless. Checked on the raw read, before anything
/// derives from it.
fn refuse_non_finite_placement(project: &Project, label: &str) -> Result<(), String> {
    let refuse = |what: String| {
        Err(format!(
            "'{label}' was not added: {what}. The file is damaged or was written by \
             another tool. No changes were made."
        ))
    };
    for datum in &project.zero_datums {
        let gps = &datum.gps;
        if let Err(e) = GpsCoordinates::validate(gps.latitude, gps.longitude, gps.depth_reference) {
            return refuse(format!("a zero datum has invalid GPS coordinates ({e})"));
        }
    }
    if project
        .section_offsets
        .values()
        .any(|(x, y)| !x.is_finite() || !y.is_finite())
    {
        return refuse("a section offset is not a finite number".to_string());
    }
    if project
        .station_offsets
        .values()
        .any(|(x, y, z)| !x.is_finite() || !y.is_finite() || !z.is_finite())
    {
        return refuse("a station offset is not a finite number".to_string());
    }
    Ok(())
}

/// The largest `.stalx` an add reads: the ceiling a SpeleoDB download of a
/// project file is already held to. The read runs under `command_lock`, so an
/// unbounded one would stall every edit behind it (and fill the disk).
const MAX_ADD_SOURCE_BYTES: u64 = crate::speleodb::client::NATIVE_LIMIT;

/// Copy `path` to `dest`, refusing anything that is not a regular file of at
/// most [`MAX_ADD_SOURCE_BYTES`]: a FIFO or a device named `*.stalx` would
/// block or never end, and a directory cannot be a project.
///
/// The metadata check runs before the file is opened (opening a FIFO for
/// reading blocks), and the copy itself is bounded too, so a file that grows
/// or is swapped after the check still cannot exceed the ceiling.
fn copy_regular_file(path: &Path, dest: &Path, label: &str) -> Result<(), String> {
    let limit_mb = MAX_ADD_SOURCE_BYTES / (1024 * 1024);
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("File not found: {label}"));
        }
        Err(e) => return Err(format!("Failed to read '{label}': {e}")),
    };
    if !metadata.is_file() {
        return Err(format!("'{label}' is not a project file"));
    }
    if metadata.len() > MAX_ADD_SOURCE_BYTES {
        return Err(format!(
            "'{label}' is larger than {limit_mb} MB, the largest project file that can be added"
        ));
    }
    let source = std::fs::File::open(path).map_err(|e| format!("Failed to read '{label}': {e}"))?;
    let mut target =
        std::fs::File::create(dest).map_err(|e| format!("Failed to read '{label}': {e}"))?;
    let copied = std::io::copy(
        &mut std::io::Read::take(source, MAX_ADD_SOURCE_BYTES + 1),
        &mut target,
    )
    .map_err(|e| format!("Failed to read '{label}': {e}"))?;
    if copied > MAX_ADD_SOURCE_BYTES {
        return Err(format!(
            "'{label}' grew past {limit_mb} MB while it was being read, so it was not added"
        ));
    }
    Ok(())
}

/// Translate the copied sections so a georeferenced source lands at its GPS
/// position: its first resolvable datum's station goes to that datum's local
/// position in the open project's frame, or to the origin when the open project
/// had no frame (the copied datum then BECOMES the frame).
///
/// The whole copy moves rigidly, unanchored components included, so the
/// source's own layout is preserved. Nothing pre-existing moves. A source with
/// no datum is left where it is - placing it is the orphan-placement dialog's
/// job, not a guess made here.
fn place_by_datum(
    project: &mut Project,
    new_ids: &[SectionId],
    frame_before: Option<GpsCoordinates>,
) {
    let new: HashSet<SectionId> = new_ids.iter().copied().collect();
    let Some((station, gps)) = project
        .zero_datums
        .iter()
        .filter(|zd| new.contains(&zd.station_ref.section_id))
        .find_map(|zd| project.resolve_key(&zd.station_ref).map(|sr| (sr, zd.gps)))
    else {
        return;
    };
    let Some(current) = project.compute_station_raw_coords(station) else {
        return;
    };
    let (target_x, target_y) = frame_before.map_or((0.0, 0.0), |frame| gps_to_local(&gps, &frame));
    let (dx, dy) = (target_x - current.x, target_y - current.y);
    // Non-finite is refused at the read; this keeps a NaN (which passes the dust
    // test below) from ever reaching an offset should another route produce one.
    if !dx.is_finite() || !dy.is_finite() {
        tracing::warn!(
            purpose = "place an added source by its datum",
            "Datum placement is not finite; leaving the copy where it is"
        );
        return;
    }
    if dx.abs() <= Project::OFFSET_DUST_M && dy.abs() <= Project::OFFSET_DUST_M {
        return;
    }
    for id in new_ids {
        project.adjust_section_offset(*id, dx, dy);
    }
}

/// Re-key the source's ignored warnings onto the copied sections, by shot uuid.
/// A legacy position-keyed row is resolved through the source shot at that
/// position. Rows an ignore can never honor (error severity) and rows whose
/// shot is gone are dropped.
fn remap_ignores(
    source: &Project,
    rows: &[ResolvedWarning],
    outcome: &AppendOutcome,
) -> Vec<CarriedIgnore> {
    let shots = ShotIndex::of(source, rows.iter().map(|row| row.section_id));
    let mut carried = IgnoreList::default();
    for row in rows {
        if !warning_is_ignorable(&row.warning_code) {
            continue;
        }
        let Some(&section_id) = outcome.section_id_map.get(&row.section_id) else {
            continue;
        };
        let shot_uuid = match row.shot_uuid.as_deref() {
            Some(uuid) => shots.id_of(row.section_id, uuid).map(|_| uuid.to_string()),
            None => shots
                .uuid_of(row.section_id, row.shot_id)
                .map(str::to_string),
        };
        let Some(shot_uuid) = shot_uuid else {
            continue;
        };
        // The copy may have given this shot a fresh uuid (the open project
        // already held its own); the ignore follows the shot, not the old id,
        // or `add_pending_ignores` would find no such shot and drop it.
        let shot_uuid = outcome
            .reminted_uuids
            .get(&(section_id, shot_uuid.clone()))
            .cloned()
            .unwrap_or(shot_uuid);
        carried.push(CarriedIgnore {
            section_id,
            shot_uuid,
            warning_code: row.warning_code.clone(),
            resolved_at: row.resolved_at,
            resolved_comment: row.resolved_comment.clone(),
        });
    }
    carried.items
}

/// The shots of some sections, indexed both ways, built once so a pass over N
/// ignore rows costs O(N) rather than a shot scan per row (a hostile source
/// controls both counts). The FIRST shot wins a key, as a linear scan would.
#[derive(Default)]
struct ShotIndex<'a> {
    id_by_uuid: HashMap<(SectionId, &'a str), u32>,
    uuid_by_id: HashMap<(SectionId, u32), Option<&'a str>>,
}

impl<'a> ShotIndex<'a> {
    fn of(project: &'a Project, sections: impl IntoIterator<Item = SectionId>) -> Self {
        let mut index = Self::default();
        let wanted: HashSet<SectionId> = sections.into_iter().collect();
        for section_id in wanted {
            let Some(section) = project.get_section(section_id) else {
                continue;
            };
            for shot in &section.shots {
                if let Some(uuid) = shot.uuid.as_deref() {
                    index
                        .id_by_uuid
                        .entry((section_id, uuid))
                        .or_insert(shot.id);
                }
                index
                    .uuid_by_id
                    .entry((section_id, shot.id))
                    .or_insert(shot.uuid.as_deref());
            }
        }
        index
    }

    /// The id of the shot carrying `uuid` in `section_id`.
    fn id_of(&self, section_id: SectionId, uuid: &str) -> Option<u32> {
        self.id_by_uuid.get(&(section_id, uuid)).copied()
    }

    /// The uuid of the shot with `shot_id` in `section_id`, if it has one.
    fn uuid_of(&self, section_id: SectionId, shot_id: u32) -> Option<&'a str> {
        self.uuid_by_id
            .get(&(section_id, shot_id))
            .copied()
            .flatten()
    }
}

/// Carried ignores in first-seen order, one per (section, shot, warning).
#[derive(Default)]
struct IgnoreList {
    items: Vec<CarriedIgnore>,
    seen: HashSet<(SectionId, String, String)>,
}

impl IgnoreList {
    fn push(&mut self, ignore: CarriedIgnore) {
        let key = (
            ignore.section_id,
            ignore.shot_uuid.clone(),
            ignore.warning_code.clone(),
        );
        if self.seen.insert(key) {
            self.items.push(ignore);
        }
    }
}

/// What happened during the append, for the receipt.
struct AppendReport {
    outcome: AppendOutcome,
    closures_created: u32,
    ignores: Vec<CarriedIgnore>,
    duplicate: Option<&'static str>,
}

impl AppState {
    /// The slot generation an add's undo stub was captured against, which
    /// `execute` hands the forward step. `None` when no stub was captured (no
    /// project was open).
    pub(super) fn add_slot_generation(undo_data: &UndoData) -> Option<u64> {
        match undo_data {
            UndoData::AddedProjectContent(stub) => stub.slot_generation(),
            _ => None,
        }
    }

    /// `Command::AddProjectFile`'s forward step (reached only through
    /// `execute`, so the read-only gate, the geometry epilogue and the undo push
    /// all apply). Refuses with nothing changed when no project is open, when
    /// `path` is the open project's own file, or when the file cannot be read.
    ///
    /// Bound to `slot_generation`, the generation `execute`'s undo stub was
    /// captured under: a project load does not take `command_lock`, so another
    /// project can be committed into the slot between that capture and either
    /// of the two `project` locks below, and every slot commit bumps the
    /// generation (local projects included - see `bump_speleodb_session` and the
    /// bump-site tripwire in `speleodb_session`). Appending into that successor
    /// would pair it with the first project's baseline, so its undo would delete
    /// the successor's own sections. `push_add_undo` closes the same window after
    /// the append.
    pub(super) fn add_project_file(
        &self,
        path: &Path,
        slot_generation: Option<u64>,
    ) -> CommandResult {
        let label = path
            .file_name()
            .map(|n| sanitize_display_text(&n.to_string_lossy(), MAX_FILE_NAME_LEN))
            .unwrap_or_default();
        let Some(slot_generation) = slot_generation else {
            return CommandResult::failure("No project is open to add the file to".to_string());
        };
        let slot_moved = || {
            CommandResult::failure(format!(
                "The open project changed while '{label}' was being added, so it was not \
                 added. No changes were made."
            ))
        };
        add_window(self, AddWindow::BeforeFirstLock);

        // The refusals that need no file read come first.
        let open_path = {
            let guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(project) = guard.as_ref() else {
                return CommandResult::failure("No project is open to add the file to".to_string());
            };
            if self.speleodb_session_generation() != slot_generation {
                return slot_moved();
            }
            project.file_path.clone()
        };
        if open_path
            .as_deref()
            .is_some_and(|open| same_file(open, path))
        {
            return CommandResult::failure(format!(
                "'{label}' is the open project itself, so it cannot be added to it"
            ));
        }

        let source = match load_add_source(path, &label) {
            Ok(source) => source,
            Err(e) => return CommandResult::failure(e),
        };
        add_window(self, AddWindow::DuringRead);

        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(project) = guard.as_mut() else {
            return CommandResult::failure("No project is open to add the file to".to_string());
        };
        // The project the add was started against left the slot during the
        // read. Nothing has been changed yet. (A SpeleoDB sign-in or sign-out in
        // the window bumps the generation too; refusing then is conservative,
        // and the user simply adds again.)
        if self.speleodb_session_generation() != slot_generation {
            return slot_moved();
        }
        let report = match Self::append_into_open(project, &source) {
            Ok(report) => report,
            Err(e) => return CommandResult::failure(e),
        };
        // Under the same project guard as the copy, so a save can never see the
        // sections without their ignores (the save reads both under it).
        self.add_pending_ignores(project, &report.ignores);
        let shot_count: u32 = report
            .outcome
            .new_section_ids
            .iter()
            .filter_map(|id| project.get_section(*id))
            .map(|s| s.shots.len() as u32)
            .sum();
        drop(guard);

        tracing::info!(
            sections = report.outcome.new_section_ids.len(),
            ignores = report.ignores.len(),
            purpose = "add a project file into the open project",
            "Added a .stalx file's survey to the open project"
        );
        let receipt = Self::add_receipt(&label, &source, &report, shot_count);
        CommandResult::success()
            .with_section_ids(report.outcome.new_section_ids)
            .with_import_receipt(receipt)
    }

    /// The copy and everything that settles it. On any failure the project is
    /// returned to exactly what it was, by the same removal the undo runs.
    fn append_into_open(project: &mut Project, source: &AddSource) -> Result<AppendReport, String> {
        // Section-id budget (#555), before a single section is appended: shot
        // ids encode as `section_id * FEATURE_ID_STRIDE + position` in 32 bits,
        // and `Section::new` computes that unchecked.
        let incoming = source.project.sections.len();
        let spent = project.next_section_id as usize;
        if incoming.saturating_add(spent) > MAX_SECTIONS_IN_ID_SPACE {
            return Err(format!(
                "Add refused: the file holds {incoming} sections and this project has \
                 already used {spent} section ids, past the limit of \
                 {MAX_SECTIONS_IN_ID_SPACE} for one project. Shot ids encode as \
                 section_id * {FEATURE_ID_STRIDE} + position in 32 bits, so section ids \
                 past {MAX_SECTION_ID} cannot hold shots. No changes were made."
            ));
        }
        let identity = stalx_identity(&source.project);
        let duplicate = if identity == stalx_identity(project) {
            Some("is a copy of the open project")
        } else if project.imported_file_hashes.contains(&identity) {
            Some("was already added to this project")
        } else {
            None
        };
        let frame_before = project.frame_datum().map(|d| d.gps);
        let pre_critical = project.has_critical_integrity_violations();
        let closures_before = project.loop_closures.len();
        let baseline = AddedContent::stub(project, None);
        // A refused add must leave no trace, so beyond the removal the undo
        // runs it also winds back what that removal deliberately keeps: the id
        // allocators (a refused add must not spend the section-id budget an
        // exact-fit add needs) and the modification time.
        let (next_section_id, next_closure_id, modified_at) = (
            project.next_section_id,
            project.next_closure_id,
            project.modified_at,
        );
        let roll_back = |project: &mut Project, baseline: AddedContent| {
            if let Some(added) = baseline.finalize(project, Vec::new()) {
                added.remove_from(project);
            }
            project.next_section_id = next_section_id;
            project.next_closure_id = next_closure_id;
            project.modified_at = modified_at;
        };

        let outcome = match append_project(project, &source.project) {
            Ok(outcome) => outcome,
            Err(e) => {
                roll_back(project, baseline);
                return Err(e);
            }
        };
        project.assign_closure_ids();
        // The append stores transitional positional keys; give them the stable
        // anchors an open would, before anything can renumber a station.
        project.reanchor_station_refs();
        place_by_datum(project, &outcome.new_section_ids, frame_before);

        // Carry the source's dedup state, so re-importing the Ariane/DMP file it
        // was built from is still caught here, plus its own identity.
        extend_unique_hashes(
            &mut project.imported_file_hashes,
            &source.project.imported_file_hashes,
        );
        extend_unique_hashes(
            &mut project.imported_section_hashes,
            &source.project.imported_section_hashes,
        );
        extend_unique_hashes(
            &mut project.imported_file_hashes,
            std::slice::from_ref(&identity),
        );

        // Block corruption at the source without a whole-project snapshot: the
        // append only ever ADDS, so taking out what it added is an exact
        // rollback. An already-critical project (opened that way) is not
        // blocked, matching `execute`'s guard.
        if !pre_critical && project.has_critical_integrity_violations() {
            roll_back(project, baseline);
            return Err(
                "Blocked: the file's survey has integrity problems that would corrupt \
                        this project. Open it, repair it, and add it again. No changes were made."
                    .to_string(),
            );
        }

        let ignores = source
            .ignores
            .as_deref()
            .map(|rows| remap_ignores(&source.project, rows, &outcome))
            .unwrap_or_default();
        Ok(AppendReport {
            closures_created: project.loop_closures.len().saturating_sub(closures_before) as u32,
            outcome,
            ignores,
            duplicate,
        })
    }

    fn add_receipt(
        label: &str,
        source: &AddSource,
        report: &AppendReport,
        shot_count: u32,
    ) -> ImportReceipt {
        let mut warnings = Vec::new();
        if let Some(why) = report.duplicate {
            warnings.push(format!("'{label}' {why}; its sections were added again"));
        }
        if let Some(link) = &source.link {
            let name = sanitize_display_text(&link.project_name, MAX_FILE_NAME_LEN);
            warnings.push(format!(
                "'{label}' is linked to SpeleoDB project '{name}'; its data was copied, \
                 that project is unchanged"
            ));
        }
        if let Some((incoming, existing)) = report.outcome.elevation_conflict {
            warnings.push(format!(
                "'{label}' has an entrance elevation of {incoming:.2} m, which differs from \
                 this project's {existing:.2} m; kept {existing:.2} m"
            ));
        }
        if source.ignores.is_none() {
            warnings.push(format!(
                "The ignored warnings in '{label}' could not be read and were not carried over"
            ));
        }
        if source.project.sections.is_empty() {
            warnings.push(format!("'{label}' holds no survey sections"));
        }
        let mut receipt = ImportReceipt {
            source_format: "Stalagmight".to_string(),
            file_name: label.to_string(),
            sections_found: source.project.sections.len() as u32,
            sections_imported: report.outcome.new_section_ids.len() as u32,
            shot_count,
            closures_requested: source.project.loop_closures.len() as u32,
            closures_created: report.closures_created,
            unit_conversion: None,
            warnings,
            info: Vec::new(),
            zero_datum_created: None,
        };
        receipt.cap_warnings();
        receipt
    }

    /// Finalize the add's undo record and push it, under ONE `project` guard
    /// and only while the slot still holds the project the stub was captured
    /// against (lock order: `project` outranks `undo_stack`, as a load's own
    /// commit takes them). When another project was committed since - after the
    /// append, during `execute`'s epilogue - the record would measure that
    /// project's sections as the add's and its undo would delete them, so
    /// nothing is pushed and the command reports failure: the project the add
    /// went into has left the slot, and the one there now is untouched.
    ///
    /// Returns that failure, or `None` once the entry is pushed.
    pub(super) fn push_add_undo(
        &self,
        cmd: &Command,
        undo_data: UndoData,
    ) -> Option<CommandResult> {
        add_window(self, AddWindow::BeforePush);
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bound = Self::add_slot_generation(&undo_data);
        let project = match guard.as_ref() {
            Some(project) if bound == Some(self.speleodb_session_generation()) => project,
            _ => {
                tracing::warn!(
                    purpose = "bind an add's undo record to its project",
                    "The open project changed before the add was recorded; not pushing its undo entry"
                );
                let label = match cmd {
                    Command::AddProjectFile { file_name, .. } => file_name.as_str(),
                    _ => "",
                };
                return Some(CommandResult::failure(format!(
                    "The open project changed while '{label}' was being added, so the add \
                     was not recorded. The project open now was not changed."
                )));
            }
        };
        let final_undo_data = self.finalize_add_undo(project, undo_data);
        if let Ok(mut undo_stack) = self.undo_stack.lock() {
            // As `execute`'s generic push: an entry undo cannot apply never
            // enters the stack, but the forward command still invalidates redo.
            if matches!(final_undo_data, UndoData::None) {
                undo_stack.clear_redo();
            } else {
                undo_stack.push_with_data(cmd.clone(), final_undo_data);
            }
        }
        drop(guard);
        None
    }

    // === Ignores carried by an add, until a save persists them ===
    //
    // `resolved_warnings` lives in the open file's database with a FOREIGN KEY
    // on `sections(id)`, and an added section is not in that table until the
    // next save - so an ignore carried in for it cannot be written there yet.
    // It waits here, is honored by `compute_project_warnings` meanwhile, and is
    // written by the save that writes its section. Lock order: `project`
    // outranks this one, and nothing is ever taken while holding it.

    /// The carried ignores not yet in the database.
    pub(super) fn pending_ignores(&self) -> Vec<ResolvedWarning> {
        self.pending_ignores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Forget every pending ignore: a save wrote them, or the project they
    /// belonged to left the slot.
    pub(super) fn clear_pending_ignores(&self) {
        self.pending_ignores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Hold `ignores` until the next save, keyed by the live shot's id.
    pub(super) fn add_pending_ignores(&self, project: &Project, ignores: &[CarriedIgnore]) {
        let shots = ShotIndex::of(project, ignores.iter().map(|i| i.section_id));
        let mut pending = self
            .pending_ignores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut held: HashSet<(SectionId, String, String)> = pending
            .iter()
            .filter_map(|row| {
                let uuid = row.shot_uuid.clone()?;
                Some((row.section_id, uuid, row.warning_code.clone()))
            })
            .collect();
        for ignore in ignores {
            let Some(shot_id) = shots.id_of(ignore.section_id, &ignore.shot_uuid) else {
                continue;
            };
            let key = (
                ignore.section_id,
                ignore.shot_uuid.clone(),
                ignore.warning_code.clone(),
            );
            if held.insert(key) {
                pending.push(ResolvedWarning {
                    section_id: ignore.section_id,
                    shot_id,
                    warning_code: ignore.warning_code.clone(),
                    resolved_at: ignore.resolved_at,
                    resolved_comment: ignore.resolved_comment.clone(),
                    shot_uuid: Some(ignore.shot_uuid.clone()),
                });
            }
        }
    }

    /// Drop the pending ignores of `sections` (the undo of an add). Rows a save
    /// already wrote stay in the database: they name sections the undo just
    /// removed, so they match nothing, and the next save drops them - while a
    /// close WITHOUT saving keeps the file and its ignores consistent.
    pub(super) fn remove_pending_ignores(&self, sections: &HashSet<SectionId>) {
        self.pending_ignores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|row| !sections.contains(&row.section_id));
    }

    /// Un-ignore (#57) of a pending row; true when one was removed.
    pub(super) fn forget_pending_ignore(
        &self,
        section_id: SectionId,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> bool {
        let mut pending = self
            .pending_ignores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = pending.len();
        pending.retain(|row| {
            // Every pending row is uuid-keyed; the in-session id is only the
            // address a caller with no uuid has.
            let same_shot = match shot_uuid {
                Some(uuid) => row.shot_uuid.as_deref() == Some(uuid),
                None => row.shot_id == shot_id,
            };
            !(row.section_id == section_id && row.warning_code == warning_code && same_shot)
        });
        pending.len() != before
    }

    /// Every uuid-keyed ignore currently in force on `sections` - pending or
    /// already saved - as an add's undo records them, so the redo puts back the
    /// ignores the undo found rather than the ones the add first brought.
    pub(super) fn ignores_on_sections(&self, sections: &HashSet<SectionId>) -> Vec<CarriedIgnore> {
        let saved = {
            let db = self
                .database
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            db.as_ref()
                .and_then(|db| db.load_resolved_warnings_verbatim().ok())
                .unwrap_or_default()
        };
        let mut found = IgnoreList::default();
        for row in self.pending_ignores().iter().chain(saved.iter()) {
            let Some(uuid) = row.shot_uuid.clone() else {
                continue;
            };
            if !sections.contains(&row.section_id) {
                continue;
            }
            found.push(CarriedIgnore {
                section_id: row.section_id,
                shot_uuid: uuid,
                warning_code: row.warning_code.clone(),
                resolved_at: row.resolved_at,
                resolved_comment: row.resolved_comment.clone(),
            });
        }
        found.items
    }
}

/// The windows in which no `project` guard is held during an add, where a
/// project load (which does not take `command_lock`) can commit another
/// project into the slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AddWindow {
    /// After `execute` captured the undo stub, before the forward step's first
    /// `project` lock.
    BeforeFirstLock,
    /// Between the forward step's two `project` locks, where the source file
    /// is read.
    DuringRead,
    /// After the append and `execute`'s epilogue, before the undo push.
    BeforePush,
}

/// A no-op outside tests.
#[cfg(not(test))]
fn add_window(_state: &AppState, _window: AddWindow) {}

/// Test-only: run the hook a test installed with [`set_add_window_hook`] for
/// `window`, once. Per thread, so it reaches no other test. Defined below every
/// production item on purpose: the slot-commit tripwire in `speleodb_session`
/// reads each file only up to its first test gate.
#[cfg(test)]
fn add_window(state: &AppState, window: AddWindow) {
    let hook = ADD_WINDOW_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|(at, _)| *at == window) {
            slot.take()
        } else {
            None
        }
    });
    if let Some((_, hook)) = hook {
        hook(state);
    }
}

#[cfg(test)]
type AddWindowHook = (AddWindow, Box<dyn FnOnce(&AppState)>);

#[cfg(test)]
thread_local! {
    static ADD_WINDOW_HOOK: std::cell::RefCell<Option<AddWindowHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only: run `hook` inside `window` of the next add on this thread.
#[cfg(test)]
pub(super) fn set_add_window_hook(window: AddWindow, hook: impl FnOnce(&AppState) + 'static) {
    ADD_WINDOW_HOOK.with(|slot| *slot.borrow_mut() = Some((window, Box::new(hook))));
}

/// Test-only: the append step on an in-memory source, bypassing the file read
/// and its load-time heals. The post-append integrity guard is defense in
/// depth - the reader sanitizes non-finite shots, the load re-mints duplicate
/// uuids and the append skips references that do not resolve, so no loadable
/// file is known to trip it - and this is the way to exercise its rollback.
#[cfg(test)]
pub(super) fn append_in_memory(project: &mut Project, source: Project) -> Result<(), String> {
    let source = AddSource {
        project: source,
        ignores: None,
        link: None,
    };
    AppState::append_into_open(project, &source).map(|_| ())
}
