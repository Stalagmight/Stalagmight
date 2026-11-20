//! Test-only (`feature = "writer"`): the 0.6.0 app's own save, so property
//! tests can produce "what an old build wrote".

use std::collections::HashSet;

use crate::models::{LineId, Project};
use crate::persistence::Database;
use crate::survey::LegacyExtras;
use crate::{LegacyError, LegacyProject};

/// Which v18 file shape to write. Schema 18 spans several shapes: side tables
/// arrived later without a version bump.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileShape {
    /// What the 0.6.0 app writes (every side table).
    #[default]
    Current,
    /// What 0.5.5, the last release before 0.6.0, wrote: no `station_ids`
    /// (nor its `next_station_id` counter) and no Sections (`cave_sections`,
    /// `cave_section_lines`, `next_cave_section_id`). Station IDs and Sections
    /// in the project are simply not written.
    Release055,
}

/// Knobs for [`write_v18_with`].
#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    /// Skip the save's undo-less heals (`apply_save_heals` in the app): mint
    /// missing shot uuids, re-mint in-line duplicates, capture gauge pairs,
    /// drop orphan and non-finite line offsets, sanitize non-finite shots.
    /// Off (heals applied) is what the app does. A project that needs one
    /// of the destructive heals may fail to write without them.
    pub skip_save_heals: bool,
    pub shape: FileShape,
    /// Undo and redo rows, as `(command json, undo data json, timestamp)`,
    /// written the way the app persists its stacks. Empty by default.
    pub undo_rows: Vec<(String, String, Option<String>)>,
    pub redo_rows: Vec<(String, String, Option<String>)>,
}

/// The bytes the 0.6.0 app's save writes for `project` and `extras`
/// (the current v18 shape, save heals applied, no history).
pub fn write_v18(project: &LegacyProject, extras: &LegacyExtras) -> Result<Vec<u8>, LegacyError> {
    write_v18_with(project, extras, &WriteOptions::default())
}

/// [`write_v18`] with [`WriteOptions`].
///
/// Mirrors `AppState::save_project`: the heals, a fresh temp database
/// (`Database::open`), `save_project`, then the display settings in key
/// order, the resolved warnings, the SpeleoDB link, the Unwind chain and
/// watermark, and the history stacks.
pub fn write_v18_with(
    project: &LegacyProject,
    extras: &LegacyExtras,
    opts: &WriteOptions,
) -> Result<Vec<u8>, LegacyError> {
    let mut project = project.clone();
    if !opts.skip_save_heals {
        apply_save_heals(&mut project);
    }
    let dir = tempfile::Builder::new()
        .prefix("stalx-legacy-write-")
        .tempdir()?;
    let path = dir.path().join("project.stalx.tmp");
    let corrupt = |e: anyhow::Error| LegacyError::Corrupt(format!("{e:#}"));
    {
        let mut db = Database::open(&path).map_err(corrupt)?;
        db.save_project(&project).map_err(corrupt)?;
        // `BTreeMap` iterates in key order, which is the app's sorted copy.
        for (key, value) in &extras.display_settings {
            db.save_display_setting(key, value).map_err(corrupt)?;
        }
        if !extras.resolved_warnings.is_empty() {
            db.save_resolved_warnings_bulk(&extras.resolved_warnings)
                .map_err(corrupt)?;
        }
        if let Some(link) = &extras.speleo_link {
            db.save_speleo_link(Some(link)).map_err(corrupt)?;
        }
        if let Some(sha) = &extras.speleo_undo_chain {
            db.save_speleo_undo_chain(Some(sha)).map_err(corrupt)?;
        }
        if let Some(ts) = &extras.speleo_undo_watermark {
            db.save_speleo_undo_watermark(Some(ts)).map_err(corrupt)?;
        }
        db.replace_history_stacks_with_timestamps(opts.undo_rows.clone(), opts.redo_rows.clone())
            .map_err(corrupt)?;
        if opts.shape == FileShape::Release055 {
            db.strip_unreleased_side_tables().map_err(corrupt)?;
        }
    }
    Ok(std::fs::read(&path)?)
}

/// `apply_save_heals` from the app's `core/state/project_io.rs`, verbatim.
fn apply_save_heals(project: &mut Project) {
    project.ensure_shot_uuids();
    project.ensure_unique_shot_uuids();
    project.capture_missing_gauge_anchors();
    let existing_lines: HashSet<LineId> = project.lines.iter().map(|s| s.id).collect();
    project
        .line_offsets
        .retain(|sid, _| existing_lines.contains(sid));
    let non_finite: Vec<LineId> = project
        .line_offsets
        .iter()
        .filter(|(_, offset)| !offset.0.is_finite() || !offset.1.is_finite())
        .map(|(id, _)| *id)
        .collect();
    for id in non_finite {
        let offset = project.line_offset(id);
        let finite = (
            if offset.0.is_finite() { offset.0 } else { 0.0 },
            if offset.1.is_finite() { offset.1 } else { 0.0 },
        );
        Project::write_line_offset(&mut project.line_offsets, id, finite);
    }
    for line in &mut project.lines {
        for shot in &mut line.shots {
            shot.sanitize_non_finite();
        }
    }
}
