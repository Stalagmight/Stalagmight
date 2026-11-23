//! Detach Line from Station (unmerge, plan 5.2; RS 4.2): give one line its
//! own station back at a junction.
//!
//! Detaching line L from junction N, in the three cases of RS 4.2:
//!
//! 1. **A record R of N names L** (L's own station was archived by a merge):
//!    L gets a new node with R's identity (id, name, comment, external id),
//!    and R leaves N. When R names other lines too (a split junction merged
//!    away), all of R's lines come off together onto that node, since they
//!    were one station (M-P2-3).
//! 2. **L is native and every other line is named by a record**
//!    ([`Project::detach_keeps`]): L keeps N's identity; the other lines get
//!    a new node whose identity is the record naming them, or, when several
//!    records remain, the one `rest_keep` picks (default: rule K over their
//!    own lines); the others stay archived in the new node.
//! 3. **L and another line are native** (Split Line Here, Create Line, an
//!    import join): L gets a fresh station with N's own name and no comment.
//!
//! What belonged to the lines that come off follows them: the datums and
//! closure ends whose `via` is one of them (with `kept`, those whose `via` is
//! not L), and their hand moves at N. A closure's `archived` marker follows
//! its end. History records (A5) stay with N, which keeps the identity they
//! were archived under. Every identity shown anew is checked by N8: the
//! user's renames answer a collision, an unanswered one refuses
//! ([`super::OpError::NameTaken`]); the forms without a dialog letter it by
//! rule L. A letter an earlier op gave because of the junction stays. The
//! anchor algorithm then settles both pieces (a fix that came along and is
//! now the first datum of its piece is promoted and puts that piece on its
//! GPS, as today's unmerge does), and the orphan-closure sweep promotes a
//! closure that only joined the pieces.
//!
//! Two forms, both detaching ONE line (or one record's lines): the pair form
//! detaches the second slot's line from the station the two slots share; the
//! station form detaches the slot's line from every other line at that
//! station (a no-op on a station that is not a junction, as today).
//!
//! The mutation ([`Project::detach_ids`]) is the one core every detach goes
//! through: these commands and Combine Lines' torn junctions
//! (`edit/combine/`, plan 5.4 steps 2, 3 and 6; RS 4.4: a tear is a
//! restoring detach of one line, whose tear closure then archives the end
//! that does not hold the identity the torn slot showed).

use crate::{LineId, StationId, StationRef};

use super::ops::{OpError, OpOutcome, Sweep};
use super::ops_merge::Answers;
use super::{
    ArchivedStation, ClosureEnd, ClosureId, EditContext, LetteredStation, Project, Station,
};

/// How a command answers the name collisions a detach makes (RS 4.2).
#[derive(Debug, Clone, Copy)]
pub enum NameAnswers<'a> {
    /// The dialog's renames, each `(Station ID, new name)`; an unanswered
    /// collision refuses ([`OpError::NameTaken`]).
    Renames(&'a [(StationId, String)]),
    /// No dialog: rule L letters every collision and the outcome reports it.
    RuleL,
}

impl<'a> NameAnswers<'a> {
    fn answers(self) -> Answers<'a> {
        match self {
            Self::Renames(renames) => Answers::Given(renames),
            Self::RuleL => Answers::RuleL,
        }
    }
}

/// What a detach did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachReport {
    /// The line that was detached.
    pub line: LineId,
    /// The junction it was detached from (it stays on the other lines).
    pub station: StationId,
    /// The station the lines that came off are on now, where the junction
    /// was: the detached line's (with `kept`, the other lines').
    pub new_station: StationId,
    /// The lines that came off onto `new_station`, in line order: `line` and
    /// the other lines of its record (RS 4.2 case 1), or with `kept` every
    /// other line.
    pub lines_off: Vec<LineId>,
    /// Datums (list indices) that moved to the new station.
    pub moved_datums: Vec<usize>,
    /// Closure ends that moved to the new station.
    pub moved_closure_ends: Vec<(ClosureId, ClosureEnd)>,
    /// Whether a hand move at the junction moved too.
    pub moved_hand_move: bool,
    /// The new station took back an archived identity (a record of the
    /// junction): its id, name, comment and external id.
    pub restored: bool,
    /// The line kept the junction and the other lines, which were all named
    /// by records, got back an archived station (`new_station`): RS 4.2
    /// case 2.
    pub kept: bool,
}

impl DetachReport {
    /// The node the detached line is on now, where the junction was: the
    /// junction itself when it kept it (`kept`), else `new_station`.
    pub fn line_node(&self) -> StationId {
        if self.kept {
            self.station
        } else {
            self.new_station
        }
    }

