//! Previews and queries that name stations, in today's DTOs (E5 section 2).
//!
//! Each mapping takes the project state the preview ran on (before the
//! edit), so every slot it names exists in the state the user sees. What
//! the station graph makes impossible is not on the wire: the delete
//! previews have no replacement merges, rewired closures or "rewire
//! skipped" flags (plan 9.3), and the split preview reports no promotion
//! that moves a half.

use crate::config::ClosureAdjustmentConfig;
use crate::{
    AdjustmentReport, AlongLineResult, ClosurePair, ClosurePreview, ConnectivityResult,
    DeleteShotImpact, DeleteShotRangeImpact, DistanceField, GpsCoordinates, LineId,
    LineRelationships, RulerEndpoint, SplitPreview, StationId, StationPair, StationRef,
    ZeroDatumPreview,
};

use super::super::{
    DeleteRangePreview, DeleteShotPreview, LoopClosure, MergeCheck, MergeDatumShift, OpError,
    Project, RulerPoint, SplitLinePreview, StationPositions,
};

impl Project {
    /// `preview_delete_shot`'s answer: what deleting the shot does to station
    /// relationships, from M1's [`DeleteShotPreview`].
    ///
    /// - `removed_merges`: a junction stays for its other lines; the slot
    ///   that leaves it is no longer merged with them (one pair per other
    ///   slot of the station).
    /// - `removed_closures`: the closures deleted with the station (at a
    ///   junction, the `Tie`s the line's slot takes with it, S13), the ones
    ///   the delete leaves across one shot (#737) and the ones the sweep
    ///   removes (both ends on one station).
    /// - `closure_to_merge`: the closures the sweep promotes to a merge
    ///   (today's "closure replaced by a merge", #8).
    pub fn delete_shot_impact(&self, preview: &DeleteShotPreview) -> DeleteShotImpact {
        let removed_merges = if preview.station_stays_on.is_empty() {
            Vec::new()
        } else {
            self.pairs_leaving(preview.removed_slot, preview.removed_station)
        };
        DeleteShotImpact {
            removed_station: preview.removed_slot,
            removed_merges,
            removed_closures: self.closure_pairs(
                preview
                    .removed_closures
                    .iter()
                    .chain(&preview.removed_across_one_shot)
                    .chain(&preview.swept_closures),
            ),
            closure_to_merge: self.promoted_pairs(&preview.promoted_closures),
            shift_blocked: preview.shift_blocked,
            shift_blocked_reasons: preview.shift_blocked_reasons.clone(),
            split_closure_to_merge: preview
                .split_weld
                .as_ref()
                .map(|weld| self.promoted_pairs(&weld.promoted))
                .unwrap_or_default(),
            split_max_move_m: preview.split_weld.as_ref().map_or(0.0, |w| w.max_move_m),
        }
    }

    /// `preview_delete_shot_range`'s answer, from M1's [`DeleteRangePreview`]
    /// (the relationship lists as [`Self::delete_shot_impact`]; the datums
    /// and comments the split deletes on their slot on the line).
    pub fn delete_range_impact(&self, preview: &DeleteRangePreview) -> DeleteShotRangeImpact {
        let topology = self.topology();
        let removed_merges = preview
            .removed_slots
            .iter()
            .filter(|(_, station)| preview.junctions_kept.contains(station))
            .flat_map(|&(slot, station)| self.pairs_leaving(slot, station))
            .collect();
        let on_line = |station: StationId| topology.slot_on(station, preview.line);
        DeleteShotRangeImpact {
            line_id: preview.line,
            start_pos: preview.start as u32,
            end_pos: preview.end as u32,
            kind: preview.kind,
            removed_stations: preview
                .removed_slots
                .iter()
                .map(|(slot, _)| *slot)
                .collect(),
            removed_merges,
            removed_closures: self.closure_pairs(
                preview
                    .removed_closures
                    .iter()
                    .chain(&preview.swept_closures),
            ),
            closure_to_merge: self.promoted_pairs(&preview.promoted_closures),
            shift_blocked: preview.shift_blocked,
            shift_blocked_reasons: preview.shift_blocked_reasons.clone(),
            removed_datums: preview
                .removed_datums
                .iter()
                .filter_map(|datum| on_line(datum.station))
                .collect(),
            removed_comments: preview
                .removed_comments
                .iter()
                .filter_map(|(station, _)| on_line(*station))
                .collect(),
            closures_removed_across_one_shot: preview
                .closures_removed_across_one_shot
                .as_ref()
                .map(|closures| self.closure_pairs(closures)),
        }
    }

