use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::{LineId, StationAnchor, StationKey, StationRef};

use super::Project;

/// Severity of a structural-integrity violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationSeverity {
    /// A dangling reference that will cause runtime panics or geometry
    /// corruption if used (e.g. a merge pointing at a deleted station).
    Critical,
    /// A stale/orphaned entry that is ignored by lookups but signals an
    /// inconsistent project (e.g. an offset keyed by a deleted line).
    Warning,
}

/// A single structural-integrity violation found by
/// [`Project::check_structural_integrity`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityViolation {
    pub severity: ViolationSeverity,
    /// Stable machine-readable category (e.g. `"dangling_merge_ref"`).
    pub category: String,
    /// Human-readable description.
    pub message: String,
}

impl Project {
    /// Reason a stored [`StationKey`] is invalid - it names a line that no
    /// longer exists, or its anchor no longer resolves to a real station (its
    /// anchoring shot was deleted) - or `None` if it points at a real station.
    /// With stable identity this catches a *dangling* anchor (the integrity win
    /// over positional refs, which could only ever be out-of-range, never
    /// silently mispointed at the wrong in-range station).
    fn station_ref_violation(&self, key: &StationKey) -> Option<String> {
        match self.get_line(key.line_id) {
            None => Some("references a missing line".to_string()),
            Some(_) => {
                if self.resolve_key(key).is_some() {
                    None
                } else {
                    Some(format!(
                        "anchor no longer resolves in line {} (its anchoring shot was deleted)",
                        self.quoted_line(key.line_id)
                    ))
                }
            }
        }
    }

    /// A line as integrity messages name it: `"Main Line"`, quoted, never by
    /// its numeric id.
    fn quoted_line(&self, line_id: LineId) -> String {
        format!("\"{}\"", self.line_label(line_id).unwrap_or_default())
    }

