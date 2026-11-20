//! Files from older builds whose merge-connected groups were welded across two
//! datums by a past bug (#464, #741 D6).
//!
//! An older build could leave a RIGID merge between two pieces that each held
//! their own zero datum, its two endpoints metres (or kilometres) apart: each
//! half was pinned to its datum, so the merge stayed torn open. Since #741 the
//! later datum of a merge-connected group is a GPS fix, which never holds its
//! line in raw geometry, so the offset re-solve on open would heal such a
//! tear by translating the later piece off its GPS by the whole tear, with no
//! undo and no word to the user.
//!
//! Opening a file must not move anything. So before the load re-solve,
//! [`Project::demote_double_anchoring_merges`] turns each such merge back into
//! the loop closure it should have stayed: nothing moves, each piece keeps its
//! own datum as its placement datum, and the misclosure is recorded on the
//! closure, where the Closures Computed View and the closure warnings see it.
//! A user who wants the pieces joined merges them explicitly, which moves the
//! later piece onto the earlier one, says so, and can be undone.
//!
//! A legacy group with two datums whose merges are NOT torn needs nothing: the
//! re-solve moves nothing, the later datum derives as a GPS fix, and its gap
//! is reported like any other.

use std::collections::{HashMap, HashSet};

use crate::models::{ClosureStrategy, LineId, LoopClosure, StationRef};

use super::Project;

impl Project {
    /// Index of the first merge that a past bug promoted out of a loop closure:
    /// a rigid cross-line merge that is visibly torn open and, with every OTHER
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
        local: &HashMap<LineId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
    ) -> Option<usize> {
        // The torn set: every cross-line merge the offset solver has visibly
        // failed to satisfy. An aligned merge is doing its job, and an unmeasurable
        // one (`endpoint_gap` = None, broken geometry) is not evidence of anything.
        let mut torn: Vec<usize> = Vec::new();
        for (index, merge) in self.station_merges.iter().enumerate() {
            // Same-line merges are physically meaningless (test-only).
            if merge.station_a.line_id == merge.station_b.line_id {
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
            let (a_line, b_line) = (merge.station_a.line_id, merge.station_b.line_id);
            !components.same_component(a_line, b_line)
                && components.is_anchored(a_line)
                && components.is_anchored(b_line)
        })
    }

    /// Demote every over-constrained merge back to the loop closure it should
    /// have stayed, returning how many were demoted.
    ///
    /// A torn merge bridging two independently zero-datum-anchored halves is
    /// the damage the module doc describes. Left a merge, the load re-solve
    /// would heal it by moving the later half off its GPS (its datum is a GPS
    /// fix once the halves are one group). The soft edge (a loop closure,
    /// `is_aligned: false`) records the same coincidence without moving
    /// anything, which is what the pair was before a past bug promoted it.
    ///
    /// Run on the load path, BEFORE the offset re-solve
    /// ([`Self::redrive_offsets_after_load_reporting`]): it changes the
    /// topology (a merge becomes a closure between the same two stations) but
    /// never a coordinate, which is the one thing opening a file must not do.
    pub fn demote_double_anchoring_merges(&mut self) -> usize {
        // Legacy files can carry closures with id 0 and a stale counter; normalize
        // once so the demoted closures get unique ids (same convention as
        // `create_loop_closure` / `create_loop_closures_batch`).
        self.normalize_next_closure_id();

        // Loop-invariant across every pass, so both tables are built once and
        // reused: a demotion only removes a merge and adds (or softens) a loop
        // closure. It never touches shots, line offsets, or station offsets,
        // which are the only inputs to the per-line local-coordinate walk and
        // the resolved station-offset map.
        let local = self.line_local_coords();
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

    /// Label every line with its merge-only component in the HYPOTHETICAL
    /// graph that drops the merges at the indices in `skip`, and record which of
    /// those components an anchor still reaches.
    ///
    /// ONE O(V + E) pass answers both questions
    /// [`Self::demote_double_anchoring_merges`] asks of every candidate - "are
    /// these two halves still connected without the torn merges?" and "is each
    /// half independently anchored?" - so the separation test and the anchoring
    /// test can never be answered against different graphs, and neither costs a
    /// BFS per candidate.
    ///
    /// Only datums that still RESOLVE anchor a component, mirroring
    /// [`Self::component_datum_refs`]: a corrupt project can be opened with a
    /// dangling zero datum (its anchoring shot was deleted) so the user can repair
    /// it, and a dangling datum is not a real anchor - counting it would make a
    /// half look anchored when nothing holds it in place.
    ///
    /// Walks `station_merges` into a throwaway local adjacency rather than going
    /// through the memoized [`Self::get_merged_line_ids`]: the shared memo
    /// describes the merges that actually exist, and answering a "what if these
    /// merges were gone" question must never mutate the project or that memo.
    pub(crate) fn merge_components_excluding(
        &self,
        skip: &HashSet<usize>,
    ) -> ExcludedMergeComponents {
        let mut adjacency: HashMap<LineId, Vec<LineId>> = HashMap::new();
        for (i, m) in self.station_merges.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            adjacency
                .entry(m.station_a.line_id)
                .or_default()
                .push(m.station_b.line_id);
            adjacency
                .entry(m.station_b.line_id)
                .or_default()
                .push(m.station_a.line_id);
        }

        // Roots: every line in the project, then any line a surviving merge
        // names that the project does not hold (a dangling endpoint in a project
        // opened for repair). Nothing is left unlabelled, so a line that
        // participates in no surviving merge is its own singleton component and
        // both accessors answer for it.
        let mut component_of: HashMap<LineId, usize> = HashMap::new();
        let mut next_component = 0usize;
        let roots: Vec<LineId> = self
            .lines
            .iter()
            .map(|s| s.id)
            .chain(adjacency.keys().copied())
            .collect();
        for root in roots {
            if component_of.contains_key(&root) {
                continue;
            }
            let component = next_component;
            next_component += 1;
            let mut queue = vec![root];
            while let Some(current) = queue.pop() {
                if component_of.insert(current, component).is_some() {
                    continue;
                }
                if let Some(neighbours) = adjacency.get(&current) {
                    for &n in neighbours {
                        if !component_of.contains_key(&n) {
                            queue.push(n);
                        }
                    }
                }
            }
        }

        let mut anchored: HashSet<usize> = HashSet::new();
        for zd in &self.zero_datums {
            let Some(sr) = self.resolve_key(&zd.station_ref) else {
                continue;
            };
            if let Some(&component) = component_of.get(&sr.line_id) {
                anchored.insert(component);
            }
        }

        ExcludedMergeComponents {
            component_of,
            anchored,
        }
    }
}

/// Merge-only connected components of every line in a project, computed as if
/// some set of merges did not exist, plus which components hold at least one
/// resolved zero datum. Built by [`Project::merge_components_excluding`].
pub(crate) struct ExcludedMergeComponents {
    /// line id -> the index of its component in the hypothetical graph.
    component_of: HashMap<LineId, usize>,
    /// Component indices reached by at least one resolved zero datum.
    anchored: HashSet<usize>,
}

impl ExcludedMergeComponents {
    /// True when `a` and `b` are still merge-connected with the excluded merges
    /// gone. A line this graph does not know is connected to nothing.
    pub(crate) fn same_component(&self, a: LineId, b: LineId) -> bool {
        match (self.component_of.get(&a), self.component_of.get(&b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }

    /// True when `line`'s component holds at least one resolved zero datum.
    pub(crate) fn is_anchored(&self, line: LineId) -> bool {
        self.component_of
            .get(&line)
            .is_some_and(|c| self.anchored.contains(c))
    }
}
