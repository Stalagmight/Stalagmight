//! Loop closures: create, remove by id, the closure dialog's figures and
//! refusal (plan 5.5), and which end a closure keeps (RS 5).
//!
//! Creation refuses, in today's order: an end that does not exist; two ends
//! that are one station; ends in different pieces unless both pieces are
//! placed by a datum; a second closure between the same two stations; a
//! closure that would tie the two ends of one shot together (#737, today's
//! knot rule); the deferred distribution strategies; and `Tie`, which only
//! the migration writes. Its figures and whether a snap is aligned are
//! derived (plan 3.6). Creating or removing a closure moves no station and
//! never orphans one, so neither runs the sweep.
//!
//! **Kept end** (RS 5.2): a closure may archive one of its ends (`keep`
//! names the other): the archived end then shows the kept end's shown
//! identity on every line through it ([`Project::shown`]), while it stays a
//! node of its own (geometry, datums and selection keep its id). A keep is
//! allowed when the end it archives is not archived by another closure (C1)
//! and the kept end does not already show it (C2); `None` ("Keep both
//! names") archives nothing. The dialog's default is rule K's end when it
//! is allowed, else "Keep both names" ([`Project::closure_keep_choices`]).
//! The user's renames answer the names the archive shows twice on a line
//! (N8, on every line the kept identity will be shown on, closure subtrees
//! included); an unanswered one refuses ([`OpError::NameTaken`]); the forms
//! without a dialog letter it by rule L.
//!
//! **Remove** (RS 5.3): removing a closure drops its archive with it; the
//! archived end (and every node that showed it through the closure) shows
//! its own identity again. Remove Closure carries the dialog's renames; every
//! other removal path (the #737 removal, deleting the kept end, `Tie`
//! removal, Combine's dedupe, which moves the archive instead) letters by
//! rule L in its epilogue.

use crate::{ClosurePreview, KeepRefusalReason, LineId, StationId, StationRef};

use super::ops::{OpError, OpOutcome, Sweep};
use super::ops_merge::Answers;
use super::{
    ClosureEnd, ClosureId, ClosureStrategy, EditContext, LetteredStation, LoopClosure, Project,
};

/// What creating a closure did: the closure as stored (its `archived` end
/// included), and its figures at creation (derived, for the result and the
/// history label only).
#[derive(Debug, Clone)]
pub struct ClosureCreated {
    pub closure: LoopClosure,
    pub metrics: Option<ClosurePreview>,
}

/// Which ends a closure between two stations may keep (RS 5.2), for the
/// Create Loop Closure dialog's "Both ends show" choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureKeepChoices {
    /// The ends that may be kept (C1, C2), `a` first.
    pub allowed: Vec<StationId>,
    /// Rule K's end over the two (node candidates), allowed or not.
    pub rule_k: Option<StationId>,
    /// Why rule K's end may not be kept (C1 or C2); `None` when it may.
    pub rule_k_refused: Option<KeepRefusalReason>,
    /// The dialog's default: rule K's end when it is allowed, else `None`
    /// ("Keep both names": the closure archives nothing).
    pub default: Option<StationId>,
}

/// What creating or removing a closure does to the names shown (RS 5.2,
/// 5.3), for the dialog before the user confirms: the op run on a copy with
/// every collision answered by rule L.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureNaming {
    /// The kept end and the archived end (`None` when the closure archives
    /// nothing).
    pub kept: Option<StationId>,
    pub archived: Option<StationId>,
    /// Creating: the identity that becomes hidden; removing: the identity
    /// that comes back. Its Station ID, name and comment.
    pub identity: Option<(StationId, String, Option<String>)>,
    /// The lines on which some station shows another identity afterwards
    /// (the archived end's and its closure subtree's), in line order.
    pub renamed_lines: Vec<LineId>,
    /// The names the op would show twice on a line, with rule L's answers
    /// (the other answer the dialog offers is renaming the kept station).
    pub collisions: Vec<LetteredStation>,
    /// The dialog's other answer to each collision, in the same order
    /// ([`Project::alternative_letters`]).
    pub alternatives: Vec<LetteredStation>,
}

