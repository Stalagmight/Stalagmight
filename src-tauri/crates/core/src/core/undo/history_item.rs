//! The undo history as the frontend lists it (`UndoHistoryItem`, unchanged
//! IPC shape), built from a [`HistoryEntry`]: its record-time label, kind and
//! time, and a preview from the recorded rows plus the changeset digest.
//!
//! C0 builds a generic preview from the digest (the fields a step changed,
//! the lines and stations it created or deleted, the datums and closures it
//! added or removed). The per-command wording of 0.6.0's previews
//! (`history_view.rs` at the C0 base) is ported by the history-labels work
//! package, with the K goldens as the oracle.

use serde::Serialize;
use ts_rs::TS;

use crate::core::Command;
use crate::models::survey::changeset::{Digest, ProjectField, StationField, Val};
use crate::models::survey::HistoryEntry;

/// DTO for undo history item sent to frontend
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UndoHistoryItem {
    /// Index in the stack (0 = most recent)
    pub index: usize,
    /// Human-readable description of the action
    pub description: String,
    /// ISO 8601 timestamp or null for legacy entries
    pub timestamp: Option<String>,
    /// Command type for icon mapping (e.g., "create_line", "update_shot")
    pub command_type: String,
    /// Preview data showing what changed (null for commands without meaningful diff)
    pub preview_data: Option<PreviewData>,
}

/// Preview data showing field changes
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PreviewData {
    /// List of field changes (up to 3)
    pub changes: Vec<FieldChange>,
    /// Count of additional fields beyond the shown 3
    pub additional_changes: usize,
}

/// A single field change
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FieldChange {
    /// Field name (e.g., "length", "azimuth")
    pub field: String,
    /// Old value as string
    pub old_value: String,
    /// New value as string
    pub new_value: String,
}

/// The most rows a generic preview shows before it counts the rest.
const PREVIEW_ROWS: usize = 3;

/// The history kind of a command (the icon the frontend maps), persisted as
/// the entry's `"k"`.
pub fn command_kind(command: &Command) -> &'static str {
    match command {
        Command::CreateProject { .. } => "create_project",
        Command::OpenProject { .. } => "open_project",
        Command::SaveProject { .. } => "save_project",
        Command::CloseProject => "close_project",
        Command::CreateLine { .. } => "create_line",
        Command::DeleteLine { .. } => "delete_line",
        Command::UpdateLineMetadata { .. } => "update_line_metadata",
        Command::ReverseLine { .. } => "reverse_line",
        Command::CreateShot { .. } => "create_shot",
        Command::UpdateShot { .. } => "update_shot",
        Command::DeleteShot { .. } => "delete_shot",
        Command::SetShotKind { .. } => "set_shot_kind",
        Command::EditShotCredit { .. } => "edit_shot_credit",
        Command::MergeStations { .. } => "merge_stations",
        Command::UnmergeStations { .. } => "unmerge_stations",
        Command::UnmergeStationPair { .. } => "unmerge_station_pair",
        Command::CreateLoopClosure { .. } => "create_loop_closure",
        Command::RemoveLoopClosure { .. } => "remove_loop_closure",
        Command::SetZeroDatum { .. } => "set_zero_datum",
        Command::RemoveZeroDatum { .. } => "remove_zero_datum",
        Command::SetStationComment { .. } => "set_station_comment",
        Command::SetStationName { .. } => "set_station_name",
        Command::MoveLine { .. } => "move_line",
        Command::MoveSegment { .. } => "move_segment",
        Command::MoveStation { .. } => "move_station",
        Command::MoveComment { .. } => "move_comment",
        Command::Batch { .. } => "batch",
        Command::UntrackImport { .. } => "untrack_import",
        Command::CreateCaveSection { .. } => "create_cave_section",
        Command::UpdateCaveSection { .. } => "update_cave_section",
        Command::DeleteCaveSection { .. } => "delete_cave_section",
        Command::MoveLinesToCaveSection { .. } => "move_lines_to_cave_section",
        Command::ApplyCaveSectionColor { .. } => "apply_cave_section_color",
        Command::MergeProjects { .. } => "merge_projects",
        Command::AddProjectFile { .. } => "add_project_file",
        Command::RepairIntegrity => "repair_integrity",
        Command::UpdateProjectSettings => "update_project_settings",
        Command::SetSurfaceElevation => "set_surface_elevation",
    }
}

