//! The import entry points: `execute_import_lines` (every importer's funnel,
//! `import_funnel.rs`), the Ariane variant carrying the entrance elevation,
//! and the New Line dialog's line snapped onto a station
//! (`execute_create_and_merge`). Each is one recorded edit: the pipeline's
//! `run_recorded` snapshots, validates (rolling back on a violation),
//! captures one changeset and pushes ONE undo entry, the dedup hashes, the
//! Sections, the closures and the Jump/Reference kinds included.

use std::collections::HashMap;

use super::commands::Record;
use super::import_funnel::{self, ImportOutcome, ImportRequest, LineBuilder, LineError};
use super::AppState;
use crate::core::command::ImportReceipt;
use crate::core::{CommandResult, ValidationWarning};
use crate::models::survey::changeset::HistoryRefs;
use crate::models::survey::EditContext;
use crate::models::{
    ClosureStrategy, ExistingShotUpdate, ImportClosure, ImportLineConfig, ImportPlacement,
    SectionId, ShotData, StationRef,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a freshly-imported file's virtual shots should be classified (#27 q3).
///
/// A parser can only tell that a shot is virtual; which of the two virtual kinds
/// it is needs the project graph, which only exists once the import is placed
/// and merged. [`Auto`](VirtualShotMode::Auto) runs the chain rule at that
/// point. The two explicit modes let a user who knows the file override it in
/// one control, instead of the per-shot picker the issue originally proposed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum VirtualShotMode {
    /// Chain rule: a virtual run bounded by real survey on both ends is a Jump,
    /// anything else a Reference.
    #[default]
    Auto,
    /// Every virtual shot in this file is a swum shortcut.
    AllJumps,
    /// Every virtual shot in this file is a non-swimmable survey tie.
    AllReferences,
}

/// The most ids of each kind a history entry's refs list (an import can make
/// thousands of lines; the label names three).
const MAX_REFS: usize = 256;

/// `ids`, at most [`MAX_REFS`] of them, sorted and deduplicated.
fn capped<T: Ord + Copy>(ids: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut ids: Vec<T> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(MAX_REFS);
    ids
}

/// The receipt of a successful import (0.6.0's fields; the importer adds its
/// source, file name and unit note in `decorate_receipt`).
fn receipt_of(outcome: &ImportOutcome, closures_requested: u32) -> ImportReceipt {
    let mut warnings = Vec::new();
    if let Some(warning) = &outcome.surface_elevation_warning {
        warnings.push(warning.clone());
    }
    let skipped = outcome.doubly_anchored_shots;
    if skipped > 0 {
        let one = skipped == 1;
        warnings.push(format!(
            "{skipped} shot{} kept {} position{}: line anchored on both sides",
            if one { "" } else { "s" },
            if one { "its" } else { "their" },
            if one { "" } else { "s" },
        ));
    }
    let dropped = outcome.closures_dropped;
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} loop closure{} could not be created and {} skipped",
            if dropped == 1 { "" } else { "s" },
            if dropped == 1 { "was" } else { "were" }
        ));
    }
    // "and N more": naming only the first zero-datum line without saying so
    // would read as a claim that it was the only one.
    let zero_datum_created = outcome.zero_datum_line.clone().map(|name| {
        if outcome.zero_datum_count > 1 {
            format!("{name} (and {} more)", outcome.zero_datum_count - 1)
        } else {
            name
        }
    });
    let mut receipt = ImportReceipt {
        source_format: String::new(),
        file_name: String::new(),
        lines_found: 0,
        lines_imported: outcome.line_ids.len() as u32,
        shot_count: outcome.shot_count,
        closures_requested,
        closures_created: outcome.closures_created,
        unit_conversion: None,
        warnings,
        info: Vec::new(),
        zero_datum_created,
    };
    receipt.cap_warnings();
    receipt
}

/// The result of a successful edit: `result` with `warnings` attached.
fn with_warnings(result: CommandResult, warnings: Vec<ValidationWarning>) -> CommandResult {
    if warnings.is_empty() {
        result
    } else {
        result.with_warnings(warnings)
    }
}

