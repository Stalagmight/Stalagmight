//! Names for the undo-history labels and previews: a line by its name, a
//! station as `Line name / station name`, a shot as `Line name: from → to` -
//! never by the positional ids (`line:index`, shot ids) the commands address
//! them with, which are session handles the user never sees.
//!
//! The names are read off the project when the history is listed
//! ([`NameBook::of`]), so a label names what the project holds then. Where a
//! name cannot be resolved (the line or shot is gone, or an entry from an
//! older file addresses something that moved) the label falls back to its
//! wording without the handle (`Delete shot`), never to the number.
//!
//! A shot is named only through the uuid an entry records
//! (`UndoData::PreviousValues::shot_uuid`), never through the command's shot
//! id: that id is an in-session handle the loader renumbers, so after a
//! reload it can address a different shot. A command with no recorded uuid
//! names the shot's line instead (`Delete shot in line "Main"`).

use std::collections::{HashMap, HashSet};

use crate::core::{Command, CommentHolder, UndoData, UndoEntry};
use crate::models::{
    CreditTargets, LineId, Project, ShotKind, StationAnchor, StationKey, StationRef,
};

/// One line's names, by position.
#[derive(Debug, Default, Clone)]
struct LineNames {
    name: String,
    /// Station names by station index.
    stations: Vec<String>,
    /// Shot uuid -> shot index, to resolve a stored station key or a shot.
    uuids: HashMap<String, usize>,
}

/// The names a set of history labels needs, read off the project once.
#[derive(Debug, Default, Clone)]
pub struct NameBook {
    lines: HashMap<LineId, LineNames>,
}

impl NameBook {
    /// The names of `lines` as `project` holds them now. A line the project
    /// does not hold is simply absent (its labels fall back).
    pub fn of(project: &Project, lines: impl IntoIterator<Item = LineId>) -> Self {
        let mut book = Self::default();
        for line_id in lines {
            let Some(line) = project.get_line(line_id) else {
                continue;
            };
            let stations = project
                .line_station_names(line_id)
                .into_iter()
                .enumerate()
                .map(|(index, name)| name.unwrap_or_else(|| index.to_string()))
                .collect();
            book.lines.insert(
                line_id,
                LineNames {
                    name: project.line_label(line_id).unwrap_or_default(),
                    stations,
                    uuids: line
                        .shots
                        .iter()
                        .enumerate()
                        .filter_map(|(i, s)| s.uuid.clone().map(|u| (u, i)))
                        .collect(),
                },
            );
        }
        book
    }

    /// A line's name.
    pub fn line(&self, line_id: LineId) -> Option<&str> {
        self.lines.get(&line_id).map(|l| l.name.as_str())
    }

    /// `Line name / station name`.
    pub fn station(&self, station: StationRef) -> Option<String> {
        let line = self.lines.get(&station.line_id)?;
        let name = line.stations.get(station.station_index as usize)?;
        Some(format!("{} / {name}", line.name))
    }

    /// [`Self::station`] for a stored key (a closure recorded in undo data).
    pub fn station_key(&self, key: &StationKey) -> Option<String> {
        let line = self.lines.get(&key.line_id)?;
        let index = match &key.anchor {
            StationAnchor::LineStart => 0,
            StationAnchor::AfterShot(uuid) => line.uuids.get(uuid)? + 1,
            StationAnchor::Index(i) => *i as usize,
        };
        self.station(StationRef::new(key.line_id, u32::try_from(index).ok()?))
    }

    /// `Line name: from → to`, for the shot with `uuid`.
    pub fn shot(&self, line_id: LineId, uuid: &str) -> Option<String> {
        let line = self.lines.get(&line_id)?;
        let index = *line.uuids.get(uuid)?;
        let from = line.stations.get(index)?;
        let to = line.stations.get(index + 1)?;
        Some(format!("{}: {from} \u{2192} {to}", line.name))
    }
}

/// Every line `cmd` addresses (batch sub-commands included), so the history
/// can read just those lines' names.
pub fn referenced_lines(cmd: &Command, out: &mut HashSet<LineId>) {
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
fn quoted(names: &NameBook, line_id: LineId) -> Option<String> {
    names.line(line_id).map(|n| format!("\"{n}\""))
}

fn holder(names: &NameBook, holder: &CommentHolder) -> String {
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
pub fn describe_entry(entry: &UndoEntry, names: &NameBook) -> String {
    if let (
        Command::SetStationName { station_ref, name },
        UndoData::InverseCommand(Command::SetStationName { name: previous, .. }),
    ) = (&entry.command, &entry.undo_data)
    {
        let name = name.trim();
        return match names.line(station_ref.line_id) {
            Some(line) => format!("Rename station {line} / {previous} to \"{name}\""),
            None => format!("Rename station \"{previous}\" to \"{name}\""),
        };
    }
    // A shot edit whose entry recorded the shot's uuid names the shot.
    if let UndoData::PreviousValues {
        line_id,
        shot_uuid: Some(uuid),
        ..
    } = &entry.undo_data
    {
        if let Some(shot) = names.shot(*line_id, uuid) {
            match &entry.command {
                Command::UpdateShot { .. } => return format!("Update shot {shot}"),
                Command::SetShotKind { kind, .. } => {
                    return format!("Mark shot {shot} as {}", kind_word(*kind));
                }
                _ => {}
            }
        }
    }
    if let (Command::DeleteLine { line_id }, UndoData::ProjectSnapshotPair { before, .. }) =
        (&entry.command, &entry.undo_data)
    {
        if names.line(*line_id).is_none() {
            if let Some(name) = super::history_view::snapshot_line_name(before, *line_id) {
                return format!("Delete line \"{name}\"");
            }
        }
    }
    describe(&entry.command, names)
}

/// The history label of `cmd` with names (`Merge stations Main Line / 0 and
/// Side / 1`, `Delete line "Main Line"`, `Update shot in line "Main Line"` -
/// [`describe_entry`] names the shot itself when its entry recorded the uuid).
/// Commands that address nothing positional keep [`Command::description`].
pub fn describe(cmd: &Command, names: &NameBook) -> String {
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
