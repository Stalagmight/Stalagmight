//! Kept and archived stations (plan 2026-11-23, "RS"): the identity each
//! node shows, the records a node keeps, its native lines, the keep rule
//! (rule K) and the epilogue steps that hold the archive invariants.
//!
//! - A **node** is a [`super::Station`] record: geometry, shots, datums,
//!   placements, hand moves and closure ends refer to node ids.
//! - A node's **records** ([`ArchivedStation`]) are the identities merges
//!   put away in it, each with the lines whose own station it was. A line
//!   no record of the node names is a **native** line of the node.
//! - The **shown identity** of a node is its own, unless a loop closure
//!   archives the node: then it is the shown identity of that closure's kept
//!   end ([`Project::shown`], RS 5.1). Every resolution is bounded, so a
//!   crafted file whose archives cycle reports a violation instead of
//!   hanging.
//! - **Rule K** keeps, of several candidates, the one on the longest line
//!   ([`super::Line::measured_length`]), ties to the lowest Station ID; a
//!   candidate with no line loses ([`Project::rule_k`]).
//!
//! The epilogue of every command runs [`Project::restore_archived_identities`]
//! (A4 on every node, with the comment carry-over of RS 4.3) and
//! [`Project::normalise_closure_archives`] (C1 and C2, RS 5.4).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::{LineId, StationId, StationRef};

use super::{ArchivedStation, ClosureId, LetteredStation, LoopClosure, Project, Station};

/// Which node's identity each closure-archived node shows (RS 5.1), built
/// once from the closure list: the first closure in list order archiving a
/// node counts (invariant C1 allows only one).
#[derive(Debug, Clone, Default)]
pub struct ShownIndex {
    kept_of: BTreeMap<StationId, StationId>,
}

impl ShownIndex {
    /// The index of `closures`.
    pub fn of(closures: &[LoopClosure]) -> Self {
        let mut kept_of = BTreeMap::new();
        for closure in closures {
            if let Some((archived, kept)) = closure.archive() {
                kept_of.entry(archived).or_insert(kept);
            }
        }
        Self { kept_of }
    }

    /// Whether no closure archives anything.
    pub fn is_empty(&self) -> bool {
        self.kept_of.is_empty()
    }

    /// Whether a closure archives `node`.
    pub fn is_archived(&self, node: StationId) -> bool {
        self.kept_of.contains_key(&node)
    }

    /// The kept end of the closure archiving `node`.
    pub fn kept_end(&self, node: StationId) -> Option<StationId> {
        self.kept_of.get(&node).copied()
    }

    /// The node whose identity `node` shows, following kept ends; `None`
    /// when the chain cycles (invariant C2 broken).
    pub fn resolve(&self, node: StationId) -> Option<StationId> {
        let mut current = node;
        for _ in 0..=self.kept_of.len() {
            match self.kept_of.get(&current) {
                None => return Some(current),
                Some(&next) => current = next,
            }
        }
        None
    }

    /// [`Self::resolve`], or `node` itself on a cycle (total, for display).
    pub fn shown(&self, node: StationId) -> StationId {
        self.resolve(node).unwrap_or(node)
    }

    /// Every closure-archived node, ascending.
    pub fn archived_nodes(&self) -> impl Iterator<Item = StationId> + '_ {
        self.kept_of.keys().copied()
    }
}

/// A node an epilogue gave back an archived identity (RS 4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredIdentity {
    /// The node's id before (now a history record's id).
    pub old: StationId,
    /// Its id now: the restored record's.
    pub new: StationId,
    /// The old identity's comment moved with the node (the record had none).
    pub comment_moved: bool,
    /// The old identity's comment, now hidden in a history record (the
    /// record had one of its own): undo brings it back.
    pub comment_hidden: Option<String>,
}

/// A comment an op hid (RS 12, U-P1-3): an identity showed it on some slot
/// before the op, and afterwards it is kept in an identity no slot shows (a
/// record, or a node a loop closure archives). Undo brings it back; for a
/// record, so does Detach Line from Station.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenComment {
    /// The identity that showed the comment before the op: its Station ID,
    /// its name then and its home line then (the pill's colour).
    pub station: StationId,
    pub name: String,
    pub home_line: Option<LineId>,
    /// The comment.
    pub comment: String,
    /// The identity holding it now: `station`, or the record a restore of
    /// the op moved it to before a promotion archived that record.
    pub held_by: StationId,
}

