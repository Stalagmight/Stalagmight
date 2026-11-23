//! The kept-stations rules the two identity upgrades share (plan
//! `docs/plans/2026-11-23_kept-stations.md`, RS 8.2 and 8.3): the 0.6.0
//! migration ([`super::migrate`]) and the load-time upgrade of a version 2
//! file an earlier build of version 2 wrote
//! ([`super::stalx_v2::Decoded::upgrade`]). Both start from a project whose
//! junctions show each line the identity that line knew them by (a record
//! naming the line, else the station's own) and end with the RS rules:
//!
//! 1. [`Before::of`]: what every slot showed, and which identity showed on
//!    which line ("already shown on the line" for rule L).
//! 2. [`keep_longest`]: each junction keeps the identity on its longest line
//!    (rule K over own lines, ties to the lowest Station ID); the others are
//!    records. When a record wins, the node takes its id (re-keyed
//!    everywhere), so a Station ID always names the identity it named.
//! 3. [`settle`]: each closure archives the end rule K does not keep, in list
//!    order, when C1 and C2 allow it, else nothing (never the other end,
//!    U-P2-11); then N8 by rule L, never by chain order; then what lines show
//!    differently ([`IdentityChanges`]).
//!
//! The change lists are survey content: for the conversion notice only,
//! never logged nor persisted. Beside each name they keep the stable ids
//! (line ids, Station IDs) the notice's prose parts draw it with (RS-PROSE).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::convert::Infallible;

use crate::models::prose::{self, ProsePart, ProseStation};
use crate::models::survey::{ClosureEnd, ClosureId, LetteredStation, Project, RecordIndex};
use crate::models::{LineId, StationId};

/// Asked inside the upgrade's long steps (once per line or junction) and
/// between them: `Err` stops the upgrade, for a reader with a deadline
/// (`stalx_v2::DecodeOptions::deadline`, RS-FX9). Every pass here is
/// linear, so a check per line bounds the overrun to one line's work.
pub(crate) type InTime<'a, E> = &'a dyn Fn() -> Result<(), E>;

/// The [`InTime`] of an upgrade with no deadline (the 0.6.0 migration,
/// salvage).
pub(crate) fn no_deadline() -> Result<(), Infallible> {
    Ok(())
}

/// One slot whose line shows another name than the file showed there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameChange {
    /// The slot's line, by the name the converted project gives it.
    pub line: String,
    /// The name the file showed at that slot.
    pub from: String,
    /// The name the slot shows now.
    pub to: String,
    /// `true`: the slot shows the same station, now with a letter suffix
    /// (rule L). `false`: the slot shows another station (a junction's kept
    /// station, or a loop closure's kept end); Detach Line from Station or
    /// Remove Closure brings its own back.
    pub lettered: bool,
    /// For a slot that shows another station: the line that station belongs
    /// to (its home line), by name, so the notice can say "Main / 12".
    pub kept_line: Option<String>,
    /// The slot's line (`line`'s id), for the pill's colour.
    pub line_id: LineId,
    /// `kept_line`'s id.
    pub kept_line_id: Option<LineId>,
    /// The identity (Station ID) whose name the file showed at the slot.
    pub from_identity: StationId,
    /// The identity the slot shows now.
    pub to_identity: StationId,
}

/// One slot whose own comment the file showed there is now hidden: the slot
/// shows another station, whose comment (or none) is shown instead. Detach
/// Line from Station or Remove Closure brings it back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentHidden {
    /// The slot's line, by the name the converted project gives it.
    pub line: String,
    /// `line`'s id, for the pill's colour.
    pub line_id: LineId,
    /// The station's name as the file showed it at that slot.
    pub station: String,
    /// The identity (Station ID) whose comment the slot showed: one station
    /// on several lines hides one comment, which the notice counts and
    /// names once, with its lines (RS-FX6).
    pub identity: StationId,
}