    /// `split_line_preview`'s answer, from M1's [`SplitLinePreview`]. The
    /// halves stay joined at their shared station, so no closure becomes a
    /// merge to hold them and nothing moves; only the orphan sweep the split
    /// ends in can report anything (never in a valid model).
    pub fn split_preview_wire(&self, preview: &SplitLinePreview) -> SplitPreview {
        SplitPreview {
            closures_to_merges: preview.promoted_closures.len() as u32,
            moved_side: None,
            moved_m: 0.0,
            removed_closures: self.closure_pairs(&preview.swept_closures),
            removed_already_one_point: Vec::new(),
        }
    }

    /// `check_merge_connectivity` on two slots, with today's refusals and
    /// words, from M2's [`Project::merge_check`]. The closure preview reads
    /// each end's depth on the picked slot's line, as today.
    pub fn check_merge_connectivity_wire(
        &self,
        a: StationRef,
        b: StationRef,
    ) -> Result<ConnectivityResult, String> {
        let station_a = self
            .station_at(a)
            .ok_or_else(|| "Station A does not exist.".to_string())?;
        let station_b = self
            .station_at(b)
            .ok_or_else(|| "Station B does not exist.".to_string())?;
        let check = self
            .merge_check(station_a, station_b)
            .map_err(|error| match error {
                OpError::AlreadyMerged => "These stations are already merged.".to_string(),
                other => other.to_string(),
            })?;
        let mut result = self.connectivity_wire(&check);
        if result.preview.is_some() {
            result.preview = self
                .closure_metrics_via((station_a, Some(a.line_id)), (station_b, Some(b.line_id)));
        }
        if result.can_merge {
            result.merge_preview = self.merge_preview_wire(a, station_a, b, station_b);
        }
        Ok(result)
    }

    /// The Merge dialog's preview of slots `a` and `b` (demo issue I-4, RS
    /// 4.1 and 7): the geometry (the piece that moves, how far, onto which
    /// station, the depths), the two stations it may keep, the default keep,
    /// and what each keep the dialog offers does to names. The geometry is
    /// the same for either keep; it is read on the default keep's run.
    fn merge_preview_wire(
        &self,
        a: StationRef,
        station_a: StationId,
        b: StationRef,
        station_b: StationId,
    ) -> Option<crate::MergePreview> {
        let default_keep = self.merge_default_keep(station_a, station_b);
        let m = self.merge_preview(station_a, station_b, default_keep)?;
        let keep_forced_by = m.keep_forced_by.or_else(|| {
            self.forced_merge_keep(station_a, station_b)
                .map(|(closure, _)| closure)
        });
        let offered: Vec<StationId> = if keep_forced_by.is_some() {
            vec![m.kept]
        } else {
            vec![station_a, station_b]
        };
        let keeps = offered
            .into_iter()
            .filter_map(|keep| {
                if keep == m.kept {
                    Some(self.merge_keep_wire(&m))
                } else {
                    let other = self.merge_preview(station_a, station_b, keep)?;
                    Some(self.merge_keep_wire(&other))
                }
            })
            .collect();
        let [a_m, b_m] = m.depths_m;
        let depths =
            ((a_m - b_m).abs() > crate::JUNCTION_DEPTH_SPREAD_M).then(|| crate::MergeDepths {
                a_m,
                b_m,
                joined_m: m.joined_depth_m,
                readings: self.merge_readings_in_pick_order(a, station_a, b, &m.readings),
            });
        Some(crate::MergePreview {
            moving_line_ids: m.moving_lines,
            shift_m: m.shift_m,
            joins: if m.stationary == station_a { a } else { b },
            depths,
            junctions: [(a, station_a), (b, station_b)]
                .into_iter()
                .filter_map(|(slot, station)| self.merge_junction(slot, station))
                .collect(),
            candidates: [(a, station_a), (b, station_b)]
                .into_iter()
                .filter_map(|(slot, station)| self.keep_candidate(slot, station))
                .collect(),
            default_keep,
            keep_forced_by,
            keeps,
        })
    }