    /// The node every other line of the junction is on now: `new_station`
    /// when the line kept the junction (`kept`), else the junction.
    pub fn rest_node(&self) -> StationId {
        if self.kept {
            self.new_station
        } else {
            self.station
        }
    }

    /// The node `line` is on now, for a line that passed the junction:
    /// [`Self::line_node`] for the detached line, [`Self::rest_node`] for
    /// the others (the lines of its record that came off with it included,
    /// which [`Self::lines_off`] names).
    pub fn node_of(&self, line: LineId) -> StationId {
        if line == self.line || (!self.kept && self.lines_off.contains(&line)) {
            self.line_node()
        } else {
            self.rest_node()
        }
    }
}

/// What detaching a line would give it, for the Detach dialog: the station
/// the lines that come off get, whether it is one they had before a merge,
/// and what every line through the junction ends up with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachNaming {
    /// The station the lines that come off get: its name, id and comment.
    pub name: String,
    pub id: StationId,
    pub comment: Option<String>,
    /// It is an archived station coming back.
    pub restored: bool,
    /// The line keeps the junction; `name` and `id` are what the other lines
    /// get back.
    pub kept: bool,
    /// The lines that come off with the detached one because one archived
    /// station names them all (RS 4.2 case 1; the dialog says so).
    pub together: Vec<LineId>,
    /// With `kept` and several records naming the other lines: the records
    /// `rest_keep` may pick (their Station IDs), and the default (rule K).
    pub rest_choices: Vec<StationId>,
    pub rest_default: Option<StationId>,
    /// The identity the detached line showed at the junction before, when a
    /// loop closure made it another station's (RS 4.2 case 3: it will show
    /// the junction's own name).
    pub shown_before: Option<StationId>,
    /// The names the detach would show twice, with rule L's answers.
    pub collisions: Vec<LetteredStation>,
    /// The dialog's other answer to each collision, in the same order
    /// ([`Project::alternative_letters`]).
    pub alternatives: Vec<LetteredStation>,
    /// Every line through the junction, in slot order: the station it is on
    /// there after the detach (demo finding V-1: the dialog says what each
    /// line ends up with, not one Station ID for all).
    pub lines: Vec<DetachLineOutcome>,
}

/// What one line through a junction ends up with after a detach
/// ([`DetachNaming::lines`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachLineOutcome {
    pub line: LineId,
    /// The station the line is on there after the detach.
    pub station: StationId,
    /// The name the line shows it by.
    pub name: String,
    /// The line comes off the junction; otherwise it stays on it.
    pub comes_off: bool,
    /// The other lines on that station after the detach, in slot order.
    pub joined_with: Vec<LineId>,
}

/// Which detach [`Project::detach_ids`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetachMode {
    /// The command (RS 4.2): restore a record, keep the junction on the line
    /// when every other line is named by a record (`rest_keep` picks the
    /// record those lines take).
    Restore { rest_keep: Option<StationId> },
    /// Combine Lines' tear of one line (RS 4.4, a restoring detach): the
    /// fresh station takes the record naming the line, if any (only that
    /// line comes off: the record's other lines stay, native, on the
    /// junction); when the torn line is the junction's only native line, the
    /// case-2 distribution applies (the line keeps the junction, the other
    /// lines take the record rule K picks among those naming them).
    Tear,
}

impl Project {
    /// Detach line `line` from junction `station` (plan 5.2, RS 4.2),
    /// applying `renames` in the same op; `rest_keep` picks the record the
    /// other lines take when the line keeps the junction (case 2).
    pub fn detach_line(
        &mut self,
        station: StationId,
        line: LineId,
        renames: &[(StationId, String)],
        rest_keep: Option<StationId>,
    ) -> Result<OpOutcome<DetachReport>, OpError> {
        self.detach_with(station, line, NameAnswers::Renames(renames), rest_keep)
    }

    /// [`Self::detach_line`] with every collision settled by rule L (no
    /// dialog).
    pub fn detach_line_auto(
        &mut self,
        station: StationId,
        line: LineId,
        rest_keep: Option<StationId>,
    ) -> Result<OpOutcome<DetachReport>, OpError> {
        self.detach_with(station, line, NameAnswers::RuleL, rest_keep)
    }

    fn detach_with(
        &mut self,
        station: StationId,
        line: LineId,
        answers: NameAnswers<'_>,
        rest_keep: Option<StationId>,
    ) -> Result<OpOutcome<DetachReport>, OpError> {
        self.detach_refusal(station, line)?;
        self.run_op(Sweep::Run, |p| {
            let report = p.detach_ids(station, line, DetachMode::Restore { rest_keep })?;
            answers.answers().apply(p)?;
            let ctx = EditContext::line(line).with_renamed(station, report.new_station);
            Ok((report, ctx))
        })
    }

