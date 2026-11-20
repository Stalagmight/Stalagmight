//! The comment family: Set Station Comment (the station's one comment,
//! shared by every slot of a junction) and Move Comment (move or swap a
//! comment between two holders, stations or shots, as one undo step).
//!
//! An empty comment is no comment (`None`), on stations and shots alike;
//! comments are stored as typed (not trimmed), as in 0.6.0.

use super::commands::{Applied, Record};
use crate::core::{Command, CommentHolder};
use crate::models::survey::Project;
use crate::models::{LineId, StationId, StationRef, LINE_GONE, SHOT_GONE};

/// The execute pipeline's mutation for the comment family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it.
pub(super) fn apply(
    project: &mut Project,
    _record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    match cmd {
        Command::SetStationComment {
            station_ref,
            comment,
        } => {
            let station = commented_station(project, *station_ref)?;
            write(project, Holder::Station(station), comment.clone());
            tracing::info!(
                ?station_ref,
                station,
                purpose = "set station comment",
                "Station comment updated"
            );
            Ok(Applied::default())
        }
        Command::MoveComment {
            source,
            target,
            swap,
        } => {
            move_comment(project, source, target, *swap)?;
            tracing::info!(
                swap,
                purpose = "move comment atomically",
                "Move comment completed"
            );
            Ok(Applied::default())
        }
        _ => Err(format!(
            "{} is not a comment command",
            crate::core::command_kind(cmd)
        )),
    }
}

/// A comment holder resolved on the model: a station (every slot of a
/// junction is the one station), or a shot by its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    Station(StationId),
    Shot { line: LineId, index: usize },
}

/// The station Set Station Comment addresses, or 0.6.0's refusal.
fn commented_station(project: &Project, slot: StationRef) -> Result<StationId, String> {
    let line = project
        .line(slot.line_id)
        .ok_or_else(|| LINE_GONE.to_string())?;
    let last = line.shots.len();
    project.station_at(slot).ok_or_else(|| {
        format!(
            "Station index {} out of range (line has {} stations)",
            slot.station_index,
            last + 1
        )
    })
}

/// Resolve a Move Comment holder, or 0.6.0's refusal.
fn resolve(project: &Project, holder: &CommentHolder) -> Result<Holder, String> {
    match holder {
        CommentHolder::Station { station_ref } => {
            if project.line(station_ref.line_id).is_none() {
                return Err(LINE_GONE.to_string());
            }
            project
                .station_at(*station_ref)
                .map(Holder::Station)
                .ok_or_else(|| format!("Station index {} out of range", station_ref.station_index))
        }
        CommentHolder::Shot { line_id, shot_id } => {
            let line = project
                .line(*line_id)
                .ok_or_else(|| LINE_GONE.to_string())?;
            let index = line
                .shots
                .iter()
                .position(|s| s.id == *shot_id)
                .ok_or_else(|| SHOT_GONE.to_string())?;
            Ok(Holder::Shot {
                line: *line_id,
                index,
            })
        }
    }
}

/// The comment `holder` has.
fn read(project: &Project, holder: Holder) -> Option<String> {
    match holder {
        Holder::Station(station) => project.stations.get(&station)?.comment.clone(),
        Holder::Shot { line, index } => project.line(line)?.shots.get(index)?.comment.clone(),
    }
}

/// Give `holder` `comment`; an empty one is none.
fn write(project: &mut Project, holder: Holder, comment: Option<String>) {
    let comment = comment.filter(|text| !text.is_empty());
    match holder {
        Holder::Station(station) => {
            if let Some(s) = project.stations.get_mut(&station) {
                s.comment = comment;
            }
        }
        Holder::Shot { line, index } => {
            if let Some(shot) = project.line_mut(line).and_then(|l| l.shots.get_mut(index)) {
                shot.comment = comment;
            }
        }
    }
}

/// Move the comment of `source` to `target` (the target's own comment is
/// replaced), or swap the two. The source must have a comment. Two slots of
/// one junction are one holder: moving a comment onto the station it is on
/// changes nothing (0.6.0's two writes would have deleted it).
fn move_comment(
    project: &mut Project,
    source: &CommentHolder,
    target: &CommentHolder,
    swap: bool,
) -> Result<(), String> {
    let from = resolve(project, source)?;
    let to = resolve(project, target)?;
    let moved = read(project, from).ok_or_else(|| "Source has no comment to move".to_string())?;
    if from == to {
        return Ok(());
    }
    let replaced = read(project, to);
    write(project, to, Some(moved));
    write(project, from, if swap { replaced } else { None });
    Ok(())
}
