//! Merge stations (plan 5.1, direction 4.2) and the Merge dialog's check.
//!
//! A merge makes two stations of different pieces one station. The station
//! on the side that stays put survives (keeps its id, name and comment); the
//! other one is retired into the survivor's aliases and replaced by it
//! everywhere: line starts, shot ends, datums, closure ends, hand-move keys.
//! The moving piece then follows by derivation (the anchor algorithm lays it
//! out from the stationary piece through the shared station), its placement
//! is dropped, and when both pieces were placed the later placement datum is
//! a GPS fix from then on (its correction cleared). Datums and closure ends
//! keep their `via` hints, so a later detach can hand them back. Every line
//! keeps the name it showed for the two stations (the retired station's
//! lines through an alias naming each, plan 5.3 per line), so nothing is
//! renamed. The orphan-closure sweep runs afterwards, which also removes
//! every closure between the two stations (#738).
//!
//! Refused: two ends that are one station already; two stations already
//! connected (one piece, or tied by an aligned closure), so the user is routed
//! to Create Loop Closure, as today.
//!
//! The mutation itself ([`Project::merge_ids`]: one station replaced by
//! another everywhere) is the one core every merge goes through: Merge
//! Stations, the sweep's closure promotions (`sweep.rs`) and Combine Lines'
//! join (`edit/combine/apply.rs`, with the dialog's comment pick). Each caller
//! decides the direction and settles anchors itself.

use crate::{ClosurePreview, CombinePick, LineId, StationId, StationRef, MAX_STATION_ID};

use super::ops::{OpError, OpOutcome, Sweep};
use super::topology::find;
use super::{EditContext, Project, StationAlias};