impl AppState {
    /// Execute a create-line-and-merge operation as a single atomic undo/redo unit.
    /// This is used when importing a new line and immediately merging one of its stations
    /// with an existing station. Both operations are combined into a single undo entry.
    pub fn execute_create_and_merge(
        &self,
        name: Option<String>,
        surveyor: Option<String>,
        date: Option<chrono::NaiveDate>,
        color: String,
        direction: Option<crate::models::SurveyDirection>,
        shots: Vec<ShotData>,
        import_station_index: u32,
        target_station: StationRef,
        closure_strategy: ClosureStrategy,
    ) -> CommandResult {
        self.execute_create_and_merge_in_cave_section(
            name,
            surveyor,
            date,
            color,
            direction,
            shots,
            import_station_index,
            target_station,
            closure_strategy,
            None,
        )
    }

    /// [`Self::execute_create_and_merge`] that also puts the new line in the
    /// Section `cave_section_id` (#327), inside the same undo step: a New
    /// Line placed by snapping onto a station is ONE undo, Section included.
    /// A Section that no longer exists refuses the whole step before anything
    /// changes, as `CreateLine` does.
    ///
    /// On the station graph the line is created ON the target station: its
    /// station `import_station_index` is the target (a junction), so nothing
    /// is merged or moved and no station id is spent on it. The history entry
    /// keeps 0.6.0's batch: `Create line "X" at Line / station`, with the
    /// `Create line` and `Merge stations` rows.
    pub fn execute_create_and_merge_in_cave_section(
        &self,
        name: Option<String>,
        surveyor: Option<String>,
        date: Option<chrono::NaiveDate>,
        color: String,
        direction: Option<crate::models::SurveyDirection>,
        shots: Vec<ShotData>,
        import_station_index: u32,
        target_station: StationRef,
        _closure_strategy: ClosureStrategy,
        cave_section_id: Option<SectionId>,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6).
        if let Err(f) = self.guard_mutation("Import line and merge") {
            return f;
        }
        tracing::info!(
            purpose = "keep undo atomic for import merge",
            "Executing create_line_and_merge as single undo unit"
        );
        let config = ImportLineConfig {
            line_index: 0,
            name,
            surveyor,
            date,
            surveyors: None,
            explorers: None,
            override_date: None,
            color,
            direction,
            quality: 1.0,
            shots,
            placement: ImportPlacement::FromExisting {
                import_station: import_station_index,
                existing_station: target_station,
            },
            station_comments: Vec::new(),
            station_names: Vec::new(),
            reverse: false,
            azimuth_reference: None,
            cave_section: None,
        };
        let recorded = self.run_recorded(
            |_| Record {
                kind: "batch".to_string(),
                ..Record::default()
            },
            |project, record| {
                if let Some(section) = cave_section_id {
                    if !project.sections.iter().any(|s| s.id == section) {
                        return Err("The section for the new line no longer exists; \
                                    it may have been deleted"
                            .to_string());
                    }
                }
                let before = project.anchor_snapshot();
                let mut builder = LineBuilder::new(project, [target_station]);
                let mut ctx = EditContext::none();
                let mut steps = Vec::new();
                let built = builder
                    .build(project, &config, &HashMap::new(), &mut ctx, &mut steps)
                    .map_err(|e| match e {
                        LineError::Create(why) | LineError::Placement(why) => why,
                        LineError::NotCreatedYet(index) => {
                            format!("Import line index {index} has not been created yet")
                        }
                    })?;
                if let Some(section) = cave_section_id {
                    if let Some(line) = project.line_mut(built.id) {
                        line.section_id = Some(section);
                    }
                }
                project.settle_anchors(&before, &ctx);
                record.label =
                    super::labels::create_line_at_label(project, built.id, target_station);
                record.refs = HistoryRefs {
                    lines: capped([built.id, target_station.line_id]),
                    stations: capped(built.junction.map(|(_, s, _)| s)),
                    sections: capped(cave_section_id),
                    ..HistoryRefs::default()
                };
                record.preview = import_funnel::import_preview(&steps, project);
                Ok((built.id, built.warnings))
            },
        );
        match recorded {
            Ok((line_id, warnings)) => {
                with_warnings(CommandResult::success().with_line_id(line_id), warnings)
            }
            Err(failure) => failure,
        }
    }

    /// Execute a batch import of lines as a single atomic undo/redo unit.
    /// Creates all lines with their placements (junctions, zero datums,
    /// moves) and optionally tracks the file hash, so one undo reverts
    /// everything including the hash.
    ///
    /// `mode` picks how this file's virtual shots are split into Jump vs
    /// Reference (#27). The pass runs inside this edit, so undo-then-redo of
    /// an import keeps the classified kinds.
    pub fn execute_import_lines(
        &self,
        lines: Vec<ImportLineConfig>,
        closures: Vec<ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<ExistingShotUpdate>,
        mode: VirtualShotMode,
    ) -> CommandResult {
        self.execute_import_lines_inner(
            lines,
            closures,
            file_hash,
            line_hashes,
            existing_shot_updates,
            mode,
            None,
        )
    }

    /// Same as [`Self::execute_import_lines`], but also merges an Ariane
    /// file's project-level entrance elevation (#431) into the project's
    /// `surface_elevation` atomically with the import, so undo/redo covers
    /// both. The only caller that has this value is the Ariane TML/TMLU
    /// importer.
    pub fn execute_tml_import_sections(
        &self,
        lines: Vec<ImportLineConfig>,
        closures: Vec<ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        self.execute_import_lines_inner(
            lines,
            closures,
            file_hash,
            line_hashes,
            existing_shot_updates,
            mode,
            surface_elevation,
        )
    }

    fn execute_import_lines_inner(
        &self,
        lines: Vec<ImportLineConfig>,
        closures: Vec<ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6), before the empty early return so
        // the chokepoint stays uniform.
        if let Err(f) = self.guard_mutation("Import lines") {
            return f;
        }
        if lines.is_empty() {
            // An EMPTY receipt, not none: every line of the file was dropped,
            // and `decorate_receipt` attaches the converter's warnings saying
            // why to it. (The DMP wizard only sends overlap updates with the
            // lines they belong to, so none are lost here.)
            return CommandResult::success().with_import_receipt(ImportReceipt::empty());
        }
        // Overlap updates come from parsed file content: they hold the same
        // finite invariant as UpdateShot (#275), checked before any change.
        if let Some(update) = existing_shot_updates
            .iter()
            .find(|u| !u.updates.all_finite())
        {
            return CommandResult::failure(format!(
                "Import shot update for line {} shot {} has a non-finite (NaN/inf) value",
                update.line_id, update.shot_id
            ));
        }
        tracing::info!(
            count = lines.len(),
            purpose = "apply imported lines as single undo",
            "Executing import_lines as single undo unit"
        );
        let started = std::time::Instant::now();
        let request = ImportRequest {
            lines: &lines,
            closures: &closures,
            file_hash: file_hash.as_deref(),
            line_hashes: &line_hashes,
            updates: &existing_shot_updates,
            mode,
            surface_elevation,
        };
        let recorded = self.run_recorded(
            |_| Record {
                kind: "batch".to_string(),
                ..Record::default()
            },
            |project, record| {
                let outcome = import_funnel::import_lines(project, &request)?;
                record.label = super::labels::import_lines_label(project, &outcome.labelled_lines);
                record.refs = HistoryRefs {
                    lines: capped(outcome.line_ids.iter().copied()),
                    stations: capped(outcome.junctions.iter().copied()),
                    closures: capped(outcome.closure_ids.iter().copied()),
                    sections: capped(outcome.section_ids.iter().copied()),
                    ..HistoryRefs::default()
                };
                record.preview = import_funnel::import_preview(&outcome.steps, project);
                Ok(outcome)
            },
        );
        let outcome = match recorded {
            Ok(outcome) => outcome,
            Err(failure) => return failure,
        };
        tracing::debug!(
            lines = outcome.line_ids.len(),
            total_ms = started.elapsed().as_millis() as u64,
            purpose = "import timing",
            "execute_import_lines done"
        );
        let receipt = receipt_of(&outcome, closures.len() as u32);
        let result = CommandResult::success()
            .with_line_ids(outcome.line_ids)
            .with_import_receipt(receipt);
        with_warnings(result, outcome.warnings)
    }
}
