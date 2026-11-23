//! The undo history as the frontend lists it (`UndoHistoryItem`, unchanged
//! IPC shape), built from a [`HistoryEntry`] alone: its record-time label,
//! kind and time, its recorded preview rows, and its changeset's digest.
//! Never from a `Command` (the entry keeps none) and never from the live
//! project, so an entry reads the same deep in the stack, after an undo, a
//! redo, a reverse, an insert or a reopen.
//!
//! The preview reads as 0.6.0's `derive_preview_data` did, per kind:
//! - **recorded rows** win: what the record words by a slot or from the
//!   command (`core::state::labels::command_preview`: merges, closures,
//!   datums, moves, a batch's sub-commands) and what a recipe records;
//! - otherwise the **digest**: a shot edit's cells (old -> new, at most
//!   [`SHOT_EDIT_ROWS`]), a kind change, a created shot's length and
//!   azimuth, a created or deleted line's name and shots, a line metadata
//!   edit's fields (at most [`LINE_EDIT_ROWS`], surveyor and date read off
//!   the shot credits), a credit edit's names and dates over the shots it
//!   changed, a station comment;
//! - otherwise one `Action` row repeating the label, so a hover is never
//!   empty.

use std::collections::BTreeSet;

use serde::Serialize;
use ts_rs::TS;

use crate::core::Command;
use crate::models::survey::changeset::{Digest, LineField, ShotField, StationField, Val};
use crate::models::survey::{Changeset, HistoryEntry};
use crate::models::{
    dedupe_people, join_names, legacy_credit_text, Person, ShotKind, SurveyCredit,
};

/// DTO for undo history item sent to frontend
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UndoHistoryItem {
    /// Index in the stack (0 = most recent)
    pub index: usize,
    /// Human-readable description of the action
    pub description: String,
    /// `description` as prose parts (RS-PROSE): the entry's persisted
    /// `label_parts` (`HistoryEntry::label_parts`), each station, shot or
    /// line the label names as a part the history row draws as a pill.
    /// `prose::plain` of the parts is `description`, byte for byte; absent
    /// when the label names nothing, was cut, or comes from an entry of an
    /// earlier history format. Never carries a slot (the parts are persisted
    /// with stable ids only). Plain-text sinks (the row's tooltip, the undo
    /// toasts) read `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description_parts: Option<Vec<crate::models::ProsePart>>,
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

/// The most cells a shot edit's preview lists before it counts the rest.
pub const SHOT_EDIT_ROWS: usize = 5;

/// The most fields a line metadata edit's preview lists before it counts
/// the rest.
pub const LINE_EDIT_ROWS: usize = 3;

/// The longest name list a credit row shows, in characters.
const MAX_NAMES_CHARS: usize = 80;

/// The longest station comment a preview row shows, in characters.
pub(crate) const COMMENT_PREVIEW_CHARS: usize = 80;

/// `s` cut to at most `max_chars` characters, with an ellipsis only when
/// characters were dropped. Counted by character, so the cut never lands
/// inside a multi-byte codepoint.
pub(crate) fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

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
        // Defensive: the entry keeps parts only when they say its label.
        description_parts: (crate::models::prose::plain(&entry.label_parts) == entry.label)
            .then(|| crate::models::prose::named(entry.label_parts.clone()))
            .flatten(),
        timestamp: Some(entry.timestamp.to_rfc3339()),
        command_type: entry.kind.clone(),
        preview_data: Some(preview(entry)),
    }
}

fn row(field: impl Into<String>, old: impl Into<String>, new: impl Into<String>) -> FieldChange {
    FieldChange {
        field: field.into(),
        old_value: old.into(),
        new_value: new.into(),
    }
}

/// At most `max` rows, and the count of the rest.
fn capped(mut rows: Vec<FieldChange>, max: usize) -> PreviewData {
    let additional_changes = rows.len().saturating_sub(max);
    rows.truncate(max);
    PreviewData {
        changes: rows,
        additional_changes,
    }
}

fn all_of(rows: Vec<FieldChange>) -> PreviewData {
    PreviewData {
        changes: rows,
        additional_changes: 0,
    }
}

/// The entry's preview (module docs).
fn preview(entry: &HistoryEntry) -> PreviewData {
    if !entry.preview.rows.is_empty() {
        return PreviewData {
            changes: entry
                .preview
                .rows
                .iter()
                .map(|r| row(r.field.clone(), r.old.clone(), r.new.clone()))
                .collect(),
            additional_changes: usize::try_from(entry.preview.more).unwrap_or(usize::MAX),
        };
    }
    digest_preview(&entry.kind, &entry.changes)
        .filter(|p| !p.changes.is_empty())
        .unwrap_or_else(|| all_of(vec![row("Action", "", entry.label.clone())]))
}

