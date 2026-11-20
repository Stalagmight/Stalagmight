//! History labels and refs of a command, worded at record time with the
//! names the open project holds (plan 8.2: record-time labels; names never
//! positional). Ported from 0.6.0's `history_names.rs` onto the station
//! graph: a station reads `Line name / station name` on the slot the
//! command addresses. The batch summaries (`history_batch.rs`) and the
//! per-entry refinements (`describe_entry`) are ported by the
//! history-labels work package.

use std::collections::HashSet;

use crate::core::{Command, CommentHolder};
use crate::models::survey::changeset::HistoryRefs;
use crate::models::survey::Project;
use crate::models::{CreditTargets, LineId, ShotKind, StationRef};

/// The names a label reads, off the project as it is now.
pub(super) struct NameBook<'p> {
    project: Option<&'p Project>,
}

impl Default for NameBook<'_> {
    fn default() -> Self {
        Self { project: None }
    }
}

impl<'p> NameBook<'p> {
    pub(super) fn of(project: &'p Project) -> Self {
        Self {
            project: Some(project),
        }
    }

    /// A line's name (`Line N` when it has none), if the line exists.
    pub(super) fn line(&self, line_id: LineId) -> Option<String> {
        let project = self.project?;
        project.line(line_id)?;
        Some(project.line_label_of(line_id))
    }

    /// `Line name / station name` of the station at a slot.
    pub(super) fn station(&self, slot: StationRef) -> Option<String> {
        let project = self.project?;
        let id = project.topology().station_at(slot)?;
        let station = project.stations.get(&id)?;
        Some(format!("{} / {}", self.line(slot.line_id)?, station.name))
    }
}

/// The label of `cmd` worded on `project` (before it runs).
pub(super) fn command_label(project: &Project, cmd: &Command) -> String {
    describe(cmd, &NameBook::of(project))
}

/// The stable ids `cmd` names on `project` (before it runs): its lines, the
/// stations at its slots, the uuids of its shots. Never session ids or slots.
pub(super) fn command_refs(project: &Project, cmd: &Command) -> HistoryRefs {
    let mut lines = HashSet::new();
    referenced_lines(cmd, &mut lines);
    let mut refs = HistoryRefs::default();
    let mut lines: Vec<LineId> = lines.into_iter().filter(|l| project.line(*l).is_some()).collect();
    lines.sort_unstable();
    refs.lines = lines;
    let mut slot = |sr: &StationRef| {
        if let Some(id) = project.topology().station_at(*sr) {
            if !refs.stations.contains(&id) {
                refs.stations.push(id);
            }
        }
    };
    match cmd {
        Command::MergeStations { station_a, station_b }
        | Command::UnmergeStationPair { station_a, station_b }
        | Command::CreateLoopClosure { station_a, station_b, .. } => {
            slot(station_a);
            slot(station_b);
        }
        Command::UnmergeStations { station_ref }
        | Command::SetZeroDatum { station_ref, .. }
        | Command::RemoveZeroDatum { station_ref }
        | Command::SetStationComment { station_ref, .. }
        | Command::SetStationName { station_ref, .. }
        | Command::MoveStation { station_ref, .. } => slot(station_ref),
        _ => {}
    }
    let shot = |line_id: LineId, shot_id: crate::models::ShotId| {
        project
            .line(line_id)?
            .shots
            .iter()
            .find(|s| s.id == shot_id)
            .map(|s| s.uuid.clone())
    };
    match cmd {
        Command::UpdateShot { line_id, shot_id, .. }
        | Command::DeleteShot { line_id, shot_id }
        | Command::SetShotKind { line_id, shot_id, .. } => {
            refs.shots.extend(shot(*line_id, *shot_id));
        }
        Command::RemoveLoopClosure { closure_id } => refs.closures.push(*closure_id),
        _ => {}
    }
    refs
}

/// Every line `cmd` addresses (batch sub-commands included).
pub(super) fn referenced_lines(cmd: &Command, out: &mut HashSet<LineId>) {
    let mut station = |sr: &StationRef| {
        out.insert(sr.line_id);
    };
    match cmd {
        Command::DeleteLine { line_id }
        | Command::UpdateLineMetadata { line_id, .. }
        | Command::ReverseLine { line_id, .. }
        | Command::CreateShot { line_id, .. }
        | Command::UpdateShot { line_id, .. }
        | Command::DeleteShot { line_id, .. }
        | Command::SetShotKind { line_id, .. }
        | Command::MoveLine { line_id, .. }
        | Command::MoveSegment { line_id, .. }
        | Command::EditShotCredit {
            targets: CreditTargets::Line { line_id },
            ..
        } => {
            out.insert(*line_id);
        }
        Command::MergeStations {
            station_a,
            station_b,
        }
        | Command::UnmergeStationPair {
            station_a,
            station_b,
        }
        | Command::CreateLoopClosure {
            station_a,
            station_b,
            ..
        } => {
            station(station_a);
            station(station_b);
        }
        Command::UnmergeStations { station_ref }
        | Command::SetZeroDatum { station_ref, .. }
        | Command::RemoveZeroDatum { station_ref }
        | Command::SetStationComment { station_ref, .. }
        | Command::SetStationName { station_ref, .. }
        | Command::MoveStation { station_ref, .. } => station(station_ref),
        Command::MoveComment { source, target, .. } => {
            for holder in [source, target] {
                match holder {
                    CommentHolder::Station { station_ref } => {
                        out.insert(station_ref.line_id);
                    }
                    CommentHolder::Shot { line_id, .. } => {
                        out.insert(*line_id);
                    }
                }
            }
        }
        Command::Batch { commands, .. } => {
            for sub in commands {
                referenced_lines(sub, out);
            }
        }
        _ => {}
    }
}