/// What an epilogue did to identities (RS 3), for the op's notice and the
/// previews.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdentityReport {
    /// Nodes left with no native line that took an archived identity back
    /// (RS 4.3), in old-id order.
    pub restored: Vec<RestoredIdentity>,
    /// Closures whose archive was dropped (RS 5.4), in the order dropped: a
    /// merge's or promotion's for the station it keeps (so the kept station
    /// is shown, RS 1.3), the sweep's normalisation, then the epilogue's.
    pub unarchived: Vec<ClosureId>,
    /// Rule-L letters (RS 2), in the order given: the orphan-closure
    /// sweep's (also in `SweepReport::lettered`), then the epilogue's.
    pub lettered: Vec<LetteredStation>,
    /// Every comment the op hid ([`Project::hidden_comments`]), but the one
    /// its own dialog names (a merge's archived station, a closure's
    /// archived end) and those a restore names
    /// ([`RestoredIdentity::comment_hidden`]): what a promotion, a restore
    /// whose node a promotion then archived, or a closure archive the op
    /// moved made disappear. By Station ID.
    pub hidden: Vec<HiddenComment>,
}

impl IdentityReport {
    /// Whether the epilogue changed no identity.
    pub fn is_empty(&self) -> bool {
        self.restored.is_empty()
            && self.unarchived.is_empty()
            && self.lettered.is_empty()
            && self.hidden.is_empty()
    }

    /// Fill [`Self::hidden`] from the comments `after` hides that `before`
    /// showed, leaving out those `restored` already names.
    pub(super) fn with_hidden(mut self, before: &Project, after: &Project) -> Self {
        let mut hidden = after.hidden_comments(before);
        hidden.retain(|h| {
            !self
                .restored
                .iter()
                .any(|r| r.old == h.station && r.comment_hidden.is_some())
        });
        self.hidden = hidden;
        self
    }

    /// Leave out the comment of `identity` (the one the op's own dialog
    /// names as hidden).
    pub(super) fn without_hidden(mut self, identity: Option<StationId>) -> Self {
        if let Some(id) = identity {
            self.hidden.retain(|h| h.station != id);
        }
        self
    }
}

/// Whose station a slot shows (RS 5.4, U-P2-5): what a rename or a comment
/// edit through the slot addresses, and the two ways back the prompt
/// offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotOwner {
    /// The slot's node.
    pub node: StationId,
    /// The identity the slot shows: a rename or comment edit through it
    /// addresses this one, on every line it is shown on.
    pub shown: StationId,
    /// The shown identity's home line (RS 2): "12 belongs to Main Line".
    pub home_line: Option<LineId>,
    /// The slot is foreign (RS 2): its line is not a native line of the
    /// node holding the shown identity.
    pub foreign: bool,
    /// The closure archiving the node, when the slot is a loop end: "shown
    /// as Main / 20 through a loop closure; Remove the closure".
    pub loop_closure: Option<ClosureId>,
    /// The line's own station there, archived by a merge (the record naming
    /// the slot's line at the node): "Detach Side to get back its own
    /// station 0".
    pub own_station: Option<StationId>,
}

/// Whether a comment holds text.
pub(super) fn has_text(comment: Option<&String>) -> bool {
    comment.is_some_and(|c| !c.is_empty())
}

impl Project {
    // ----- shown identity ---------------------------------------------------

    /// The closure archives of this project ([`ShownIndex`]).
    pub fn shown_index(&self) -> ShownIndex {
        ShownIndex::of(&self.closures)
    }

    /// The node whose identity `node` shows (RS 5.1): itself, unless a
    /// closure archives it. Total: on a cycle (a broken file) the node
    /// itself.
    pub fn shown(&self, node: StationId) -> StationId {
        if self.closures.iter().all(|c| c.archived.is_none()) {
            return node;
        }
        self.shown_index().shown(node)
    }

    /// The station record whose identity `node` shows.
    pub fn shown_station(&self, node: StationId) -> Option<&Station> {
        self.stations.get(&self.shown(node))
    }

    /// The closure archiving `node` (the first in list order), if any.
    pub fn archiving_closure(&self, node: StationId) -> Option<&LoopClosure> {
        self.closures
            .iter()
            .find(|c| c.archive().is_some_and(|(archived, _)| archived == node))
    }

    /// Whether a closure archives `node` (it is a loop end shown as another
    /// station).
    pub fn is_closure_archived(&self, node: StationId) -> bool {
        self.archiving_closure(node).is_some()
    }

