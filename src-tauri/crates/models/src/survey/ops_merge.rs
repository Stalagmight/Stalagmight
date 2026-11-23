//! Merge stations (plan 5.1, direction 4.2; RS 4.1) and the Merge dialog's
//! check and preview.
//!
//! A merge makes two stations of different pieces one node. Geometry and
//! identity are independent:
//!
//! - **Geometry** is plan 4.2's: the piece that stays put is the stationary
//!   one, and [`Project::merge_ids`] folds the moving node into the
//!   stationary one everywhere (line starts, shot ends, datums, closure
//!   ends, hand-move keys). The moving piece then follows by derivation, its
//!   placement is dropped, and when both pieces were placed the later
//!   placement datum is a GPS fix from then on. Datums and closure ends keep
//!   their `via` hints, so a later detach can hand them back.
//! - **Identity** is the user's: the kept station (`keep`) is shown on every
//!   line through the node; the other is archived whole (one record with its
//!   id, name, comment, external id and native lines; its own records move
//!   along; nothing is minted and no comment is joined). When the kept
//!   station is the moving one, the merged node is re-identified in the same
//!   op (it takes the moving identity, and the stationary identity becomes a
//!   record naming the stationary node's pre-merge native lines), and the
//!   anchor algorithm is told ([`EditContext::with_reidentified`]), so the
//!   raw layout and depths are bit-identical for either keep (M-P1-1).
//! - Merging the two ends of an archiving closure keeps the closure's kept
//!   end (RS 4.1 step 4): its archived end becomes the merge record, whatever
//!   `keep` says. A closure-archived station kept by the merge keeps showing
//!   its closure's kept end after it (step 5); when the other station is
//!   kept, the closure archiving the one not kept drops its archive, so the
//!   kept station is shown on every line, as the user asked (RS 1.3), and
//!   the report names that closure ([`MergeReport::unarchived`]).
//! - The user's `renames` apply in the same op; a name the merge shows twice
//!   on a line that they do not answer refuses it ([`OpError::NameTaken`],
//!   invariant N8). [`Project::merge_stations_auto`] answers every collision
//!   by rule L instead (the paths without a dialog).
//!
//! The orphan-closure sweep runs afterwards, which also removes every closure
//! between the two stations (#738).
//!
//! Refused: two ends that are one station already; two stations already
//! connected (one piece, or tied by an aligned closure), so the user is routed
//! to Create Loop Closure, as today; a keep that is neither station.
//!
//! The fold itself ([`Project::merge_ids`]) is the one core every merge goes
//! through: Merge Stations, the sweep's closure promotions (`sweep.rs`) and
//! Combine Lines' join (`edit/combine/apply.rs`). Each caller decides the
//! direction and settles anchors itself.

use crate::{ClosurePreview, LineId, StationId, StationRef};

use super::ops::{OpError, OpOutcome, Sweep};
use super::topology::find;
use super::{ClosureId, EditContext, LetteredStation, Project};

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReport {
    /// The station kept: the identity the merged node shows, and the merged
    /// node's id.
    pub kept: StationId,
    /// The station archived into it (a record of the merged node now).
    pub archived: StationId,
    /// The station whose piece stayed put (plan 4.2).
    pub stationary: StationId,
    /// The station whose piece moved onto it.
    pub moving: StationId,
    /// The lines of the piece that moved, ascending.
    pub moving_lines: Vec<LineId>,
    /// The closure whose kept end the merge had to keep (RS 4.1 step 4),
    /// when the caller's `keep` was the other end.
    pub keep_forced_by: Option<ClosureId>,
    /// The closure that archived the station not kept as a loop end: its
    /// archive is dropped, so the kept station is shown on every line (RS
    /// 1.3). Also first in the outcome's `identities.unarchived`.
    pub unarchived: Option<ClosureId>,
}

/// How a command with a dialog answers the name collisions it makes.
#[derive(Debug, Clone, Copy)]
pub(super) enum Answers<'a> {
    /// The user's renames; an unanswered collision refuses.
    Given(&'a [(StationId, String)]),
    /// No dialog: rule L letters every collision (the epilogue does).
    RuleL,
}