/// The preview a kind reads off its changeset; `None` for a kind whose
/// preview is its recorded rows or the label.
fn digest_preview(kind: &str, changes: &Changeset) -> Option<PreviewData> {
    let wants_digest = matches!(
        kind,
        "update_shot"
            | "set_shot_kind"
            | "create_shot"
            | "create_line"
            | "delete_line"
            | "update_line_metadata"
            | "edit_shot_credit"
            | "set_station_comment"
            | "add_project_file"
    );
    if !wants_digest {
        return None;
    }
    let d = changes.digest();
    Some(match kind {
        "update_shot" => capped(shot_cell_rows(&d), SHOT_EDIT_ROWS),
        "set_shot_kind" => all_of(
            shot_cell_rows(&d)
                .into_iter()
                .filter(|r| r.field == ShotField::Kind.label())
                .take(1)
                .collect(),
        ),
        "create_shot" => {
            let shot = &d.shots_created.first()?.shot;
            all_of(vec![
                row("Length", "", format!("{:.2}", shot.length)),
                row("Azimuth", "", format!("{:.1}", shot.azimuth_in)),
            ])
        }
        "create_line" => {
            let line = d.lines_created.first()?;
            all_of(vec![row("Name", "", name_or_unnamed(line.name.as_deref()))])
        }
        "delete_line" => {
            let line = d.lines_deleted.first()?;
            all_of(vec![
                row("Name", name_or_unnamed(line.name.as_deref()), ""),
                row("Shots", line.shots.to_string(), "0"),
            ])
        }
        "update_line_metadata" => capped(line_edit_rows(&d, changes), LINE_EDIT_ROWS),
        "edit_shot_credit" => all_of(credit_rows(&d, changes)),
        "set_station_comment" => {
            let c = d
                .station_fields
                .iter()
                .find(|c| c.field == StationField::Comment)?;
            let old = c
                .old
                .as_str()
                .map(|s| truncate_with_ellipsis(s, COMMENT_PREVIEW_CHARS))
                .unwrap_or_default();
            let new = c.new.as_str().map_or_else(
                || "(removed)".to_string(),
                |s| truncate_with_ellipsis(s, COMMENT_PREVIEW_CHARS),
            );
            all_of(vec![row("Comment", old, new)])
        }
        // An Add .stalx whose recipe recorded no rows: what it brought.
        _ => {
            let shots: u64 = d.lines_created.iter().map(|l| u64::from(l.shots)).sum();
            all_of(vec![
                row("Lines added", "", d.lines_created.len().to_string()),
                row("Shots added", "", shots.to_string()),
            ])
        }
    })
}

fn name_or_unnamed<S: AsRef<str>>(name: Option<S>) -> String {
    match name.as_ref().map(|n| n.as_ref().trim()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => "unnamed".to_string(),
    }
}

/// The user-facing label of a recorded kind (`ShotKind::label`).
fn kind_label(value: &Val) -> String {
    match value.as_str() {
        Some("Normal") => ShotKind::Normal.label().to_string(),
        Some("Jump") => ShotKind::Jump.label().to_string(),
        Some("Reference") => ShotKind::Reference.label().to_string(),
        _ => value.display(),
    }
}

/// A shot cell as 0.6.0's preview printed it: lengths, depths, pitches and
/// LRUD to 2 decimals, azimuths and temperature to 1, a kind by its label,
/// an absent value empty.
fn cell(field: ShotField, value: &Val) -> String {
    let decimals = match field {
        ShotField::AzimuthIn | ShotField::AzimuthOut | ShotField::Temperature => Some(1),
        ShotField::Length
        | ShotField::DepthIn
        | ShotField::DepthOut
        | ShotField::PitchIn
        | ShotField::PitchOut
        | ShotField::Left
        | ShotField::Right
        | ShotField::Up
        | ShotField::Down => Some(2),
        _ => None,
    };
    match (value, decimals) {
        (Val::Null, _) => String::new(),
        (Val::Num(n), Some(places)) => format!("{n:.places$}"),
        _ if field == ShotField::Kind => kind_label(value),
        _ => value.display(),
    }
}

/// Every shot cell the step changed, in line, shot and field order.
fn shot_cell_rows(d: &Digest) -> Vec<FieldChange> {
    d.shot_cells
        .iter()
        .map(|c| {
            row(
                c.field.label(),
                c.old.as_ref().map(|v| cell(c.field, v)).unwrap_or_default(),
                cell(c.field, &c.new),
            )
        })
        .collect()
}

fn text(value: &Val) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// The credits the step's credit cells held before and after, one pair per
/// changed shot. An unpaired before value (a reversed run, which no credit
/// edit records) reads as no credit.
fn credit_pairs(d: &Digest, changes: &Changeset) -> Vec<(SurveyCredit, SurveyCredit)> {
    let credit = |v: Option<&Val>| {
        v.and_then(|v| changes.credit(v))
            .cloned()
            .unwrap_or_default()
    };
    d.shot_cells
        .iter()
        .filter(|c| c.field == ShotField::Credit)
        .map(|c| (credit(c.old.as_ref()), credit(Some(&c.new))))
        .collect()
}