    /// Every node that shows `identity` (the node holding it, and the
    /// closure-archived nodes resolving to it), ascending.
    pub fn nodes_showing(&self, identity: StationId) -> Vec<StationId> {
        let index = self.shown_index();
        let mut nodes = vec![identity];
        nodes.extend(
            index
                .archived_nodes()
                .filter(|&n| n != identity && index.shown(n) == identity),
        );
        nodes.sort_unstable();
        nodes
    }

    /// Every line `identity` is shown on (RS 2 "shown on"): the lines of
    /// every node showing it, in the model's line order.
    pub fn lines_shown_on(&self, identity: StationId) -> Vec<LineId> {
        let topology = self.topology();
        let mut lines: Vec<(usize, LineId)> = Vec::new();
        for node in self.nodes_showing(identity) {
            for line in topology.lines_of(node) {
                if let Some(position) = topology.line_position(line) {
                    lines.push((position, line));
                }
            }
        }
        lines.sort_unstable();
        lines.dedup();
        lines.into_iter().map(|(_, line)| line).collect()
    }

    // ----- records and native lines ----------------------------------------

    /// The record of `node` naming `line` (A3: at most one).
    pub fn record_naming(&self, node: StationId, line: LineId) -> Option<&ArchivedStation> {
        self.stations.get(&node)?.record_for(line)
    }

    /// Whether `line` passes `node` and no record of the node names it.
    pub fn is_native(&self, node: StationId, line: LineId) -> bool {
        self.topology().slot_on(node, line).is_some() && self.record_naming(node, line).is_none()
    }

    /// The native lines of `node`, in the model's line order.
    pub fn native_lines(&self, node: StationId) -> Vec<LineId> {
        let station = self.stations.get(&node);
        self.topology()
            .lines_of(node)
            .into_iter()
            .filter(|&line| station.is_none_or(|s| s.record_for(line).is_none()))
            .collect()
    }

    /// The home line of the identity `identity` holds (RS 2): its first
    /// native line in the model's line order. Only picks a dot's colour.
    pub fn home_line(&self, identity: StationId) -> Option<LineId> {
        self.native_lines(identity).into_iter().next()
    }

    /// Whether a slot is foreign (RS 2): its line is not a native line of
    /// the node holding the identity the slot shows (a record names the
    /// line, or the slot's node is closure-archived away from that line).
    pub fn is_foreign_slot(&self, slot: StationRef) -> bool {
        match self.station_at(slot) {
            Some(node) => !self.is_native(self.shown(node), slot.line_id),
            None => false,
        }
    }

    /// Whose station `slot` shows ([`SlotOwner`], RS 5.4); `None` for a slot
    /// that does not exist.
    pub fn slot_owner(&self, slot: StationRef) -> Option<SlotOwner> {
        let node = self.station_at(slot)?;
        let shown = self.shown(node);
        Some(SlotOwner {
            node,
            shown,
            home_line: self.home_line(shown),
            foreign: !self.is_native(shown, slot.line_id),
            loop_closure: self.archiving_closure(node).map(|c| c.id),
            own_station: self.record_naming(node, slot.line_id).map(|r| r.id),
        })
    }

    /// The comment each shown identity shows: every identity some slot
    /// shows (RS 5.1) whose comment has text, by Station ID.
    pub fn shown_comments(&self) -> BTreeMap<StationId, &str> {
        let index = self.shown_index();
        let mut out = BTreeMap::new();
        for &node in self.topology().stations() {
            let shown = index.shown(node);
            if let Some(text) = self
                .stations
                .get(&shown)
                .and_then(|s| s.comment.as_deref())
                .filter(|c| !c.is_empty())
            {
                out.insert(shown, text);
            }
        }
        out
    }

    /// The comment the identity `id` holds, a node's or a record's.
    fn stored_comment(&self, id: StationId) -> Option<&str> {
        match self.stations.get(&id) {
            Some(s) => s.comment.as_deref(),
            None => self
                .stations
                .values()
                .flat_map(|s| &s.archived)
                .find(|r| r.id == id)
                .and_then(|r| r.comment.as_deref()),
        }
    }