/// The history item of `entry` at `index` of its stack.
pub fn history_item(index: usize, entry: &HistoryEntry) -> UndoHistoryItem {
    UndoHistoryItem {
        index,
        description: entry.label.clone(),
        timestamp: Some(entry.timestamp.to_rfc3339()),
        command_type: entry.kind.clone(),
        preview_data: preview(entry),
    }
}

fn row(field: impl Into<String>, old: impl Into<String>, new: impl Into<String>) -> FieldChange {
    FieldChange {
        field: field.into(),
        old_value: old.into(),
        new_value: new.into(),
    }
}

fn shown(value: &Val) -> String {
    match value {
        Val::Null => String::new(),
        Val::Num(n) => format!("{n:.2}"),
        other => other.display(),
    }
}

/// The recorded rows when the entry has any, else the digest's.
fn preview(entry: &HistoryEntry) -> Option<PreviewData> {
    if !entry.preview.rows.is_empty() {
        return Some(PreviewData {
            changes: entry
                .preview
                .rows
                .iter()
                .map(|r| row(r.field.clone(), r.old.clone(), r.new.clone()))
                .collect(),
            additional_changes: usize::try_from(entry.preview.more).unwrap_or(usize::MAX),
        });
    }
    let rows = digest_rows(&entry.changes.digest());
    if rows.is_empty() {
        return None;
    }
    let additional_changes = rows.len().saturating_sub(PREVIEW_ROWS);
    Some(PreviewData {
        changes: rows.into_iter().take(PREVIEW_ROWS).collect(),
        additional_changes,
    })
}

fn project_field_label(field: ProjectField) -> &'static str {
    match field {
        ProjectField::Name => "Name",
        ProjectField::Location => "Location",
        ProjectField::CreatedAt => "Created",
        ProjectField::SurfaceElevation => "Surface elevation",
        ProjectField::DefaultAzimuthReference => "North reference",
        ProjectField::Validation => "Validation",
    }
}

fn station_field_label(field: StationField) -> &'static str {
    match field {
        StationField::Name => "Station name",
        StationField::Comment => "Comment",
        StationField::Aliases => "Aliases",
        StationField::External => "External id",
    }
}

fn digest_rows(d: &Digest) -> Vec<FieldChange> {
    let mut rows = Vec::new();
    for c in &d.shot_cells {
        rows.push(row(
            c.field.label(),
            c.old.as_ref().map(shown).unwrap_or_default(),
            shown(&c.new),
        ));
    }
    for c in &d.line_fields {
        rows.push(row(format!("{:?}", c.field), shown(&c.old), shown(&c.new)));
    }
    for c in &d.station_fields {
        rows.push(row(
            station_field_label(c.field),
            shown(&c.old),
            shown(&c.new),
        ));
    }
    for c in &d.project {
        if c.field != ProjectField::Validation {
            rows.push(row(
                project_field_label(c.field),
                shown(&c.old),
                shown(&c.new),
            ));
        }
    }
    for line in &d.lines_created {
        let name = line.name.clone().unwrap_or_else(|| "unnamed".to_string());
        rows.push(row("Line", "", format!("{name} ({} shots)", line.shots)));
    }
    for line in &d.lines_deleted {
        rows.push(row("Name", line.name.as_deref().unwrap_or("unnamed"), ""));
        rows.push(row("Shots", line.shots.to_string(), "0"));
    }
    if d.lines_created.is_empty() && d.shots_created_total > 0 {
        rows.push(row("Shots", "", format!("+{}", d.shots_created_total)));
    }
    if d.lines_deleted.is_empty() && d.shots_deleted_total > 0 {
        rows.push(row("Shots", "", format!("-{}", d.shots_deleted_total)));
    }
    for datum in &d.datums.removed {
        rows.push(row(
            "GPS",
            format!("{:.6}, {:.6}", datum.gps.latitude, datum.gps.longitude),
            "",
        ));
    }
    for datum in &d.datums.added {
        rows.push(row(
            "GPS",
            "",
            format!("{:.6}, {:.6}", datum.gps.latitude, datum.gps.longitude),
        ));
    }
    for closure in &d.closures.added {
        rows.push(row("Loop closure", "", format!("#{}", closure.id)));
    }
    for closure in &d.closures.removed {
        rows.push(row("Loop closure", format!("#{}", closure.id), ""));
    }
    for section in &d.sections.added {
        rows.push(row("Section", "", section.name.clone()));
    }
    for section in &d.sections.removed {
        rows.push(row("Section", section.name.clone(), ""));
    }
    rows
}
