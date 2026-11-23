//! CSV and JSON on the station-graph model: one row (record) per shot, in line
//! order then shot order, exactly 0.6.0's layouts (`super::super::csv`,
//! `super::super::json`), with each shot's two stations read through the
//! chain.
//!
//! Stations are written by their **shown identity** (RS 9): the name and the
//! Station ID the app shows for them.
//! - A junction is one station, so every line through it names it by the
//!   kept station's name and Station ID; the stations a merge archived are
//!   not written.
//! - A loop end a closure archives is written under its kept end's name and
//!   Station ID. These files hold readings only (no coordinates, no
//!   closures), so a shared name and id is how a name- or id-tying reader
//!   ties the loop. A closure whose two ends are on one line lists that one
//!   name and id twice on the line.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::models::survey::{Line, Project, Shot, ShownIndex};
use crate::models::{LineCreditSummary, SurveyCredit, EMPTY_CREDIT};

use super::super::atomic_write::write_atomic;
use super::super::csv::{escape_csv, format_optional, CSV_HEADER};
use super::super::json::{names, JsonExport, JsonLine, JsonProjectMeta, JsonShot};

/// A shot's credit, the empty one when it has none.
pub(crate) fn shot_credit(shot: &Shot) -> &SurveyCredit {
    shot.credit.as_deref().unwrap_or(&EMPTY_CREDIT)
}

/// The credit aggregate of a line's shots (the legacy `Line::credit_summary`).
fn line_credit_summary(line: &Line) -> LineCreditSummary {
    LineCreditSummary::from_credits(line.shots.iter().map(shot_credit))
}

/// The `(name, Station ID)` of each station of `line`'s chain, by index: its
/// shown identity's (module docs), read through `shown` (the project's
/// closure archives, [`Project::shown_index`]).
fn station_columns(project: &Project, shown: &ShownIndex, line: &Line) -> Vec<(String, u32)> {
    line.stations()
        .map(|node| {
            let id = shown.shown(node);
            let name = project
                .station(id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            (name, id)
        })
        .collect()
}

/// Export the project to CSV (`CSV_HEADER`, one row per shot).
pub fn export_csv(project: &Project, path: &Path) -> Result<()> {
    write_atomic(path, |file| write_csv(project, file)).context("Failed to write CSV file")?;
    tracing::info!(path = ?path, "Exported CSV");
    Ok(())
}

/// Write the CSV document to `out`.
pub fn write_csv(project: &Project, out: &mut impl Write) -> Result<()> {
    writeln!(out, "{CSV_HEADER}")?;
    let shown = project.shown_index();
    for line in project.lines() {
        let line_name = line.name.as_deref().unwrap_or("");
        let stations = station_columns(project, &shown, line);
        for (index, shot) in line.shots.iter().enumerate() {
            let (from, from_id) = &stations[index];
            let (to, to_id) = &stations[index + 1];
            writeln!(
                out,
                "{},{},{},{},{},{:.2},{:.1},{:.1},{:.2},{:.2},{},{},{},{},{},{}",
                escape_csv(line_name),
                escape_csv(from),
                escape_csv(to),
                from_id,
                to_id,
                shot.length,
                shot.azimuth_in,
                shot.azimuth_out,
                shot.depth_in,
                shot.depth_out,
                format_optional(shot.left),
                format_optional(shot.right),
                format_optional(shot.up),
                format_optional(shot.down),
                shot.kind.label(),
                escape_csv(shot.comment.as_deref().unwrap_or(""))
            )?;
        }
    }
    Ok(())
}

/// Export the project to JSON, stamped with the current time.
pub fn export_json(project: &Project, path: &Path) -> Result<()> {
    write_atomic(path, |w| write_json(project, Utc::now(), w))
        .context("Failed to write JSON file")?;
    tracing::info!(path = ?path, "Exported JSON");
    Ok(())
}

/// Write the JSON document to `out`, `exported_at` being the only input that
/// is not the project: the same project and time give the same bytes.
pub fn write_json(
    project: &Project,
    exported_at: DateTime<Utc>,
    out: &mut impl Write,
) -> Result<()> {
    let json = serde_json::to_string_pretty(&json_export(project, exported_at))
        .context("Failed to serialize JSON")?;
    out.write_all(json.as_bytes())?;
    Ok(())
}

/// The JSON records of the project (0.6.0's [`JsonExport`] layout).
pub fn json_export(project: &Project, exported_at: DateTime<Utc>) -> JsonExport {
    let shown = project.shown_index();
    JsonExport {
        project: JsonProjectMeta {
            name: project.name.clone(),
            location: project.location.clone(),
            exported_at: exported_at.to_rfc3339(),
        },
        lines: project
            .lines()
            .iter()
            .map(|line| json_line(project, &shown, line))
            .collect(),
    }
}

fn json_line(project: &Project, shown: &ShownIndex, line: &Line) -> JsonLine {
    let summary = line_credit_summary(line);
    let surveyors = summary.surveyor_people();
    let first_date = summary.date_min;
    let stations = station_columns(project, shown, line);
    JsonLine {
        name: line.name.clone(),
        surveyor: crate::models::join_names(&surveyors),
        date: first_date.map(|d| d.to_string()),
        surveyors: names(&surveyors),
        explorers: names(&summary.explorer_people()),
        date_last: summary
            .date_max
            .filter(|last| Some(*last) != first_date)
            .map(|d| d.to_string()),
        color: line.color.clone(),
        section: line
            .section_id
            .and_then(|id| project.sections.iter().find(|s| s.id == id))
            .map(|section| section.name.clone()),
        shots: line
            .shots
            .iter()
            .enumerate()
            .map(|(index, shot)| {
                let credit = shot_credit(shot);
                let lrud = [shot.left, shot.right, shot.up, shot.down];
                JsonShot {
                    from: stations[index].0.clone(),
                    to: stations[index + 1].0.clone(),
                    from_id: Some(stations[index].1),
                    to_id: Some(stations[index + 1].1),
                    length: shot.length,
                    azimuth_in: shot.azimuth_in,
                    azimuth_out: shot.azimuth_out,
                    depth_in: shot.depth_in,
                    depth_out: shot.depth_out,
                    lrud: lrud
                        .iter()
                        .any(Option::is_some)
                        .then(|| lrud.map(|v| v.unwrap_or(0.0))),
                    kind: shot.kind,
                    comment: shot.comment.clone(),
                    surveyors: names(&credit.surveyors),
                    explorers: names(&credit.explorers),
                    date: credit.date.map(|d| d.to_string()),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "tabular_tests.rs"]
mod tests;