impl Project {
    /// Create a loop closure between `a` and `b`, attached via `a_via` and
    /// `b_via` (the lines whose slots the user picked; each must pass its
    /// station), keeping `keep` (RS 5.2: the other end is archived; `None`
    /// archives nothing) and applying `renames` (each renames the identity
    /// whose Station ID it names) in the same op. A keep that is not one of
    /// the ends, or not allowed by C1 or C2, refuses
    /// ([`OpError::InvalidKeep`]); a name shown twice on a line that the
    /// renames do not answer refuses ([`OpError::NameTaken`]).
    #[expect(
        clippy::too_many_arguments,
        reason = "the closure's two attached ends, its strategy and the dialog's two answers"
    )]
    pub fn create_closure(
        &mut self,
        a: StationId,
        a_via: Option<LineId>,
        b: StationId,
        b_via: Option<LineId>,
        strategy: ClosureStrategy,
        keep: Option<StationId>,
        renames: &[(StationId, String)],
    ) -> Result<OpOutcome<ClosureCreated>, OpError> {
        self.create_with(
            (a, a_via),
            (b, b_via),
            strategy,
            keep,
            Answers::Given(renames),
        )
    }

    /// [`Self::create_closure`] without a dialog: every name collision is
    /// settled by rule L and reported (`OpOutcome::identities`).
    pub fn create_closure_auto(
        &mut self,
        a: StationId,
        a_via: Option<LineId>,
        b: StationId,
        b_via: Option<LineId>,
        strategy: ClosureStrategy,
        keep: Option<StationId>,
    ) -> Result<OpOutcome<ClosureCreated>, OpError> {
        self.create_with((a, a_via), (b, b_via), strategy, keep, Answers::RuleL)
    }

    /// [`Self::create_closure`] between two slots: each end is the slot's
    /// station, attached via the slot's line.
    pub fn create_closure_at(
        &mut self,
        a: StationRef,
        b: StationRef,
        strategy: ClosureStrategy,
        keep: Option<StationId>,
        renames: &[(StationId, String)],
    ) -> Result<OpOutcome<ClosureCreated>, OpError> {
        let (sa, sb) = (self.station_of_slot(a)?, self.station_of_slot(b)?);
        self.create_closure(
            sa,
            Some(a.line_id),
            sb,
            Some(b.line_id),
            strategy,
            keep,
            renames,
        )
    }

    fn create_with(
        &mut self,
        (a, a_via): (StationId, Option<LineId>),
        (b, b_via): (StationId, Option<LineId>),
        strategy: ClosureStrategy,
        keep: Option<StationId>,
        answers: Answers<'_>,
    ) -> Result<OpOutcome<ClosureCreated>, OpError> {
        self.closure_refusal(a, b, strategy)?;
        for (station, via) in [(a, a_via), (b, b_via)] {
            if let Some(line) = via {
                if self.topology().slot_on(station, line).is_none() {
                    return Err(OpError::StationNotOnLine { station, line });
                }
            }
        }
        let archived = match keep {
            None => None,
            Some(k) if self.closure_keep_allowed(a, b, k) => {
                Some(if k == a { ClosureEnd::B } else { ClosureEnd::A })
            }
            Some(k) => return Err(OpError::InvalidKeep(k)),
        };
        let metrics = self.closure_metrics_via((a, a_via), (b, b_via));
        let hidden_end = match archived {
            Some(ClosureEnd::A) => Some(a),
            Some(ClosureEnd::B) => Some(b),
            None => None,
        };
        let mut outcome = self.run_op_answered(Sweep::Skip, answers, |p| {
            let id = p.push_closure(a, a_via, b, b_via, strategy);
            let closure = p
                .closures
                .last_mut()
                .filter(|c| c.id == id)
                .ok_or(OpError::UnknownClosure(id))?;
            closure.archived = archived;
            let closure = *closure;
            Ok((ClosureCreated { closure, metrics }, EditContext::none()))
        })?;
        // The archived end's comment is the dialog's own `hidden`.
        outcome.identities = std::mem::take(&mut outcome.identities).without_hidden(hidden_end);
        Ok(outcome)
    }

    /// Whether a closure between `a` and `b` may keep `keep` (RS 5.2): it is
    /// one of the two, the end it archives is not archived by another
    /// closure (C1), and `keep` does not already show that end (C2).
    pub fn closure_keep_allowed(&self, a: StationId, b: StationId, keep: StationId) -> bool {
        (keep == a || keep == b) && a != b && self.closure_keep_refusal(a, b, keep).is_none()
    }

    /// Why a closure between `a` and `b` may not keep `keep`, an end of it
    /// (RS 5.2): C1 when the other end is already archived by another
    /// closure, else C2 when `keep` already shows the other end. `None` when
    /// the keep is allowed, and for a `keep` that is not an end.
    pub fn closure_keep_refusal(
        &self,
        a: StationId,
        b: StationId,
        keep: StationId,
    ) -> Option<KeepRefusalReason> {
        let archived = if keep == a {
            b
        } else if keep == b {
            a
        } else {
            return None;
        };
        if self.is_closure_archived(archived) {
            Some(KeepRefusalReason::OtherEndArchived)
        } else if self.shown(keep) == archived {
            Some(KeepRefusalReason::KeptShowsOtherEnd)
        } else {
            None
        }
    }

    /// The Create Loop Closure dialog's choices between `a` and `b` (RS 5.2):
    /// the ends that may be kept, rule K's end, and the default (rule K's end
    /// when allowed, else "Keep both names").
    pub fn closure_keep_choices(&self, a: StationId, b: StationId) -> ClosureKeepChoices {
        let allowed: Vec<StationId> = [a, b]
            .into_iter()
            .filter(|&k| self.closure_keep_allowed(a, b, k))
            .collect();
        let rule_k = self.rule_k_nodes(&[a, b]);
        let default = rule_k.filter(|k| allowed.contains(k));
        let rule_k_refused = rule_k
            .filter(|_| a != b)
            .and_then(|k| self.closure_keep_refusal(a, b, k));
        ClosureKeepChoices {
            allowed,
            rule_k,
            rule_k_refused,
            default,
        }
    }

    /// What creating the closure keeping `keep` would do to the names shown
    /// (RS 5.2): the creation run on a copy with every collision answered by
    /// rule L. Refused as the creation is.
    pub fn closure_naming(
        &self,
        (a, a_via): (StationId, Option<LineId>),
        (b, b_via): (StationId, Option<LineId>),
        strategy: ClosureStrategy,
        keep: Option<StationId>,
    ) -> Result<ClosureNaming, OpError> {
        let mut copy = self.clone();
        let outcome = copy.create_closure_auto(a, a_via, b, b_via, strategy, keep)?;
        let archive = outcome.value.closure.archive();
        let identity = archive.and_then(|(archived, _)| self.identity_text(archived));
        Ok(ClosureNaming {
            kept: archive.map(|(_, kept)| kept),
            archived: archive.map(|(archived, _)| archived),
            identity,
            renamed_lines: self.lines_shown_differently(&copy),
            alternatives: copy.alternative_letters(&outcome.identities.lettered),
            collisions: outcome.identities.lettered,
        })
    }

    /// Remove the closure with `id` (RS 5.3), applying `renames` in the same
    /// op: the end it archived shows its own identity again, and a name that
    /// then repeats on a line that the renames do not answer refuses
    /// ([`OpError::NameTaken`]). Returns the closure as it was stored.
    pub fn remove_closure(
        &mut self,
        id: ClosureId,
        renames: &[(StationId, String)],
    ) -> Result<OpOutcome<LoopClosure>, OpError> {
        self.remove_with(id, Answers::Given(renames))
    }

    /// [`Self::remove_closure`] without a dialog: every name collision is
    /// settled by rule L and reported (`OpOutcome::identities`).
    pub fn remove_closure_auto(
        &mut self,
        id: ClosureId,
    ) -> Result<OpOutcome<LoopClosure>, OpError> {
        self.remove_with(id, Answers::RuleL)
    }

    fn remove_with(
        &mut self,
        id: ClosureId,
        answers: Answers<'_>,
    ) -> Result<OpOutcome<LoopClosure>, OpError> {
        let position = self
            .closures
            .iter()
            .position(|c| c.id == id)
            .ok_or(OpError::UnknownClosure(id))?;
        self.run_op_answered(Sweep::Skip, answers, |p| {
            let removed = p.closures.remove(position);
            Ok((removed, EditContext::none()))
        })
    }

    /// What removing the closure with `id` would do to the names shown (RS
    /// 5.3, the Remove Closure dialog): the identity that comes back (the
    /// archived end's own), the lines that show it again, and the names that
    /// would repeat, with rule L's answers. Refused as the removal is.
    pub fn closure_removal(&self, id: ClosureId) -> Result<ClosureNaming, OpError> {
        let closure = self
            .closures
            .iter()
            .find(|c| c.id == id)
            .copied()
            .ok_or(OpError::UnknownClosure(id))?;
        let mut copy = self.clone();
        let outcome = copy.remove_closure_auto(id)?;
        let archive = closure.archive();
        Ok(ClosureNaming {
            kept: archive.map(|(_, kept)| kept),
            archived: archive.map(|(archived, _)| archived),
            identity: archive.and_then(|(archived, _)| copy.identity_text(archived)),
            renamed_lines: self.lines_shown_differently(&copy),
            alternatives: copy.alternative_letters(&outcome.identities.lettered),
            collisions: outcome.identities.lettered,
        })
    }

    /// A node's own identity as text: its Station ID, name and comment.
    pub(super) fn identity_text(
        &self,
        node: StationId,
    ) -> Option<(StationId, String, Option<String>)> {
        let s = self.stations.get(&node)?;
        Some((s.id, s.name.clone(), s.comment.clone()))
    }

    /// The lines (in this state's line order) on which some node, present in
    /// both states under one id, shows another identity in `after` (rule-L
    /// letters are the collisions', not counted here).
    pub(super) fn lines_shown_differently(&self, after: &Project) -> Vec<LineId> {
        let topology = self.topology();
        let mut out = Vec::new();
        for (position, line) in self.lines.iter().enumerate() {
            let differs = topology
                .chain_at(position)
                .unwrap_or(&[])
                .iter()
                .any(|&node| {
                    after.stations.contains_key(&node) && after.shown(node) != self.shown(node)
                });
            if differs {
                out.push(line.id);
            }
        }
        out
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
        if let Some((line, index)) = self.closure_spans_one_shot(a, b) {
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
        self.closure_metrics(a, b).ok_or(OpError::UnknownStation(a))
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
                || format!("{} / {i}", self.line_label_of(line)),
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