/// The people of one role over `credits`, each once, as a name list cut at
/// [`MAX_NAMES_CHARS`] (0.6.0's `history_credit::union`).
fn union<'a>(people: impl Iterator<Item = &'a Person>) -> String {
    let people = dedupe_people(people.cloned().collect());
    let text = join_names(&people).unwrap_or_default();
    match text.char_indices().nth(MAX_NAMES_CHARS) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text,
    }
}

/// `2025-03-01` or `2025-03-01 - 2025-03-04` over the dates `credits` hold.
fn date_range<'a>(credits: impl Iterator<Item = &'a SurveyCredit>) -> String {
    let dates: BTreeSet<_> = credits.filter_map(|c| c.date).collect();
    match (dates.first(), dates.last()) {
        (Some(lo), Some(hi)) if lo == hi => lo.to_string(),
        (Some(lo), Some(hi)) => format!("{lo} - {hi}"),
        _ => String::new(),
    }
}

/// A credit edit's rows (0.6.0's `credit_preview`): the surveyors, explorers
/// and dates before and after over the changed shots (only the ones that
/// differ), then the number of shots.
fn credit_rows(d: &Digest, changes: &Changeset) -> Vec<FieldChange> {
    let pairs = credit_pairs(d, changes);
    let before = || pairs.iter().map(|(b, _)| b);
    let after = || pairs.iter().map(|(_, a)| a);
    let mut rows = vec![
        row(
            "Surveyors",
            union(before().flat_map(|c| c.surveyors.iter())),
            union(after().flat_map(|c| c.surveyors.iter())),
        ),
        row(
            "Explorers",
            union(before().flat_map(|c| c.explorers.iter())),
            union(after().flat_map(|c| c.explorers.iter())),
        ),
        row("Date", date_range(before()), date_range(after())),
    ];
    rows.retain(|r| r.old_value != r.new_value);
    rows.push(row("Shots", "", pairs.len().to_string()));
    rows
}

/// A line metadata edit's rows in 0.6.0's order: name, surveyor, date,
/// colour, direction, closure weight. A line with shots keeps its surveyor
/// and date on the shots' credit, so those two rows are read off the credit
/// cells: the surveyors before (the union over the changed shots), the
/// people after (in the `explorers / surveyors` form when the explorers
/// changed too), the earliest date before and after.
fn line_edit_rows(d: &Digest, changes: &Changeset) -> Vec<FieldChange> {
    let field = |f: LineField| d.line_fields.iter().find(|c| c.field == f);
    let mut rows = Vec::new();
    if let Some(c) = field(LineField::Name) {
        rows.push(row(
            "Name",
            name_or_unnamed(c.old.as_str()),
            name_or_unnamed(c.new.as_str()),
        ));
    }
    let pairs = credit_pairs(d, changes);
    if let Some(c) = field(LineField::Surveyor) {
        rows.push(row("Surveyor", text(&c.old), text(&c.new)));
    } else if !pairs.is_empty() {
        let explorers = |side: &dyn Fn(&(SurveyCredit, SurveyCredit)) -> &SurveyCredit| {
            dedupe_people(
                pairs
                    .iter()
                    .flat_map(|p| side(p).explorers.iter().cloned())
                    .collect(),
            )
        };
        let surveyors_after = dedupe_people(
            pairs
                .iter()
                .flat_map(|(_, a)| a.surveyors.iter().cloned())
                .collect(),
        );
        let explorers_after = explorers(&|p| &p.1);
        let old = union(pairs.iter().flat_map(|(b, _)| b.surveyors.iter()));
        let new = if explorers(&|p| &p.0) == explorers_after {
            join_names(&surveyors_after).unwrap_or_default()
        } else {
            legacy_credit_text(&explorers_after, &surveyors_after).unwrap_or_default()
        };
        if old != new {
            rows.push(row("Surveyor", old, new));
        }
    }
    if let Some(c) = field(LineField::Date) {
        rows.push(row("Date", text(&c.old), text(&c.new)));
    } else if !pairs.is_empty() {
        let earliest = |side: &dyn Fn(&(SurveyCredit, SurveyCredit)) -> &SurveyCredit| {
            pairs
                .iter()
                .filter_map(|p| side(p).date)
                .min()
                .map(|d| d.to_string())
                .unwrap_or_default()
        };
        let (old, new) = (earliest(&|p| &p.0), earliest(&|p| &p.1));
        if old != new {
            rows.push(row("Date", old, new));
        }
    }
    if let Some(c) = field(LineField::Color) {
        rows.push(row("Color", text(&c.old), text(&c.new)));
    }
    if let Some(c) = field(LineField::Direction) {
        rows.push(row("Direction", text(&c.old), text(&c.new)));
    }
    if let Some(c) = field(LineField::Quality) {
        let weight = |v: &Val| v.as_f64().map(|q| format!("{q:.2}")).unwrap_or_default();
        rows.push(row("Closure Adj. Weight", weight(&c.old), weight(&c.new)));
    }
    rows
}