/// A loop closure an upgrade left archiving nothing because the end rule K
/// keeps is refused by C1 or C2 (RS 5.4): both ends keep their own names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClosureKeepsBoth {
    /// End `a` as the user reads it: `Line / name`.
    pub a: String,
    /// End `b`.
    pub b: String,
    /// `a` as prose parts (RS-PROSE): the same words, each station a part
    /// without a slot; empty when no part says them.
    pub a_parts: Vec<ProsePart>,
    /// `b` as prose parts.
    pub b_parts: Vec<ProsePart>,
}

/// What lines show differently after an identity upgrade: the conversion
/// notice's lists, each in line order, then chain order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityChanges {
    /// Every slot whose shown name changed.
    pub names: Vec<NameChange>,
    /// Every slot whose own comment is now hidden (raised even when the
    /// names match, U-P1-2).
    pub comments_hidden: Vec<CommentHidden>,
    /// Every closure left with no archive.
    pub closures_keep_both: Vec<ClosureKeepsBoth>,
    /// The lines `names` and `comments_hidden` name, each once, in line
    /// order: the notice groups its lists by them.
    pub lines: Vec<String>,
}

impl IdentityChanges {
    /// Whether no line shows anything differently.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
            && self.comments_hidden.is_empty()
            && self.closures_keep_both.is_empty()
    }

    /// Slots now showing another station's name.
    pub fn shown_as_other(&self) -> usize {
        self.names.iter().filter(|n| !n.lettered).count()
    }

    /// Slots whose own station now goes by a letter suffix.
    pub fn lettered(&self) -> usize {
        self.names.iter().filter(|n| n.lettered).count()
    }
}

/// Where a junction's comments sat before the upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Comments {
    /// An earlier version 2 file (U1): one comment per station, joined from
    /// its merged members and shown on every line; an alias had none. It
    /// stays with the identity the junction shows (RS 8.2 step 2).
    Shared,
    /// A 0.6.0 merge group: every member its own comment, shown on its own
    /// line, which travels with its identity (RS 8.3: records keep their own
    /// comments; nothing is joined).
    Own,
}

/// What one slot showed before an upgrade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownBefore {
    /// The identity (Station ID) whose name the slot showed: a node's id, or
    /// the id of the record naming the slot's line. Ids travel with their
    /// identities through the upgrade, so this names the same identity
    /// after it.
    pub(crate) identity: StationId,
    pub(crate) name: String,
    pub(crate) comment: Option<String>,
}

/// What a project with per-line junction identities showed, taken before
/// [`keep_longest`] changes it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Before {
    /// Per line (list position), per chain index.
    slots: Vec<Vec<ShownBefore>>,
    /// Every `(identity, line)` pair shown: rule L's "already shown on the
    /// line".
    pairs: BTreeSet<(StationId, LineId)>,
}

impl Before {
    /// What `project` shows per slot: on a line a record names, that record
    /// (its name; its comment, or under [`Comments::Shared`] the station's);
    /// else the node's own identity, or at a node a closure archives its
    /// kept end's (only a file written with kept stations has one). Each
    /// slot's record is one lookup ([`RecordIndex`]), so a hub of many lines
    /// and records costs its slots plus its record lines. `in_time` is asked
    /// per line.
    pub(crate) fn of<E>(
        project: &Project,
        comments: Comments,
        in_time: InTime<'_, E>,
    ) -> Result<Self, E> {
        let topology = project.topology();
        let index = project.shown_index();
        let records = RecordIndex::of(project);
        let mut pairs = BTreeSet::new();
        let mut slots = Vec::with_capacity(project.lines().len());
        for (position, line) in project.lines().iter().enumerate() {
            in_time()?;
            let chain = topology.chain_at(position).unwrap_or(&[]);
            let mut shown_on_line = Vec::with_capacity(chain.len());
            for &node in chain {
                let shown = if let Some(record) = records.record(project, node, line.id) {
                    let comment = match comments {
                        Comments::Own => record.comment.clone(),
                        Comments::Shared => record.comment.clone().or_else(|| {
                            project.stations.get(&node).and_then(|s| s.comment.clone())
                        }),
                    };
                    ShownBefore {
                        identity: record.id,
                        name: record.name.clone(),
                        comment,
                    }
                } else {
                    let identity = index.shown(node);
                    let station = project.stations.get(&identity);
                    ShownBefore {
                        identity,
                        name: station.map(|s| s.name.clone()).unwrap_or_default(),
                        comment: station.and_then(|s| s.comment.clone()),
                    }
                };
                pairs.insert((shown.identity, line.id));
                shown_on_line.push(shown);
            }
            slots.push(shown_on_line);
        }
        Ok(Self { slots, pairs })
    }

