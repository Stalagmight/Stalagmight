//! The move family: Move Line / Move Segment (translate a free piece's
//! `Placement`) and Move Station (rewrite the shots incident to one station),
//! plan 4.4, on the model's `move_line` / `move_segment` / `move_station`.
//!
//! Refusals keep 0.6.0's words: a non-finite offset, a missing line or slot,
//! a piece (rigid group) that holds a zero datum, a zero datum station, a
//! rewrite that would leave a shot without length. A move by nothing (every
//! axis within 1e-9 m) changes nothing, so the pipeline records no entry.
//!
//! A batch that moves several slots of ONE station (the map's co-located
//! drag: a junction is shown on each of its lines) moves that station once
//! (plan 9.1, [`StationMoves`]).

use super::commands::{Applied, Record};
use crate::core::Command;
use crate::models::survey::changeset::PreviewRow;
use crate::models::survey::{OpError, Project, SegmentMoved};
use crate::models::{Coordinates, LineId, StationId, StationRef};

/// 0.6.0's Move Station refusal of a NaN or infinite offset (its Move
/// Segment refusal, the model's [`OpError::NonFiniteOffset`], is worded
/// differently).
const STATION_OFFSET_NOT_FINITE: &str = "Move offset has a non-finite (NaN/inf) value";

/// The execute pipeline's mutation for the move family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit). A move on its own records 0.6.0's
/// preview rows (the offset, and the line or the number of lines moved); in
/// a batch the batch's own rows stand.
pub(super) fn apply(
    project: &mut Project,
    record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    let alone = record.kind == crate::core::command_kind(cmd);
    match cmd {
        Command::MoveLine { line_id, offset } => {
            let line = project
                .line(*line_id)
                .map(|_| row("Line", project.line_label_of(*line_id)));
            let moved = project
                .move_line(*line_id, xyz(offset))
                .map_err(|e| e.to_string())?;
            log_segment_move(*line_id, &moved.value);
            if alone {
                record.preview.rows = line
                    .into_iter()
                    .chain([row("Offset", offset.display_offset())])
                    .collect();
            }
            Ok(Applied::default())
        }
        Command::MoveSegment { line_id, offset } => {
            let moved = project
                .move_segment(*line_id, xyz(offset))
                .map_err(|e| e.to_string())?;
            log_segment_move(*line_id, &moved.value);
            if alone {
                record.preview.rows = vec![
                    row("Offset", offset.display_offset()),
                    row("Affected lines", moved.value.lines.len().to_string()),
                ];
            }
            Ok(Applied::default())
        }
        Command::MoveStation {
            station_ref,
            offset,
        } => {
            move_station(project, *station_ref, offset)?;
            if alone {
                record.preview.rows = vec![row("Offset", offset.display_offset())];
            }
            Ok(Applied::default())
        }
        _ => Err(format!(
            "{} is not a move command",
            crate::core::command_kind(cmd)
        )),
    }
}

/// Move the station at `slot` by `offset`, with 0.6.0's refusals in its
/// order: the offset, the line, the slot, then the model's (a datum on the
/// station, a shot left without length).
fn move_station(
    project: &mut Project,
    slot: StationRef,
    offset: &Coordinates,
) -> Result<(), String> {
    if !offset.is_finite() {
        return Err(STATION_OFFSET_NOT_FINITE.to_string());
    }
    let station = moved_station(project, slot)?;
    let moved = project
        .move_station(station, xyz(offset))
        .map_err(|e| match e {
            OpError::NonFiniteOffset => STATION_OFFSET_NOT_FINITE.to_string(),
            other => other.to_string(),
        })?;
    tracing::info!(
        ?slot,
        station,
        shots_rewritten = moved.value.rewritten.len(),
        placement_moved = moved.value.placement_moved,
        hand_moves_folded = moved.value.hand_moves_folded.len(),
        purpose = "adjust station position",
        "Station moved (shot data updated)"
    );
    Ok(())
}

/// 0.6.0's log line of a segment move (ids and counts only).
fn log_segment_move(line_id: LineId, moved: &SegmentMoved) {
    tracing::info!(
        line_id,
        line_count = moved.lines.len(),
        placements_moved = moved.placements.len(),
        purpose = "move connected segment as rigid group",
        "Segment moved"
    );
}

/// The station a Move Station addresses, or 0.6.0's refusal.
fn moved_station(project: &Project, slot: StationRef) -> Result<StationId, String> {
    if project.line(slot.line_id).is_none() {
        return Err("Line not found".to_string());
    }
    project
        .station_at(slot)
        .ok_or_else(|| "Invalid station index".to_string())
}

/// One preview row, `field: -> new`, as 0.6.0's move previews wrote it.
fn row(field: &str, new: String) -> PreviewRow {
    PreviewRow {
        field: field.to_string(),
        old: String::new(),
        new,
    }
}

fn xyz(offset: &Coordinates) -> [f64; 3] {
    [offset.x, offset.y, offset.z]
}

/// The run of consecutive Move Station sub-commands of one batch, so that
/// a batch moving several slots of ONE station by the same offset moves it
/// once (plan 9.1).
///
/// The map's station, shot and snap drags send one Move Station per
/// co-located slot: every slot of a junction (one station on each of its
/// lines) and the ends of aligned closures. In 0.6.0 each slot was a station
/// of its own; on the station graph a junction's slots are one station, and
/// moving it once per slot would carry it by a multiple of the offset. A
/// repeat is the same station with the same offset (bit for bit) within one
/// unbroken run of Move Station sub-commands; any other sub-command ends the
/// run (it may change which station a slot addresses). Slots of different
/// stations (an aligned closure's two ends) all move, as before.
#[derive(Debug, Default)]
pub(super) struct StationMoves {
    run: Vec<(StationId, [u64; 3])>,
}

impl StationMoves {
    /// Whether `cmd` repeats a station move this run already made (the
    /// batch skips it); otherwise notes it, or ends the run for any other
    /// command. A slot that does not resolve is not a repeat: its own apply
    /// refuses it.
    pub(super) fn repeats(&mut self, project: &Project, cmd: &Command) -> bool {
        let Command::MoveStation {
            station_ref,
            offset,
        } = cmd
        else {
            self.run.clear();
            return false;
        };
        let Some(station) = project.station_at(*station_ref) else {
            return false;
        };
        let key = (station, xyz(offset).map(f64::to_bits));
        if self.run.contains(&key) {
            return true;
        }
        self.run.push(key);
        false
    }
}
