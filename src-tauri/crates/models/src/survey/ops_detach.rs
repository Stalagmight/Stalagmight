//! Detach (unmerge, plan 5.2): give one line its own copy of a junction.
//!
//! Detaching line L from station S mints S' (a fresh id; the name L shows S
//! by, then uniquified; no comment, aliases or external id) and puts it in
//! L's chain where S was, so S' sits exactly where S did. When one of S's
//! aliases names L (the station L had before it was merged into S: a 0.6.0
//! merge member, or the moving station of a merge), S' takes that alias's id
//! and name back, and the alias leaves S (review finding UX-4: merging by
//! mistake and detaching gives the line back as it was). When instead every
//! other line knows S by an alias, L keeps S (its id, name and comment) and
//! the other lines get back the station they had, as detaching them would
//! (I-1: a station is not left only on lines that know it by another name).
//! Combine Lines' torn junctions keep the fresh id. What belonged to L's
//! member of the old merge follows it: the datums and closure ends whose
//! `via` is L, and L's hand move at S. The anchor algorithm then settles
//! both pieces (a fix that came along with L and is now the first datum of
//! L's piece is promoted and puts that piece on its GPS, as today's unmerge
//! does), and the orphan-closure sweep promotes a closure that only joined
//! L's piece to the rest.
//!
//! Two forms, both detaching ONE line: the pair form detaches the second
//! slot's line from the station the two slots share; the station form
//! detaches the slot's line from every other line at that station (a no-op
//! on a station that is not a junction, as today). Declared change (plan
//! 5.2): on a hub of three or more lines the other lines stay joined.
//!
//! The mutation ([`Project::detach_ids`]) is the one core every detach goes
//! through: these commands and Combine Lines' torn junctions
//! (`edit/combine/`, plan 5.4 steps 2, 3 and 6).

use crate::{LineId, StationId, StationRef};

use super::ops::{OpError, OpOutcome, Sweep};
use super::{
    AnchorReport, ClosureEnd, ClosureId, EditContext, Project, Station, StationAlias, SweepReport,
    Topology,
};

/// What a detach did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachReport {
    /// The line that was detached.
    pub line: LineId,
    /// The junction it was detached from (it stays on the other lines).
    pub station: StationId,
    /// The line's new station where the junction was.
    pub new_station: StationId,
    /// Datums (list indices) that moved to the new station (`via` = line).
    pub moved_datums: Vec<usize>,
    /// Closure ends that moved to the new station (`via` = line).
    pub moved_closure_ends: Vec<(ClosureId, ClosureEnd)>,
    /// Whether the line's hand move at the junction moved too.
    pub moved_hand_move: bool,
    /// Whether the new station took back the id and name of the junction's
    /// alias that named its line.
    pub restored_alias: bool,
    /// The line kept the junction and the other lines, which all knew it by
    /// aliases, got back their own station (`new_station`): see
    /// [`Project::detach_line`].
    pub kept: bool,
}

/// What detaching a line would give it, for the Detach dialog: the new
/// station's name and id, and whether they are the ones the line had before
/// it was merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachNaming {
    pub name: String,
    pub id: StationId,
    pub restored_alias: bool,
    /// The line keeps the junction; `name` and `id` are what the other lines
    /// get back.
    pub kept: bool,
}