    /// The merge's depth readings (`MergeMove::readings`, by slot) by line,
    /// in the order the dialog lists them: A's line, the other lines of A's
    /// piece, B's line, the rest; each group in the order the mean sums them.
    fn merge_readings_in_pick_order(
        &self,
        a: StationRef,
        station_a: StationId,
        b: StationRef,
        readings: &[(StationRef, f64)],
    ) -> Vec<crate::MergeDepthReading> {
        let a_piece = self.piece_lines(station_a);
        let rank = |line: LineId| {
            if line == a.line_id {
                0
            } else if a_piece.contains(&line) {
                1
            } else if line == b.line_id {
                2
            } else {
                3
            }
        };
        let mut out: Vec<crate::MergeDepthReading> = readings
            .iter()
            .map(|&(slot, depth_m)| crate::MergeDepthReading {
                line_id: slot.line_id,
                depth_m,
            })
            .collect();
        out.sort_by_key(|reading| rank(reading.line_id));
        out
    }

    /// The picked slot's station when it is already a junction, with every
    /// line through it, the slot's line first.
    fn merge_junction(&self, slot: StationRef, station: StationId) -> Option<crate::MergeJunction> {
        let mut line_ids = vec![slot.line_id];
        for line in self.topology().lines_of(station) {
            if !line_ids.contains(&line) {
                line_ids.push(line);
            }
        }
        (line_ids.len() > 1).then_some(crate::MergeJunction {
            station: slot,
            line_ids,
        })
    }

    /// Today's `ConnectivityResult` of M2's [`MergeCheck`].
    pub fn connectivity_wire(&self, check: &MergeCheck) -> ConnectivityResult {
        ConnectivityResult {
            connected: check.connected,
            preview: check.preview.clone(),
            can_merge: check.can_merge,
            reason: check.reason.clone(),
            closure_refusal: check.closure_refusal.clone(),
            datum_shift: check
                .datum_shift
                .as_ref()
                .map(|shift| self.merge_datum_shift_wire(shift)),
            merge_preview: None,
        }
    }

    /// Today's `MergeDatumShift` of M2's: the datum named by its slot.
    pub fn merge_datum_shift_wire(&self, shift: &MergeDatumShift) -> crate::MergeDatumShift {
        let datum_station = self
            .datums
            .get(shift.datum)
            .and_then(|datum| self.datum_slot(datum))
            .or_else(|| self.topology().first_slot(shift.datum_station))
            .unwrap_or_else(|| StationRef::new(0, 0));
        crate::MergeDatumShift {
            moving_line_ids: shift.moving_lines.clone(),
            shift_m: shift.shift_m,
            datum_station,
        }
    }

    /// The closure dialog's figures for two picked slots (each end's depth
    /// on its slot's line), or why there are none.
    pub fn closure_preview_at(
        &self,
        a: StationRef,
        b: StationRef,
    ) -> Result<ClosurePreview, OpError> {
        let (sa, sb) = self.resolve_slot_pair(a, b)?;
        self.closure_metrics_via((sa.station, Some(sa.line)), (sb.station, Some(sb.line)))
            .ok_or(OpError::UnknownSlot(a))
    }

    /// `preview_zero_datum` on a slot (M2's preview, attached via the slot's
    /// line). `None` for a slot that names no station.
    pub fn preview_zero_datum_at(
        &self,
        slot: StationRef,
        gps: &GpsCoordinates,
    ) -> Option<ZeroDatumPreview> {
        let target = self.resolve_slot(slot).ok()?;
        self.preview_zero_datum(target.station, Some(target.line), gps)
    }

