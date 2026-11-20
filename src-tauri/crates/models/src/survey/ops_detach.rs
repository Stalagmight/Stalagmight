//! Detach (unmerge, plan 5.2): give one line its own copy of a junction.
//!
//! Detaching line L from station S mints S' (a fresh id; S's name, then
//! uniquified; no comment, aliases or external id) and puts it in L's chain
//! where S was, so S' sits exactly where S did. What belonged to L's member of
//! the old merge follows it: the datums and closure ends whose `via` is L, and
//! L's hand move at S. The anchor algorithm then settles both pieces (a fix
//! that came along with L and is now the first datum of L's piece is promoted
//! and puts that piece on its GPS, as today's unmerge does), and the
//! orphan-closure sweep promotes a closure that only joined L's piece to the
//! rest.
//!
//! Two forms, both detaching ONE line: the pair form detaches the second
//! slot's line from the station the two slots share; the station form
//! detaches the slot's line from every other line at that station (a no-op
//! on a station that is not a junction, as today). Declared change (plan
//! 5.2): on a hub of three or more lines the other lines stay joined.

use crate::{LineId, StationId, StationRef};

use super::ops::{OpError, OpOutcome, Sweep};
use super::{AnchorReport, ClosureEnd, ClosureId, EditContext, Project, Station, SweepReport};

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
}

impl Project {
    /// Detach line `line` from junction `station` (plan 5.2).
    pub fn detach_line(
        &mut self,
        station: StationId,
        line: LineId,
    ) -> Result<OpOutcome<DetachReport>, OpError> {
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
        self.run_op(Sweep::Run, |p| {
            let report = p.detach_ids(station, line)?;
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

    /// The low-level detach: mint the new station, rewrite the line's chain,
    /// move what the line's member owned. No anchors, no sweep.
    fn detach_ids(&mut self, station: StationId, line: LineId) -> Result<DetachReport, OpError> {
        let name = self
            .stations
            .get(&station)
            .map(|s| s.name.clone())
            .ok_or(OpError::UnknownStation(station))?;
        let new_station = self.mint_station_id()?;
        self.stations
            .insert(new_station, Station::new(new_station, name));

        let mut moved_hand_move = false;
        if let Some(l) = self.line_mut(line) {
            if l.start == station {
                l.start = new_station;
            } else if let Some(shot) = l.shots.iter_mut().find(|shot| shot.to == station) {
                shot.to = new_station;
            }
            if let Some(hand) = l.hand_moves.remove(&station) {
                l.hand_moves.insert(new_station, hand);
                moved_hand_move = true;
            }
        }

        let mut moved_datums = Vec::new();
        for (index, datum) in self.datums.iter_mut().enumerate() {
            if datum.station == station && datum.via == Some(line) {
                datum.station = new_station;
                moved_datums.push(index);
            }
        }
        let mut moved_closure_ends = Vec::new();
        for closure in &mut self.closures {
            if closure.a == station && closure.a_via == Some(line) {
                closure.a = new_station;
                moved_closure_ends.push((closure.id, ClosureEnd::A));
            }
            if closure.b == station && closure.b_via == Some(line) {
                closure.b = new_station;
                moved_closure_ends.push((closure.id, ClosureEnd::B));
            }
        }
        self.uniquify_station_name(new_station);
        Ok(DetachReport {
            line,
            station,
            new_station,
            moved_datums,
            moved_closure_ends,
            moved_hand_move,
        })
    }
}