    fn at(&self, position: usize, index: usize) -> Option<&ShownBefore> {
        self.slots.get(position)?.get(index)
    }
}

/// Each junction keeps the identity on its longest line: rule K over own
/// lines among the station's own identity (its native lines) and its
/// records (each its own lines; a history record is no candidate). When a
/// record wins, the node takes its identity and id (re-keyed everywhere:
/// slots, datums, closure ends, placements, hand moves), and the old
/// identity becomes a record naming the old native lines, with its external
/// id; the comments follow `comments`. Returns the `(old, new)` ids of the
/// re-keyed junctions, in old-id order.
///
/// Linear in the survey: lines are measured once and every junction is
/// re-keyed in one walk ([`Project::rekey_nodes`]), as each junction's
/// choice reads only its own lines and records, which no other junction's
/// re-key changes.
pub(crate) fn keep_longest<E>(
    project: &mut Project,
    comments: Comments,
    in_time: InTime<'_, E>,
) -> Result<Vec<(StationId, StationId)>, E> {
    // Neither earlier form knew owners (U1 alias rows, 0.6.0 merge
    // members): every record is archived under its node's own identity
    // (RS-FX4), whatever an earlier file's record objects say.
    for station in project.stations.values_mut() {
        station.flatten_records();
    }
    let junctions: Vec<StationId> = project
        .stations
        .iter()
        .filter(|(_, s)| s.archived.iter().any(|r| !r.is_history()))
        .map(|(&id, _)| id)
        .collect();
    let lengths = project.measured_lengths();
    let mut swapped = Vec::new();
    for node in junctions {
        in_time()?;
        let native = project.native_lines(node);
        let Some(station) = project.stations.get(&node) else {
            continue;
        };
        let candidates: Vec<(StationId, Vec<LineId>)> = std::iter::once((node, native.clone()))
            .chain(
                station
                    .archived
                    .iter()
                    .filter(|r| !r.is_history())
                    .map(|r| (r.id, r.lines.clone())),
            )
            .collect();
        let Some(winner) = Project::rule_k_measured(&lengths, candidates) else {
            continue;
        };
        if winner == node {
            continue;
        }
        let Some(station) = project.stations.get_mut(&node) else {
            continue;
        };
        let Some(at) = station.archived.iter().position(|r| r.id == winner) else {
            continue;
        };
        let record = station.archived.remove(at);
        let mut old = station.as_record(native);
        station.name = record.name;
        station.external = record.external;
        match (comments, record.comment) {
            // The junction's one comment was shown on every line: it stays
            // with the identity shown there (U-P2-13c); the record gets none.
            (Comments::Shared, None) => old.comment = None,
            // Each identity keeps its own comment.
            (_, own) => station.comment = own,
        }
        station.archived.push(old);
        swapped.push((node, winner));
    }
    project.rekey_nodes(&swapped);
    Ok(swapped)
}

/// What [`settle`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Settled {
    /// Closures given an archived end.
    pub(crate) closures_archived: usize,
    /// Stations given a letter suffix (rule L), in the order given.
    pub(crate) lettered: Vec<LetteredStation>,
    /// What lines show differently than `before` showed.
    pub(crate) changes: IdentityChanges,
}

