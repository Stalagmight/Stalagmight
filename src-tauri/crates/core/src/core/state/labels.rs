//! History labels, refs and recorded preview rows of an edit, worded at
//! record time with the names the open project holds (plan 8.2, 8.3).
//!
//! A history entry never keeps its `Command` (its payload holds session shot
//! ids and positional slots), so everything the history list shows that the
//! changeset cannot say is worded here, once, when the edit is recorded:
//!
//! - the **label** ([`entry_label`]): 0.6.0's `describe_entry` rules (a shot
//!   as `Line: from -> to` by its stations, a rename by the name the station
//!   had, a frontend batch summarized from its sub-commands, a credit edit
//!   with the number of shots it changes, a Section by its name);
//! - the **refs** ([`command_refs`]): the stable ids the command names;
//! - the **recorded rows** ([`command_preview`]): what a preview names by a
//!   slot (`Line name / station name`), a move's offset, a datum's GPS before
//!   it goes, and a batch's sub-commands (the first [`BATCH_PREVIEW_ROWS`]
//!   and a count). Rows the changeset can say (shot cells, line fields,
//!   lines created and deleted, credits, comments) are read off its digest
//!   when the history is listed (`undo::history_item`).
//!
//! Names are never positional handles: a station reads `Line name / station
//! name` on the slot the command addresses, a line its name; where a name
//! cannot be read (the slot is out of range) the wording falls back to the
//! verb alone, or `-` in a preview row. Because everything is worded before
//! the edit runs, a later rename, reverse, insert or reopen never changes an
//! entry's text.
//!
//! The recipe labels (Insert Station, Split, Combine, the range deletes,
//! Create Line At, Import, Add .stalx) are worded by their recipes with the
//! helpers below, from the project before or after their edit as each one
//! says.
//!
//! Ported from 0.6.0's `undo/history_names.rs`, `history_batch.rs`,
//! `history_credit.rs`, `history_cave_section.rs` and the record half of
//! `history_view.rs` (the C0 base) onto the station graph.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::core::command::lines_label;
use crate::core::undo::history_item::truncate_with_ellipsis;
use crate::core::{Command, CommentHolder};
use crate::models::survey::changeset::{EntryLimits, HistoryRefs, PreviewRow, RecordedPreview};
use crate::models::survey::Project;
use crate::models::{
    CreditTargets, LineId, SectionId, ShotId, ShotKind, StationId, StationRef, SurveyCredit,
};

/// The most sub-commands a batch's preview lists before it counts the rest.
pub(crate) const BATCH_PREVIEW_ROWS: usize = 8;

/// How many names a label lists before it counts the rest.
const NAMES_LISTED: usize = 3;

/// What a preview row shows for a station no name can be read for: never
/// the positional handle.
const UNKNOWN_STATION: &str = "-";

/// The longest comment a batch row's detail shows, in characters.
const COMMENT_DETAIL_CHARS: usize = 50;

/// `s` within what a persisted entry may hold ([`EntryLimits::max_text_chars`]):
/// a longer label or row would make the next open drop the whole history.
fn bounded(s: String) -> String {
    let max = EntryLimits::default().max_text_chars;
    if s.chars().count() <= max {
        s
    } else {
        truncate_with_ellipsis(&s, max.saturating_sub(1))
    }
}

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

/// The names a label reads, off the project as it is now.
pub(crate) struct NameBook<'p> {
    project: &'p Project,
}

impl<'p> NameBook<'p> {
    pub(crate) fn of(project: &'p Project) -> Self {
        Self { project }
    }

    /// A line's name (`unnamed line` when it has none), if the line exists.
    pub(crate) fn line(&self, line_id: LineId) -> Option<String> {
        self.project.line(line_id)?;
        Some(self.project.line_label_of(line_id))
    }

    /// The station's own name at a slot (`4a`).
    pub(crate) fn station_name(&self, slot: StationRef) -> Option<String> {
        let id = self.project.topology().station_at(slot)?;
        Some(self.project.stations.get(&id)?.name.clone())
    }

    /// `Line name / station name` of the station at a slot.
    pub(crate) fn station(&self, slot: StationRef) -> Option<String> {
        let name = self.station_name(slot)?;
        Some(format!("{} / {name}", self.line(slot.line_id)?))
    }

