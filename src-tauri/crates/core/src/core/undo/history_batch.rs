//! The history labels of the edits recorded as one `Command::Batch`
//! (its `label`): a line by its name, a station as `Line name / station
//! name`, never by the positional handles the sub-commands address them with.
//!
//! The label is worded when the edit is recorded, from the project as it is
//! then, not when the history is listed. A composite edit (Insert Station,
//! Combine Lines, Split Line, Delete and Split, a New Line snapped onto a
//! station, an import) records placeholder sub-commands that only sketch what
//! it did, and what it is named by is not in them: the station an Insert
//! Station made has a name only the after-state knows, and a shot id is an
//! in-session handle a reload renumbers. A batch the frontend sends (a paste
//! in the data table, Reverse Lines, a move snapped onto a station) is
//! summarized from its sub-commands ([`summarize`]) with the names the
//! project held before it ran. An entry written before labels existed keeps
//! `Batch of N commands`.

use std::collections::BTreeSet;

use super::history_names::{describe, NameBook};
use crate::core::Command;
use crate::models::{LineId, Project, ShotId, ShotKind, StationRef};

/// How many names a label lists before it counts the rest.
const NAMES_LISTED: usize = 3;

fn quoted(name: &str) -> String {
    format!("\"{name}\"")
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// `A`, `A and B`, `A, B and C`, `A, B, C and 2 more`.
fn listed(names: &[String]) -> String {
    if names.len() > NAMES_LISTED {
        return format!(
            "{} and {} more",
            names[..NAMES_LISTED].join(", "),
            names.len() - NAMES_LISTED
        );
    }
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// A line as a label names it, quoted: `"Main Line"`.
fn line(project: &Project, line_id: LineId) -> String {
    project
        .line_label(line_id)
        .map_or_else(|| "a line".to_string(), |name| quoted(&name))
}

/// A station's own name, without its line: `4a`.
fn station_name(project: &Project, station: StationRef) -> String {
    project
        .station_name_at(station)
        .map_or_else(|| station.station_index.to_string(), str::to_string)
}

/// `Insert station Main Line / 4a`: Insert Station (a shot split at a
/// distance), read after the split; `head` is the new shot that ends at the
/// station it made.
pub(crate) fn insert_station(project: &Project, line_id: LineId, head: ShotId) -> String {
    let index = project
        .get_line(line_id)
        .and_then(|l| l.shots.iter().position(|s| s.id == head));
    match index.and_then(|i| u32::try_from(i + 1).ok()) {
        Some(index) => format!(
            "Insert station {}",
            project.station_text(StationRef::new(line_id, index))
        ),
        None => format!("Insert station in line {}", line(project, line_id)),
    }
}

/// `Combine lines "Main Line" and "Downstream"`, with ` into "Name"` when
/// the combined line takes a name neither had. Read before the combine.
pub(crate) fn combine_lines(
    project: &Project,
    kept: LineId,
    other: LineId,
    combined: &str,
) -> String {
    let combined = combined.trim();
    let names = [kept, other].map(|id| project.line_label(id));
    let label = format!(
        "Combine lines {} and {}",
        line(project, kept),
        line(project, other)
    );
    if combined.is_empty() || names.iter().flatten().any(|n| n == combined) {
        label
    } else {
        format!("{label} into {}", quoted(combined))
    }
}

/// `Create line "Side Passage" at Main Line / Junction`: a new line placed
/// by snapping one of its stations onto `target`, read after the merge.
pub(crate) fn create_line_at(project: &Project, line_id: LineId, target: StationRef) -> String {
    format!(
        "Create line {} at {}",
        line(project, line_id),
        project.station_text(target)
    )
}

/// `Split line "Main Line" at 5`: Split Line Here. `line_name` is the
/// line's name before the cut; `at` is the station it was cut at.
pub(crate) fn split_line(line_name: &str, at: &str) -> String {
    format!("Split line {} at {at}", quoted(line_name))
}

/// What a range delete does with the line besides deleting the shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShotsDeleted {
    /// The line is split in two around the gap.
    Split,
    /// The line closes up (shots at an end: the rest stays one line).
    CloseUp,
    /// The line's last shot goes, and the line with it.
    LastOfLine,
}

/// `Delete shot 4 → 5 and split line "Main Line"`, `Delete 3 shots (4 → 7)
/// from line "Main Line"`, `Delete line "Main Line" with its last shot`: the
/// shots at positions `first..=last` of `line_id`, read before the delete.
pub(crate) fn delete_shots(
    project: &Project,
    line_id: LineId,
    first: usize,
    last: usize,
    then: ShotsDeleted,
) -> String {
    let line = line(project, line_id);
    if then == ShotsDeleted::LastOfLine {
        return format!("Delete line {line} with its last shot");
    }
    let station = |index: usize| {
        u32::try_from(index)
            .ok()
            .map(|i| station_name(project, StationRef::new(line_id, i)))
    };
    let count = last.saturating_sub(first) + 1;
    let span = station(first)
        .zip(station(last + 1))
        .map(|(from, to)| format!("{from} \u{2192} {to}"));
    let shots = match (count, span) {
        (1, Some(span)) => format!("shot {span}"),
        (1, None) => "a shot".to_string(),
        (n, Some(span)) => format!("{n} shots ({span})"),
        (n, None) => format!("{n} shots"),
    };
    match then {
        ShotsDeleted::Split => format!("Delete {shots} and split line {line}"),
        _ => format!("Delete {shots} from line {line}"),
    }
}

/// `Import line "Main"`, `Import lines "A" and "B"`, `Import 5 lines ("A",
/// "B", "C" and 2 more)`: the lines one import made, read after it.
pub(crate) fn import_lines(project: &Project, line_ids: &[LineId]) -> String {
    let names: Vec<String> = line_ids.iter().map(|&id| line(project, id)).collect();
    match names.len() {
        0 => "Import".to_string(),
        1 => format!("Import line {}", listed(&names)),
        n if n <= NAMES_LISTED => format!("Import lines {}", listed(&names)),
        n => format!("Import {n} lines ({})", listed(&names)),
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

/// The kinds of command a summary tells apart, and the order in which one
/// stands for a batch of several kinds: a station move snapped onto another
/// station is the merge (or closure) it ends in, Edit Line's Save the line's
/// metadata edit, the DMP wizard's withdrawal the lines it deletes.
const PRINCIPAL: &[&str] = &[
    "merge_stations",
    "create_loop_closure",
    "delete_line",
    "update_line_metadata",
    "reverse_line",
];

/// The tag a summary groups a command under.
fn kind(cmd: &Command) -> &'static str {
    match cmd {
        Command::MergeStations { .. } => "merge_stations",
        Command::CreateLoopClosure { .. } => "create_loop_closure",
        Command::DeleteLine { .. } => "delete_line",
        Command::UpdateLineMetadata { .. } => "update_line_metadata",
        Command::ReverseLine { .. } => "reverse_line",
        Command::UpdateShot { .. } => "update_shot",
        Command::SetShotKind { .. } => "set_shot_kind",
        Command::MoveStation { .. } => "move_station",
        Command::SetStationName { .. } => "set_station_name",
        Command::MoveLine { .. } => "move_line",
        Command::MoveSegment { .. } => "move_segment",
        _ => "other",
    }
}

/// The lines of `cmds`, by name where the book has one.
fn line_names(cmds: &[&Command], names: &NameBook) -> Option<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for cmd in cmds {
        let id = match cmd {
            Command::DeleteLine { line_id }
            | Command::UpdateLineMetadata { line_id, .. }
            | Command::ReverseLine { line_id, .. }
            | Command::MoveLine { line_id, .. }
            | Command::MoveSegment { line_id, .. } => *line_id,
            _ => return None,
        };
        if seen.insert(id) {
            out.push(quoted(names.line(id)?));
        }
    }
    Some(out)
}

/// Shots `cmds` address, and their lines (`UpdateShot` / `SetShotKind`).
fn shots_of(cmds: &[&Command]) -> (usize, BTreeSet<LineId>) {
    let mut shots = BTreeSet::new();
    for cmd in cmds {
        if let Command::UpdateShot {
            line_id, shot_id, ..
        }
        | Command::SetShotKind {
            line_id, shot_id, ..
        } = cmd
        {
            shots.insert((*line_id, *shot_id));
        }
    }
    let lines = shots.iter().map(|(line, _)| *line).collect();
    (shots.len(), lines)
}

/// ` in line "Main"`, ` in 2 lines`.
fn in_lines(lines: &BTreeSet<LineId>, names: &NameBook) -> String {
    match lines.iter().next() {
        Some(&only) if lines.len() == 1 => names
            .line(only)
            .map_or_else(String::new, |n| format!(" in line {}", quoted(n))),
        _ => format!(" in {}", plural(lines.len(), "line", "lines")),
    }
}

/// The label of commands of one kind (`cmds`, at least one).
fn summarize_kind(kind: &str, cmds: &[&Command], names: &NameBook) -> String {
    let n = cmds.len();
    if let [one] = cmds {
        return describe(one, names);
    }
    let names_or_count = |verb: &str, noun: &str| match line_names(cmds, names) {
        Some(list) if list.len() == 1 => format!("{verb} line {}", listed(&list)),
        Some(list) => format!("{verb} lines {}", listed(&list)),
        None => format!("{verb} {n} {noun}"),
    };
    match kind {
        "update_shot" => {
            let (shots, lines) = shots_of(cmds);
            format!(
                "Update {}{}",
                plural(shots, "shot", "shots"),
                in_lines(&lines, names)
            )
        }
        "set_shot_kind" => {
            let (shots, lines) = shots_of(cmds);
            let kinds: BTreeSet<&str> = cmds
                .iter()
                .filter_map(|c| match c {
                    Command::SetShotKind { kind, .. } => Some(kind_word(*kind)),
                    _ => None,
                })
                .collect();
            let shots = plural(shots, "shot", "shots");
            match kinds.iter().next() {
                Some(word) if kinds.len() == 1 => {
                    format!("Mark {shots}{} as {word}", in_lines(&lines, names))
                }
                _ => format!("Change the kind of {shots}{}", in_lines(&lines, names)),
            }
        }
        "delete_line" => names_or_count("Delete", "lines"),
        "reverse_line" => names_or_count("Reverse", "lines"),
        "update_line_metadata" => names_or_count("Update metadata of", "lines"),
        "move_line" => names_or_count("Move", "lines"),
        "move_segment" => format!("Move {n} segments"),
        "move_station" => {
            let stations: Option<Vec<String>> = cmds
                .iter()
                .map(|c| match c {
                    Command::MoveStation { station_ref, .. } => names.station(*station_ref),
                    _ => None,
                })
                .collect();
            match stations {
                Some(list) => format!("Move stations {}", listed(&list)),
                None => format!("Move {n} stations"),
            }
        }
        "set_station_name" => {
            let lines: BTreeSet<LineId> = cmds
                .iter()
                .filter_map(|c| match c {
                    Command::SetStationName { station_ref, .. } => Some(station_ref.line_id),
                    _ => None,
                })
                .collect();
            format!("Rename {n} stations{}", in_lines(&lines, names))
        }
        "merge_stations" => format!("Merge {n} station pairs"),
        "create_loop_closure" => format!("Create {n} loop closures"),
        _ => {
            let first = describe(cmds[0], names);
            if cmds.iter().all(|c| describe(c, names) == first) {
                format!("{first} ({n} times)")
            } else {
                format!(
                    "{first} and {}",
                    plural(n - 1, "more change", "more changes")
                )
            }
        }
    }
}

/// The label of a batch the frontend sent, from its sub-commands and the
/// names the project holds before it runs: one kind of command says what
/// they did together (`Update 5 shots in line "Main"`, `Reverse lines "A"
/// and "B"`); several kinds are named by the one that stands for the rest
/// ([`PRINCIPAL`]), or by the first and a count.
pub(crate) fn summarize(commands: &[Command], names: &NameBook) -> String {
    let all: Vec<&Command> = commands.iter().collect();
    let Some(first) = all.first() else {
        return "Batch of 0 commands".to_string();
    };
    let first_kind = kind(first);
    if all.iter().all(|c| kind(c) == first_kind) {
        return summarize_kind(first_kind, &all, names);
    }
    // Data-table edits of several kinds (a paste across columns).
    let (shots, lines) = shots_of(&all);
    if shots > 0
        && all.iter().all(|c| {
            matches!(
                c,
                Command::UpdateShot { .. }
                    | Command::SetShotKind { .. }
                    | Command::SetStationName { .. }
                    | Command::EditShotCredit { .. }
            )
        })
    {
        return format!(
            "Edit {}{}",
            plural(shots, "shot", "shots"),
            in_lines(&lines, names)
        );
    }
    for principal in PRINCIPAL {
        let of_kind: Vec<&Command> = all
            .iter()
            .copied()
            .filter(|c| kind(c) == *principal)
            .collect();
        if !of_kind.is_empty() {
            return summarize_kind(principal, &of_kind, names);
        }
    }
    format!(
        "{} and {}",
        describe(first, names),
        plural(all.len() - 1, "more change", "more changes")
    )
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::models::{Coordinates, LineData, LineUpdates, ShotData, ShotUpdates};

    /// A project of named lines (ids 0, 1, ...), `shots` shots each.
    fn project(lines: &[&str], shots: usize) -> Project {
        let mut project = Project::new("Labels".to_string(), None);
        for name in lines {
            project.add_line(LineData {
                name: Some((*name).to_string()),
                shots: (0..shots)
                    .map(|_| ShotData {
                        length: 5.0,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            });
        }
        project.ensure_station_identities();
        project
    }

    fn book(project: &Project) -> NameBook {
        NameBook::of(project, project.lines.iter().map(|l| l.id))
    }

    #[test]
    fn lists_up_to_three_names_then_counts() {
        let names = |n: usize| (1..=n).map(|i| i.to_string()).collect::<Vec<_>>();
        assert_eq!(listed(&names(1)), "1");
        assert_eq!(listed(&names(2)), "1 and 2");
        assert_eq!(listed(&names(3)), "1, 2 and 3");
        assert_eq!(listed(&names(5)), "1, 2, 3 and 2 more");
    }

    #[test]
    fn insert_station_names_the_station_it_made() {
        let mut p = project(&["Main Line"], 3);
        let line_id = p.lines[0].id;
        let shot_id = p.lines[0].shots[1].id;
        let (head, _, _) = p.split_shot_at_distance(line_id, shot_id, 2.0).unwrap();
        p.ensure_station_identities();
        let name = p
            .station_name_at(StationRef::new(line_id, 2))
            .unwrap()
            .to_string();
        assert_eq!(
            insert_station(&p, line_id, head),
            format!("Insert station Main Line / {name}")
        );
        assert_eq!(
            insert_station(&p, line_id, 999_999),
            "Insert station in line \"Main Line\""
        );
    }

    #[test]
    fn combine_names_both_lines_and_a_new_name() {
        let p = project(&["Main Line", "Downstream"], 1);
        let (a, b) = (p.lines[0].id, p.lines[1].id);
        assert_eq!(
            combine_lines(&p, a, b, "Main Line"),
            "Combine lines \"Main Line\" and \"Downstream\""
        );
        assert_eq!(
            combine_lines(&p, a, b, " Big Loop "),
            "Combine lines \"Main Line\" and \"Downstream\" into \"Big Loop\""
        );
    }

    #[test]
    fn create_line_at_names_the_line_and_the_station() {
        let p = project(&["Main Line", "Side Passage"], 2);
        let target = StationRef::new(p.lines[0].id, 1);
        let station = p.station_text(target);
        assert_eq!(
            create_line_at(&p, p.lines[1].id, target),
            format!("Create line \"Side Passage\" at {station}")
        );
    }

    #[test]
    fn split_line_names_the_line_and_the_station() {
        assert_eq!(
            split_line("Main Line", "5"),
            "Split line \"Main Line\" at 5"
        );
    }

    #[test]
    fn delete_shots_names_the_span_and_what_became_of_the_line() {
        let p = project(&["Main Line"], 5);
        let id = p.lines[0].id;
        let name = |i: u32| {
            p.station_name_at(StationRef::new(id, i))
                .unwrap()
                .to_string()
        };
        assert_eq!(
            delete_shots(&p, id, 2, 2, ShotsDeleted::Split),
            format!(
                "Delete shot {} \u{2192} {} and split line \"Main Line\"",
                name(2),
                name(3)
            )
        );
        assert_eq!(
            delete_shots(&p, id, 1, 3, ShotsDeleted::Split),
            format!(
                "Delete 3 shots ({} \u{2192} {}) and split line \"Main Line\"",
                name(1),
                name(4)
            )
        );
        assert_eq!(
            delete_shots(&p, id, 0, 1, ShotsDeleted::CloseUp),
            format!(
                "Delete 2 shots ({} \u{2192} {}) from line \"Main Line\"",
                name(0),
                name(2)
            )
        );
        assert_eq!(
            delete_shots(&p, id, 0, 0, ShotsDeleted::LastOfLine),
            "Delete line \"Main Line\" with its last shot"
        );
    }

    #[test]
    fn import_lists_the_lines_it_made() {
        let p = project(&["A", "B", "C", "D", "E"], 1);
        let ids: Vec<LineId> = p.lines.iter().map(|l| l.id).collect();
        assert_eq!(import_lines(&p, &ids[..1]), "Import line \"A\"");
        assert_eq!(import_lines(&p, &ids[..2]), "Import lines \"A\" and \"B\"");
        assert_eq!(
            import_lines(&p, &ids),
            "Import 5 lines (\"A\", \"B\", \"C\" and 2 more)"
        );
    }

    #[test]
    fn summarizes_a_paste_by_its_shots_and_line() {
        let p = project(&["Main", "Side"], 3);
        let names = book(&p);
        let (main, side) = (&p.lines[0], &p.lines[1]);
        let update = |line: &crate::models::Line, i: usize| Command::UpdateShot {
            line_id: line.id,
            shot_id: line.shots[i].id,
            updates: ShotUpdates::default(),
        };
        assert_eq!(
            summarize(&[update(main, 0), update(main, 1), update(main, 1)], &names),
            "Update 2 shots in line \"Main\""
        );
        assert_eq!(
            summarize(&[update(main, 0), update(side, 1)], &names),
            "Update 2 shots in 2 lines"
        );
        let kind = |i: usize, kind| Command::SetShotKind {
            line_id: main.id,
            shot_id: main.shots[i].id,
            kind,
        };
        assert_eq!(
            summarize(&[kind(0, ShotKind::Jump), kind(1, ShotKind::Jump)], &names),
            "Mark 2 shots in line \"Main\" as jump"
        );
        assert_eq!(
            summarize(&[kind(0, ShotKind::Jump), update(main, 2)], &names),
            "Edit 2 shots in line \"Main\""
        );
        let rename = |i: u32| Command::SetStationName {
            station_ref: StationRef::new(main.id, i),
            name: format!("S{i}"),
        };
        assert_eq!(
            summarize(&[rename(1), rename(2)], &names),
            "Rename 2 stations in line \"Main\""
        );
    }

    #[test]
    fn summarizes_line_batches_by_name() {
        let p = project(&["Main", "Side"], 1);
        let names = book(&p);
        let ids: Vec<LineId> = p.lines.iter().map(|l| l.id).collect();
        let reverse: Vec<Command> = ids
            .iter()
            .map(|&line_id| Command::ReverseLine {
                line_id,
                swap_left_right: false,
            })
            .collect();
        assert_eq!(
            summarize(&reverse, &names),
            "Reverse lines \"Main\" and \"Side\""
        );
        // A withdrawal: the deletes stand for the import bookkeeping.
        let withdraw = vec![
            Command::DeleteLine { line_id: ids[0] },
            Command::DeleteLine { line_id: ids[1] },
            Command::UntrackImport {
                file_hashes: vec![],
                line_hashes: vec![],
            },
        ];
        assert_eq!(
            summarize(&withdraw, &names),
            "Delete lines \"Main\" and \"Side\""
        );
        // Edit Line's Save: the metadata edit stands for its credit edit.
        let edit = vec![
            Command::UpdateLineMetadata {
                line_id: ids[0],
                updates: LineUpdates::default(),
            },
            Command::UpdateLineMetadata {
                line_id: ids[0],
                updates: LineUpdates::default(),
            },
        ];
        assert_eq!(summarize(&edit, &names), "Update metadata of line \"Main\"");
    }

    #[test]
    fn a_move_snapped_onto_a_station_is_the_merge() {
        let p = project(&["Main", "Side"], 2);
        let names = book(&p);
        let a = StationRef::new(p.lines[0].id, 1);
        let b = StationRef::new(p.lines[1].id, 0);
        let batch = vec![
            Command::MoveStation {
                station_ref: b,
                offset: Coordinates::default(),
            },
            Command::MergeStations {
                station_a: b,
                station_b: a,
            },
        ];
        assert_eq!(
            summarize(&batch, &names),
            format!(
                "Merge stations {} and {}",
                p.station_text(b),
                p.station_text(a)
            )
        );
        let moves = vec![
            Command::MoveStation {
                station_ref: a,
                offset: Coordinates::default(),
            },
            Command::MoveStation {
                station_ref: b,
                offset: Coordinates::default(),
            },
        ];
        assert_eq!(
            summarize(&moves, &names),
            format!(
                "Move stations {} and {}",
                p.station_text(a),
                p.station_text(b)
            )
        );
    }

    #[test]
    fn unrelated_kinds_name_the_first_and_count_the_rest() {
        let p = project(&["Main"], 2);
        let names = book(&p);
        let a = StationRef::new(p.lines[0].id, 1);
        let batch = vec![
            Command::SetZeroDatum {
                station_ref: a,
                coordinates: crate::models::GpsCoordinates::new(20.0, -87.0, 0.0).unwrap(),
                accuracy_m: None,
                entrance: None,
            },
            Command::RemoveZeroDatum { station_ref: a },
        ];
        assert_eq!(
            summarize(&batch, &names),
            format!("Set zero datum at {} and 1 more change", p.station_text(a))
        );
        assert_eq!(summarize(&[], &names), "Batch of 0 commands");
    }
}