impl Project {
    /// Detach line `line` from junction `station` (plan 5.2).
    pub fn detach_line(
        &mut self,
        station: StationId,
        line: LineId,
    ) -> Result<OpOutcome<DetachReport>, OpError> {
        self.detach_refusal(station, line)?;
        self.run_op(Sweep::Run, |p| {
            let report = p.detach_ids(station, line, true)?;
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
    ) -> Result<OpOutcome<DetachReport>, OpError> {
        let (sa, sb) = (self.station_of_slot(a)?, self.station_of_slot(b)?);
        if sa != sb || a.line_id == b.line_id {
            return Err(OpError::NotDirectlyMerged);
        }
        self.detach_line(sb, b.line_id)
    }

    /// Unmerge a station (today's `UnmergeStations`): the slot's line is
    /// detached from every other line at its station. `None` (nothing done)
    /// when the station is on one line only.
    pub fn unmerge_station(
        &mut self,
        slot: StationRef,
    ) -> Result<OpOutcome<Option<DetachReport>>, OpError> {
        let station = self.station_of_slot(slot)?;
        if !self.topology().is_junction(station) {
            return Ok(OpOutcome {
                value: None,
                anchors: AnchorReport::default(),
                sweep: SweepReport::default(),
            });
        }
        let outcome = self.detach_line(station, slot.line_id)?;
        Ok(OpOutcome {
            value: Some(outcome.value),
            anchors: outcome.anchors,
            sweep: outcome.sweep,
        })
    }

    /// The name and id the new station of [`Self::detach_line`] would get,
    /// from the detach run on a copy. Refused as the detach is.
    pub fn detach_naming(&self, station: StationId, line: LineId) -> Result<DetachNaming, OpError> {
        let mut copy = self.clone();
        let report = copy.detach_ids(station, line, true)?;
        let name = copy
            .stations
            .get(&report.new_station)
            .map(|s| s.name.clone())
            .ok_or(OpError::UnknownStation(report.new_station))?;
        Ok(DetachNaming {
            name,
            id: report.new_station,
            restored_alias: report.restored_alias,
            kept: report.kept,
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

    /// The low-level detach (plan 5.2's mutation): mint the new station
    /// (named as the line shows the junction, then letter-suffixed if taken
    /// on the line; no comment, aliases or external id), rewrite the line's
    /// chain, move what the line's member owned (datums and closure ends
    /// whose `via` is the line, its hand move). With `restore`, an alias of
    /// the junction that names the line gives the new station its id and
    /// name (module docs); and when every other line knows the junction by an
    /// alias the line keeps the junction and those lines get back their own
    /// station instead ([`Self::detach_others`]). Refused, with nothing
    /// changed, as [`Self::detach_line`]. No anchors, no sweep, no
    /// validation.
    pub(super) fn detach_ids(
        &mut self,
        station: StationId,
        line: LineId,
        restore: bool,
    ) -> Result<DetachReport, OpError> {
        self.detach_refusal(station, line)?;
        let topology = self.topology_arc();
        let others: Vec<LineId> = topology
            .lines_of(station)
            .into_iter()
            .filter(|&l| l != line)
            .collect();
        if restore
            && !self.known_by_alias_on(station, line)
            && others.iter().all(|&l| self.known_by_alias_on(station, l))
        {
            return self.detach_others(station, line, &others);
        }
        let junction = self
            .stations
            .get(&station)
            .ok_or(OpError::UnknownStation(station))?;
        // The name the line shows the junction by (its alias's, when one
        // names it): the line keeps showing it.
        let name = self
            .name_on(station, line)
            .map_or_else(|| junction.name.clone(), str::to_string);
        // The line's own alias: the last one naming it (the latest merge).
        let own = if restore {
            junction.aliases.iter().rposition(|a| a.line == Some(line))
        } else {
            None
        };
        let restored = match own {
            Some(index) => self
                .stations
                .get_mut(&station)
                .map(|j| j.aliases.remove(index).id),
            None => None,
        };
        let new_station = match restored {
            Some(id) => id,
            None => self.mint_station_id()?,
        };
        self.stations
            .insert(new_station, Station::new(new_station, name));

        let moved_hand_move = self.move_slots(station, new_station, &[line]);
        let (moved_datums, moved_closure_ends) =
            self.move_attachments(station, new_station, |via| via == Some(line));
        self.uniquify_station_name(new_station);
        Ok(DetachReport {
            line,
            station,
            new_station,
            moved_datums,
            moved_closure_ends,
            moved_hand_move,
            restored_alias: restored.is_some(),
            kept: false,
        })
    }

    /// Detach `line` from `station` when every other line (`others`) knows
    /// the station by an alias (I-1): `line` keeps the station (its id, name,
    /// comment and external id), and the other lines get back the identity
    /// the first of those aliases (list order) records, its id and name, as
    /// a Detach of one of them would give it back; the aliases naming the
    /// other lines go with them. What was not the line's own follows the
    /// other lines: datums and closure ends whose `via` is not the line, and
    /// their hand moves.
    fn detach_others(
        &mut self,
        station: StationId,
        line: LineId,
        others: &[LineId],
    ) -> Result<DetachReport, OpError> {
        let junction = self
            .stations
            .get_mut(&station)
            .ok_or(OpError::UnknownStation(station))?;
        let names_other = |a: &StationAlias| a.line.is_some_and(|l| others.contains(&l));
        let (moving, staying): (Vec<StationAlias>, Vec<StationAlias>) =
            std::mem::take(&mut junction.aliases)
                .into_iter()
                .partition(|a| names_other(a));
        junction.aliases = staying;
        let fallback = junction.name.clone();
        let mut moving = moving.into_iter();
        let first = moving.next().ok_or(OpError::NotDirectlyMerged)?;
        let new_station = first.id;
        let mut identity = Station::new(new_station, first.name.unwrap_or(fallback));
        identity.aliases = moving.collect();
        self.stations.insert(new_station, identity);

        let moved_hand_move = self.move_slots(station, new_station, others);
        let (moved_datums, moved_closure_ends) =
            self.move_attachments(station, new_station, |via| via != Some(line));
        Ok(DetachReport {
            line,
            station,
            new_station,
            moved_datums,
            moved_closure_ends,
            moved_hand_move,
            restored_alias: true,
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

    /// I-1: every station an edit took a line off and left only on lines
    /// that know it by an alias goes back to the identity the first of those
    /// aliases (list order) records, as a Detach gives a line back its
    /// station: the alias's id and name, the alias leaving the station. The
    /// station keeps its comment (it may hold every line's notes), its
    /// external id and its other aliases; its slots, hand moves, datums,
    /// closure ends and placement follow the new id. Each line still shows
    /// the name it showed (invariant 8 holds as before). Runs in the epilogue
    /// of every edit and command (not Combine Lines, whose torn junctions
    /// keep their ids) after stale attachments are dropped (so an alias of a
    /// line that left is no longer a reason); `before` is the topology before
    /// the edit, so a station the edit did not take a line off is left
    /// alone. Returns `(old, new)` per station, in id order, for the anchor
    /// algorithm ([`EditContext::reidentified`]).
    pub(super) fn restore_alias_identities(
        &mut self,
        before: &Topology,
    ) -> Vec<(StationId, StationId)> {
        let topology = self.topology_arc();
        let candidates: Vec<StationId> = self
            .stations
            .iter()
            .filter(|(&id, s)| {
                s.aliases.iter().any(|a| a.line.is_some())
                    && topology.slots_of(id).len() < before.slots_of(id).len()
            })
            .map(|(&id, _)| id)
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for station in candidates {
            let lines = topology.lines_of(station);
            if lines.is_empty() || !lines.iter().all(|&l| self.known_by_alias_on(station, l)) {
                continue;
            }
            let Some(mut identity) = self.stations.remove(&station) else {
                continue;
            };
            let Some(index) = identity
                .aliases
                .iter()
                .position(|a| a.line.is_some_and(|l| lines.contains(&l)))
            else {
                self.stations.insert(station, identity);
                continue;
            };
            let alias = identity.aliases.remove(index);
            let new = alias.id;
            identity.id = new;
            if let Some(name) = alias.name {
                identity.name = name;
            }
            self.stations.insert(new, identity);
            self.move_slots(station, new, &lines);
            self.move_attachments(station, new, |_| true);
            for placement in &mut self.placements {
                if placement.station == station {
                    placement.station = new;
                }
            }
            out.push((station, new));
        }
        out
    }
}