impl Answers<'_> {
    /// Apply the renames and return the collisions still standing (the op's
    /// own, judged after the sweep, [`Project::refuse_unanswered`]); with
    /// rule L none: the epilogue letters them.
    pub(super) fn apply(self, p: &mut Project) -> Result<Vec<(LineId, String)>, OpError> {
        match self {
            Self::Given(renames) => p.apply_renames(renames),
            Self::RuleL => Ok(Vec::new()),
        }
    }
}

/// One of the two stations of a merge, for the dialog's "Keep" radios.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCandidate {
    /// The station (its own Station ID).
    pub station: StationId,
    /// Its own name and comment (what it shows unless a closure archives it).
    pub name: String,
    pub comment: Option<String>,
    /// The identity a loop closure shows it as, when one archives it (RS
    /// 4.1 step 5: it keeps showing that after the merge).
    pub shown_as: Option<StationId>,
    /// Its home line (RS 2), for the pill.
    pub home_line: Option<LineId>,
}

/// A name the merge would show twice on a line, with rule L's answer
/// (RS 4.1 step 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCollision {
    pub line: LineId,
    pub name: String,
    /// Rule L's answer: the station it letters, and the new name. The other
    /// answer the dialog offers is renaming the kept station.
    pub suggestion: LetteredStation,
}

/// What merging two separately placed pieces does to the later-placed one
/// (#741), for the Merge dialog to say before the user confirms
/// (`merge_datum_shift`).
#[derive(Debug, Clone, PartialEq)]
pub struct MergeDatumShift {
    /// Every line of the piece that moves, ascending.
    pub moving_lines: Vec<LineId>,
    /// How far it moves, horizontal metres.
    pub shift_m: f64,
    /// The list index of its placement datum, which becomes a GPS fix.
    pub datum: usize,
    /// That datum's station.
    pub datum_station: StationId,
}

/// What merging two unconnected stations does (demo issue I-4), for the
/// Merge dialog to say before the user confirms ([`Project::merge_preview`]).
#[derive(Debug, Clone, PartialEq)]
pub struct MergeMove {
    /// The station that stays put (geometry only: it says nothing about which
    /// station is kept).
    pub stationary: StationId,
    /// Every line of the piece that moves, ascending.
    pub moving_lines: Vec<LineId>,
    /// How far the merge moves anything, horizontal metres: the moving
    /// piece's shift (the moving station onto the stationary one), or more
    /// when the sweep the merge runs moves more.
    pub shift_m: f64,
    /// The two stations' depths before the merge, `a`'s then `b`'s.
    pub depths_m: [f64; 2],
    /// The joined station's depth: the mean of every depth reading at it,
    /// as 0.6.0 computed a merged station's.
    pub joined_depth_m: f64,
    /// Every depth reading that mean takes, with the slot it is read at
    /// (after the merge, so a station that already joined several lines
    /// gives one per line; demo finding V-2), in the order it sums them.
    pub readings: Vec<(StationRef, f64)>,
    /// The two stations, `a`'s then `b`'s (the dialog's radios).
    pub candidates: [MergeCandidate; 2],
    /// The station kept by this preview (the `keep` asked for, unless a
    /// closure forces its kept end, `keep_forced_by`).
    pub kept: StationId,
    /// The dialog's default keep ([`Project::merge_default_keep`]).
    pub default_keep: StationId,
    /// The closure between the two whose kept end must be kept (RS 4.1 step
    /// 4): the dialog offers no choice and says why.
    pub keep_forced_by: Option<ClosureId>,
    /// The lines whose shown name for the junction changes, in line order.
    pub renamed_lines: Vec<LineId>,
    /// The names the merge would show twice, with rule L's answers.
    pub collisions: Vec<MergeCollision>,
    /// The dialog's other answer to each collision, in the same order: the
    /// identity rule L kept the name for is lettered instead
    /// ([`Project::alternative_letters`]).
    pub alternatives: Vec<LetteredStation>,
    /// The identity that becomes hidden: its Station ID, name and comment.
    pub hidden: (StationId, String, Option<String>),
    /// The closure whose archive the merge drops so the kept station is
    /// shown on every line ([`MergeReport::unarchived`]): the station not
    /// kept was its loop end, shown as that closure's kept end until now.
    pub unarchived: Option<ClosureId>,
    /// Every other comment the merge hides (RS 12): a promotion the merge's
    /// sweep makes, or a closure archive it moves, may hide one. `hidden`'s
    /// own comment is not repeated here.
    pub comments_hidden: Vec<super::HiddenComment>,
    /// Every name a line shows twice after the merge and not before
    /// ([`Project::name_repeats`]): the junction shown through a dotted slot
    /// or a loop end beside the line's own station of that name, which
    /// RS-FX5 allows and the dialog says (RS-FX6, P2-6).
    pub repeats: Vec<NameRepeat>,
}