    /// The comments this state (after an op) hides that `before` showed
    /// (RS 12): each identity that showed a comment on some slot in
    /// `before` and is shown with it on none now, while the comment is kept
    /// in an identity no slot shows: the identity itself (a record now, or
    /// a node a loop closure archives), or the identity a restore moved the
    /// comment to (RS 4.3) when that one is shown nowhere either. A comment
    /// that is gone (its station deleted, or the text replaced) or that
    /// another identity shows now and did not before (a restore moved it,
    /// or Combine's comment pick took or joined it) is not hidden. By
    /// Station ID.
    pub fn hidden_comments(&self, before: &Project) -> Vec<HiddenComment> {
        let shown_before = before.shown_comments();
        if shown_before.is_empty() {
            return Vec::new();
        }
        let shown_after = self.shown_comments();
        let mut out = Vec::new();
        for (&id, &text) in &shown_before {
            if shown_after.get(&id) == Some(&text) {
                continue;
            }
            let shown_elsewhere = shown_after.iter().any(|(&other, &now)| {
                other != id
                    && now.contains(text)
                    && shown_before
                        .get(&other)
                        .is_none_or(|was| !was.contains(text))
            });
            if shown_elsewhere {
                continue;
            }
            let held_by = if self.stored_comment(id) == Some(text) {
                Some(id)
            } else {
                // Moved by a restore: the identity that holds it now and did
                // not before.
                self.stations
                    .values()
                    .flat_map(|s| {
                        std::iter::once((s.id, s.comment.as_deref()))
                            .chain(s.archived.iter().map(|r| (r.id, r.comment.as_deref())))
                    })
                    .find(|&(other, comment)| {
                        other != id
                            && comment == Some(text)
                            && before.stored_comment(other) != Some(text)
                    })
                    .map(|(other, _)| other)
            };
            let Some(held_by) = held_by else { continue };
            if shown_after.get(&held_by) == Some(&text) {
                continue;
            }
            let Some(station) = before.stations.get(&id) else {
                continue;
            };
            out.push(HiddenComment {
                station: id,
                name: station.name.clone(),
                home_line: before.home_line(id),
                comment: text.to_string(),
                held_by,
            });
        }
        out
    }

    /// Whether `id` is the id of some station's record.
    pub fn is_archived_id(&self, id: StationId) -> bool {
        self.stations
            .values()
            .any(|s| s.archived.iter().any(|r| r.id == id))
    }

    // ----- rule K -----------------------------------------------------------

    /// The longest measured length among `lines`, `None` when there are
    /// none.
    fn longest_of(&self, lines: &[LineId]) -> Option<f64> {
        lines
            .iter()
            .filter_map(|&l| self.line(l).map(super::Line::measured_length))
            .max_by(f64::total_cmp)
    }