/// How a merge settles the two stations' comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommentRule {
    /// Plan 5.1: both kept, the retired one appended after a blank line, when
    /// both are non-empty and different; otherwise whichever exists.
    Join,
    /// Combine Lines: the dialog's pick between the survivor's (kept) and the
    /// retired one's (other), or both joined by a newline (today's
    /// `pick_text`). Identical texts are kept once.
    Pick(CombinePick),
}

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReport {
    /// The station that stays (its id, name and comment are kept).
    pub survivor: StationId,
    /// The station replaced by the survivor; now one of its aliases.
    pub retired: StationId,
    /// The lines of the piece that moved, ascending.
    pub moving_lines: Vec<LineId>,
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
    /// The station that stays put (the survivor).
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
    /// Merge station `a` with station `b` (plan 5.1). The piece of `a` moves
    /// onto `b`, unless `a`'s piece is placed and `b`'s is not, or both are
    /// placed and `a`'s placement datum comes first (plan 4.2).
    pub fn merge_stations(
        &mut self,
        a: StationId,
        b: StationId,
    ) -> Result<OpOutcome<MergeReport>, OpError> {
        self.merge_refusal(a, b)?;
        let (stationary, moving) = self.merge_direction(a, b);
        self.run_op(Sweep::Run, |p| {
            let report = p.merge_ids(stationary, moving, CommentRule::Join)?;
            let ctx = EditContext::merge(stationary, moving).with_renamed(moving, stationary);
            Ok((report, ctx))
        })
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

    /// What merging `a` with `b` would do (demo issue I-4): the merge run on a
    /// copy, its raw layout compared with this one's (as the Delete Shot
    /// preview measures a split's weld). `None` when the merge is refused
    /// (the copy's `merge_stations` checks [`Self::merge_refusal`]).
    /// Read-only.
    pub fn merge_preview(&self, a: StationId, b: StationId) -> Option<MergeMove> {
        let (stationary, moving) = self.merge_direction(a, b);
        let before = self.raw_positions();
        let depths_m = [before.z(a)?, before.z(b)?];
        let [mx, my] = before.xy(moving)?;
        let mut copy = self.clone();
        copy.merge_stations(a, b).ok()?;
        let after = copy.raw_positions();
        let [sx, sy] = after.xy(stationary)?;
        let shift_m = before
            .iter()
            .filter_map(|(s, [x, y, _])| {
                let [ax, ay] = after.xy(s)?;
                Some((ax - x).hypot(ay - y))
            })
            .fold((sx - mx).hypot(sy - my), f64::max);
        Some(MergeMove {
            stationary,
            moving_lines: self.piece_lines(moving),
            shift_m,
            depths_m,
            joined_depth_m: after.z(stationary)?,
            readings: copy.depth_readings_of(&after, stationary),
        })
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
    /// placement on `retired`, fold the retired station into the survivor
    /// (aliases, the comment by `comment`, its `external` id when the survivor
    /// has none). Each line that showed the retired station by its own name
    /// keeps showing that name (an alias naming the line), so no line's names
    /// change. A closure between the two stations is left with both ends on
    /// the survivor for the caller to remove (the sweep does, #738). No
    /// anchors, no sweep, no validation (callers run the pipeline). Refused,
    /// with nothing changed, when a line passes both stations (it would pass
    /// the survivor twice, invariant 3) or either station is missing.
    pub(super) fn merge_ids(
        &mut self,
        survivor: StationId,
        retired: StationId,
        comment: CommentRule,
    ) -> Result<MergeReport, OpError> {
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
        // The lines that show the retired station by its own name (none of
        // its aliases names them), list order; ids for all but the first.
        let native: Vec<LineId> = junction_lines
            .iter()
            .copied()
            .filter(|&line| !gone.aliases.iter().any(|a| a.line == Some(line)))
            .collect();
        let extra = native.len().saturating_sub(1);
        let free = MAX_STATION_ID.saturating_sub(self.counters.next_station_id);
        if (free as usize) < extra {
            return Err(OpError::Build(super::BuildError::StationIdsExhausted));
        }
        let mut minted = Vec::with_capacity(extra);
        for _ in 0..extra {
            minted.push(self.mint_station_id()?);
        }

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

        // Every line that showed the retired station by its own name keeps
        // that name for the junction: each gets an alias with it (plan 5.3
        // per line, I-3). The first such line (list order) gets the retired
        // id, which a Detach gives back; another gets a fresh id, as Split
        // Line Here gives its second half. A retired station no line showed
        // by its own name is kept as a plain alias (no line).
        let mut alias_ids = Vec::with_capacity(native.len());
        for (index, &line) in native.iter().enumerate() {
            let id = if index == 0 {
                retired
            } else {
                minted[index - 1]
            };
            alias_ids.push(StationAlias {
                id,
                name: Some(gone.name.clone()),
                line: Some(line),
            });
        }
        if alias_ids.is_empty() {
            alias_ids.push(StationAlias {
                id: retired,
                name: Some(gone.name.clone()),
                line: None,
            });
        }
        self.stations.remove(&retired);
        if let Some(keep) = self.stations.get_mut(&survivor) {
            keep.aliases.extend(gone.aliases.iter().cloned());
            keep.aliases.extend(alias_ids);
            keep.comment = merged_comment(keep.comment.take(), gone.comment.clone(), comment);
            if keep.external.is_none() {
                keep.external.clone_from(&gone.external);
            }
        }
        // Every line shows what it showed before, so no name repeats on a
        // line (invariant 8) and nothing is renamed.
        Ok(MergeReport {
            survivor,
            retired,
            moving_lines,
        })
    }
}

/// The survivor's comment after a merge by `rule` (plan 5.1 for
/// [`CommentRule::Join`], today's `pick_text` for a pick). Identical texts
/// are kept once; with one side empty, whichever exists.
fn merged_comment(
    survivor: Option<String>,
    retired: Option<String>,
    rule: CommentRule,
) -> Option<String> {
    let keep = survivor.filter(|c| !c.is_empty());
    let gone = retired.filter(|c| !c.is_empty());
    match (keep, gone) {
        (Some(k), Some(g)) if k == g => Some(k),
        (Some(k), Some(g)) => Some(match rule {
            CommentRule::Join => format!("{k}\n\n{g}"),
            CommentRule::Pick(CombinePick::Kept) => k,
            CommentRule::Pick(CombinePick::Other) => g,
            CommentRule::Pick(CombinePick::Both) => format!("{k}\n{g}"),
        }),
        (k, g) => k.or(g),
    }
}