    /// Audit every cross-reference in the project and return all structural
    /// violations (an empty vec means the project is structurally sound). This
    /// does not mutate the project and does not look at shot geometry - it only
    /// checks that merges, loop closures, zero datums, and the offset/comment
    /// maps point at stations and lines that actually exist.
    ///
    /// Used to (a) block commands that would introduce corruption and (b) warn
    /// about - and repair - corruption already present in a loaded project.
    pub fn check_structural_integrity(&self) -> Vec<IntegrityViolation> {
        let mut violations = Vec::new();

        // Non-finite (NaN/inf) shot geometry - loaded from an older/corrupt file
        // or an unguarded import. It silently poisons coordinates (NaN passes the
        // tolerance comparisons in the alignment checks), so surface it.
        for line in &self.lines {
            if let Some(pos) = line.shots.iter().position(|s| !s.all_finite()) {
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Critical,
                    category: "non_finite_geometry".to_string(),
                    message: match self.shot_label_at(line.id, pos) {
                        Some(shot) => {
                            format!("Shot {shot} has a non-finite (NaN/inf) value")
                        }
                        None => format!(
                            "Line {} has a shot with a non-finite (NaN/inf) value",
                            self.quoted_line(line.id)
                        ),
                    },
                });
            }
        }

        // One pass over every shot UUID, for both kinds of repeat.
        //
        // Within a line: two shots sharing a UUID make AfterShot anchors
        // mis-resolve - resolve_key takes the FIRST match via position(), so a
        // merge/closure/datum pointing at the second station silently snaps to
        // the first, geometrically wrong with no other signal.
        //
        // Across lines: shot UUIDs are unique across the whole project.
        // Every door that brings foreign ids in renames the newcomers
        // (`reassign_repeated_shot_uuids`) and the load heals a file saved
        // before that (`make_shot_uuids_project_unique`). A repeat therefore
        // means a door forgot its rename, or an undo of a Repair put one back.
        // On an otherwise sound project the integrity guard rolls the first case
        // back rather than persist two shots that undo history, ignored warnings
        // and the exports cannot tell apart (the guard refuses only NEWLY
        // introduced corruption). Reported once per id.
        //
        // Both Critical. Empty UUIDs are "no identity yet" (ensure_shot_uuids
        // fills them) so they don't count.
        let mut holders: HashMap<&str, LineId> = HashMap::new();
        let mut reported_across: HashSet<&str> = HashSet::new();
        for line in &self.lines {
            let mut seen: HashSet<&str> = HashSet::new();
            for u in line.shots.iter().filter_map(|shot| shot.uuid.as_deref()) {
                if u.is_empty() {
                    continue;
                }
                if !seen.insert(u) {
                    violations.push(IntegrityViolation {
                        severity: ViolationSeverity::Critical,
                        category: "duplicate_shot_uuid".to_string(),
                        message: format!(
                            "Line {} has two shots sharing UUID {u}",
                            self.quoted_line(line.id)
                        ),
                    });
                    continue;
                }
                match holders.get(u) {
                    None => {
                        holders.insert(u, line.id);
                    }
                    Some(&other) if reported_across.insert(u) => {
                        violations.push(IntegrityViolation {
                            severity: ViolationSeverity::Critical,
                            category: "duplicate_shot_uuid_across_lines".to_string(),
                            message: format!(
                                "Lines {} and {} both hold shot UUID {u}",
                                self.quoted_line(other),
                                self.quoted_line(line.id)
                            ),
                        });
                    }
                    Some(_) => {}
                }
            }
        }

        // Validate every StationRef-bearing collection through the single
        // traversal, so a newly-added ref-bearing collection is checked by
        // construction. Dangling merge/closure/datum refs are Critical (they
        // corrupt connectivity); orphaned offset/comment keys are Warning (they
        // are inert until their station is recreated).
        self.for_each_station_ref(|label, key| {
            let Some(reason) = self.station_ref_violation(&key) else {
                return;
            };
            let (severity, category) = match label {
                "merge" => (ViolationSeverity::Critical, "dangling_merge_ref"),
                "loop closure" => (ViolationSeverity::Critical, "dangling_closure_ref"),
                "zero datum" => (ViolationSeverity::Critical, "dangling_zero_datum_ref"),
                "station offset" => (ViolationSeverity::Warning, "orphan_station_offset"),
                "station id" => (ViolationSeverity::Warning, "orphan_station_id"),
                _ => (ViolationSeverity::Warning, "orphan_station_comment"),
            };
            let mut subject = label.to_string();
            if let Some(first) = subject.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            violations.push(IntegrityViolation {
                severity,
                category: category.to_string(),
                message: format!("{subject} {reason}"),
            });
        });

        // line_offsets is keyed by LineId (not a StationRef), so it isn't
        // covered by for_each_station_ref - check it for a missing line here.
        for line_id in self.line_offsets.keys() {
            if self.get_line(*line_id).is_none() {
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Warning,
                    category: "orphan_line_offset".to_string(),
                    message: "A line offset names a missing line".to_string(),
                });
            }
        }

        // Several zero datums in one merge-connected component are valid since
        // #741: the first places the group and the rest are GPS fixes, which never
        // move raw geometry. There is no over-anchoring left to report; a fix whose
        // GPS disagrees with the survey is a project warning (`datum_gps_gap`), not
        // a structural violation.

        violations
    }

    /// True if the project has any [`ViolationSeverity::Critical`] violation -
    /// the cheap gate used to block corrupting commands.
    pub fn has_critical_integrity_violations(&self) -> bool {
        self.check_structural_integrity()
            .iter()
            .any(|v| v.severity == ViolationSeverity::Critical)
    }

    /// True if ANY persisted cross-reference (a merge/closure/datum endpoint, or
    /// an offset/comment key) no longer resolves - its anchoring shot was deleted
    /// or its line is gone. These are precisely the keys that would otherwise
    /// be collapsed to a phantom `(line, 0)` ref on save and can collide on the
    /// `(line_id, station_index)` PRIMARY KEY. The save path blocks on this so
    /// corruption is never persisted; `repair_structural_integrity` drops exactly
    /// these keys, so repair unblocks the save. Includes Warning-severity orphan
    /// offsets/comments (they cause the same PK collision), so this is broader
    /// than [`has_critical_integrity_violations`].
    pub fn has_unresolvable_cross_references(&self) -> bool {
        let mut dangling = false;
        self.for_each_station_ref(|_label, key| {
            if self.station_ref_violation(&key).is_some() {
                dangling = true;
            }
        });
        dangling
    }

    /// Drop every structurally-invalid cross-reference (dangling merges, loop
    /// closures, zero datums) and orphaned offset/comment entries, returning the
    /// violations that were repaired (empty = nothing to do). Valid data is left
    /// untouched. Used by the load-time "warn and offer repair" path.
    pub fn repair_structural_integrity(&mut self) -> Vec<IntegrityViolation> {
        let violations = self.check_structural_integrity();
        if violations.is_empty() {
            return violations;
        }

        // Re-mint duplicate shot UUIDs first (keeping the first occurrence), so
        // the per-line UUID snapshot below reflects the de-duplicated set and
        // AfterShot anchors that pointed at the kept shot stay valid.
        self.ensure_unique_shot_uuids();
        // Then across lines, rewriting the station keys of every renamed shot.
        self.make_shot_uuids_project_unique();

        // Snapshot, per line, the station count and the set of shot UUIDs, so
        // the retain closures can validate a stored `StationKey` without
        // borrowing `self` while mutating its fields. Owns the UUID strings so no
        // borrow of `self.lines` outlives into the mutating retains.
        let sect_info: HashMap<LineId, (u32, HashSet<String>)> = self
            .lines
            .iter()
            .map(|s| {
                let uuids = s.shots.iter().filter_map(|sh| sh.uuid.clone()).collect();
                (s.id, (s.shots.len() as u32, uuids))
            })
            .collect();
        let key_valid = |key: &StationKey| -> bool {
            match sect_info.get(&key.line_id) {
                None => false,
                Some((count, uuids)) => match &key.anchor {
                    StationAnchor::LineStart => true,
                    StationAnchor::AfterShot(u) => uuids.contains(u),
                    StationAnchor::Index(i) => *i <= *count,
                },
            }
        };

        self.station_merges
            .retain(|m| key_valid(&m.station_a) && key_valid(&m.station_b));
        self.loop_closures
            .retain(|c| key_valid(&c.station_a) && key_valid(&c.station_b));
        self.zero_datums.retain(|z| key_valid(&z.station_ref));
        self.line_offsets
            .retain(|line_id, _| sect_info.contains_key(line_id));
        self.station_offsets.retain(|key, _| key_valid(key));
        self.station_comments.retain(|key, _| key_valid(key));
        self.station_names.retain(|key, _| key_valid(key));
        self.station_ids.retain(|key, _| key_valid(key));

        self.touch();

        violations
    }

    /// World-space gap between two stored endpoints: the pair resolved against
    /// the CURRENT structure plus their horizontal distance in metres, or `None`
    /// when either endpoint dangles, has no computable position, or sits on
    /// non-finite (NaN/inf) geometry.
    ///
    /// The single measurement used by the merge and closure alignment checks and
    /// by the closure-alignment predicate [`Self::closure_alignment_stamp`], so
    /// they cannot disagree about how far apart a pair is - including about a
    /// pair they cannot measure at all. A NaN gap compares false against the
    /// tolerance in either direction, so an unguarded distance would read as
    /// "aligned" to the checks by accident rather than by decision. `None` makes
    /// it a decision: broken geometry means "cannot measure - hands off", so the
    /// checks stay quiet (`non_finite_geometry` is the violation that describes
    /// the real problem). Testing the final distance covers a
    /// non-finite value in either input coordinate.
    pub(super) fn endpoint_gap(
        &self,
        key_a: &StationKey,
        key_b: &StationKey,
        local: &HashMap<LineId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
    ) -> Option<(StationRef, StationRef, f64)> {
        let a = self.resolve_key(key_a)?;
        let b = self.resolve_key(key_b)?;
        let (ax, ay) = self.station_world_xy(a, local, station_offsets)?;
        let (bx, by) = self.station_world_xy(b, local, station_offsets)?;
        let dist = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
        dist.is_finite().then_some((a, b, dist))
    }

    /// Cross-line merges whose two stations have drifted apart - the
    /// references are valid but the geometry is wrong (a stale line offset
    /// from a missed cascade). Distinct from [`Self::check_structural_integrity`];
    /// repaired by re-cascading offsets via [`Self::realign_merges`] rather than
    /// by dropping data.
    pub fn check_merge_alignment(&self) -> Vec<IntegrityViolation> {
        self.merge_alignment_beyond(Self::MERGE_ALIGNMENT_TOLERANCE_M)
    }

    /// [`Self::check_merge_alignment`] with the threshold made explicit, so the
    /// SELF-HEAL can gate on the tighter [`Self::MERGE_ALIGNMENT_INVARIANT_M`]
    /// while the user-facing report stays at the 5 cm survey tolerance. The two
    /// numbers answer different questions - "is this worth telling the user
    /// about" and "is this a state the code considers valid" - and #413 finding
    /// 20 is what happens when one is used for both.
    pub fn merge_alignment_beyond(&self, threshold: f64) -> Vec<IntegrityViolation> {
        let local = self.line_local_coords();
        let station_offsets = self.resolved_station_offsets();
        let mut violations = Vec::new();
        for merge in &self.station_merges {
            // Same-line merges are physically meaningless (test-only).
            if merge.station_a.line_id == merge.station_b.line_id {
                continue;
            }
            let Some((a, b, dist)) =
                self.endpoint_gap(&merge.station_a, &merge.station_b, &local, &station_offsets)
            else {
                continue;
            };
            if dist >= threshold {
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Critical,
                    category: "merge_misaligned".to_string(),
                    message: format!(
                        "Merged stations {} and {} are {:.2} m apart but should be the same point",
                        self.station_text(a),
                        self.station_text(b),
                        dist
                    ),
                });
            }
        }
        violations
    }

    /// Cross-line *aligned* loop closures whose two stations have drifted
    /// apart. An aligned closure (`is_aligned == true`) asserts its loop is
    /// closed - its endpoints coincide - and the offset solver enforces it as a
    /// rigid edge, so a stale claim silently rigidifies geometry that is metres
    /// out. The merge-only [`Self::check_merge_alignment`] cannot see this;
    /// surface it so the user can recompute or remove the loop closure (forcing a
    /// loop misclosure to zero is over-constrained, so this is detect-and-report,
    /// not auto-heal).
    ///
    /// # A TRIPWIRE, and what can still trip it (#469)
    ///
    /// This used to be the only thing standing between the user and a permanent
    /// lie: `is_aligned` was ASSERTED at creation and never set back to false, so
    /// any later edit could tear an aligned closure and nothing in the app could
    /// heal it. It is MEASURED now - [`Self::closure_alignment_stamp`] is the one
    /// predicate, re-derived after every geometry change by
    /// [`Self::remeasure_closure_alignment`] - and a measured stamp cannot trip
    /// this check by construction, because the stamp's gate
    /// ([`Self::MERGE_ALIGNMENT_INVARIANT_M`]) is at or below the reporting
    /// threshold this one uses and that ordering is enforced.
    ///
    /// What is left is exactly one population, and it is why this check stays: a
    /// closure whose strategy is NOT [`crate::ClosureStrategy::Move`] carrying
    /// `is_aligned = true`. Only a build that predates 0.5.0 could write one - the
    /// destructive "Adjust Closures" action baked its correction into the shots
    /// and marked every closure it adjusted aligned - and the re-measure
    /// deliberately leaves those alone (see [`Self::remeasure_closure_alignment`]).
    /// Their shots were rewritten to make the claim true, so a torn one here is a
    /// real finding about a real file rather than a stamp the app got wrong.
    pub fn check_closure_alignment(&self) -> Vec<IntegrityViolation> {
        let local = self.line_local_coords();
        let station_offsets = self.resolved_station_offsets();
        let mut violations = Vec::new();
        for closure in &self.loop_closures {
            if !closure.is_aligned || closure.station_a.line_id == closure.station_b.line_id {
                continue;
            }
            let Some((a, b, dist)) = self.endpoint_gap(
                &closure.station_a,
                &closure.station_b,
                &local,
                &station_offsets,
            ) else {
                continue;
            };
            if dist >= Self::MERGE_ALIGNMENT_TOLERANCE_M {
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Critical,
                    category: "closure_misaligned".to_string(),
                    message: format!(
                        "Loop closure {} ↔ {} is marked closed but its ends are {:.2} m apart - recompute or remove it",
                        self.station_text(a),
                        self.station_text(b),
                        dist
                    ),
                });
            }
        }
        violations
    }

    /// Closures that are not true loops: their two endpoints are connected ONLY
    /// through the closure itself (a sole bridge), which is a merge, not a loop.
    /// [`Self::convert_orphan_closures_to_merges`] normally fixes these the
    /// moment they appear (on delete/split and on load), so this is a safety-net
    /// detector for any path that bypassed that (e.g. a repair or an import
    /// edit) - it lets the integrity banner surface the condition.
    pub fn check_orphan_closures(&self) -> Vec<IntegrityViolation> {
        let mut violations = Vec::new();
        for closure in &self.loop_closures {
            if closure.station_a.line_id == closure.station_b.line_id {
                continue;
            }
            let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) else {
                continue;
            };
            if !self.are_stations_connected_excluding_shot(a, b, None, false) {
                // A sole-bridge closure between two independently zero-datum-anchored
                // groups is intentionally KEPT as a closure (converting it to a rigid
                // merge would double-anchor one merge component and tear it - see
                // `convert_orphan_closures_to_merges`). Don't flag it as an
                // orphan-that-should-be-a-merge: that warning would be misleading and
                // Repair could never clear it, since the converter correctly refuses
                // to merge it. The same `merge_would_double_anchor` predicate keeps the
                // checker and the converter in agreement.
                if self.merge_would_double_anchor(a.line_id, b.line_id) {
                    continue;
                }
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Warning,
                    category: "orphan_closure".to_string(),
                    message: format!(
                        "Loop closure {} ↔ {} is the only connection between its lines - it should be a merge, not a loop",
                        self.station_text(a),
                        self.station_text(b)
                    ),
                });
            }
        }
        violations
    }
}
