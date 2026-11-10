use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::{ClosureStrategy, LoopClosure, SectionId, StationAnchor, StationKey, StationRef};

use super::Project;

/// Severity of a structural-integrity violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationSeverity {
    /// A dangling reference that will cause runtime panics or geometry
    /// corruption if used (e.g. a merge pointing at a deleted station).
    Critical,
    /// A stale/orphaned entry that is ignored by lookups but signals an
    /// inconsistent project (e.g. an offset keyed by a deleted section).
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
    /// Reason a stored [`StationKey`] is invalid - it names a section that no
    /// longer exists, or its anchor no longer resolves to a real station (its
    /// anchoring shot was deleted) - or `None` if it points at a real station.
    /// With stable identity this catches a *dangling* anchor (the integrity win
    /// over positional refs, which could only ever be out-of-range, never
    /// silently mispointed at the wrong in-range station).
    fn station_ref_violation(&self, key: &StationKey) -> Option<String> {
        match self.get_section(key.section_id) {
            None => Some(format!("references missing section {}", key.section_id)),
            Some(_) => {
                if self.resolve_key(key).is_some() {
                    None
                } else {
                    Some(format!(
                        "anchor no longer resolves in section {} (its anchoring shot was deleted)",
                        key.section_id
                    ))
                }
            }
        }
    }

    /// Audit every cross-reference in the project and return all structural
    /// violations (an empty vec means the project is structurally sound). This
    /// does not mutate the project and does not look at shot geometry - it only
    /// checks that merges, loop closures, zero datums, and the offset/comment
    /// maps point at stations and sections that actually exist.
    ///
    /// Used to (a) block commands that would introduce corruption and (b) warn
    /// about - and repair - corruption already present in a loaded project.
    pub fn check_structural_integrity(&self) -> Vec<IntegrityViolation> {
        let mut violations = Vec::new();

        // Non-finite (NaN/inf) shot geometry - loaded from an older/corrupt file
        // or an unguarded import. It silently poisons coordinates (NaN passes the
        // tolerance comparisons in the alignment checks), so surface it.
        for section in &self.sections {
            if let Some(pos) = section.shots.iter().position(|s| !s.all_finite()) {
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Critical,
                    category: "non_finite_geometry".to_string(),
                    message: format!(
                        "Section {} has a shot with a non-finite (NaN/inf) value (at index {pos})",
                        section.id
                    ),
                });
            }
        }

        // One pass over every shot UUID, for both kinds of repeat.
        //
        // Within a section: two shots sharing a UUID make AfterShot anchors
        // mis-resolve - resolve_key takes the FIRST match via position(), so a
        // merge/closure/datum pointing at the second station silently snaps to
        // the first, geometrically wrong with no other signal.
        //
        // Across sections: shot UUIDs are unique across the whole project.
        // Every door that brings foreign ids in renames the newcomers
        // (`reassign_repeated_shot_uuids`) and the load heals a file saved
        // before that (`make_shot_uuids_project_unique`), so a repeat means a
        // door forgot its rename; the integrity guard then rolls it back instead
        // of persisting two shots that undo history, ignored warnings and the
        // exports cannot tell apart. Reported once per id.
        //
        // Both Critical. Empty UUIDs are "no identity yet" (ensure_shot_uuids
        // fills them) so they don't count.
        let mut holders: HashMap<&str, SectionId> = HashMap::new();
        let mut reported_across: HashSet<&str> = HashSet::new();
        for section in &self.sections {
            let mut seen: HashSet<&str> = HashSet::new();
            for u in section.shots.iter().filter_map(|shot| shot.uuid.as_deref()) {
                if u.is_empty() {
                    continue;
                }
                if !seen.insert(u) {
                    violations.push(IntegrityViolation {
                        severity: ViolationSeverity::Critical,
                        category: "duplicate_shot_uuid".to_string(),
                        message: format!("Section {} has two shots sharing UUID {u}", section.id),
                    });
                    continue;
                }
                match holders.get(u) {
                    None => {
                        holders.insert(u, section.id);
                    }
                    Some(&other) if reported_across.insert(u) => {
                        violations.push(IntegrityViolation {
                            severity: ViolationSeverity::Critical,
                            category: "duplicate_shot_uuid_across_sections".to_string(),
                            message: format!(
                                "Sections {other} and {} both hold shot UUID {u}",
                                section.id
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

        // section_offsets is keyed by SectionId (not a StationRef), so it isn't
        // covered by for_each_station_ref - check it for a missing section here.
        for section_id in self.section_offsets.keys() {
            if self.get_section(*section_id).is_none() {
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Warning,
                    category: "orphan_section_offset".to_string(),
                    message: format!("Section offset for missing section {section_id}"),
                });
            }
        }

        // Two or more zero datums in one merge-connected component over-constrain
        // the rigid group (two GPS anchors), so the offset cascade tears a merge
        // edge apart. The interactive guards (set_zero_datum / merge_stations)
        // prevent this on clean state, but a file written by an older build or a
        // closure-to-merge conversion can still arrive in this state - surface it
        // instead of silently mis-solving the geometry. Warning, not Critical: the
        // project still opens and the user resolves it by removing a datum.
        for component in self.over_anchored_merge_components() {
            let datums = component
                .iter()
                .map(|sr| format!("{}:{}", sr.section_id, sr.station_index))
                .collect::<Vec<_>>()
                .join(", ");
            violations.push(IntegrityViolation {
                severity: ViolationSeverity::Warning,
                category: "multiple_datums_per_merge_component".to_string(),
                message: format!(
                    "Merge-connected group has {} zero datums ({datums}); remove all but one to keep the group's geometry consistent",
                    component.len()
                ),
            });
        }

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
    /// or its section is gone. These are precisely the keys that would otherwise
    /// be collapsed to a phantom `(section, 0)` ref on save and can collide on the
    /// `(section_id, station_index)` PRIMARY KEY. The save path blocks on this so
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
        // the per-section UUID snapshot below reflects the de-duplicated set and
        // AfterShot anchors that pointed at the kept shot stay valid.
        self.ensure_unique_shot_uuids();
        // Then across sections, rewriting the station keys of every renamed shot.
        self.make_shot_uuids_project_unique();

        // Snapshot, per section, the station count and the set of shot UUIDs, so
        // the retain closures can validate a stored `StationKey` without
        // borrowing `self` while mutating its fields. Owns the UUID strings so no
        // borrow of `self.sections` outlives into the mutating retains.
        let sect_info: HashMap<SectionId, (u32, HashSet<String>)> = self
            .sections
            .iter()
            .map(|s| {
                let uuids = s.shots.iter().filter_map(|sh| sh.uuid.clone()).collect();
                (s.id, (s.shots.len() as u32, uuids))
            })
            .collect();
        let key_valid = |key: &StationKey| -> bool {
            match sect_info.get(&key.section_id) {
                None => false,
                Some((count, uuids)) => match &key.anchor {
                    StationAnchor::SectionStart => true,
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
        self.section_offsets
            .retain(|section_id, _| sect_info.contains_key(section_id));
        self.station_offsets.retain(|key, _| key_valid(key));
        self.station_comments.retain(|key, _| key_valid(key));
        self.station_names.retain(|key, _| key_valid(key));

        self.touch();

        violations
    }

    /// World-space gap between two stored endpoints: the pair resolved against
    /// the CURRENT structure plus their horizontal distance in metres, or `None`
    /// when either endpoint dangles, has no computable position, or sits on
    /// non-finite (NaN/inf) geometry.
    ///
    /// The single measurement used by the merge and closure alignment checks, by
    /// [`Self::demote_double_anchoring_merges`] and by the closure-alignment
    /// predicate [`Self::closure_alignment_stamp`], so they cannot disagree about
    /// how far apart a pair is - including about a pair they cannot measure
    /// at all. A NaN gap compares false against the tolerance in either direction,
    /// so an unguarded distance would read as "aligned" to the two checks and as
    /// "not torn" to the demotion by accident rather than by decision. `None` makes
    /// it a decision: broken geometry means "cannot measure - hands off", so the
    /// checks stay quiet (`non_finite_geometry` is the violation that describes
    /// the real problem) and the demotion never deletes a merge on the strength of
    /// a distance that does not exist. Testing the final distance covers a
    /// non-finite value in either input coordinate.
    pub(super) fn endpoint_gap(
        &self,
        key_a: &StationKey,
        key_b: &StationKey,
        local: &HashMap<SectionId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
    ) -> Option<(StationRef, StationRef, f64)> {
        let a = self.resolve_key(key_a)?;
        let b = self.resolve_key(key_b)?;
        let (ax, ay) = self.station_world_xy(a, local, station_offsets)?;
        let (bx, by) = self.station_world_xy(b, local, station_offsets)?;
        let dist = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
        dist.is_finite().then_some((a, b, dist))
    }

    /// Cross-section merges whose two stations have drifted apart - the
    /// references are valid but the geometry is wrong (a stale section offset
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
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();
        let mut violations = Vec::new();
        for merge in &self.station_merges {
            // Same-section merges are physically meaningless (test-only).
            if merge.station_a.section_id == merge.station_b.section_id {
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
                        "Merged stations {}:{} and {}:{} are {:.2} m apart but should be the same point",
                        a.section_id, a.station_index, b.section_id, b.station_index, dist
                    ),
                });
            }
        }
        violations
    }

    /// Index of the first merge that a past bug promoted out of a loop closure:
    /// a rigid cross-section merge that is visibly torn open and, with every OTHER
    /// torn merge also set aside, still separates two independently
    /// zero-datum-anchored merge components. `None` when there is none.
    ///
    /// The candidate is judged against the graph with the WHOLE torn set removed,
    /// not just itself. Two torn merges bridging the same two anchored halves
    /// otherwise shield each other: each looks redundant because the other still
    /// joins the halves, so neither is demoted and the project stays permanently
    /// torn. Excluding the set asks the question that matters - "is this pair of
    /// anchored halves welded together only by merges the solver cannot satisfy?"
    ///
    /// Both conservative-safety properties survive that widening, and they are the
    /// point of the design:
    /// - A torn merge whose halves remain connected through a NON-torn merge is
    ///   skipped. That merge is redundant, not over-constraining: the halves stay
    ///   one rigid component either way, so dropping it heals nothing.
    /// - A torn merge whose far half carries no datum is skipped. That is an
    ///   ordinary single-anchor group with drift, which `resolve_offsets` can and
    ///   does heal by moving the unanchored half; demoting it would destroy a real
    ///   connection.
    ///
    /// The anchoring test is the same one `convert_orphan_closures_to_merges` asks
    /// before it refuses to CREATE such a merge, so demotion is that guard applied
    /// retroactively to a merge that predates it.
    ///
    /// `local` / `station_offsets` come from the caller because they are invariant
    /// across the whole demotion scan (see [`Self::demote_double_anchoring_merges`]).
    fn first_double_anchoring_merge(
        &self,
        local: &HashMap<SectionId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
    ) -> Option<usize> {
        // The torn set: every cross-section merge the offset solver has visibly
        // failed to satisfy. An aligned merge is doing its job, and an unmeasurable
        // one (`endpoint_gap` = None, broken geometry) is not evidence of anything.
        let mut torn: Vec<usize> = Vec::new();
        for (index, merge) in self.station_merges.iter().enumerate() {
            // Same-section merges are physically meaningless (test-only).
            if merge.station_a.section_id == merge.station_b.section_id {
                continue;
            }
            let Some((_, _, gap)) =
                self.endpoint_gap(&merge.station_a, &merge.station_b, local, station_offsets)
            else {
                continue;
            };
            if gap < Self::MERGE_ALIGNMENT_TOLERANCE_M {
                continue;
            }
            torn.push(index);
        }
        if torn.is_empty() {
            return None;
        }

        // ONE components computation for the whole scan, with the whole torn set
        // excluded - both the separation test and the anchoring test below read it,
        // so they cannot be answered against different graphs.
        let skip: HashSet<usize> = torn.iter().copied().collect();
        let components = self.merge_components_excluding(&skip);
        // Ascending index order, so which candidate a damaged file yields first is
        // deterministic. The caller demotes it and re-scans; the torn set shrinks
        // as merges become closures, so parallel torn bridges between the same
        // anchored halves are demoted one per pass until the tear is healed.
        torn.into_iter().find(|&index| {
            let merge = &self.station_merges[index];
            let (a_section, b_section) = (merge.station_a.section_id, merge.station_b.section_id);
            !components.same_component(a_section, b_section)
                && components.is_anchored(a_section)
                && components.is_anchored(b_section)
        })
    }

    /// Demote every over-constrained merge back to the loop closure it should
    /// have stayed, returning how many were demoted.
    ///
    /// A merge bridging two independently zero-datum-anchored components cannot
    /// be satisfied: [`Self::resolve_offsets`] anchors each half at its own datum,
    /// so the merge stays torn open and the project reports `merge_misaligned`
    /// plus `multiple_datums_per_merge_component` on every check, forever. Re-
    /// cascading offsets cannot heal it - only removing the rigid constraint can.
    /// The soft edge (a loop closure, `is_aligned: false`) records the same
    /// coincidence without over-constraining the geometry, which is what the pair
    /// was before a past bug promoted it.
    ///
    /// EXPLICIT REPAIR ONLY - deliberately not called on the load path: it deletes
    /// a user-visible merge, so it belongs behind the undoable Repair action, not
    /// behind opening a file.
    pub fn demote_double_anchoring_merges(&mut self) -> usize {
        // Legacy files can carry closures with id 0 and a stale counter; normalize
        // once so the demoted closures get unique ids (same convention as
        // `create_loop_closure` / `create_loop_closures_batch`).
        self.normalize_next_closure_id();

        // Loop-invariant across every pass, so both tables are built once and
        // reused: a demotion only removes a merge and adds (or softens) a loop
        // closure. It never touches shots, section offsets, or station offsets,
        // which are the only inputs to the per-section local-coordinate walk and
        // the resolved station-offset map.
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();

        let mut demoted = 0;
        // Re-scan from scratch after each demotion rather than collecting indices
        // up front: removing a merge shifts every later index AND changes the
        // components the remaining candidates are judged against. A damaged file
        // holds 0-2 of these, so the repeated scan costs nothing.
        while let Some(index) = self.first_double_anchoring_merge(&local, &station_offsets) {
            // Pull the merge out BEFORE measuring: merges are zero-cost
            // pathfinding edges, so measuring with it still present would ride
            // the very edge being demoted and record path_length = 0 (and so
            // error_percentage = 0) on the replacement closure. Removing first
            // measures the same graph the closure will actually live in - the
            // creation path (`create_loop_closure`) rejects merged pairs, so a
            // user-created closure is always measured merge-free too. On any
            // failure the merge goes back at its original index, leaving the
            // project untouched.
            let merge = self.station_merges.remove(index);
            // The scan already resolved both endpoints and measured their
            // positions, so these cannot fail here - but restore rather than
            // drop a merge we could not replace with an equivalent closure.
            let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) else {
                self.station_merges.insert(index, merge);
                break;
            };
            // A closure already sitting directly on this pair is itself a
            // zero-cost pathfinding edge between the very stations being
            // measured - left in, it would zero `path_length` exactly like the
            // merge did. Pull the direct matches out before measuring and
            // re-insert them (restamped) after. Closures linking the pair only
            // indirectly still shorten the walk, but normal closure creation
            // measures under the same rule, so the numbers stay comparable.
            //
            // `key_is_ref` borrows `&self`, so the matching indices are
            // collected in an immutable pass first. Endpoint order is compared
            // both ways, mirroring `are_stations_in_closure`. Removal runs
            // highest-index-first so the remaining indices stay valid;
            // re-insertion runs lowest-first, restoring the original order.
            let match_indices: Vec<usize> = self
                .loop_closures
                .iter()
                .enumerate()
                .filter(|(_, lc)| {
                    (self.key_is_ref(&lc.station_a, a) && self.key_is_ref(&lc.station_b, b))
                        || (self.key_is_ref(&lc.station_a, b) && self.key_is_ref(&lc.station_b, a))
                })
                .map(|(i, _)| i)
                .collect();
            let mut direct_closures: Vec<(usize, LoopClosure)> = match_indices
                .iter()
                .rev()
                .map(|&i| (i, self.loop_closures.remove(i)))
                .collect();
            direct_closures.reverse();

            let metrics = match self.compute_closure_metrics(a, b) {
                Ok(metrics) => metrics,
                Err(_) => {
                    for (i, lc) in direct_closures {
                        self.loop_closures.insert(i, lc);
                    }
                    self.station_merges.insert(index, merge);
                    break;
                }
            };
            tracing::warn!(
                station_a = %a,
                station_b = %b,
                misclosure_m = metrics.misclosure_distance,
                purpose = "remove an unsatisfiable rigid constraint",
                "Demoted a merge bridging two zero-datum-anchored groups back to a loop closure"
            );
            if !direct_closures.is_empty() {
                // A closure over this pair already exists, so no new one is
                // invented - but the existing one is not automatically the soft
                // edge the demotion needs. If it is `is_aligned: true` the offset
                // solver treats it as a RIGID edge, which re-imposes the exact
                // over-constraint just removed and leaves the project torn as
                // before. Its stored metrics are stale too: they were measured
                // while the merge (and the closure itself) welded the pair
                // together. Soften and restamp EVERY match from the measurement
                // above: one un-softened duplicate would re-impose the
                // constraint on its own.
                for (i, mut closure) in direct_closures {
                    // Keep the existing id: this is the same user-visible closure,
                    // corrected, not a replacement.
                    closure.is_aligned = false;
                    closure.path_length = metrics.path_length;
                    closure.misclosure_distance = metrics.misclosure_distance;
                    closure.error_percentage = metrics.error_percentage;
                    closure.horizontal_misclosure = metrics.horizontal_misclosure;
                    closure.vertical_misclosure = metrics.vertical_misclosure;
                    self.loop_closures.insert(i, closure);
                }
            } else {
                let id = self.next_closure_id;
                self.next_closure_id += 1;
                self.loop_closures.push(LoopClosure {
                    id,
                    station_a: merge.station_a,
                    station_b: merge.station_b,
                    path_length: metrics.path_length,
                    misclosure_distance: metrics.misclosure_distance,
                    error_percentage: metrics.error_percentage,
                    horizontal_misclosure: metrics.horizontal_misclosure,
                    vertical_misclosure: metrics.vertical_misclosure,
                    strategy: ClosureStrategy::Batch,
                    // Never aligned: the endpoints are metres apart and each half
                    // is pinned to its own datum. An aligned closure would rigidify
                    // the same over-constraint the demotion just removed.
                    is_aligned: false,
                });
            }
            demoted += 1;
        }

        if demoted > 0 {
            self.touch();
        }
        demoted
    }

    /// Cross-section *aligned* loop closures whose two stations have drifted
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
    /// closure whose strategy is NOT [`ClosureStrategy::Move`] carrying
    /// `is_aligned = true`. Only a build that predates 0.5.0 could write one - the
    /// destructive "Adjust Closures" action baked its correction into the shots
    /// and marked every closure it adjusted aligned - and the re-measure
    /// deliberately leaves those alone (see [`Self::remeasure_closure_alignment`]).
    /// Their shots were rewritten to make the claim true, so a torn one here is a
    /// real finding about a real file rather than a stamp the app got wrong.
    pub fn check_closure_alignment(&self) -> Vec<IntegrityViolation> {
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();
        let mut violations = Vec::new();
        for closure in &self.loop_closures {
            if !closure.is_aligned || closure.station_a.section_id == closure.station_b.section_id {
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
                        "Loop closure {} ({}:{} ↔ {}:{}) is marked closed but its ends are {:.2} m apart - recompute or remove it",
                        closure.id,
                        a.section_id,
                        a.station_index,
                        b.section_id,
                        b.station_index,
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
            if closure.station_a.section_id == closure.station_b.section_id {
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
                if self.merge_would_double_anchor(a.section_id, b.section_id) {
                    continue;
                }
                violations.push(IntegrityViolation {
                    severity: ViolationSeverity::Warning,
                    category: "orphan_closure".to_string(),
                    message: format!(
                        "Loop closure {} ({}:{} ↔ {}:{}) is the only connection between its sections - it should be a merge, not a loop",
                        closure.id,
                        a.section_id,
                        a.station_index,
                        b.section_id,
                        b.station_index
                    ),
                });
            }
        }
        violations
    }
}