    /// `Line name / station name` of `station` on its `via` line, else its
    /// first slot (a closure end, a datum).
    pub(crate) fn station_via(&self, station: StationId, via: Option<LineId>) -> Option<String> {
        self.station(self.project.closure_end_slot(station, via)?)
    }

    /// `Line name: from -> to` of the shot with session id `shot_id`.
    pub(crate) fn shot(&self, line_id: LineId, shot_id: ShotId) -> Option<String> {
        let index = self
            .project
            .line(line_id)?
            .shots
            .iter()
            .position(|s| s.id == shot_id)?;
        let index = u32::try_from(index).ok()?;
        let from = self.station_name(StationRef::new(line_id, index))?;
        let to = self.station_name(StationRef::new(line_id, index.checked_add(1)?))?;
        Some(format!("{}: {from} \u{2192} {to}", self.line(line_id)?))
    }

    /// The name of Section `id`, or `#id`.
    fn section(&self, id: SectionId) -> String {
        self.project
            .sections
            .iter()
            .find(|s| s.id == id)
            .map_or_else(|| format!("#{id}"), |s| s.name.clone())
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
fn quoted_line(names: &NameBook<'_>, line_id: LineId) -> Option<String> {
    names.line(line_id).map(|n| quoted(&n))
}

fn holder(names: &NameBook<'_>, holder: &CommentHolder) -> String {
    match holder {
        CommentHolder::Station { station_ref } => names
            .station(*station_ref)
            .map_or_else(|| "a station".to_string(), |s| format!("station {s}")),
        CommentHolder::Shot { line_id, .. } => quoted_line(names, *line_id)
            .map_or_else(|| "a shot".to_string(), |n| format!("a shot in line {n}")),
    }
}

/// The label of `cmd` with names, as a batch's preview rows and a recipe's
/// placeholder sub-commands word it (0.6.0's `describe`): `Merge stations
/// Main Line / 0 and Side / 1`, `Delete line "Main Line"`, `Update shot in
/// line "Main Line"`. Commands that address nothing positional keep
/// [`Command::description`]. Worded on `project` as it is now.
pub(crate) fn command_label(project: &Project, cmd: &Command) -> String {
    bounded(describe(cmd, &NameBook::of(project)))
}

fn describe(cmd: &Command, names: &NameBook<'_>) -> String {
    let station = |sr: &StationRef| names.station(*sr);
    let pair = |verb: &str, a: &StationRef, b: &StationRef| match (station(a), station(b)) {
        (Some(a), Some(b)) => format!("{verb} {a} and {b}"),
        _ => verb.to_string(),
    };
    let on_station = |verb: &str, sr: &StationRef| match station(sr) {
        Some(s) => format!("{verb} {s}"),
        None => verb.to_string(),
    };
    let on_line = |verb: &str, line_id: &LineId, tail: &str, bare: &str| {
        quoted_line(names, *line_id)
            .map_or_else(|| bare.to_string(), |n| format!("{verb}{n}{tail}"))
    };
    match cmd {
        Command::DeleteLine { line_id } => on_line("Delete line ", line_id, "", "Delete line"),
        Command::UpdateLineMetadata { line_id, .. } => {
            on_line("Update line ", line_id, " metadata", "Update line metadata")
        }
        Command::ReverseLine { line_id, .. } => {
            on_line("Reverse line ", line_id, "", "Reverse line")
        }
        Command::CreateShot { line_id, .. } => {
            on_line("Create shot in line ", line_id, "", "Create shot")
        }
        Command::UpdateShot { line_id, .. } => {
            on_line("Update shot in line ", line_id, "", "Update shot")
        }
        Command::DeleteShot { line_id, .. } => {
            on_line("Delete shot in line ", line_id, "", "Delete shot")
        }
        Command::SetShotKind { line_id, kind, .. } => match quoted_line(names, *line_id) {
            Some(n) => format!("Mark shot in line {n} as {}", kind_word(*kind)),
            None => format!("Mark shot as {}", kind_word(*kind)),
        },
        Command::EditShotCredit {
            targets: CreditTargets::Line { line_id },
            edit,
        } => match quoted_line(names, *line_id) {
            Some(n) => format!("{} in line {n}", edit.label()),
            None => edit.label().to_string(),
        },
        Command::MergeStations {
            station_a,
            station_b,
        } => pair("Merge stations", station_a, station_b),
        Command::UnmergeStations { station_ref } => on_station("Unmerge station", station_ref),
        Command::UnmergeStationPair {
            station_a,
            station_b,
        } => pair("Unmerge station pair", station_a, station_b),
        Command::CreateLoopClosure {
            station_a,
            station_b,
            ..
        } => pair("Create loop closure between", station_a, station_b),
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
        // The name the station has now is the one it had: 0.6.0's
        // `describe_entry` read it off the inverse command.
        Command::SetStationName { station_ref, name } => match station(station_ref) {
            Some(s) => format!("Rename station {s} to \"{}\"", name.trim()),
            None => format!("Rename station to \"{}\"", name.trim()),
        },
        Command::MoveLine { line_id, .. } => on_line("Move line ", line_id, "", "Move line"),
        Command::MoveSegment { line_id, .. } => {
            on_line("Move segment (line ", line_id, ")", "Move segment")
        }
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

/// The history label of an entry recording `cmd`, worded on `project`
/// before it runs (what `Record::of` stores): [`command_label`], except
/// where the entry knows better, as 0.6.0's `describe_entry` and its
/// entry-level labels said it:
/// - a frontend batch is summarized from its sub-commands ([`batch_summary`];
///   a batch that brings its own `label` keeps it);
/// - a shot edit names the shot by its stations (`Update shot Main: 2 -> 3`);
/// - a credit edit counts the shots it changes (`Edit surveyors (12 shots)`);
/// - a Section command names its Section (`Rename section "A" to "B"`).
pub(crate) fn entry_label(project: &Project, cmd: &Command) -> String {
    let names = NameBook::of(project);
    let label = match cmd {
        Command::Batch {
            label: Some(label), ..
        } => label.clone(),
        Command::Batch {
            commands,
            label: None,
        } => summarize(commands, &names),
        Command::UpdateShot {
            line_id, shot_id, ..
        } => names
            .shot(*line_id, *shot_id)
            .map_or_else(|| describe(cmd, &names), |s| format!("Update shot {s}")),
        Command::SetShotKind {
            line_id,
            shot_id,
            kind,
        } => names.shot(*line_id, *shot_id).map_or_else(
            || describe(cmd, &names),
            |s| format!("Mark shot {s} as {}", kind_word(*kind)),
        ),
        Command::EditShotCredit { targets, edit } => {
            let n = credit_changes(project, targets, edit);
            format!(
                "{} ({n} shot{})",
                edit.label(),
                if n == 1 { "" } else { "s" }
            )
        }
        _ => section_label(cmd, &names).unwrap_or_else(|| describe(cmd, &names)),
    };
    bounded(label)
}

/// How many shots `edit` changes on `targets` (0.6.0 counted the changed
/// shots: a shot whose credit the edit leaves as it is is not one).
fn credit_changes(
    project: &Project,
    targets: &CreditTargets,
    edit: &crate::models::CreditEdit,
) -> usize {
    let empty = SurveyCredit::default();
    let changes = |credit: Option<&SurveyCredit>| {
        let before = credit.unwrap_or(&empty);
        edit.apply(before) != *before
    };
    match targets {
        CreditTargets::Line { line_id } => project.line(*line_id).map_or(0, |line| {
            line.shots
                .iter()
                .filter(|s| changes(s.credit.as_deref()))
                .count()
        }),
        CreditTargets::Shots { shots } => {
            let mut by_line: HashMap<LineId, HashMap<ShotId, Option<&SurveyCredit>>> =
                HashMap::new();
            let mut seen = HashSet::with_capacity(shots.len());
            let mut n = 0usize;
            for &(line_id, shot_id) in shots {
                if !seen.insert((line_id, shot_id)) {
                    continue;
                }
                let credits = by_line.entry(line_id).or_insert_with(|| {
                    project.line(line_id).map_or_else(HashMap::new, |line| {
                        line.shots
                            .iter()
                            .map(|s| (s.id, s.credit.as_deref()))
                            .collect()
                    })
                });
                if credits.get(&shot_id).is_some_and(|c| changes(*c)) {
                    n += 1;
                }
            }
            n
        }
    }
}

/// The label of a Section command (#327), naming the Section as the
/// project holds it before the command and as the command leaves it; `None`
/// for any other command.
fn section_label(cmd: &Command, names: &NameBook<'_>) -> Option<String> {
    let distinct = |ids: &[LineId]| ids.iter().collect::<HashSet<_>>().len();
    Some(match cmd {
        Command::CreateCaveSection { name, line_ids, .. } => {
            let name = name.trim();
            if line_ids.is_empty() {
                format!("Create section \"{name}\"")
            } else {
                format!(
                    "Create section \"{name}\" with {}",
                    lines_label(distinct(line_ids))
                )
            }
        }
        Command::UpdateCaveSection {
            section_id, name, ..
        } => {
            let before = names.section(*section_id);
            let after = name
                .as_deref()
                .map(str::trim)
                .map_or_else(|| before.clone(), str::to_string);
            if before == after {
                format!("Change colour of section \"{after}\"")
            } else {
                format!("Rename section \"{before}\" to \"{after}\"")
            }
        }
        Command::DeleteCaveSection { section_id } => {
            format!("Delete section \"{}\"", names.section(*section_id))
        }
        Command::MoveLinesToCaveSection {
            line_ids,
            section_id,
        } => {
            let lines = lines_label(distinct(line_ids));
            match section_id {
                Some(id) => format!("Move {lines} to section \"{}\"", names.section(*id)),
                None => format!("Move {lines} out of their section"),
            }
        }
        Command::ApplyCaveSectionColor { section_id } => format!(
            "Apply colour of section \"{}\" to its lines",
            names.section(*section_id)
        ),
        _ => return None,
    })
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
fn summary_kind(cmd: &Command) -> &'static str {
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

/// The lines of `cmds`, by name, or `None` when one is not a line command
/// or names no line the project holds.
fn line_names(cmds: &[&Command], names: &NameBook<'_>) -> Option<Vec<String>> {
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
            out.push(quoted(&names.line(id)?));
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
fn in_lines(lines: &BTreeSet<LineId>, names: &NameBook<'_>) -> String {
    match lines.iter().next() {
        Some(&only) if lines.len() == 1 => names
            .line(only)
            .map_or_else(String::new, |n| format!(" in line {}", quoted(&n))),
        _ => format!(" in {}", plural(lines.len(), "line", "lines")),
    }
}

/// The label of commands of one kind (`cmds`, at least one).
fn summarize_kind(kind: &str, cmds: &[&Command], names: &NameBook<'_>) -> String {
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

fn summarize(commands: &[Command], names: &NameBook<'_>) -> String {
    let all: Vec<&Command> = commands.iter().collect();
    let Some(first) = all.first() else {
        return "Batch of 0 commands".to_string();
    };
    let first_kind = summary_kind(first);
    if all.iter().all(|c| summary_kind(c) == first_kind) {
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
            .filter(|c| summary_kind(c) == *principal)
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

/// The label of a batch from its sub-commands and the names `project` holds
/// before it runs: one kind of command says what they did together (`Update
/// 5 shots in line "Main"`, `Reverse lines "A" and "B"`); several kinds are
/// named by the one that stands for the rest (a merge, a closure, a line
/// delete, a metadata edit, a reverse), or by the first and a count
/// (0.6.0's `history_batch::summarize`). A batch with its own `label` is
/// labelled by [`entry_label`], not here.
pub(crate) fn batch_summary(project: &Project, commands: &[Command]) -> String {
    bounded(summarize(commands, &NameBook::of(project)))
}

/// `Insert station Main Line / 4a`: Insert Station (a shot split at a
/// distance), worded AFTER the split; `station` is the station it made.
pub(crate) fn insert_station_label(
    project: &Project,
    line_id: LineId,
    station: StationId,
) -> String {
    let label = if project.line(line_id).is_some() && project.stations.contains_key(&station) {
        format!(
            "Insert station {}",
            project.station_text(station, Some(line_id))
        )
    } else {
        format!(
            "Insert station in line {}",
            line_or_a_line(project, line_id)
        )
    };
    bounded(label)
}

/// `"Main Line"`, or `a line` when the project does not hold it.
fn line_or_a_line(project: &Project, line_id: LineId) -> String {
    match project.line(line_id) {
        Some(_) => quoted(&project.line_label_of(line_id)),
        None => "a line".to_string(),
    }
}

/// `Combine lines "Main Line" and "Downstream"`, with ` into "Name"` when
/// the combined line takes a name neither had. Worded BEFORE the combine.
pub(crate) fn combine_lines_label(
    project: &Project,
    kept: LineId,
    other: LineId,
    combined: &str,
) -> String {
    let combined = combined.trim();
    let label = format!(
        "Combine lines {} and {}",
        line_or_a_line(project, kept),
        line_or_a_line(project, other)
    );
    let taken = [kept, other]
        .iter()
        .any(|&id| project.line(id).is_some() && project.line_label_of(id) == combined);
    bounded(if combined.is_empty() || taken {
        label
    } else {
        format!("{label} into {}", quoted(combined))
    })
}

/// `Create line "Side Passage" at Main Line / Junction`: a new line placed by
/// snapping one of its stations onto `target`, worded AFTER the edit.
pub(crate) fn create_line_at_label(
    project: &Project,
    line_id: LineId,
    target: StationRef,
) -> String {
    let at = NameBook::of(project)
        .station(target)
        .unwrap_or_else(|| UNKNOWN_STATION.to_string());
    bounded(format!(
        "Create line {} at {at}",
        line_or_a_line(project, line_id)
    ))
}

/// `Split line "Main Line" at 5`: Split Line Here. `line_name` is the line's
/// name before the cut; `at` is the name of the station it was cut at.
pub(crate) fn split_line_label(line_name: &str, at: &str) -> String {
    bounded(format!("Split line {} at {at}", quoted(line_name)))
}

/// What a range delete does with the line besides deleting the shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShotsDeleted {
    /// The line is split in two around the gap.
    Split,
    /// The line closes up (shots at an end, or a gap closed: the rest stays
    /// one line).
    CloseUp,
    /// The line's last shot goes, and the line with it.
    LastOfLine,
}

/// `Delete shot 4 -> 5 and split line "Main Line"`, `Delete 3 shots (4 ->
/// 7) from line "Main Line"`, `Delete line "Main Line" with its last shot`:
/// the shots at positions `first..=last` of `line_id`, worded BEFORE the
/// delete. A station is named by its own name (its index when the slot is
/// out of range).
pub(crate) fn delete_shots_label(
    project: &Project,
    line_id: LineId,
    first: usize,
    last: usize,
    then: ShotsDeleted,
) -> String {
    let line = line_or_a_line(project, line_id);
    if then == ShotsDeleted::LastOfLine {
        return bounded(format!("Delete line {line} with its last shot"));
    }
    let names = NameBook::of(project);
    let station = |index: usize| {
        u32::try_from(index)
            .ok()
            .and_then(|i| names.station_name(StationRef::new(line_id, i)))
            .unwrap_or_else(|| index.to_string())
    };
    let count = last.saturating_sub(first) + 1;
    let span = format!("{} \u{2192} {}", station(first), station(last + 1));
    let shots = if count == 1 {
        format!("shot {span}")
    } else {
        format!("{count} shots ({span})")
    };
    bounded(match then {
        ShotsDeleted::Split => format!("Delete {shots} and split line {line}"),
        _ => format!("Delete {shots} from line {line}"),
    })
}

/// `Import line "Main"`, `Import lines "A" and "B"`, `Import 5 lines ("A",
/// "B", "C" and 2 more)`: the lines one import made, worded AFTER it.
pub(crate) fn import_lines_label(project: &Project, line_ids: &[LineId]) -> String {
    let names: Vec<String> = line_ids
        .iter()
        .map(|&id| line_or_a_line(project, id))
        .collect();
    bounded(match names.len() {
        0 => "Import".to_string(),
        1 => format!("Import line {}", listed(&names)),
        n if n <= NAMES_LISTED => format!("Import lines {}", listed(&names)),
        n => format!("Import {n} lines ({})", listed(&names)),
    })
}

/// The recorded preview of Add .stalx (#674): the file and what it brought.
pub(crate) fn add_project_file_preview(
    file_name: &str,
    lines: usize,
    shots: usize,
) -> RecordedPreview {
    RecordedPreview {
        rows: vec![
            row("File", file_name.to_string()),
            row("Lines added", lines.to_string()),
            row("Shots added", shots.to_string()),
        ],
        more: 0,
    }
}

/// Every line `cmd` addresses (batch sub-commands included).
pub(crate) fn referenced_lines(cmd: &Command, out: &mut HashSet<LineId>) {
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
        Command::EditShotCredit {
            targets: CreditTargets::Shots { shots },
            ..
        } => out.extend(shots.iter().map(|(line_id, _)| *line_id)),
        Command::CreateCaveSection { line_ids, .. }
        | Command::MoveLinesToCaveSection { line_ids, .. } => out.extend(line_ids.iter().copied()),
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

/// The slots `cmd` addresses (batch sub-commands included), in order.
fn referenced_slots(cmd: &Command, out: &mut Vec<StationRef>) {
    match cmd {
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
        } => out.extend([*station_a, *station_b]),
        Command::UnmergeStations { station_ref }
        | Command::SetZeroDatum { station_ref, .. }
        | Command::RemoveZeroDatum { station_ref }
        | Command::SetStationComment { station_ref, .. }
        | Command::SetStationName { station_ref, .. }
        | Command::MoveStation { station_ref, .. } => out.push(*station_ref),
        Command::MoveComment { source, target, .. } => {
            for holder in [source, target] {
                if let CommentHolder::Station { station_ref } = holder {
                    out.push(*station_ref);
                }
            }
        }
        Command::Batch { commands, .. } => {
            for sub in commands {
                referenced_slots(sub, out);
            }
        }
        _ => {}
    }
}

/// The shots `cmd` addresses by session id (batch sub-commands included).
fn referenced_shots(cmd: &Command, out: &mut Vec<(LineId, ShotId)>) {
    match cmd {
        Command::UpdateShot {
            line_id, shot_id, ..
        }
        | Command::DeleteShot { line_id, shot_id }
        | Command::SetShotKind {
            line_id, shot_id, ..
        } => out.push((*line_id, *shot_id)),
        Command::EditShotCredit {
            targets: CreditTargets::Shots { shots },
            ..
        } => out.extend(shots.iter().copied()),
        Command::MoveComment { source, target, .. } => {
            for holder in [source, target] {
                if let CommentHolder::Shot { line_id, shot_id } = holder {
                    out.push((*line_id, *shot_id));
                }
            }
        }
        Command::Batch { commands, .. } => {
            for sub in commands {
                referenced_shots(sub, out);
            }
        }
        _ => {}
    }
}

/// The stable ids `cmd` names on `project` (before it runs): its lines, the
/// stations at its slots, the uuids of its shots, the closure and Sections
/// it names (batch sub-commands included). Never session ids or slots. Each
/// list holds at most [`EntryLimits::max_refs`] ids, so an entry stays one
/// the next open reads.
pub(crate) fn command_refs(project: &Project, cmd: &Command) -> HistoryRefs {
    let max = EntryLimits::default().max_refs;
    let mut refs = HistoryRefs::default();
    let mut lines = HashSet::new();
    referenced_lines(cmd, &mut lines);
    let mut lines: Vec<LineId> = lines
        .into_iter()
        .filter(|l| project.line(*l).is_some())
        .collect();
    lines.sort_unstable();
    lines.truncate(max);
    refs.lines = lines;

    let mut slots = Vec::new();
    referenced_slots(cmd, &mut slots);
    let topology = project.topology();
    let mut seen: HashSet<StationId> = HashSet::new();
    for slot in slots {
        if let Some(id) = topology.station_at(slot) {
            if refs.stations.len() < max && seen.insert(id) {
                refs.stations.push(id);
            }
        }
    }

    let mut shots = Vec::new();
    referenced_shots(cmd, &mut shots);
    let mut uuids: HashMap<LineId, HashMap<ShotId, &str>> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    // A shot ref is a 36-character uuid, so the list is bounded far below
    // `max_refs`: as many as a preview lists (the digest's
    // `MAX_LISTED_SHOTS`). Every uuid of a 2,000-cell paste or a 1,000-shot
    // credit edit would cost more than the plan 8.1 size gate allows (C16).
    let max = max.min(crate::models::survey::changeset::MAX_LISTED_SHOTS);
    for (line_id, shot_id) in shots {
        let of_line = uuids.entry(line_id).or_insert_with(|| {
            project.line(line_id).map_or_else(HashMap::new, |line| {
                line.shots.iter().map(|s| (s.id, s.uuid.as_str())).collect()
            })
        });
        if let Some(uuid) = of_line.get(&shot_id).copied() {
            if seen.insert(uuid) && refs.shots.len() < max {
                refs.shots.push(uuid.to_string());
            }
        }
    }

    match cmd {
        Command::RemoveLoopClosure { closure_id } => refs.closures.push(*closure_id),
        Command::UpdateCaveSection { section_id, .. }
        | Command::DeleteCaveSection { section_id }
        | Command::ApplyCaveSectionColor { section_id }
        | Command::MoveLinesToCaveSection {
            section_id: Some(section_id),
            ..
        } => refs.sections.push(*section_id),
        _ => {}
    }
    refs
}

fn row(field: &str, new: String) -> PreviewRow {
    PreviewRow {
        field: field.to_string(),
        old: String::new(),
        new: bounded(new),
    }
}

/// `Line name / station name` at a slot, or `-`.
fn station_or_dash(names: &NameBook<'_>, slot: StationRef) -> String {
    names
        .station(slot)
        .unwrap_or_else(|| UNKNOWN_STATION.to_string())
}

/// The rows of `cmd` worded at record time that its changeset cannot say:
/// what it names by a slot, a move's offset, a datum's GPS, a batch's
/// sub-commands (0.6.0's `derive_preview_data` arms that read the command).
/// Empty for every command whose preview is read off its changeset when the
/// history is listed (`undo::history_item`).
pub(crate) fn command_preview(project: &Project, cmd: &Command) -> RecordedPreview {
    let names = NameBook::of(project);
    let station = |slot: &StationRef| station_or_dash(&names, *slot);
    let rows = match cmd {
        Command::Batch { commands, .. } => return batch_preview(project, commands),
        Command::MergeStations {
            station_a,
            station_b,
        }
        | Command::UnmergeStationPair {
            station_a,
            station_b,
        } => vec![
            row("Station A", station(station_a)),
            row("Station B", station(station_b)),
        ],
        Command::UnmergeStations { station_ref } => vec![row("Station", station(station_ref))],
        Command::CreateLoopClosure {
            station_a,
            station_b,
            closure_strategy,
        } => vec![
            row(
                "Stations",
                format!("{} \u{2194} {}", station(station_a), station(station_b)),
            ),
            row("Strategy", closure_strategy.as_str().to_string()),
        ],
        Command::RemoveLoopClosure { closure_id } => {
            let mut rows = vec![row("Closure ID", closure_id.to_string())];
            if let Some(closure) = project.closures.iter().find(|c| c.id == *closure_id) {
                let end = |s: StationId, via: Option<LineId>| {
                    names
                        .station_via(s, via)
                        .unwrap_or_else(|| UNKNOWN_STATION.to_string())
                };
                rows.push(row(
                    "Stations",
                    format!(
                        "{} \u{2194} {}",
                        end(closure.a, closure.a_via),
                        end(closure.b, closure.b_via)
                    ),
                ));
            }
            rows
        }
        Command::SetZeroDatum { coordinates, .. } => vec![
            row("Latitude", format!("{:.6}", coordinates.latitude)),
            row("Longitude", format!("{:.6}", coordinates.longitude)),
        ],
        Command::RemoveZeroDatum { station_ref } => {
            let mut rows = vec![row("Station", station(station_ref))];
            let gps = project
                .topology()
                .station_at(*station_ref)
                .and_then(|id| project.datum_index_for(id, Some(station_ref.line_id)))
                .and_then(|i| project.datums.get(i))
                .map(|d| d.gps);
            if let Some(gps) = gps {
                for (field, value) in [("Latitude", gps.latitude), ("Longitude", gps.longitude)] {
                    rows.push(PreviewRow {
                        field: field.to_string(),
                        old: format!("{value:.6}"),
                        new: String::new(),
                    });
                }
            }
            rows
        }
        Command::MoveLine { line_id, offset } => {
            let mut rows = Vec::new();
            if let Some(name) = names.line(*line_id) {
                rows.push(row("Line", name));
            }
            rows.push(row("Offset", offset.display_offset()));
            rows
        }
        Command::MoveSegment { line_id, offset } => vec![
            row("Offset", offset.display_offset()),
            row(
                "Affected lines",
                rigid_line_count(project, *line_id).to_string(),
            ),
        ],
        Command::MoveStation { offset, .. } => vec![row("Offset", offset.display_offset())],
        _ => Vec::new(),
    };
    RecordedPreview { rows, more: 0 }
}

/// How many lines move with `line` under Move Segment: its rigid group
/// (shots and aligned closures), as the move computes it.
fn rigid_line_count(project: &Project, line: LineId) -> usize {
    let groups = project.rigid_groups(&project.closure_alignment(&project.raw_positions()));
    project.rigid_lines_of(line, &groups).len()
}

/// A compact detail of a batch sub-command, from the command alone (0.6.0's
/// `derive_command_details`); `None` where it says nothing useful.
fn command_details(cmd: &Command, names: &NameBook<'_>) -> Option<String> {
    let station = |slot: &StationRef| station_or_dash(names, *slot);
    match cmd {
        Command::MoveLine { line_id, offset } => {
            let mut parts = Vec::new();
            if let Some(name) = names.line(*line_id) {
                parts.push(name);
            }
            parts.push(offset.display_offset());
            Some(parts.join(", "))
        }
        Command::MoveStation { offset, .. } | Command::MoveSegment { offset, .. } => {
            Some(offset.display_offset())
        }
        Command::MergeStations {
            station_a,
            station_b,
        } => Some(format!(
            "{} \u{2194} {}",
            station(station_a),
            station(station_b)
        )),
        Command::CreateLoopClosure {
            station_a,
            station_b,
            closure_strategy,
        } => Some(format!(
            "{} \u{2194} {}, {}",
            station(station_a),
            station(station_b),
            closure_strategy.as_str()
        )),
        Command::SetShotKind { kind, .. } => Some(format!("Kind: {}", kind.label())),
        Command::CreateShot { shot, .. } => Some(format!(
            "Length: {:.2}, Azimuth: {:.1}",
            shot.length, shot.azimuth_in
        )),
        Command::CreateLine { name, .. } => name.as_ref().map(|n| format!("Name: {n}")),
        Command::UpdateShot { updates, .. } => {
            let mut parts = Vec::new();
            if let Some(l) = updates.length {
                parts.push(format!("Length: {l:.2}"));
            }
            if let Some(a) = updates.azimuth_in {
                parts.push(format!("Azimuth: {a:.1}"));
            }
            if let Some(d) = updates.depth_in {
                parts.push(format!("Depth: {d:.2}"));
            }
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        Command::SetZeroDatum { coordinates, .. } => Some(format!(
            "Lat: {:.6}, Lon: {:.6}",
            coordinates.latitude, coordinates.longitude
        )),
        Command::SetStationComment { comment, .. } => Some(comment.as_ref().map_or_else(
            || "(removed)".to_string(),
            |c| {
                format!(
                    "Comment: {}",
                    truncate_with_ellipsis(c, COMMENT_DETAIL_CHARS)
                )
            },
        )),
        _ => None,
    }
}

/// The recorded preview of a batch: its first [`BATCH_PREVIEW_ROWS`]
/// sub-commands, each worded by [`command_label`] with its detail, and a
/// count of the rest (0.6.0's batch preview). Worded on `project` before the
/// batch runs, so a line it deletes is named by the name it had. A recipe
/// that records placeholder sub-commands (an import, the DMP wizard) uses
/// it with its own `project` state.
pub(crate) fn batch_preview(project: &Project, commands: &[Command]) -> RecordedPreview {
    let names = NameBook::of(project);
    let rows = commands
        .iter()
        .take(BATCH_PREVIEW_ROWS)
        .map(|cmd| PreviewRow {
            field: bounded(describe(cmd, &names)),
            old: String::new(),
            new: bounded(command_details(cmd, &names).unwrap_or_default()),
        })
        .collect();
    RecordedPreview {
        rows,
        more: commands.len().saturating_sub(BATCH_PREVIEW_ROWS) as u64,
    }
}
