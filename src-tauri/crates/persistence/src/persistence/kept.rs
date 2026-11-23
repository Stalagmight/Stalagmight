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
//! never logged.

use std::collections::{BTreeMap, BTreeSet};

use crate::models::survey::{ClosureEnd, ClosureId, LetteredStation, Project};
use crate::models::{LineId, StationId};

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
}

/// One slot whose own comment the file showed there is now hidden: the slot
/// shows another station, whose comment (or none) is shown instead. Detach
/// Line from Station or Remove Closure brings it back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentHidden {
    /// The slot's line, by the name the converted project gives it.
    pub line: String,
    /// The station's name as the file showed it at that slot.
    pub station: String,
}

/// A loop closure an upgrade left archiving nothing because the end rule K
/// keeps is refused by C1 or C2 (RS 5.4): both ends keep their own names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClosureKeepsBoth {
    /// End `a` as the user reads it: `Line / name`.
    pub a: String,
    /// End `b`.
    pub b: String,
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
    /// kept end's (only a file written with kept stations has one).
    pub(crate) fn of(project: &Project, comments: Comments) -> Self {
        let topology = project.topology();
        let index = project.shown_index();
        let mut pairs = BTreeSet::new();
        let slots = project
            .lines()
            .iter()
            .enumerate()
            .map(|(position, line)| {
                topology
                    .chain_at(position)
                    .unwrap_or(&[])
                    .iter()
                    .map(|&node| {
                        let shown = if let Some(record) = project.record_naming(node, line.id) {
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
                        shown
                    })
                    .collect()
            })
            .collect();
        Self { slots, pairs }
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
pub(crate) fn keep_longest(
    project: &mut Project,
    comments: Comments,
) -> Vec<(StationId, StationId)> {
    let junctions: Vec<StationId> = project
        .stations
        .iter()
        .filter(|(_, s)| s.archived.iter().any(|r| !r.is_history()))
        .map(|(&id, _)| id)
        .collect();
    let mut swapped = Vec::new();
    for node in junctions {
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
        let Some(winner) = project.rule_k(candidates) else {
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
        project.rekey_node(node, winner);
        swapped.push((node, winner));
    }
    swapped
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
/// `before` showed (steps 3 to 5 of RS 8.2; RS 8.3).
pub(crate) fn settle(project: &mut Project, before: &Before) -> Settled {
    let (closures_archived, refused) = archive_closures(project);
    let pairs = before.pairs.clone();
    let lettered = project.letter_collisions_with(move || pairs);
    let (names, comments_hidden, lines) = slot_changes(project, before);
    let closures_keep_both = closures_keep_both(project, &refused);
    Settled {
        closures_archived,
        lettered,
        changes: IdentityChanges {
            names,
            comments_hidden,
            closures_keep_both,
            lines,
        },
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
    let picks: Vec<Option<ClosureEnd>> = project
        .closures
        .iter()
        .map(|c| {
            if c.archived.is_some() || c.a == c.b {
                return c.archived;
            }
            let kept =
                project.rule_k([(c.a, topology.lines_of(c.a)), (c.b, topology.lines_of(c.b))])?;
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
    let archived = given.iter().filter(|id| !refused.contains(id)).count();
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
fn slot_changes(
    project: &Project,
    before: &Before,
) -> (Vec<NameChange>, Vec<CommentHidden>, Vec<String>) {
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
    for (position, line) in project.lines().iter().enumerate() {
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
                let kept_line = (!lettered)
                    .then(|| project.home_line(identity))
                    .flatten()
                    .map(&mut label);
                names.push(NameChange {
                    line: label(line.id),
                    from: was.name.clone(),
                    to: now.name.clone(),
                    lettered,
                    kept_line,
                });
            }
            if comment_hidden {
                comments.push(CommentHidden {
                    line: label(line.id),
                    station: was.name.clone(),
                });
            }
            if name_changed || comment_hidden {
                let here = label(line.id);
                if !order.contains(&here) {
                    order.push(here);
                }
            }
        }
    }
    (names, comments, order)
}

/// The closures `ids` as the notice names them, ends by their shown names.
fn closures_keep_both(project: &Project, ids: &[ClosureId]) -> Vec<ClosureKeepsBoth> {
    project
        .closures
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| ClosureKeepsBoth {
            a: project.station_text(c.a, c.a_via),
            b: project.station_text(c.b, c.b_via),
        })
        .collect()
}