/// A name a line shows twice (RS-FX5, "dotted slots don't collide"): a native
/// slot shows the line's own station `own`, and a dotted slot (a foreign
/// slot, with the home line's dot) or a loop end (with the loop mark) shows
/// another station `shown` of the same name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NameRepeat {
    pub line: LineId,
    pub name: String,
    /// The line's own station of that name.
    pub own: StationId,
    /// The station the dotted slot or loop end shows.
    pub shown: StationId,
    /// Shown through a loop end (the loop mark), not a foreign slot.
    pub loop_end: bool,
}

/// The Merge tool's check of a picked pair (today's
/// `check_merge_connectivity`): connected pairs go to the closure dialog with
/// its preview figures, unconnected ones may merge.
#[derive(Debug, Clone)]
pub struct MergeCheck {
    /// Joined by shots and junctions, or through loop closures.
    pub connected: bool,
    /// The closure dialog's figures, for a connected pair.
    pub preview: Option<ClosurePreview>,
    /// Whether the pair may be merged.
    pub can_merge: bool,
    /// Why not, when it may not.
    pub reason: Option<String>,
    /// For a connected pair: why a closure would be refused (#737).
    pub closure_refusal: Option<String>,
    /// For a mergeable pair of two placed pieces: which one moves, how far.
    pub datum_shift: Option<MergeDatumShift>,
}

impl Project {
    /// Merge station `a` with station `b` (plan 5.1, RS 4.1), keeping `keep`
    /// (one of the two) and applying `renames` (each renames the identity
    /// whose Station ID it names) in the same op. The piece of `a` moves onto
    /// `b`, unless `a`'s piece is placed and `b`'s is not, or both are placed
    /// and `a`'s placement datum comes first (plan 4.2). A name the merge
    /// would show twice on a line that `renames` do not answer refuses it
    /// ([`OpError::NameTaken`]).
    pub fn merge_stations(
        &mut self,
        a: StationId,
        b: StationId,
        keep: StationId,
        renames: &[(StationId, String)],
    ) -> Result<OpOutcome<MergeReport>, OpError> {
        self.merge_with(a, b, keep, Answers::Given(renames))
    }

    /// [`Self::merge_stations`] without a dialog: every name collision is
    /// settled by rule L and reported (`OpOutcome::identities`).
    pub fn merge_stations_auto(
        &mut self,
        a: StationId,
        b: StationId,
        keep: StationId,
    ) -> Result<OpOutcome<MergeReport>, OpError> {
        self.merge_with(a, b, keep, Answers::RuleL)
    }

    /// The merge without a dialog's answers (Merge Stations with no `keep`
    /// or `renames`): the stationary station (the one the other piece moves
    /// to join, the station a merge kept before RS) is kept, and every
    /// collision is lettered by rule L.
    pub fn merge_keeping_stationary(
        &mut self,
        a: StationId,
        b: StationId,
    ) -> Result<OpOutcome<MergeReport>, OpError> {
        let keep = self.merge_direction(a, b).0;
        self.merge_stations_auto(a, b, keep)
    }