    /// Unmerge a pair (today's `UnmergeStationPair`): the two slots must be
    /// one station on two lines; `b`'s line is detached from it.
    pub fn unmerge_station_pair(
        &mut self,
        a: StationRef,
        b: StationRef,
        answers: NameAnswers<'_>,
        rest_keep: Option<StationId>,
    ) -> Result<OpOutcome<DetachReport>, OpError> {
        let (sa, sb) = (self.station_of_slot(a)?, self.station_of_slot(b)?);
        if sa != sb || a.line_id == b.line_id {
            return Err(OpError::NotDirectlyMerged);
        }
        self.detach_with(sb, b.line_id, answers, rest_keep)
    }

    /// Unmerge a station (today's `UnmergeStations`): the slot's line is
    /// detached from every other line at its station. `None` (nothing done)
    /// when the station is on one line only.
    pub fn unmerge_station(
        &mut self,
        slot: StationRef,
        answers: NameAnswers<'_>,
        rest_keep: Option<StationId>,
    ) -> Result<OpOutcome<Option<DetachReport>>, OpError> {
        let station = self.station_of_slot(slot)?;
        if !self.topology().is_junction(station) {
            return Ok(OpOutcome::unchanged(None));
        }
        Ok(self
            .detach_with(station, slot.line_id, answers, rest_keep)?
            .map(Some))
    }

    /// What [`Self::detach_line`] would do: the whole detach (epilogue
    /// included, every collision answered by rule L) run on a copy, so the
    /// dialog says what the command does. Refused as the detach is.
    pub fn detach_naming(
        &self,
        station: StationId,
        line: LineId,
        rest_keep: Option<StationId>,
    ) -> Result<DetachNaming, OpError> {
        let before = self.topology_arc();
        let mut slots: Vec<StationRef> = Vec::new();
        for &slot in before.slots_of(station) {
            if slots.iter().all(|s| s.line_id != slot.line_id) {
                slots.push(slot);
            }
        }
        let (rest_choices, rest_default) = if self.detach_keeps(station, line) {
            let choices = self.rest_candidates(station, line);
            let default = self.rule_k(choices.iter().map(|r| (r.id, r.lines.clone())));
            (choices.iter().map(|r| r.id).collect(), default)
        } else {
            (Vec::new(), None)
        };
        let shown = self.shown(station);
        let mut copy = self.clone();
        let outcome = copy.detach_line_auto(station, line, rest_keep)?;
        let report = &outcome.value;
        let after = copy.topology_arc();
        let mut lines = Vec::with_capacity(slots.len());
        for slot in slots {
            let on = after.station_at(slot).ok_or(OpError::UnknownSlot(slot))?;
            let l = slot.line_id;
            let name = copy
                .shown_name(on)
                .ok_or(OpError::UnknownStation(on))?
                .to_string();
            let mut joined_with: Vec<LineId> = Vec::new();
            for other in after.slots_of(on) {
                if other.line_id != l && !joined_with.contains(&other.line_id) {
                    joined_with.push(other.line_id);
                }
            }
            lines.push(DetachLineOutcome {
                line: l,
                station: on,
                name,
                comes_off: report.lines_off.contains(&l),
                joined_with,
            });
        }
        // The station the lines got, as the detach left it: a node, or, when
        // the detach's sweep then promoted a closure that archived it (the
        // lines it took off were bridged to the rest only by that closure),
        // a record of the promoted node. The lines' outcomes say where they
        // end up.
        let (name, comment) = match copy.stations.get(&report.new_station) {
            Some(identity) => (
                copy.shown_name(report.new_station)
                    .unwrap_or(&identity.name)
                    .to_string(),
                identity.comment.clone(),
            ),
            None => copy
                .stations
                .values()
                .flat_map(|s| s.archived.iter())
                .find(|r| r.id == report.new_station)
                .map(|r| (r.name.clone(), r.comment.clone()))
                .ok_or(OpError::UnknownStation(report.new_station))?,
        };
        Ok(DetachNaming {
            name,
            id: report.new_station,
            comment,
            restored: report.restored,
            kept: report.kept,
            together: if report.kept {
                Vec::new()
            } else {
                report
                    .lines_off
                    .iter()
                    .copied()
                    .filter(|&l| l != line)
                    .collect()
            },
            rest_choices,
            rest_default,
            shown_before: (!report.restored && !report.kept && shown != station).then_some(shown),
            collisions: outcome.identities.lettered.clone(),
            alternatives: copy.alternative_letters(&outcome.identities.lettered),
            lines,
        })
    }