/// The closure archives, N8 by rule L, and what lines show differently than
/// `before` showed (steps 3 to 5 of RS 8.2; RS 8.3). `in_time` is asked
/// between the steps and per line of the last.
pub(crate) fn settle<E>(
    project: &mut Project,
    before: &Before,
    in_time: InTime<'_, E>,
) -> Result<Settled, E> {
    let (closures_archived, refused) = archive_closures(project);
    in_time()?;
    let pairs = before.pairs.clone();
    let lettered = project.letter_collisions_with(move || pairs);
    in_time()?;
    let mut homes = HomeLines::default();
    let (names, comments_hidden, lines) = slot_changes(project, before, &mut homes, in_time)?;
    let closures_keep_both = closures_keep_both(project, &refused, &mut homes);
    Ok(Settled {
        closures_archived,
        lettered,
        changes: IdentityChanges {
            names,
            comments_hidden,
            closures_keep_both,
            lines,
        },
    })
}

/// Each identity's home line ([`Project::home_line`]), worked out once: the
/// notice names the kept station's home line on every slot that shows it,
/// and a hub's home line costs its lines and records each time.
#[derive(Default)]
struct HomeLines(HashMap<StationId, Option<LineId>>);

impl HomeLines {
    fn of(&mut self, project: &Project, identity: StationId) -> Option<LineId> {
        *self
            .0
            .entry(identity)
            .or_insert_with(|| project.home_line(identity))
    }
}

/// Rule K's end for every closure that archives nothing yet: of its two
/// ends, the one on the longest line (over every line through it: its
/// identity is shown on all of them) is kept, ties to the lower Station ID;
/// a closure whose ends are one station archives nothing (#738). Then the
/// list-order C1/C2 normalisation, which leaves a closure archiving nothing
/// when its choice is refused, never archiving the other end. A closure that
/// already archives an end keeps it, unless the normalisation drops it.
/// Returns `(closures given an archive, closures left archiving nothing by
/// the normalisation)`, the second in list order.
///
/// The normalisation is the model's shared helper
/// (`Project::normalise_closure_archives`); only the rule K pick (per-line
/// candidates, RS 2) is local.
pub(crate) fn archive_closures(project: &mut Project) -> (usize, Vec<ClosureId>) {
    let topology = project.topology_arc();
    let lengths = project.measured_lengths();
    let picks: Vec<Option<ClosureEnd>> = project
        .closures
        .iter()
        .map(|c| {
            if c.archived.is_some() || c.a == c.b {
                return c.archived;
            }
            let kept = Project::rule_k_measured(
                &lengths,
                [(c.a, topology.lines_of(c.a)), (c.b, topology.lines_of(c.b))],
            )?;
            Some(if kept == c.a {
                ClosureEnd::B
            } else {
                ClosureEnd::A
            })
        })
        .collect();
    let mut given = BTreeSet::new();
    for (closure, pick) in project.closures.iter_mut().zip(picks) {
        if closure.archived.is_none() && pick.is_some() {
            given.insert(closure.id);
        }
        closure.archived = pick;
    }
    let refused = project.normalise_closure_archives();
    let refused_set: BTreeSet<ClosureId> = refused.iter().copied().collect();
    let archived = given.iter().filter(|id| !refused_set.contains(id)).count();
    (archived, refused)
}

/// Whether a comment holds text.
fn has_text(comment: Option<&String>) -> bool {
    comment.is_some_and(|c| !c.is_empty())
}