/// What a Set Shot Kind label calls the kind.
fn kind_word(kind: ShotKind) -> &'static str {
    match kind {
        ShotKind::Normal => "permanent",
        ShotKind::Jump => "jump",
        ShotKind::Reference => "reference",
    }
}

/// `"Name"` for a line, quoted, or `None`.
fn quoted(names: &NameBook<'_>, line_id: LineId) -> Option<String> {
    names.line(line_id).map(|n| format!("\"{n}\""))
}

fn holder(names: &NameBook<'_>, holder: &CommentHolder) -> String {
    match holder {
        CommentHolder::Station { station_ref } => names
            .station(*station_ref)
            .map_or_else(|| "a station".to_string(), |s| format!("station {s}")),
        CommentHolder::Shot { line_id, .. } => quoted(names, *line_id)
            .map_or_else(|| "a shot".to_string(), |n| format!("a shot in line {n}")),
    }
}

/// The history label of an entry: [`describe`], except where the undo data
/// knows better (a rename names the station by the name it had).
pub(super) fn describe(cmd: &Command, names: &NameBook<'_>) -> String {
    let station = |sr: &StationRef| names.station(*sr);
    let pair =
        |verb: &str, a: &StationRef, b: &StationRef, joiner: &str| match (station(a), station(b)) {
            (Some(a), Some(b)) => format!("{verb} {a}{joiner}{b}"),
            _ => verb.to_string(),
        };
    let on_station = |verb: &str, sr: &StationRef| match station(sr) {
        Some(s) => format!("{verb} {s}"),
        None => verb.to_string(),
    };
    match cmd {
        Command::DeleteLine { line_id } => match quoted(names, *line_id) {
            Some(n) => format!("Delete line {n}"),
            None => "Delete line".to_string(),
        },
        Command::UpdateLineMetadata { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Update line {n} metadata"),
            None => "Update line metadata".to_string(),
        },
        Command::ReverseLine { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Reverse line {n}"),
            None => "Reverse line".to_string(),
        },
        Command::CreateShot { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Create shot in line {n}"),
            None => "Create shot".to_string(),
        },
        Command::UpdateShot { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Update shot in line {n}"),
            None => "Update shot".to_string(),
        },
        Command::DeleteShot { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Delete shot in line {n}"),
            None => "Delete shot".to_string(),
        },
        Command::SetShotKind { line_id, kind, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Mark shot in line {n} as {}", kind_word(*kind)),
            None => format!("Mark shot as {}", kind_word(*kind)),
        },
        Command::EditShotCredit {
            targets: CreditTargets::Line { line_id },
            edit,
        } => match quoted(names, *line_id) {
            Some(n) => format!("{} in line {n}", edit.label()),
            None => edit.label().to_string(),
        },
        Command::MergeStations {
            station_a,
            station_b,
        } => pair("Merge stations", station_a, station_b, " and "),
        Command::UnmergeStations { station_ref } => on_station("Unmerge station", station_ref),
        Command::UnmergeStationPair {
            station_a,
            station_b,
        } => pair("Unmerge station pair", station_a, station_b, " and "),
        Command::CreateLoopClosure {
            station_a,
            station_b,
            ..
        } => pair("Create loop closure between", station_a, station_b, " and "),
        Command::SetZeroDatum { station_ref, .. } => on_station("Set zero datum at", station_ref),
        Command::RemoveZeroDatum { station_ref } => {
            on_station("Remove zero datum from", station_ref)
        }
        Command::SetStationComment {
            station_ref,
            comment,
        } => {
            if comment.is_some() {
                on_station("Set comment on station", station_ref)
            } else {
                on_station("Remove comment from station", station_ref)
            }
        }
        Command::SetStationName { station_ref, name } => match station(station_ref) {
            Some(s) => format!("Rename station {s} to \"{}\"", name.trim()),
            None => format!("Rename station to \"{}\"", name.trim()),
        },
        Command::MoveLine { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Move line {n}"),
            None => "Move line".to_string(),
        },
        Command::MoveSegment { line_id, .. } => match quoted(names, *line_id) {
            Some(n) => format!("Move segment (line {n})"),
            None => "Move segment".to_string(),
        },
        Command::MoveStation { station_ref, .. } => on_station("Move station", station_ref),
        Command::MoveComment {
            source,
            target,
            swap,
        } => {
            let (source, target) = (holder(names, source), holder(names, target));
            if *swap {
                format!("Swap comments between {source} and {target}")
            } else {
                format!("Move comment from {source} to {target}")
            }
        }
        // A closure is addressed by its numeric id, which is no name.
        Command::RemoveLoopClosure { .. } => "Remove loop closure".to_string(),
        // Every other command's description holds no positional handle.
        _ => cmd.description(),
    }
}