    /// Whether detaching `line` from `station` keeps the station on `line`
    /// and gives the other lines back an archived station instead (RS 4.2
    /// case 2): the line is native there and records name every other line
    /// through it. Then the lines that come off are the others, which is
    /// what the history names (N-2).
    pub fn detach_keeps(&self, station: StationId, line: LineId) -> bool {
        let Some(junction) = self.stations.get(&station) else {
            return false;
        };
        let lines = self.topology().lines_of(station);
        lines.contains(&line)
            && lines.len() > 1
            && junction.record_for(line).is_none()
            && lines
                .iter()
                .filter(|&&l| l != line)
                .all(|&l| junction.record_for(l).is_some())
    }

    /// The records of `station` naming a line other than `line` (case 2's
    /// candidates for the remaining lines), in list order.
    fn rest_candidates(&self, station: StationId, line: LineId) -> Vec<ArchivedStation> {
        self.stations.get(&station).map_or_else(Vec::new, |s| {
            s.archived
                .iter()
                .filter(|r| r.lines.iter().any(|&l| l != line))
                .cloned()
                .collect()
        })
    }

    /// Why line `line` may not be detached from `station`, if it may not:
    /// an unknown station or line, a station off the line, or a station no
    /// other line passes (nothing to detach from).
    fn detach_refusal(&self, station: StationId, line: LineId) -> Result<(), OpError> {
        self.existing_station(station)?;
        if self.line(line).is_none() {
            return Err(OpError::UnknownLine(line));
        }
        let topology = self.topology();
        if topology.slot_on(station, line).is_none() {
            return Err(OpError::StationNotOnLine { station, line });
        }
        if !topology.is_junction(station) {
            return Err(OpError::NotDirectlyMerged);
        }
        Ok(())
    }

    /// The low-level detach (plan 5.2's mutation, RS 4.2's cases, module
    /// docs): put the lines that come off on their station (a restored
    /// record, or a fresh station named as the junction with no comment),
    /// rewrite their chains and move what they owned (datums and closure
    /// ends by `via`, hand moves). No renames, no anchors, no sweep, no
    /// validation. Refused, with nothing changed, as [`Self::detach_line`].
    pub(super) fn detach_ids(
        &mut self,
        station: StationId,
        line: LineId,
        mode: DetachMode,
    ) -> Result<DetachReport, OpError> {
        self.detach_refusal(station, line)?;
        let lines = self.topology().lines_of(station);
        let junction = self
            .stations
            .get(&station)
            .ok_or(OpError::UnknownStation(station))?;
        let record = junction.record_for(line).cloned();
        match (record, mode) {
            (Some(record), DetachMode::Restore { .. }) => {
                let off: Vec<LineId> = lines
                    .iter()
                    .copied()
                    .filter(|l| record.lines.contains(l))
                    .collect();
                Ok(self.restore_record(station, line, &record, off))
            }
            (Some(record), DetachMode::Tear) => {
                // Only the torn line comes off; the record's other lines (a
                // split junction merged away) show the junction natively now.
                Ok(self.restore_record(station, line, &record, vec![line]))
            }
            (None, DetachMode::Restore { rest_keep }) if self.detach_keeps(station, line) => {
                let others: Vec<LineId> = lines.iter().copied().filter(|&l| l != line).collect();
                self.detach_others(station, line, &others, rest_keep)
            }
            (None, DetachMode::Tear) if self.detach_keeps(station, line) => {
                let others: Vec<LineId> = lines.iter().copied().filter(|&l| l != line).collect();
                self.detach_others(station, line, &others, None)
            }
            (None, _) => {
                let name = junction.name.clone();
                let new_station = self.mint_station_id()?;
                self.stations
                    .insert(new_station, Station::new(new_station, name));
                let moved_hand_move = self.move_slots(station, new_station, &[line]);
                let (moved_datums, moved_closure_ends) =
                    self.move_attachments(station, new_station, |via| via == Some(line));
                Ok(DetachReport {
                    line,
                    station,
                    new_station,
                    lines_off: vec![line],
                    moved_datums,
                    moved_closure_ends,
                    moved_hand_move,
                    restored: false,
                    kept: false,
                })
            }
        }
    }

