//! Loop closures: create, remove by id, the closure dialog's figures and
//! refusal (plan 5.5).
//!
//! Creation refuses, in today's order: an end that does not exist; two ends
//! that are one station; ends in different pieces unless both pieces are
//! placed by a datum; a second closure between the same two stations; a
//! closure that would tie the two ends of one shot together (#737, today's
//! knot rule); the deferred distribution strategies; and `Tie`, which only
//! the migration writes. Nothing is stored about the closure beyond its ends,
//! their `via` hints and its strategy: its figures and whether a snap is
//! aligned are derived (plan 3.6). Creating or removing a closure moves no
//! station and never orphans one, so neither runs the sweep.

use crate::{ClosurePreview, LineId, StationId, StationRef};

use super::m3_stubs::{closure_metrics, closure_spans_one_shot};
use super::ops::{OpError, OpOutcome, Sweep};
use super::{ClosureId, ClosureStrategy, EditContext, LoopClosure, Project};

/// What creating a closure did: the closure as stored, and its figures at
/// creation (derived, for the result and the history label only).
#[derive(Debug, Clone)]
pub struct ClosureCreated {
    pub closure: LoopClosure,
    pub metrics: Option<ClosurePreview>,
}

impl Project {
    /// Create a loop closure between `a` and `b`, attached via `a_via` and
    /// `b_via` (the lines whose slots the user picked; each must pass its
    /// station).
    pub fn create_closure(
        &mut self,
        a: StationId,
        a_via: Option<LineId>,
        b: StationId,
        b_via: Option<LineId>,
        strategy: ClosureStrategy,
    ) -> Result<OpOutcome<ClosureCreated>, OpError> {
        self.closure_refusal(a, b, strategy)?;
        for (station, via) in [(a, a_via), (b, b_via)] {
            if let Some(line) = via {
                if self.topology().slot_on(station, line).is_none() {
                    return Err(OpError::StationNotOnLine { station, line });
                }
            }
        }
        let metrics = closure_metrics(self, a, b);
        self.run_op(Sweep::Skip, |p| {
            let id = p.push_closure(a, a_via, b, b_via, strategy);
            let closure = *p
                .closures
                .last()
                .filter(|c| c.id == id)
                .ok_or(OpError::UnknownClosure(id))?;
            Ok((ClosureCreated { closure, metrics }, EditContext::none()))
        })
    }

    /// [`Self::create_closure`] between two slots: each end is the slot's
    /// station, attached via the slot's line.
    pub fn create_closure_at(
        &mut self,
        a: StationRef,
        b: StationRef,
        strategy: ClosureStrategy,
    ) -> Result<OpOutcome<ClosureCreated>, OpError> {
        let (sa, sb) = (self.station_of_slot(a)?, self.station_of_slot(b)?);
        self.create_closure(sa, Some(a.line_id), sb, Some(b.line_id), strategy)
    }

    /// Remove the closure with `id`; returns it as it was stored.
    pub fn remove_closure(&mut self, id: ClosureId) -> Result<OpOutcome<LoopClosure>, OpError> {
        let position = self
            .closures
            .iter()
            .position(|c| c.id == id)
            .ok_or(OpError::UnknownClosure(id))?;
        self.run_op(Sweep::Skip, |p| {
            Ok((p.closures.remove(position), EditContext::none()))
        })
    }

    /// Why a closure between `a` and `b` with `strategy` would be refused,
    /// in today's order (see the module docs). Read-only; the closure dialog
    /// and [`Self::create_closure`] ask the same question.
    pub fn closure_refusal(
        &self,
        a: StationId,
        b: StationId,
        strategy: ClosureStrategy,
    ) -> Result<(), OpError> {
        self.existing_station(a)?;
        self.existing_station(b)?;
        if a == b {
            return Err(OpError::ClosureOnOneStation);
        }
        let both_placed =
            self.placement_datum_of(a).is_some() && self.placement_datum_of(b).is_some();
        if !(self.topology().connected(a, b) || both_placed) {
            return Err(OpError::NotConnected);
        }
        if self.closure_between(a, b).is_some() {
            return Err(OpError::DuplicateClosure);
        }
        if let Some((line, index)) = closure_spans_one_shot(self, a, b) {
            return Err(self.across_one_shot_error(line, index));
        }
        match strategy {
            ClosureStrategy::ProportionalByLength | ClosureStrategy::EqualDistribution => {
                Err(OpError::DeferredStrategy)
            }
            ClosureStrategy::Tie => Err(OpError::TieNotCreatable),
            ClosureStrategy::Batch
            | ClosureStrategy::LeastSquaresWeighted
            | ClosureStrategy::Move => Ok(()),
        }
    }

    /// The closure dialog's figures between `a` and `b` (today's
    /// `compute_closure_metrics`).
    pub fn closure_preview(&self, a: StationId, b: StationId) -> Result<ClosurePreview, OpError> {
        self.existing_station(a)?;
        self.existing_station(b)?;
        closure_metrics(self, a, b).ok_or(OpError::UnknownStation(a))
    }

    /// The first closure (list order) joining `a` and `b`, either way round.
    pub fn closure_between(&self, a: StationId, b: StationId) -> Option<ClosureId> {
        self.closures
            .iter()
            .find(|c| (c.a == a && c.b == b) || (c.a == b && c.b == a))
            .map(|c| c.id)
    }

    /// The across-one-shot refusal for shot `index` of `line`, naming its two
    /// stations the way the user reads them (today's wording).
    pub(super) fn across_one_shot_error(&self, line: LineId, index: usize) -> OpError {
        let chain = self.topology().chain(line).unwrap_or(&[]);
        let name = |i: usize| {
            chain.get(i).map_or_else(
                || format!("{} / {i}", self.line_text(line)),
                |&s| self.station_text(s, Some(line)),
            )
        };
        let (from, to) = (name(index), name(index + 1));
        OpError::AcrossOneShot {
            line,
            index,
            message: format!(
                "This closure would join the two ends of one shot ({from} to {to}) and squeeze \
                 that shot to zero length. Pick two stations that are not the ends of a single \
                 shot."
            ),
        }
    }
}