    fn merge_with(
        &mut self,
        a: StationId,
        b: StationId,
        keep: StationId,
        answers: Answers<'_>,
    ) -> Result<OpOutcome<MergeReport>, OpError> {
        self.merge_refusal(a, b)?;
        if keep != a && keep != b {
            return Err(OpError::InvalidKeep(keep));
        }
        let (stationary, moving) = self.merge_direction(a, b);
        let forced = self.forced_merge_keep(a, b);
        let (keep, keep_forced_by) = match forced {
            Some((closure, kept)) if kept != keep => (kept, Some(closure)),
            _ => (keep, None),
        };
        let archived = if keep == moving { stationary } else { moving };
        let mut outcome = self.run_op_answered(Sweep::Run, answers, |p| {
            let stationary_native = p.native_lines(stationary);
            let unarchived = p.unarchive_for_keep(keep, archived);
            let moving_lines = p.merge_ids(stationary, moving)?;
            let mut ctx = EditContext::merge(stationary, moving).with_renamed(moving, stationary);
            if keep == moving {
                p.reidentify_merged(stationary, moving, stationary_native);
                ctx = ctx.with_reidentified(&[(stationary, moving)]);
            }
            let report = MergeReport {
                kept: keep,
                archived,
                stationary,
                moving,
                moving_lines,
                keep_forced_by,
                unarchived,
            };
            Ok((report, ctx))
        })?;
        if let Some(closure) = outcome.value.unarchived {
            outcome.identities.unarchived.insert(0, closure);
        }
        // The archived station's comment is the dialog's own `hidden`.
        outcome.identities = std::mem::take(&mut outcome.identities).without_hidden(Some(archived));
        Ok(outcome)
    }

    /// RS 1.3 (the kept station is shown everywhere): before a merge of
    /// `kept` and `other` that keeps `kept`, drop the archive of the closure
    /// that archives `other` as a loop end, unless that closure's kept end
    /// is `kept` (the two ends of one closure, RS 4.1 step 4, where `other`
    /// becomes the merge record and the closure goes, #738). Without it the
    /// merged node stays archived by that closure and shows its kept end on
    /// every line, so the station the merge keeps is shown nowhere (model
    /// review finding 1). Every merge path calls it on the ids before the
    /// fold: Merge Stations, the sweep's promotions and Combine's join.
    /// Returns the closure.
    pub(super) fn unarchive_for_keep(
        &mut self,
        kept: StationId,
        other: StationId,
    ) -> Option<ClosureId> {
        let closure = self.closures.iter_mut().find(|c| {
            c.archive()
                .is_some_and(|(archived, kept_end)| archived == other && kept_end != kept)
        })?;
        closure.archived = None;
        Some(closure.id)
    }

    /// The closure between `a` and `b` that archives one of them, with its
    /// kept end (RS 4.1 step 4: a merge of the two keeps that end).
    pub fn forced_merge_keep(&self, a: StationId, b: StationId) -> Option<(ClosureId, StationId)> {
        self.closures.iter().find_map(|c| {
            let (archived, kept) = c.archive()?;
            ((archived == a && kept == b) || (archived == b && kept == a)).then_some((c.id, kept))
        })
    }

    /// The Merge dialog's default keep (RS 4.1): the closure-forced end
    /// (step 4); else the station holding a datum, when exactly one does;
    /// else rule K over the two (node candidates).
    pub fn merge_default_keep(&self, a: StationId, b: StationId) -> StationId {
        if let Some((_, kept)) = self.forced_merge_keep(a, b) {
            return kept;
        }
        let holds = |s: StationId| self.datums.iter().any(|d| d.station == s);
        match (holds(a), holds(b)) {
            (true, false) => a,
            (false, true) => b,
            _ => self.rule_k_nodes(&[a, b]).unwrap_or(a),
        }
    }

    /// After [`Self::merge_ids`] folded `moving` into `stationary`: give the
    /// merged node the moving identity (RS 4.1 step 3). The stationary
    /// identity becomes a record naming `stationary_native` (its pre-merge
    /// native lines) with its own records under it, the moving identity's
    /// records come under the node's identity again
    /// (`Station::swap_identity`), and the node's id becomes `moving`
    /// everywhere.
    pub(super) fn reidentify_merged(
        &mut self,
        stationary: StationId,
        moving: StationId,
        stationary_native: Vec<LineId>,
    ) {
        let Some(node) = self.stations.get_mut(&stationary) else {
            return;
        };
        if node.swap_identity(moving, stationary_native).is_none() {
            return;
        }
        self.rekey_node(stationary, moving);
    }

    /// Why `a` and `b` may not be merged, if they may not.
    pub fn merge_refusal(&self, a: StationId, b: StationId) -> Result<(), OpError> {
        self.existing_station(a)?;
        self.existing_station(b)?;
        if a == b {
            return Err(OpError::AlreadyMerged);
        }
        let topology = self.topology();
        if topology.connected(a, b)
            || self
                .rigid_groups(&self.closure_alignment(&self.raw_positions()))
                .tied(a, b)
        {
            return Err(OpError::AlreadyConnected);
        }
        for line in topology.lines_of(b) {
            if topology.slot_on(a, line).is_some() {
                return Err(OpError::StationTwiceOnLine { line, station: a });
            }
        }
        Ok(())
    }