/// Every slot of `project` whose shown name or own comment differs from
/// what `before` says it showed, in line order, then chain order. A name
/// change keeps the same identity (a letter) or shows another one; a comment
/// is hidden when the slot showed text that it no longer shows.
#[expect(
    clippy::type_complexity,
    reason = "the three notice lists, returned together as they are built together"
)]
fn slot_changes<E>(
    project: &Project,
    before: &Before,
    homes: &mut HomeLines,
    in_time: InTime<'_, E>,
) -> Result<(Vec<NameChange>, Vec<CommentHidden>, Vec<String>), E> {
    let topology = project.topology();
    let index = project.shown_index();
    let mut labels: BTreeMap<LineId, String> = BTreeMap::new();
    let mut label = |line: LineId| {
        labels
            .entry(line)
            .or_insert_with(|| project.line_label_of(line))
            .clone()
    };
    let mut names = Vec::new();
    let mut comments = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut ordered: BTreeSet<String> = BTreeSet::new();
    for (position, line) in project.lines().iter().enumerate() {
        in_time()?;
        let chain = topology.chain_at(position).unwrap_or(&[]);
        for (i, &node) in chain.iter().enumerate() {
            let Some(was) = before.at(position, i) else {
                continue;
            };
            let identity = index.shown(node);
            let Some(now) = project.stations.get(&identity) else {
                continue;
            };
            let name_changed = now.name != was.name;
            let comment_hidden = has_text(was.comment.as_ref()) && now.comment != was.comment;
            if name_changed {
                let lettered = identity == was.identity;
                let kept_line_id = (!lettered).then(|| homes.of(project, identity)).flatten();
                names.push(NameChange {
                    line: label(line.id),
                    from: was.name.clone(),
                    to: now.name.clone(),
                    lettered,
                    kept_line: kept_line_id.map(&mut label),
                    line_id: line.id,
                    kept_line_id,
                    from_identity: was.identity,
                    to_identity: identity,
                });
            }
            if comment_hidden {
                comments.push(CommentHidden {
                    line: label(line.id),
                    line_id: line.id,
                    station: was.name.clone(),
                    identity: was.identity,
                });
            }
            if name_changed || comment_hidden {
                let here = label(line.id);
                if ordered.insert(here.clone()) {
                    order.push(here);
                }
            }
        }
    }
    Ok((names, comments, order))
}

/// The closures `ids` as the notice names them, each end as the user made
/// the closure: by its slot's text, except a loop end of another closure,
/// which is named by its own identity and what it is shown as now (RS-FX6:
/// "Main / M4 (now shown as Side / S4)", not "Side / S4 (loop end on
/// Main)", which reads like a closure across one Side shot).
fn closures_keep_both(
    project: &Project,
    ids: &[ClosureId],
    homes: &mut HomeLines,
) -> Vec<ClosureKeepsBoth> {
    let ids: BTreeSet<ClosureId> = ids.iter().copied().collect();
    project
        .closures
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| {
            let (a, a_parts) = closure_end_text(project, c.a, c.a_via, homes);
            let (b, b_parts) = closure_end_text(project, c.b, c.b_via, homes);
            ClosureKeepsBoth {
                a,
                b,
                a_parts,
                b_parts,
            }
        })
        .collect()
}

/// One closure end for [`closures_keep_both`]: `Main / M4 (now shown as
/// Side / S4)` when a closure archives the end's node, else its slot text;
/// with its prose parts (no slots), empty when they would name nothing.
fn closure_end_text(
    project: &Project,
    node: StationId,
    via: Option<LineId>,
    homes: &mut HomeLines,
) -> (String, Vec<ProsePart>) {
    if !project.is_closure_archived(node) {
        let (text, parts) = prose::split(&project.station_marked(node, via));
        return (text, prose::without_slots(parts));
    }
    let mut identity = |id: StationId, fallback: Option<LineId>| {
        let name = project
            .stations
            .get(&id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let line = homes.of(project, id).or(fallback);
        let context = line.map(|line| project.line_label_of(line));
        prose::mark(&ProsePart::Station(ProseStation {
            text: match &context {
                Some(context) => format!("{context} / {name}"),
                None => name.clone(),
            },
            line_id: line,
            context,
            name,
            home_line: None,
            loop_end: None,
            station_id: Some(id),
            slot: None,
        }))
    };
    let shown = project.shown(node);
    let marked = format!(
        "{} (now shown as {})",
        identity(node, via),
        identity(shown, None)
    );
    prose::split(&marked)
}