    /// `get_line_relationships` (plan 9.3): what deleting `line` takes with
    /// it, for the Delete Line confirm.
    ///
    /// - `merges`: the wire merge pairs with a slot on the line;
    /// - `closures`: the closures with an end on a station only this line
    ///   passes (deleted with it; a closure on a junction stays);
    /// - `has_line_offset`: the line holds its piece's `Placement` station
    ///   alone and that placement is not at the origin (where the piece sits
    ///   is a user decision the delete discards);
    /// - `station_offset_count`: the line's hand moves;
    /// - `zero_datum_count`: datums on stations only this line passes.
    pub fn line_relationships(&self, line: LineId) -> LineRelationships {
        let topology = self.topology();
        let only_here = |station: StationId| {
            let lines = topology.lines_of(station);
            lines.len() == 1 && lines.first() == Some(&line)
        };
        let raw = self.raw_positions();
        let aligned = self.closure_alignment(&raw);
        let closures = self
            .loop_closures_wire(&raw, &aligned)
            .into_iter()
            .filter(|wire| {
                self.closures
                    .iter()
                    .find(|c| c.id == wire.id)
                    .is_some_and(|c| only_here(c.a) || only_here(c.b))
            })
            .collect();
        LineRelationships {
            merges: self
                .station_merges_wire()
                .into_iter()
                .filter(|m| m.station_a.line_id == line || m.station_b.line_id == line)
                .collect(),
            closures,
            has_line_offset: self
                .placements
                .iter()
                .any(|p| only_here(p.station) && (p.x != 0.0 || p.y != 0.0)),
            station_offset_count: self.line(line).map_or(0, |l| l.hand_moves.len()),
            zero_datum_count: self.datums.iter().filter(|d| only_here(d.station)).count(),
        }
    }

    /// `get_station_distances_from`: the per-slot distance field from the
    /// slot `from`, or from the entrances when `None`. A `from` that names
    /// no station leaves every slot unreachable, as today.
    pub fn distance_field_wire(
        &self,
        positions: &impl StationPositions,
        from: Option<StationRef>,
        include_reference: bool,
    ) -> DistanceField {
        let root = match from {
            None => None,
            Some(slot) => match self.station_at(slot) {
                Some(station) => Some(station),
                None => return self.unreachable_field(),
            },
        };
        self.distance_field(positions, root, include_reference)
            .to_distance_field(self)
    }

    /// `get_along_line_distance`: the ruler between two wire endpoints; no
    /// distance and no path when either names nothing, as today.
    pub fn along_line_wire(
        &self,
        positions: &impl StationPositions,
        a: &RulerEndpoint,
        b: &RulerEndpoint,
        include_reference: bool,
    ) -> AlongLineResult {
        match (
            RulerPoint::from_endpoint(self, a),
            RulerPoint::from_endpoint(self, b),
        ) {
            (Some(a), Some(b)) => self
                .along_line_between(positions, a, b, include_reference)
                .to_wire(),
            _ => AlongLineResult {
                along_line: None,
                path_shots: Vec::new(),
                path_stations: Vec::new(),
            },
        }
    }

    /// `get_adjustment_report` with the Computed View solved here.
    pub fn adjustment_report_wire(&self, config: &ClosureAdjustmentConfig) -> AdjustmentReport {
        let adjusted = self.adjusted_positions(config);
        self.adjustment_report(Some(&adjusted))
    }

    /// Every slot of the station at `slot` but `slot`, paired with it.
    fn pairs_leaving(&self, slot: StationRef, station: StationId) -> Vec<StationPair> {
        self.topology()
            .slots_of(station)
            .iter()
            .filter(|&&other| other != slot && other.line_id != slot.line_id)
            .map(|&other| StationPair {
                station_a: slot,
                station_b: other,
            })
            .collect()
    }

    fn closure_pairs<'c>(
        &self,
        closures: impl IntoIterator<Item = &'c LoopClosure>,
    ) -> Vec<ClosurePair> {
        closures
            .into_iter()
            .filter_map(|closure| self.closure_pair(closure))
            .collect()
    }

    fn promoted_pairs(&self, promoted: &[super::super::Promotion]) -> Vec<StationPair> {
        promoted
            .iter()
            .filter_map(|p| self.closure_pair(&p.closure))
            .map(|pair| StationPair {
                station_a: pair.station_a,
                station_b: pair.station_b,
            })
            .collect()
    }

    /// The field with every slot unreachable.
    fn unreachable_field(&self) -> DistanceField {
        self.survey_distances_from(&[], false)
            .to_distance_field(self)
    }
}