    /// `(stationary, moving)` for a merge of `a` with `b` (plan 4.2): `a`'s
    /// piece moves unless it is placed and `b`'s is not, or both are placed
    /// and `a`'s placement datum comes first in the list.
    pub fn merge_direction(&self, a: StationId, b: StationId) -> (StationId, StationId) {
        let (pa, pb) = (self.placement_datum_of(a), self.placement_datum_of(b));
        let a_stays = match (pa, pb) {
            (Some(x), Some(y)) => x < y,
            (x, _) => x.is_some(),
        };
        if a_stays {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// The list index of the placement datum of `station`'s piece.
    pub fn placement_datum_of(&self, station: StationId) -> Option<usize> {
        let component = self.topology().component_of(station)?;
        self.placement_datums().get(component).copied().flatten()
    }

    /// What a merge of `a` and `b` would do to a placed piece: `None` unless
    /// both stations sit in two different pieces that are each placed by
    /// their own datum (today's `merge_datum_shift`). Read-only.
    pub fn merge_datum_shift(&self, a: StationId, b: StationId) -> Option<MergeDatumShift> {
        let (pa, pb) = (self.placement_datum_of(a)?, self.placement_datum_of(b)?);
        if pa == pb {
            return None;
        }
        let (anchor, moving, datum) = if pa < pb { (a, b, pb) } else { (b, a, pa) };
        let positions = self.raw_positions();
        let ([ax, ay], [mx, my]) = (positions.xy(anchor)?, positions.xy(moving)?);
        Some(MergeDatumShift {
            moving_lines: self.piece_lines(moving),
            shift_m: (ax - mx).hypot(ay - my),
            datum,
            datum_station: self.datums.get(datum)?.station,
        })
    }

    /// The Merge tool's check of `a` and `b` (today's
    /// `check_merge_connectivity`). Refuses a pair that is one station, or
    /// that a loop closure already joins.
    pub fn merge_check(&self, a: StationId, b: StationId) -> Result<MergeCheck, OpError> {
        self.existing_station(a)?;
        self.existing_station(b)?;
        if a == b {
            return Err(OpError::AlreadyMerged);
        }
        if self.closure_between(a, b).is_some() {
            return Err(OpError::DuplicateClosure);
        }
        let connected = self.topology().connected(a, b) || self.closure_linked(a, b);
        let preview = if connected {
            self.closure_metrics(a, b)
        } else {
            None
        };
        let closure_refusal = if connected {
            self.closure_spans_one_shot(a, b)
                .map(|(line, index)| self.across_one_shot_error(line, index).to_string())
        } else {
            None
        };
        Ok(MergeCheck {
            connected,
            preview,
            can_merge: !connected,
            reason: connected.then(|| "Creates survey loop".to_string()),
            closure_refusal,
            datum_shift: if connected {
                None
            } else {
                self.merge_datum_shift(a, b)
            },
        })
    }

    /// What merging `a` with `b` keeping `keep` would do (demo issue I-4,
    /// RS 4.1): the merge run on a copy with every collision answered by rule
    /// L, its raw layout compared with this one's (as the Delete Shot preview
    /// measures a split's weld). The depths are read at the merged node,
    /// whatever its id. `None` when the merge is refused. Read-only.
    pub fn merge_preview(&self, a: StationId, b: StationId, keep: StationId) -> Option<MergeMove> {
        let (stationary, moving) = self.merge_direction(a, b);
        let before = self.raw_positions();
        let depths_m = [before.z(a)?, before.z(b)?];
        let [mx, my] = before.xy(moving)?;
        let candidates = [self.merge_candidate(a)?, self.merge_candidate(b)?];
        let mut copy = self.clone();
        let outcome = copy.merge_stations_auto(a, b, keep).ok()?;
        let report = &outcome.value;
        // A promotion the sweep ran may have folded the merged node further.
        let mut node = report.kept;
        for promotion in &outcome.sweep.promoted {
            if promotion.retired == node || promotion.survivor == node {
                node = promotion.kept;
            }
        }
        let after = copy.raw_positions();
        let [sx, sy] = after.xy(node)?;
        let shift_m = before
            .iter()
            .filter_map(|(s, [x, y, _])| {
                let [ax, ay] = after.xy(s)?;
                Some((ax - x).hypot(ay - y))
            })
            .fold((sx - mx).hypot(sy - my), f64::max);
        let hidden_station = self.stations.get(&report.archived)?;
        let collisions = outcome
            .identities
            .lettered
            .iter()
            .map(|letter| MergeCollision {
                line: letter.line,
                name: letter.from.clone(),
                suggestion: letter.clone(),
            })
            .collect();
        Some(MergeMove {
            stationary,
            moving_lines: self.piece_lines(moving),
            shift_m,
            depths_m,
            joined_depth_m: after.z(node)?,
            readings: copy.depth_readings_of(&after, node),
            candidates,
            kept: report.kept,
            default_keep: self.merge_default_keep(a, b),
            keep_forced_by: report.keep_forced_by,
            renamed_lines: self.renamed_lines(&copy, &[a, b], node),
            collisions,
            alternatives: copy.alternative_letters(&outcome.identities.lettered),
            hidden: (
                hidden_station.id,
                hidden_station.name.clone(),
                hidden_station.comment.clone(),
            ),
            unarchived: report.unarchived,
            comments_hidden: outcome.identities.hidden.clone(),
            repeats: copy
                .name_repeats()
                .difference(&self.name_repeats())
                .cloned()
                .collect(),
        })
    }

    /// Every name a line shows twice ([`NameRepeat`]), by line, name and
    /// Station IDs. Linear in the slots.
    pub fn name_repeats(&self) -> std::collections::BTreeSet<NameRepeat> {
        let native = super::NativeSlots::of(self);
        let topology = self.topology();
        let mut out = std::collections::BTreeSet::new();
        for (position, line) in self.lines().iter().enumerate() {
            let chain = topology.chain_at(position).unwrap_or(&[]);
            let mut own: std::collections::HashMap<&str, StationId> =
                std::collections::HashMap::new();
            let mut dotted: Vec<(StationId, bool)> = Vec::new();
            for &node in chain {
                if native.is_native(node, line.id) {
                    if let Some(station) = self.stations.get(&node) {
                        own.entry(station.name.as_str()).or_insert(node);
                    }
                } else {
                    let loop_end = native.index().is_archived(node);
                    dotted.push((native.index().shown(node), loop_end));
                }
            }
            for (shown, loop_end) in dotted {
                let Some(station) = self.stations.get(&shown) else {
                    continue;
                };
                if let Some(&own_id) = own.get(station.name.as_str()) {
                    if own_id != shown {
                        out.insert(NameRepeat {
                            line: line.id,
                            name: station.name.clone(),
                            own: own_id,
                            shown,
                            loop_end,
                        });
                    }
                }
            }
        }
        out
    }

    /// The dialog's view of one merge station.
    fn merge_candidate(&self, station: StationId) -> Option<MergeCandidate> {
        let s = self.stations.get(&station)?;
        let shown = self.shown(station);
        Some(MergeCandidate {
            station,
            name: s.name.clone(),
            comment: s.comment.clone(),
            shown_as: (shown != station).then_some(shown),
            home_line: self.home_line(station),
        })
    }

    /// The lines whose shown name at the merge changes from this state to
    /// `after`: every slot of a node showing either merged identity before,
    /// now on `node` (or still on its own node, for a closure subtree), and
    /// every line on which a slot shows another identity after (a promotion
    /// the merge's sweep makes, RS-FX1; a merge adds and removes no slot, so
    /// slots match by line and index).
    fn renamed_lines(&self, after: &Project, merged: &[StationId], node: StationId) -> Vec<LineId> {
        let topology = self.topology();
        let mut lines: Vec<(usize, LineId)> = Vec::new();
        for &station in merged {
            for showing in self.nodes_showing(self.shown(station)) {
                let now = if merged.contains(&showing) {
                    node
                } else {
                    showing
                };
                for &slot in topology.slots_of(showing) {
                    if self.shown_name(showing) != after.shown_name(now) {
                        if let Some(position) = topology.line_position(slot.line_id) {
                            lines.push((position, slot.line_id));
                        }
                    }
                }
            }
        }
        let (before_index, after_index) = (self.shown_index(), after.shown_index());
        for (position, line) in self.lines.iter().enumerate() {
            let Some(now) = after.line(line.id) else {
                continue;
            };
            let differs = line
                .stations()
                .zip(now.stations())
                .any(|(was, is)| before_index.shown(was) != after_index.shown(is));
            if differs {
                lines.push((position, line.id));
            }
        }
        lines.sort_unstable();
        lines.dedup();
        lines.into_iter().map(|(_, line)| line).collect()
    }

    /// The lines of `station`'s piece, ascending.
    pub fn piece_lines(&self, station: StationId) -> Vec<LineId> {
        let topology = self.topology();
        let mut lines = topology
            .component_of(station)
            .and_then(|c| topology.component(c))
            .map(|component| component.lines.clone())
            .unwrap_or_default();
        lines.sort_unstable();
        lines
    }

    /// Whether the pieces of `a` and `b` are joined through loop closures
    /// (any strategy), possibly via other pieces.
    pub(super) fn closure_linked(&self, a: StationId, b: StationId) -> bool {
        let topology = self.topology();
        let (Some(ca), Some(cb)) = (topology.component_of(a), topology.component_of(b)) else {
            return false;
        };
        let mut parent: Vec<usize> = (0..topology.components().len()).collect();
        for closure in &self.closures {
            if let (Some(x), Some(y)) = (
                topology.component_of(closure.a),
                topology.component_of(closure.b),
            ) {
                let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
                if rx != ry {
                    parent[rx.max(ry)] = rx.min(ry);
                }
            }
        }
        find(&mut parent, ca) == find(&mut parent, cb)
    }

    /// The low-level merge: replace `retired` by `survivor` everywhere (line
    /// starts, shot ends, hand-move keys, datums, closure ends), drop a
    /// placement on `retired`, and archive the retired station whole in the
    /// survivor (RS 4.1 step 2): one record with its id, name, comment,
    /// external id and native lines, after its own records, which move along
    /// under it (RS-FX4, `Station::archive_whole`). Nothing is minted and
    /// no comment is joined. A closure between
    /// the two stations is left with both ends on the survivor for the caller
    /// to remove (the sweep does, #738). No anchors, no sweep, no validation
    /// (callers run the pipeline). Refused, with nothing changed, when a line
    /// passes both stations (it would pass the survivor twice, invariant 3)
    /// or either station is missing. Returns the lines of the retired
    /// station's piece, ascending.
    pub(super) fn merge_ids(
        &mut self,
        survivor: StationId,
        retired: StationId,
    ) -> Result<Vec<LineId>, OpError> {
        let topology = self.topology_arc();
        let moving_lines = self.piece_lines(retired);
        let junction_lines = topology.lines_of(retired);
        for &line in &junction_lines {
            if topology.slot_on(survivor, line).is_some() {
                return Err(OpError::StationTwiceOnLine {
                    line,
                    station: survivor,
                });
            }
        }
        let (Some(_), Some(gone)) = (
            self.stations.get(&survivor),
            self.stations.get(&retired).cloned(),
        ) else {
            return Err(OpError::UnknownStation(retired));
        };
        // The retired station's native lines: the lines whose own station it
        // was (its records name the others).
        let native: Vec<LineId> = junction_lines
            .iter()
            .copied()
            .filter(|&line| gone.record_for(line).is_none())
            .collect();

        for &id in &junction_lines {
            if let Some(line) = self.line_mut(id) {
                if line.start == retired {
                    line.start = survivor;
                }
                for shot in &mut line.shots {
                    if shot.to == retired {
                        shot.to = survivor;
                    }
                }
                if let Some(hand) = line.hand_moves.remove(&retired) {
                    line.hand_moves.insert(survivor, hand);
                }
            }
        }
        for datum in &mut self.datums {
            if datum.station == retired {
                datum.station = survivor;
            }
        }
        for closure in &mut self.closures {
            if closure.a == retired {
                closure.a = survivor;
            }
            if closure.b == retired {
                closure.b = survivor;
            }
        }
        self.placements.retain(|p| p.station != retired);

        self.stations.remove(&retired);
        if let Some(keep) = self.stations.get_mut(&survivor) {
            keep.archive_whole(gone, native);
        }
        Ok(moving_lines)
    }
}