    /// Rule K (RS 2) over per-line candidates `(id, own lines)`: the one with
    /// the longest line, ties to the lowest id; a candidate with no line
    /// loses. `None` when there is no candidate.
    pub fn rule_k(
        &self,
        candidates: impl IntoIterator<Item = (StationId, Vec<LineId>)>,
    ) -> Option<StationId> {
        let mut best: Option<(Option<f64>, StationId)> = None;
        for (id, lines) in candidates {
            let length = self.longest_of(&lines);
            let better = match &best {
                None => true,
                Some((best_length, best_id)) => match (length, best_length) {
                    (Some(_), None) => true,
                    (None, Some(_)) => false,
                    (None, None) => id < *best_id,
                    (Some(x), Some(y)) => match x.total_cmp(y) {
                        Ordering::Greater => true,
                        Ordering::Less => false,
                        Ordering::Equal => id < *best_id,
                    },
                },
            };
            if better {
                best = Some((length, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Rule K over node candidates (the merge and closure dialogs): each is
    /// measured over every line its shown identity is shown on (M-P2-5).
    pub fn rule_k_nodes(&self, candidates: &[StationId]) -> Option<StationId> {
        self.rule_k(
            candidates
                .iter()
                .map(|&node| (node, self.lines_shown_on(self.shown(node)))),
        )
    }

    // ----- re-identification -------------------------------------------------

    /// Give node `old` the id `new` everywhere: its key in `stations` and its
    /// own `id`, line starts, shot ends, hand-move keys, datums, closure ends
    /// and placements. `new` must be free (not a live id). Identity fields
    /// are the caller's. Public for the load-time upgrade of an earlier
    /// version 2 file (RS 8.2), which swaps a node's id with a record's.
    pub fn rekey_node(&mut self, old: StationId, new: StationId) {
        if old == new {
            return;
        }
        if let Some(mut station) = self.stations.remove(&old) {
            station.id = new;
            self.stations.insert(new, station);
        }
        let lines = self.topology().lines_of(old);
        for id in lines {
            if let Some(line) = self.line_mut(id) {
                if line.start == old {
                    line.start = new;
                }
                for shot in &mut line.shots {
                    if shot.to == old {
                        shot.to = new;
                    }
                }
                if let Some(hand) = line.hand_moves.remove(&old) {
                    line.hand_moves.insert(new, hand);
                }
            }
        }
        for datum in &mut self.datums {
            if datum.station == old {
                datum.station = new;
            }
        }
        for closure in &mut self.closures {
            if closure.a == old {
                closure.a = new;
            }
            if closure.b == old {
                closure.b = new;
            }
        }
        for placement in &mut self.placements {
            if placement.station == old {
                placement.station = new;
            }
        }
    }

    /// RS 4.3 (invariant A4 on every node): a node that has a slot but no
    /// native line takes as its identity the record naming its lines (rule K
    /// over the records' own lines when several do). The old identity is not
    /// dropped: when the record has no comment the old comment moves with
    /// the node (and the old external id when the record has none), and the
    /// old identity becomes a history record holding whatever did not move.
    /// The node's slots, hand moves, datums, closure ends and placement
    /// follow the new id. Run after stale record lines are cleared (so a
    /// line that left is no reason). Returns what it restored, in old-id
    /// order; `(old, new)` pairs feed the anchor algorithm
    /// ([`super::EditContext::reidentified`]). Public for salvage, which
    /// repairs a file's A4 violations with it.
    pub fn restore_archived_identities(&mut self) -> Vec<RestoredIdentity> {
        let topology = self.topology_arc();
        let lacking: Vec<StationId> = self
            .stations
            .iter()
            .filter(|(&id, s)| {
                !s.archived.is_empty() && {
                    let lines = topology.lines_of(id);
                    !lines.is_empty() && lines.iter().all(|&l| s.record_for(l).is_some())
                }
            })
            .map(|(&id, _)| id)
            .collect();
        let mut out = Vec::with_capacity(lacking.len());
        for node in lacking {
            let Some(station) = self.stations.get(&node) else {
                continue;
            };
            let candidates: Vec<(StationId, Vec<LineId>)> = station
                .archived
                .iter()
                .filter(|r| !r.lines.is_empty())
                .map(|r| (r.id, r.lines.clone()))
                .collect();
            let Some(chosen) = self.rule_k(candidates) else {
                continue;
            };
            let Some(station) = self.stations.get_mut(&node) else {
                continue;
            };
            let Some(index) = station.archived.iter().position(|r| r.id == chosen) else {
                continue;
            };
            let record = station.archived.remove(index);
            let mut old = station.as_record(Vec::new());
            let comment_moved =
                !has_text(record.comment.as_ref()) && has_text(old.comment.as_ref());
            let comment = if comment_moved {
                old.comment.take()
            } else {
                record.comment.clone()
            };
            let external = match record.external.clone() {
                Some(external) => Some(external),
                None => old.external.take(),
            };
            let comment_hidden = old.comment.clone().filter(|c| !c.is_empty());
            station.name.clone_from(&record.name);
            station.comment = comment;
            station.external = external;
            station.archived.push(old);
            self.rekey_node(node, record.id);
            out.push(RestoredIdentity {
                old: node,
                new: record.id,
                comment_moved,
                comment_hidden,
            });
        }
        out
    }

    /// RS 5.4: walk the closures in list order; a closure keeps its archive
    /// only when its archived end is not archived by an earlier kept archive
    /// (C1) and following kept archives from its kept end does not reach its
    /// archived end (C2); otherwise it archives nothing. A closure whose two
    /// ends are one station is left for the sweep (#738). Returns the ids of
    /// the closures whose archive was dropped, in list order.
    pub fn normalise_closure_archives(&mut self) -> Vec<ClosureId> {
        if self.closures.iter().all(|c| c.archived.is_none()) {
            return Vec::new();
        }
        let mut kept_of: BTreeMap<StationId, StationId> = BTreeMap::new();
        let mut dropped = Vec::new();
        for closure in &mut self.closures {
            let Some((archived, kept)) = closure.archive() else {
                continue;
            };
            if kept_of.contains_key(&archived) || reaches(&kept_of, kept, archived) {
                closure.archived = None;
                dropped.push(closure.id);
            } else {
                kept_of.insert(archived, kept);
            }
        }
        dropped
    }
}

/// Whether following `kept_of` from `from` reaches `target` (bounded by the
/// map's size, so it ends on a cycle that does not hold `target`).
pub(super) fn reaches(
    kept_of: &BTreeMap<StationId, StationId>,
    from: StationId,
    target: StationId,
) -> bool {
    let mut current = from;
    for _ in 0..=kept_of.len() {
        if current == target {
            return true;
        }
        match kept_of.get(&current) {
            Some(&next) => current = next,
            None => return false,
        }
    }
    false
}