    /// RS 4.2 case 1: `record` of `station` leaves it and becomes the station
    /// of `off` (its lines that come off), with what they owned.
    fn restore_record(
        &mut self,
        station: StationId,
        line: LineId,
        record: &ArchivedStation,
        off: Vec<LineId>,
    ) -> DetachReport {
        if let Some(junction) = self.stations.get_mut(&station) {
            junction.archived.retain(|r| r.id != record.id);
        }
        let new_station = record.id;
        self.stations.insert(new_station, record.to_station());
        let moved_hand_move = self.move_slots(station, new_station, &off);
        let (moved_datums, moved_closure_ends) =
            self.move_attachments(station, new_station, |via| {
                via.is_some_and(|l| off.contains(&l))
            });
        DetachReport {
            line,
            station,
            new_station,
            lines_off: off,
            moved_datums,
            moved_closure_ends,
            moved_hand_move,
            restored: true,
            kept: false,
        }
    }

    /// RS 4.2 case 2: `line` keeps `station` (its id, name, comment and
    /// external id), and the other lines (`others`, all named by records)
    /// come off onto a new node whose identity is the record `rest_keep`
    /// names, else rule K's over the records' own lines; the other records
    /// naming those lines go with them, history records stay. What was not
    /// the line's own follows the other lines: datums and closure ends whose
    /// `via` is not the line, and their hand moves.
    fn detach_others(
        &mut self,
        station: StationId,
        line: LineId,
        others: &[LineId],
        rest_keep: Option<StationId>,
    ) -> Result<DetachReport, OpError> {
        let candidates = self.rest_candidates(station, line);
        let chosen = match rest_keep {
            Some(id) if candidates.iter().any(|r| r.id == id) => id,
            Some(id) => return Err(OpError::InvalidKeep(id)),
            None => self
                .rule_k(candidates.iter().map(|r| (r.id, r.lines.clone())))
                .ok_or(OpError::NotDirectlyMerged)?,
        };
        let junction = self
            .stations
            .get_mut(&station)
            .ok_or(OpError::UnknownStation(station))?;
        let names_other = |r: &ArchivedStation| r.lines.iter().any(|l| others.contains(l));
        let (moving, staying): (Vec<ArchivedStation>, Vec<ArchivedStation>) =
            std::mem::take(&mut junction.archived)
                .into_iter()
                .partition(|r| names_other(r));
        junction.archived = staying;
        let mut identity = None;
        let mut rest = Vec::with_capacity(moving.len());
        for record in moving {
            if record.id == chosen {
                identity = Some(record.to_station());
            } else {
                rest.push(record);
            }
        }
        let mut identity = identity.ok_or(OpError::InvalidKeep(chosen))?;
        identity.archived = rest;
        let new_station = identity.id;
        self.stations.insert(new_station, identity);

        let moved_hand_move = self.move_slots(station, new_station, others);
        let (moved_datums, moved_closure_ends) =
            self.move_attachments(station, new_station, |via| via != Some(line));
        Ok(DetachReport {
            line,
            station,
            new_station,
            lines_off: others.to_vec(),
            moved_datums,
            moved_closure_ends,
            moved_hand_move,
            restored: true,
            kept: true,
        })
    }

    /// Put `to` where `from` is on each of `lines` (its chain slot and its
    /// hand-move key). Whether a hand move moved.
    fn move_slots(&mut self, from: StationId, to: StationId, lines: &[LineId]) -> bool {
        let mut moved_hand_move = false;
        for &id in lines {
            if let Some(l) = self.line_mut(id) {
                if l.start == from {
                    l.start = to;
                } else if let Some(shot) = l.shots.iter_mut().find(|shot| shot.to == from) {
                    shot.to = to;
                }
                if let Some(hand) = l.hand_moves.remove(&from) {
                    l.hand_moves.insert(to, hand);
                    moved_hand_move = true;
                }
            }
        }
        moved_hand_move
    }

    /// Move the datums and closure ends on `from` whose `via` passes `moves`
    /// to `to`: their list indices and ends.
    fn move_attachments(
        &mut self,
        from: StationId,
        to: StationId,
        moves: impl Fn(Option<LineId>) -> bool,
    ) -> (Vec<usize>, Vec<(ClosureId, ClosureEnd)>) {
        let mut moved_datums = Vec::new();
        for (index, datum) in self.datums.iter_mut().enumerate() {
            if datum.station == from && moves(datum.via) {
                datum.station = to;
                moved_datums.push(index);
            }
        }
        let mut moved_closure_ends = Vec::new();
        for closure in &mut self.closures {
            if closure.a == from && moves(closure.a_via) {
                closure.a = to;
                moved_closure_ends.push((closure.id, ClosureEnd::A));
            }
            if closure.b == from && moves(closure.b_via) {
                closure.b = to;
                moved_closure_ends.push((closure.id, ClosureEnd::B));
            }
        }
        (moved_datums, moved_closure_ends)
    }
}
