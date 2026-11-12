use chrono::Utc;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
// `error!` is only reached inside the `#[cfg(debug_assertions)]` block in
// `debug_assert_merge_alignment`, so gate the import to match (else it's an
// unused import under `--release`).
#[cfg(debug_assertions)]
use tracing::error;

use crate::{ClosureStrategy, LoopClosure, SectionId, StationKey, StationMerge, StationRef};

use super::union_find::UnionFind;
use super::Project;

/// The two section-level relations the orphan-closure sweep asks about, built
/// once per sweep and extended as closures are promoted to merges (#597):
///
/// - `connected`: stations joined through shots and RESOLVABLE merges, closures
///   excluded - the orphan predicate. Every station of a section is joined to
///   the next by its shots, so section-level components answer it exactly.
/// - `merge_component`: sections joined by merges by their STORED section ids,
///   resolvable or not - the same graph `MergeTopology::adjacency` gives
///   `merge_would_double_anchor`, with each component flagged when a resolvable
///   zero datum sits in it. A stored id naming no live section still gets a
///   slot (`merge_slot`), since the adjacency walk passes through it.
pub(crate) struct StationComponents {
    section_slot: HashMap<SectionId, usize>,
    merge_slot: HashMap<SectionId, usize>,
    connected: UnionFind,
    merge_component: UnionFind,
    anchored: Vec<bool>,
}

impl StationComponents {
    fn new(project: &Project) -> Self {
        let section_slot: HashMap<SectionId, usize> = project
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        let mut merge_slot = section_slot.clone();
        for merge in &project.station_merges {
            for id in [merge.station_a.section_id, merge.station_b.section_id] {
                let next = merge_slot.len();
                merge_slot.entry(id).or_insert(next);
            }
        }
        let mut components = Self {
            connected: UnionFind::new(section_slot.len()),
            merge_component: UnionFind::new(merge_slot.len()),
            anchored: vec![false; merge_slot.len()],
            section_slot,
            merge_slot,
        };
        for merge in &project.station_merges {
            if project.resolve_key(&merge.station_a).is_some()
                && project.resolve_key(&merge.station_b).is_some()
            {
                components.union_connected(merge.station_a.section_id, merge.station_b.section_id);
            }
            components.union_merge(merge.station_a.section_id, merge.station_b.section_id);
        }
        for datum in &project.zero_datums {
            if let Some(station) = project.resolve_key(&datum.station_ref) {
                if let Some(&i) = components.merge_slot.get(&station.section_id) {
                    let root = components.merge_component.find(i);
                    components.anchored[root] = true;
                }
            }
        }
        components
    }

    /// Whether two RESOLVED stations are connected through shots and resolvable
    /// merges.
    fn connected(&mut self, a: StationRef, b: StationRef) -> bool {
        match (
            self.section_slot.get(&a.section_id),
            self.section_slot.get(&b.section_id),
        ) {
            (Some(&a), Some(&b)) => self.connected.find(a) == self.connected.find(b),
            _ => false,
        }
    }

    /// `merge_would_double_anchor`, answered from the components.
    fn would_double_anchor(&mut self, a: SectionId, b: SectionId) -> bool {
        let (Some(&a), Some(&b)) = (self.merge_slot.get(&a), self.merge_slot.get(&b)) else {
            return false;
        };
        let (ra, rb) = (self.merge_component.find(a), self.merge_component.find(b));
        ra != rb && self.anchored[ra] && self.anchored[rb]
    }

    /// Record a merge just added between two resolved stations.
    fn join(&mut self, a: StationRef, b: StationRef) {
        self.union_connected(a.section_id, b.section_id);
        self.union_merge(a.section_id, b.section_id);
    }

    fn union_connected(&mut self, a: SectionId, b: SectionId) {
        if let (Some(&a), Some(&b)) = (self.section_slot.get(&a), self.section_slot.get(&b)) {
            self.connected.union(a, b);
        }
    }

    fn union_merge(&mut self, a: SectionId, b: SectionId) {
        if let (Some(&a), Some(&b)) = (self.merge_slot.get(&a), self.merge_slot.get(&b)) {
            let (ra, rb) = (self.merge_component.find(a), self.merge_component.find(b));
            if ra != rb {
                let anchored = self.anchored[ra] || self.anchored[rb];
                self.merge_component.union(ra, rb);
                let root = self.merge_component.find(ra);
                self.anchored[root] = anchored;
            }
        }
    }
}

/// A rigid merge a command MINTED out of a coincidence CLAIM - a loop closure
/// promoted because it stopped being a loop, or a deletion's broken-loop
/// conversion.
///
/// The two are not the same statement. A loop closure RECORDS a misclosure; a
/// merge ASSERTS that its two endpoints are the same point, and
/// [`Project::debug_assert_merge_alignment`] holds the app to it. So a promotion
/// owes the coincidence it declares, and pays it the way
/// [`Project::merge_stations`] pays it interactively: by translating the movable
/// side's whole rigid group onto the anchored one. Nothing else can - the offset
/// cascade that follows a promotion is structurally unable to close the gap
/// whenever both ends are authoritative to it (issue #464).
///
/// Planned by [`Project::plan_merge_promotion`], applied by
/// [`Project::settle_minted_merges`]. Two steps rather than one because the
/// command that mints usually goes on changing geometry afterwards - a shot
/// removal re-places its own section's stations, and the gap-close compensation
/// slides the whole section - so a delta computed at mint time is stale before
/// anything asserts on it. What must be captured AT MINT TIME is the SET: it is
/// the moving side's merge group taken BEFORE the new edge fuses the two groups
/// into one, which is not recoverable once the merge exists. The DELTA is
/// measured at settle time, off final geometry.
#[derive(Debug, Clone)]
pub(crate) struct MintedMerge {
    /// The moving side's merge group, snapshotted before the merge existed.
    /// Empty when nothing may move (see [`Project::plan_merge_promotion`]).
    movable: Vec<SectionId>,
    /// The endpoint that stays put.
    anchor: StationKey,
    /// The endpoint that has to reach it.
    moving: StationKey,
}

/// One beyond-invariant merge, as the fixpoint fingerprint records it: the two
/// endpoints as they currently resolve, and the EXACT gap between them.
///
/// The gap is kept as raw bits rather than as an `f64` so the fingerprint can be
/// compared for equality without a float comparison, and so that a gap differing
/// by a single ULP - the smallest change any edit can make to it - misses.
type ResidualGap = ((SectionId, u32), (SectionId, u32), u64);

/// A merge residual the offset solver has already run on and failed to reduce -
/// see [`Project::realign_merges`], which is the only producer and consumer.
///
/// Two parts, because two different things can turn an irreducible residual into
/// a reducible one:
///
/// * `structure` hashes every input to solvability that is NOT geometry - the
///   section set, the merge edges, the closure edges (id, endpoints, aligned
///   flag, strategy) and the zero-datum anchors. Dropping one of two datums from
///   an over-anchored component makes the gap healable without moving a single
///   station, so a fingerprint blind to structure would keep skipping the heal
///   that has just become possible;
/// * `residual` is the beyond-invariant merge set itself, compared by identity:
///   a pair that appears, a pair that goes, or a gap that differs by one ULP all
///   miss. That is what stands in for a geometry hook - not the stronger claim
///   that every geometry edit changes one of these gaps, which is false. What it
///   does buy is exactly the property the invariant is stated in: drift cannot
///   CROSS [`Project::MERGE_ALIGNMENT_INVARIANT_M`] without entering this set.
///   See the argument on [`Project::realign_merges`] for what that leaves and
///   why it is bounded.
///
/// The structure hash is `SipHash` via `DefaultHasher` - stable within a process,
/// which is all that is required of a field that is never persisted and never
/// compared across runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealignFixpoint {
    structure: u64,
    residual: Vec<ResidualGap>,
}

impl Project {
    /// Read a section's offset, defaulting to (0, 0).
    pub fn section_offset(&self, id: SectionId) -> (f64, f64) {
        self.section_offsets.get(&id).copied().unwrap_or((0.0, 0.0))
    }

    /// # THE `section_offsets` INVARIANT
    ///
    /// **A dust-zero entry does not exist.** `section_offsets` holds a key only
    /// for a section that is ACTUALLY DISPLACED, and "not displaced" has exactly
    /// one spelling: the key is absent. A value within [`Project::OFFSET_DUST_M`]
    /// of the origin in both components is not a position, so it is not written -
    /// it is removed. That holds for every writer, computed or restored, in memory
    /// and on disk, with no exceptions:
    ///
    /// * this function is THE write point, and everything routes through it -
    ///   [`Self::adjust_section_offset`], [`Self::set_section_offset`],
    ///   [`Self::restore_section_offsets`], the offset solver, the closure solve,
    ///   the move tools, the undo restore arms, and the two sites that cannot call
    ///   a `&mut self` method because they hold a live borrow of `self.sections`
    ///   (`Project::apply_station_move_internal`, `AppState::move_station`);
    /// * [`Self::normalize_section_offsets`] enforces it on a map this app did not
    ///   write - a `.stalx` from a build that predates the invariant, and the
    ///   snapshot payload of an undo entry persisted by one.
    ///
    /// ## Why it has to be an invariant and not a per-writer rule
    ///
    /// Because no write-local rule can satisfy both of the identities the app
    /// owes. `MoveSection(+d)` followed by `MoveSection(-d)` must leave the map it
    /// found, and a writer that sees only its own arguments cannot tell the two
    /// starting points apart:
    ///
    /// * from an ABSENT key, `(0 + d) - d` lands off zero by a residue under
    ///   [`Project::OFFSET_DUST_M`] rather than on it, and the key must end
    ///   ABSENT - the route that did nothing has none either
    ///   (#413 finding 2's dust half: a `ReverseSection` applied twice, an insert
    ///   plus the remove that undoes it, a cascade "correcting" its own noise);
    /// * from a PRESENT all-zero key, the same arithmetic must end PRESENT.
    ///
    /// The two demands are contradictory, and both were real: the second is the
    /// defect the metamorphic pair `move-out-and-back-is-identity` reported once
    /// a project carrying a legacy all-zero row was fed to it. Making the state
    /// UNREPRESENTABLE dissolves the contradiction - there is no present all-zero
    /// key to preserve, so the first rule is the only one left. The undo/redo
    /// exactness that #413 finding 27 was about follows for free: presence cannot
    /// differ between two states that agree geometrically.
    ///
    /// ## Notes on the comparison
    ///
    /// `<=` on `abs()`, so a NaN component (which compares false against
    /// everything) is INSERTED rather than dropped: the non-finite case belongs to
    /// `save_project`'s sanitize and the load boundary, not here. `-0.0 == 0.0` in
    /// IEEE, so a negated zero drops too.
    ///
    /// The decision is a PURE FUNCTION of the value: the same pair always lands on
    /// the same side, whichever route computed it and whatever the project around
    /// it looks like. That is not incidental - a scale-aware threshold would let
    /// two routes to one geometry disagree about a key BY DESIGN, which is the
    /// failure [`Project::OFFSET_DUST_M`] exists to make impossible and #469
    /// re-argued the bound over.
    ///
    /// Takes the map rather than `&mut self` so a caller holding a disjoint field
    /// borrow can still use it.
    pub fn write_section_offset(
        offsets: &mut HashMap<SectionId, (f64, f64)>,
        id: SectionId,
        value: (f64, f64),
    ) {
        if value.0.abs() <= Self::OFFSET_DUST_M && value.1.abs() <= Self::OFFSET_DUST_M {
            offsets.remove(&id);
        } else {
            offsets.insert(id, value);
        }
    }

    /// Drop every `section_offsets` entry that violates the invariant on
    /// [`Self::write_section_offset`], and report how many went.
    ///
    /// For a map this app did not write through that function: a `.stalx` from a
    /// build that predates the invariant (the load path inserts whatever rows the
    /// file holds), and the snapshot payload of an undo entry persisted by one.
    /// Both are legacy debris - a key for a section that is not displaced - and
    /// the app is free to drop it: every consumer reads the map through
    /// [`Self::section_offset`] or an inline `.get(..).unwrap_or((0.0, 0.0))`, so
    /// dropping moves the section by the value it held, which is by definition
    /// under [`Project::OFFSET_DUST_M`] - a micrometre, four decades under the
    /// gap the app calls a misalignment and six under anything it displays. What
    /// it changes that IS observable is that the two spellings of "not displaced"
    /// stop coexisting.
    pub fn normalize_section_offsets(&mut self) -> usize {
        let before = self.section_offsets.len();
        self.section_offsets.retain(|_, value| {
            !(value.0.abs() <= Self::OFFSET_DUST_M && value.1.abs() <= Self::OFFSET_DUST_M)
        });
        before - self.section_offsets.len()
    }

    /// Apply a delta to one section's offset. Low-level primitive -
    /// callers handle cascade and invariants.
    pub fn adjust_section_offset(&mut self, id: SectionId, dx: f64, dy: f64) {
        let current = self.section_offset(id);
        Self::write_section_offset(
            &mut self.section_offsets,
            id,
            (current.0 + dx, current.1 + dy),
        );
    }

    /// Set an exact offset value (for undo restoration / initial placement).
    pub fn set_section_offset(&mut self, id: SectionId, offset: (f64, f64)) {
        Self::write_section_offset(&mut self.section_offsets, id, offset);
    }

    /// Remove a section's offset entry entirely.
    pub fn remove_section_offset(&mut self, id: SectionId) {
        self.section_offsets.remove(&id);
    }

    /// Apply the same delta to every section in a merge group.
    pub fn adjust_rigid_group_offsets(&mut self, anchor: SectionId, dx: f64, dy: f64) {
        let group = self.get_merged_section_ids(anchor);
        for sid in group {
            self.adjust_section_offset(sid, dx, dy);
        }
    }

    /// May the coincidence CLAIM between `a` and `b` become a rigid merge, and
    /// which side has to move for the claim to be true?
    ///
    /// `None` REFUSES the promotion, and there is exactly one reason: it would
    /// place two zero datums in one rigid merge group. That is the state
    /// [`Self::merge_would_double_anchor`] names and
    /// [`Self::over_anchored_merge_components`] reports, and it is unrepairable
    /// rather than merely wrong - `cascade_bfs` pre-loads every datum-bearing
    /// section as authoritative and never traverses an edge into one, so the gap
    /// survives every cascade, realign and Repair (issue #464 defect A, where it
    /// was 59 km).
    ///
    /// GEOMETRY is never a reason to refuse. A closure whose endpoints stopped
    /// being connected IS a merge - the load path, Repair and every forward
    /// command agree on that ([`Self::convert_orphan_closures_to_merges`]) - so a
    /// site that declined a promotion because the endpoints were far apart would
    /// leave the session holding a closure the next open converts anyway, and
    /// memory and disk would disagree from then on. The distance is what the
    /// translation is FOR.
    ///
    /// The movable side is picked exactly as [`Self::merge_stations`] picks it:
    /// the datum-anchored side is the anchor, and with neither anchored `a`'s
    /// group moves. `movable` comes back EMPTY when the two endpoints are already
    /// in one merge group - the gap is then a cyclic misclosure inside a rigid
    /// group, which no rigid translation can reduce - so the promotion still
    /// happens and settling it is a no-op.
    pub(crate) fn plan_merge_promotion(
        &self,
        a: &StationKey,
        b: &StationKey,
    ) -> Option<MintedMerge> {
        let (Some(ref_a), Some(ref_b)) = (self.resolve_key(a), self.resolve_key(b)) else {
            // A dangling endpoint carries no position, so there is no coincidence
            // to claim and nothing for a translation to aim at.
            return None;
        };
        if self.merge_would_double_anchor(ref_a.section_id, ref_b.section_id) {
            return None;
        }
        let (anchor, moving, anchor_section, moving_section) =
            if self.section_has_zero_datum(ref_a.section_id) {
                (a, b, ref_a.section_id, ref_b.section_id)
            } else {
                (b, a, ref_b.section_id, ref_a.section_id)
            };
        let group = self.get_merged_section_ids(moving_section);
        let mut movable: Vec<SectionId> = if group.contains(&anchor_section) {
            Vec::new()
        } else {
            group.into_iter().collect()
        };
        // Deterministic order: the translation is the same sum of adjustments
        // however the merge-group set happened to hash.
        movable.sort_unstable();
        Some(MintedMerge {
            movable,
            anchor: anchor.clone(),
            moving: moving.clone(),
        })
    }

    /// Translate each minted merge's movable group so the coincidence its merge
    /// ASSERTS is actually true, measuring the residual off the geometry as it
    /// stands NOW (see [`MintedMerge`] for why the delta cannot be computed when
    /// the merge is recorded).
    ///
    /// Translating a whole merge group cannot break anything else: by
    /// construction the group is the moving side as it was BEFORE the new edge
    /// joined the two, so the only merge crossing its boundary is the one being
    /// settled, and every merge inside it moves rigidly with it. Settling several
    /// in mint order is likewise stable - a later settle either moves a superset
    /// of an earlier group (both of that merge's ends move together) or a group
    /// disjoint from it.
    ///
    /// The dust bound is `cascade_bfs`'s, for its reason: a gap of a few ULPs of
    /// the coordinates being summed is the residue of summing a survey chain, not
    /// a displacement, and "correcting" it would write a `section_offsets` key for
    /// a section that is not displaced (the invariant on
    /// [`Self::write_section_offset`]).
    pub(crate) fn settle_minted_merges(&mut self, minted: &[MintedMerge]) {
        for merge in minted {
            if merge.movable.is_empty() {
                continue;
            }
            let (Some(anchor), Some(moving)) = (
                self.resolve_key(&merge.anchor),
                self.resolve_key(&merge.moving),
            ) else {
                continue;
            };
            let (Some((ax, ay)), Some((mx, my))) = (
                self.compute_station_raw_xy(anchor),
                self.compute_station_raw_xy(moving),
            ) else {
                continue;
            };
            let (dx, dy) = (ax - mx, ay - my);
            if !dx.is_finite() || !dy.is_finite() {
                continue;
            }
            if dx.abs() <= Self::OFFSET_DUST_M && dy.abs() <= Self::OFFSET_DUST_M {
                continue;
            }
            tracing::debug!(
                anchor = ?merge.anchor,
                moving = ?merge.moving,
                dx,
                dy,
                sections = merge.movable.len(),
                purpose = "pay for the coincidence a promoted merge declares",
                "Co-locating the movable side of a minted merge"
            );
            for section in &merge.movable {
                self.adjust_section_offset(*section, dx, dy);
            }
        }
    }

    /// THE alignment predicate: is a loop closure's coincidence CLAIM true right
    /// now, and may it be made rigid?
    ///
    /// One definition, so that the three sites which ask (creation in
    /// [`Self::create_loop_closure`], the load in [`Self::assign_closure_ids`],
    /// and the per-command re-measure in
    /// [`Self::remeasure_closure_alignment`]) cannot answer it differently and
    /// leave memory and disk disagreeing about what a closure IS (#469 item 2;
    /// the single-definition doctrine of [`Self::is_orphan_closure`]).
    ///
    /// Three terms, and each is load-bearing:
    ///
    /// * **STRATEGY.** Only a [`ClosureStrategy::Move`] closure - one the
    ///   snap-to-merge workflow created because the user physically moved a
    ///   station onto another - can be aligned. A `Batch` /
    ///   `LeastSquaresWeighted` closure RECORDS a misclosure and is what the
    ///   non-destructive solve relaxes; rigidifying one because it happens to be
    ///   coincident would fight the thing it exists for.
    /// * **GEOMETRY**, gated at [`Self::MERGE_ALIGNMENT_INVARIANT_M`] and not at
    ///   the 5 cm reporting tolerance. An aligned closure is treated exactly like
    ///   a merge by [`Self::get_rigid_connected_section_ids`] and by the offset
    ///   cascade, so it is held to the merge INVARIANT - the number that answers
    ///   "is this a state the code considers valid" rather than "is this worth
    ///   telling the user about". That also makes a fresh stamp unable to trip
    ///   [`Self::check_closure_alignment`], since `INVARIANT <= TOLERANCE` is an
    ///   enforced ordering.
    /// * **DOUBLE-ANCHORING**, asked with [`Self::merge_would_double_anchor`] -
    ///   the same refusal [`Self::plan_merge_promotion`] makes, for the same
    ///   reason. Aligning a closure between two independently zero-datum-anchored
    ///   merge components mints precisely the rigid edge #464 exists to refuse:
    ///   `cascade_bfs` pre-loads every datum-bearing section as authoritative and
    ///   never traverses an edge into one, so nothing could ever satisfy it.
    ///   Refusing keeps the pair a SOFT closure, which is legitimate between two
    ///   anchored groups and costs no geometry - unlike a promotion, an alignment
    ///   never translates anything, so there is nothing to pay and nothing to
    ///   settle.
    ///
    /// # Why every term is blind to `is_aligned`
    ///
    /// Deliberately, and it is what makes the re-measure safe on an INVERSE. The
    /// gap reads shots plus offsets, and the double-anchor question reads merges
    /// plus datums, through [`Self::get_merged_section_ids`] - the MERGE-only
    /// graph, never the rigid one. So the stamp is a pure function of the state
    /// MINUS the flags it computes: re-running it cannot depend on the order the
    /// closures are visited in, cannot iterate to a fixpoint, and reproduces
    /// exactly the flag any earlier state had for the same geometry. See
    /// [`Self::remeasure_closure_alignment`] for what that buys.
    pub(crate) fn closure_alignment_stamp(
        &self,
        strategy: ClosureStrategy,
        station_a: &StationKey,
        station_b: &StationKey,
        local: &HashMap<SectionId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
        components: Option<&mut StationComponents>,
    ) -> bool {
        if strategy != ClosureStrategy::Move {
            return false;
        }
        // `None` is "cannot measure" - a dangling endpoint or non-finite geometry
        // - and an unmeasurable claim is not a claim this app will act on.
        let Some((a, b, gap)) = self.endpoint_gap(station_a, station_b, local, station_offsets)
        else {
            return false;
        };
        if gap >= Self::MERGE_ALIGNMENT_INVARIANT_M {
            return false;
        }
        // A batch caller answers from components built once: a walk per closure
        // is closures x merge-component size on every load (#597).
        !match components {
            Some(components) => components.would_double_anchor(a.section_id, b.section_id),
            None => self.merge_would_double_anchor(a.section_id, b.section_id),
        }
    }

    /// [`Self::closure_alignment_stamp`] for a caller that has ONE closure and no
    /// precomputes - the creation path. Builds the two tables itself
    /// (`O(total shots)`, next to nothing beside the Dijkstra
    /// [`Self::compute_closure_metrics`] already ran) and short-circuits before
    /// building them at all when the strategy cannot be aligned anyway.
    pub(crate) fn closure_alignment_stamp_now(
        &self,
        strategy: ClosureStrategy,
        station_a: &StationKey,
        station_b: &StationKey,
    ) -> bool {
        if strategy != ClosureStrategy::Move {
            return false;
        }
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();
        self.closure_alignment_stamp(
            strategy,
            station_a,
            station_b,
            &local,
            &station_offsets,
            None,
        )
    }

    /// Re-derive every snap-to-merge closure's `is_aligned` from the geometry as
    /// it stands NOW, and report how many claims changed.
    ///
    /// Derived state, not a hook set, for the reason
    /// [`Self::realign_structure_hash`] gives: a `MoveStation` that tears one
    /// endpoint, an `UpdateShot` whose realign walks merges only, an insert or a
    /// removal whose compensation is merge-scoped - each can falsify a coincidence
    /// claim, and a hook that is one call site short does not degrade, it silently
    /// keeps a lie. `AppState::reconcile_geometry` is the single post-geometry
    /// choke point (execute, execute_batch, the delete/split entry points,
    /// undo/redo/undo_to/redo_to, `finalize_structural_edit`), so the sweep runs
    /// there, plus at the two boundaries that build a project rather than edit one
    /// ([`Self::redrive_offsets_after_load`], Repair).
    ///
    /// # It runs on the INVERSE too, and that is not a re-derivation
    ///
    /// The rule an undo owes is that it RESTORES what was recorded. A sweep that
    /// only DEMOTED would break it outright: `UpdateShot`'s `PreviousValues`
    /// carries the shot's old measurements and its rigid group's offsets and not
    /// one closure flag, so tearing an aligned closure and undoing it would give
    /// the geometry back with the flag stuck false, landing on a state the history
    /// never held. What makes running it on both directions correct is that the
    /// stamp is TOTAL and blind to the flags (see
    /// [`Self::closure_alignment_stamp`]): the flag is a FUNCTION of the rest of
    /// the state, every state the history records satisfies
    /// `flag == stamp(state)`, and an inverse that restores the rest of the state
    /// therefore reproduces the flag it recorded - in both directions, promotion
    /// as well as demotion. It is the same argument [`Self::realign_merges`] rests
    /// on, one field over.
    ///
    /// That invariant is why REPAIR carries this too. Repair never goes through
    /// `reconcile_geometry`, and a state it left with a stale claim would be
    /// corrected by the NEXT command's sweep - i.e. a difference against Repair's
    /// own undo snapshot, appearing one command later.
    ///
    /// Nothing has to be re-solved after a change: [`Self::realign_structure_hash`]
    /// already hashes `is_aligned`, so a claim that moves invalidates any recorded
    /// [`RealignFixpoint`] and the next `realign_merges` re-solves against the
    /// rigid graph as it now stands.
    ///
    /// # What it deliberately does NOT touch (#469, and the 0.5.0 migration)
    ///
    /// A closure whose strategy is not [`ClosureStrategy::Move`] keeps whatever
    /// flag it arrived with. The app has not been able to CREATE one aligned since
    /// the destructive "Adjust Closures" action was removed in 0.5.0, so the only
    /// source is a file that action wrote - and it wrote the correction INTO the
    /// shots, which is why docs/closures.md promises such a project "opens looking
    /// identical and is never rewritten again". Demoting them would silently
    /// change co-location, station depth averaging, move-tool rigidity and which
    /// shots are frozen, on files that are geometrically exactly what they claim
    /// to be. [`Self::check_closure_alignment`] still reports a torn one.
    ///
    /// It measures the GAP only, never the stored misclosure metrics: those are
    /// per-closure Dijkstra (`O(closures x (V log V + E))`) and would put a
    /// whole-network solve in a per-command epilogue, while the gap is `O(1)` per
    /// closure off the two precomputes below. The metrics stay as RECORDED, which
    /// is what `error_percentage` already means; display staleness is not a
    /// geometry lie.
    pub fn remeasure_closure_alignment(&mut self) -> usize {
        if !self
            .loop_closures
            .iter()
            .any(|closure| closure.strategy == ClosureStrategy::Move)
        {
            return 0;
        }
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();
        let mut components = StationComponents::new(self);
        // Decided in an immutable pass, applied after: the stamp reads merges and
        // datums off `self`. Every decision is independent of every other, so the
        // batch is the same however the list is ordered.
        let changes: Vec<(usize, bool)> = self
            .loop_closures
            .iter()
            .enumerate()
            .filter(|(_, closure)| closure.strategy == ClosureStrategy::Move)
            .filter_map(|(index, closure)| {
                let stamp = self.closure_alignment_stamp(
                    closure.strategy,
                    &closure.station_a,
                    &closure.station_b,
                    &local,
                    &station_offsets,
                    Some(&mut components),
                );
                (stamp != closure.is_aligned).then_some((index, stamp))
            })
            .collect();
        if changes.is_empty() {
            return 0;
        }
        for &(index, stamp) in &changes {
            let Some(closure) = self.loop_closures.get_mut(index) else {
                continue;
            };
            tracing::info!(
                closure_id = closure.id,
                station_a = ?closure.station_a,
                station_b = ?closure.station_b,
                is_aligned = stamp,
                purpose = "keep a closure's alignment claim measured rather than asserted",
                "Re-measured a snap-to-merge closure's coincidence claim"
            );
            closure.is_aligned = stamp;
        }
        self.touch();
        changes.len()
    }

    /// Does the merge group containing `section_id` hold a zero datum?
    pub(crate) fn rigid_group_has_datum(&self, id: SectionId) -> bool {
        // Delegate to the resolve-aware check so a DANGLING datum (anchor key no
        // longer resolves) is not counted as an anchor - otherwise a move-drag is
        // wrongly blocked on a phantom-anchored, actually-unanchored group (#174).
        // Same merge-group scope as before: `component_datum_refs` (which backs
        // `section_has_zero_datum`) uses the same `get_merged_section_ids`.
        self.section_has_zero_datum(id)
    }

    /// Assert that all merge points are colocated (within
    /// [`Project::MERGE_ALIGNMENT_INVARIANT_M`]).
    /// Only active in debug builds - no-op in release.
    /// Check that all merge points are colocated. Logs an error for any merge
    /// pair past the user-facing [`Project::MERGE_ALIGNMENT_TOLERANCE_M`]. In
    /// test builds, panics past the tighter invariant threshold to catch
    /// regressions with synthetic data - the SAME threshold
    /// [`Self::realign_merges`] heals at, so nothing this asserts on can be a
    /// state the self-heal declines to repair (#413 findings 14 and 20).
    pub fn debug_assert_merge_alignment(&self, context: &str) {
        // Despite the name, this used to run in release too - and the cost
        // is brutal: O(N) merges × 2 × O(N) `compute_station_raw_coords`
        // per call, called from every mutating command. For a 1 200-section
        // TML import this contributed ~1.5 billion operations to the hot
        // path. Gate the whole body behind `debug_assertions` so it
        // remains a sanity check during development without harming
        // production performance. The release-mode `error!` log it used
        // to emit was informational only - alignment drift inside a
        // *single* command is a programmer bug, not a data condition the
        // user can fix at runtime.
        #[cfg(debug_assertions)]
        {
            // During a deferred-cascade batch (e.g. a large import) connections
            // are created in dependency order, so intermediate states are
            // transiently misaligned BY DESIGN - and running this
            // O(merges × colocation-BFS) check after every command would make the
            // batch O(N³) (the import of a large Walls project pinned a core for
            // minutes in dev builds). Skip until the batch completes; the cascade
            // that runs when the guard is disarmed restores alignment.
            if self.cascade_deferred {
                return;
            }
            // Same tripwire idea one level down: a merge-adjacency memo that no
            // longer matches the merges is how a rewritten endpoint silently
            // fools the double-anchor guard.
            self.station_merges.debug_assert_memo_fresh(context);
            for merge in &self.station_merges {
                // Skip same-section merges (physically meaningless, used only in tests)
                if merge.station_a.section_id == merge.station_b.section_id {
                    continue;
                }
                let a = self
                    .resolve_key(&merge.station_a)
                    .and_then(|s| self.compute_station_raw_coords(s));
                let b = self
                    .resolve_key(&merge.station_b)
                    .and_then(|s| self.compute_station_raw_coords(s));
                if let (Some(a_pos), Some(b_pos)) = (a, b) {
                    let dist = ((a_pos.x - b_pos.x).powi(2) + (a_pos.y - b_pos.y).powi(2)).sqrt();
                    if dist >= Self::MERGE_ALIGNMENT_TOLERANCE_M {
                        error!(
                            context,
                            a_ref = ?merge.station_a,
                            b_ref = ?merge.station_b,
                            dist,
                            a_x = a_pos.x,
                            a_y = a_pos.y,
                            b_x = b_pos.x,
                            b_y = b_pos.y,
                            "Merge alignment violated (>{threshold}m)",
                            threshold = Self::MERGE_ALIGNMENT_TOLERANCE_M,
                        );
                    }
                    #[cfg(test)]
                    debug_assert!(
                        dist < Self::MERGE_ALIGNMENT_INVARIANT_M,
                        "Merge alignment violated after {context}: stations {a_ref:?} and {b_ref:?} \
                         are {dist:.6}m apart (threshold {threshold}m). \
                         A=({ax:.6}, {ay:.6}), B=({bx:.6}, {by:.6})",
                        threshold = Self::MERGE_ALIGNMENT_INVARIANT_M,
                        a_ref = merge.station_a,
                        b_ref = merge.station_b,
                        ax = a_pos.x,
                        ay = a_pos.y,
                        bx = b_pos.x,
                        by = b_pos.y,
                    );
                }
            }
        }
        // Suppress unused warning in release.
        let _ = context;
    }

    /// One authoritative anchor section per connected merge group, preferring a
    /// zero-datum section so the realign keeps the group pinned to GPS.
    pub(super) fn realign_seeds(&self) -> Vec<SectionId> {
        let datum_sections: HashSet<SectionId> = self
            .zero_datums
            .iter()
            .map(|z| z.station_ref.section_id)
            .collect();
        // The graph `get_connected_section_ids` walks (every merge and every
        // closure, by stored section id), built once: that walk rescans every
        // edge per visited section, which made this O(sections x edges) (#602).
        let mut adjacency: HashMap<SectionId, Vec<SectionId>> = HashMap::new();
        let edges = self
            .station_merges
            .iter()
            .map(|m| (m.station_a.section_id, m.station_b.section_id))
            .chain(
                self.loop_closures
                    .iter()
                    .map(|c| (c.station_a.section_id, c.station_b.section_id)),
            );
        for (a, b) in edges {
            adjacency.entry(a).or_default().push(b);
            adjacency.entry(b).or_default().push(a);
        }
        let mut seeds = Vec::new();
        let mut visited: HashSet<SectionId> = HashSet::new();
        for section in &self.sections {
            if visited.contains(&section.id) {
                continue;
            }
            let mut component: HashSet<SectionId> = HashSet::new();
            let mut queue = vec![section.id];
            while let Some(current) = queue.pop() {
                if component.insert(current) {
                    if let Some(neighbours) = adjacency.get(&current) {
                        queue.extend(neighbours.iter().filter(|n| !component.contains(n)));
                    }
                }
            }
            let anchor = component
                .iter()
                .copied()
                .filter(|s| datum_sections.contains(s))
                .min()
                .or_else(|| component.iter().copied().min());
            if let Some(a) = anchor {
                seeds.push(a);
            }
            visited.extend(component);
        }
        seeds
    }

    /// Re-derive section offsets from the merge/datum constraints right after
    /// loading from disk. Section offsets are **derived** state that the file
    /// merely caches; re-solving them on load heals any drift a previous session
    /// persisted (e.g. an older redo bug) so the project always opens
    /// geometrically correct, without the user having to repair anything. The
    /// transient caches are rebuilt first so the solve walks the merge graph
    /// correctly. Returns the number of merges that had drifted (0 = file was
    /// already consistent, the common case).
    pub fn redrive_offsets_after_load(&mut self) -> usize {
        self.redrive_offsets_after_load_reporting().0
    }

    /// [`Self::redrive_offsets_after_load`], also returning the shot UUIDs the
    /// load renamed to make them unique across the project
    /// ([`Self::make_shot_uuids_project_unique`]). The open path hands that remap
    /// to the persisted undo history, whose entries were recorded against the old
    /// ids.
    pub fn redrive_offsets_after_load_reporting(&mut self) -> (usize, super::ShotUuidRemap) {
        self.rebuild_section_index();
        self.rebuild_shot_uuid_index();
        // Normalize any closure that is really a sole-bridge attachment into a
        // merge before solving, so a file persisted in that state (e.g. a section
        // stranded by an older delete) opens correctly.
        // De-duplicate shot UUIDs BEFORE re-anchoring: if a section persisted two
        // shots sharing a UUID, canonicalizing a positional Index(i) ref to
        // AfterShot(uuid) would later resolve to the FIRST match, silently moving
        // a ref off the duplicate it was meant to target (and Repair couldn't
        // recover the intended endpoint). Re-minting the duplicate first gives the
        // second station its own UUID so the re-anchor stays faithful.
        self.ensure_unique_shot_uuids();
        // Then ACROSS sections: a file saved before shot UUIDs were project-wide
        // unique can hold one id in several sections (an Ariane file imported
        // twice, a merge of two copies). The lowest section keeps it, the others
        // get a derived replacement - derived, so every read of the file agrees.
        // Still before the re-anchor, so positional refs canonicalize straight to
        // the new ids.
        let renamed = self.make_shot_uuids_project_unique();
        // Upgrade any positional (Index) ref from an older file to the stable
        // shot-UUID anchor now that shots (with UUIDs) are loaded.
        self.reanchor_station_refs();
        self.convert_orphan_closures_to_merges();
        // Zero-trust: never honor a persisted derivable offset - re-solve
        // unconditionally so a sub-tolerance stale offset can't survive the load.
        let healed = self.resolve_offsets();
        // The same doctrine, one step further: a persisted DUST-ZERO row is not a
        // derivable offset the re-solve rewrites - the cascade never seed-adjusts
        // an anchor or a free-floating component, so such a row survives it
        // untouched - and it is a state the app may not hold (see the invariant on
        // `write_section_offset`). A `.stalx` from a build that predates the
        // invariant therefore stops carrying one HERE, at the boundary, rather
        // than being carried into memory for every downstream rule to special-case.
        // Nothing observable changes: an absent key and an all-zero one are the
        // same geometry to every consumer. The file settles on the next save.
        let dropped = self.normalize_section_offsets();
        if dropped > 0 {
            tracing::info!(
                dropped,
                purpose = "hold the section_offsets invariant at the load boundary",
                "Dropped persisted section offsets that were within dust of the origin"
            );
        }
        // LAST, so the claims are measured off the geometry the re-solve settled
        // on rather than the geometry the file happened to cache. `is_aligned` is
        // derived state exactly as `section_offsets` is, and the same zero-trust
        // rule applies: a file written by a build that asserted the flag, or one
        // whose closure was torn by an edit that predates the re-measure, opens
        // holding the truth instead of the claim (#469 item 2).
        self.remeasure_closure_alignment();
        (healed, renamed)
    }

    /// The orphanhood question [`Self::convert_orphan_closures_to_merges`] asks of
    /// one closure: are its two endpoints still connected through the survey
    /// (shots + merges) independently of the closure itself? If not, the closure
    /// is their *sole* bridge - a merge, not a loop.
    ///
    /// A same-section closure is never an orphan (the section itself connects the
    /// endpoints), and neither is one whose endpoint no longer resolves (there is
    /// nothing to ask the connectivity walk about; a dangling ref is integrity's
    /// problem, not this one).
    ///
    /// The single definition of the predicate: the sweep below scans with it, and
    /// [`Self::orphan_closure_ids`] snapshots with it, so a mutation path's
    /// before-set and the sweep that follows can never disagree about what an
    /// orphan is.
    ///
    /// "Connected" is answered by `components`, a union-find over shots and
    /// resolvable merges built once per sweep, rather than a walk per closure
    /// (#597: the walk rescanned every merge per visited station).
    fn is_orphan_closure(&self, closure: &LoopClosure, components: &mut StationComponents) -> bool {
        if closure.station_a.section_id == closure.station_b.section_id {
            return false;
        }
        let (Some(a), Some(b)) = (
            self.resolve_key(&closure.station_a),
            self.resolve_key(&closure.station_b),
        ) else {
            return false;
        };
        !components.connected(a, b)
    }

    /// The ids of every closure that is orphaned RIGHT NOW. A mutation path takes
    /// this before it mutates and hands it to
    /// [`Self::convert_closures_orphaned_since`] afterwards; see that function for
    /// why an inverse needs the distinction.
    pub(crate) fn orphan_closure_ids(&self) -> HashSet<u64> {
        let mut components = StationComponents::new(self);
        self.loop_closures
            .iter()
            .filter(|c| self.is_orphan_closure(c, &mut components))
            .map(|c| c.id)
            .collect()
    }

    /// Enforce the invariant that a loop closure exists only for a TRUE loop. If
    /// a closure's two endpoints are no longer connected through the survey
    /// (shots + merges) independently of the closure, the closure is the *sole*
    /// connection between them - that is a merge, not a loop - so convert it.
    ///
    /// Iterative: when two closures bridge the same pair, the first becomes the
    /// merge and the second is then a genuine (redundant) loop and is kept.
    /// Returns the number converted. (Issue #8 - a shot deletion can strand a
    /// section so its only attachment is a closure.)
    ///
    /// Whole-project sweep: every orphan it finds is converted, no matter what
    /// made it one. That is right for a load, a Repair and a forward command -
    /// and wrong inside an inverse, which is what
    /// [`Self::convert_closures_orphaned_since`] exists for.
    pub fn convert_orphan_closures_to_merges(&mut self) -> usize {
        let minted = self.convert_closures_orphaned_since(&HashSet::new());
        let converted = minted.len();
        // Settled immediately: this entry point's callers - the load, Repair and
        // `execute()`'s epilogue - change no geometry between the conversion and
        // the re-solve that follows it, so "now" IS final geometry for them.
        self.settle_minted_merges(&minted);
        converted
    }

    /// [`Self::convert_orphan_closures_to_merges`] restricted to the orphans a
    /// mutation JUST created: `preexisting` is the [`Self::orphan_closure_ids`]
    /// snapshot the caller took before mutating, and every id in it is left as a
    /// closure.
    ///
    /// An inverse must not do more than invert (#413, the orphan-closure
    /// observation in `fuzz_undo_tests.rs`'s header). A
    /// closure can be orphaned by a command whose own path never converts it -
    /// `removeZeroDatum` un-anchors one side, and the sweep's double-anchor guard
    /// stops refusing it - and that un-converted closure is part of the state the
    /// history RECORDED. Sweeping the whole project from inside an undo would
    /// convert it as a side effect of removing an unrelated shot, landing on a
    /// state no snapshot ever held. Scoping by *what this mutation orphaned*
    /// keeps the pre-existing orphan exactly as recorded while still converting
    /// the closures the removal genuinely stranded - including ones nowhere near
    /// the removed station, whose loop-ness hung off a merge that was.
    ///
    /// Returns the [`MintedMerge`]s the conversion created, which the caller
    /// owes [`Self::settle_minted_merges`] once its own geometry is final: a
    /// promoted closure carries the loop's real misclosure into an edge that says
    /// the two endpoints coincide, and the promotion - not whatever cascades next
    /// - has to pay for that (#464 defects B and C).
    #[must_use = "a promotion that is never settled leaves a merge asserting a coincidence \
                  the geometry does not have - call settle_minted_merges"]
    pub(crate) fn convert_closures_orphaned_since(
        &mut self,
        preexisting: &HashSet<u64>,
    ) -> Vec<MintedMerge> {
        let mut minted = Vec::new();
        let mut components = StationComponents::new(self);
        // One forward pass. It decides exactly what restarting the scan from the
        // first closure after every conversion did: a conversion only ADDS
        // connectivity, so a closure that was not an orphan stays not one, and a
        // refused one (left as a closure) is never reconsidered either way.
        let mut idx = 0;
        while idx < self.loop_closures.len() {
            let closure = &self.loop_closures[idx];
            if preexisting.contains(&closure.id)
                || !self.is_orphan_closure(closure, &mut components)
            {
                idx += 1;
                continue;
            }

            // Converting a sole-bridge closure into a merge makes it a RIGID edge,
            // and `plan_merge_promotion` answers both questions that raises: MAY
            // it (an edge between two independently zero-datum-anchored groups
            // would place two GPS anchors in one merge component, which no offset
            // cascade can satisfy and Repair cannot reduce), and WHICH SIDE must
            // move for the coincidence the edge declares to be true. A refusal
            // keeps it a (soft) closure - legitimate between two anchored groups -
            // and lets the over-anchoring detector surface it for the user.
            let (id, a, b) = {
                let closure = &self.loop_closures[idx];
                (
                    closure.id,
                    closure.station_a.clone(),
                    closure.station_b.clone(),
                )
            };
            // The double-anchor refusal is answered from the components first, so a
            // refused closure costs a lookup instead of `plan_merge_promotion`'s two
            // merge-graph walks (#597); the plan still decides every promotion.
            let plan = if components.would_double_anchor(a.section_id, b.section_id) {
                None
            } else {
                self.plan_merge_promotion(&a, &b)
            };
            let Some(plan) = plan else {
                tracing::warn!(
                    closure_id = id,
                    purpose = "avoid double-anchoring a merge component",
                    "Left a sole-bridge loop closure unconverted: promoting it is not legal \
                     (both endpoints zero-datum-anchored, or an endpoint no longer resolves)"
                );
                idx += 1;
                continue;
            };

            let closure = self.loop_closures.remove(idx);
            let already = match (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                (Some(a), Some(b)) => {
                    components.join(a, b);
                    self.has_direct_merge(a, b)
                }
                _ => false,
            };
            if !already {
                self.record_station_merge(StationMerge {
                    station_a: closure.station_a,
                    station_b: closure.station_b,
                    merged_at: Utc::now(),
                });
            }
            minted.push(plan);
        }
        minted
    }

    /// Every merge whose endpoints are at least [`Self::MERGE_ALIGNMENT_INVARIANT_M`]
    /// apart, with the exact gap. The geometry is [`Self::merge_alignment_beyond`]'s
    /// (same precompute, same skips: a same-section merge is meaningless, a
    /// dangling endpoint is a structural problem and not a gap) - what differs is
    /// what comes out. That one reports to a human and rounds to centimetres; this
    /// one is compared for identity, so it keeps the bits.
    ///
    /// Ordered by `station_merges`, whose order the structure hash pins, so two
    /// calls on one state produce identical vectors.
    fn residual_merge_gaps(&self) -> Vec<ResidualGap> {
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();
        let mut residual = Vec::new();
        for merge in &self.station_merges {
            if merge.station_a.section_id == merge.station_b.section_id {
                continue;
            }
            let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) else {
                continue;
            };
            if let (Some((ax, ay)), Some((bx, by))) = (
                self.station_world_xy(a, &local, &station_offsets),
                self.station_world_xy(b, &local, &station_offsets),
            ) {
                let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
                if gap >= Self::MERGE_ALIGNMENT_INVARIANT_M {
                    residual.push((
                        (a.section_id, a.station_index),
                        (b.section_id, b.station_index),
                        gap.to_bits(),
                    ));
                }
            }
        }
        residual
    }

    /// Hash of everything the offset solve depends on that is NOT geometry: which
    /// sections exist and how many shots each holds, the merge edges, the closure
    /// edges, and the datum anchors. The [`RealignFixpoint`] carries it so that a
    /// structural edit which changes what the solver CAN do - a merge or closure
    /// added or removed, a closure converted, a datum added, moved or dropped, a
    /// section gone - invalidates a recorded fixpoint even though it moves nothing
    /// and leaves every gap bit-identical.
    ///
    /// This is a whole-project scan rather than a set of invalidation hooks on the
    /// mutators on purpose. Those collections are mutated from many places
    /// (including the undo restore arms, which assign whole vectors), and a hook
    /// set that is one call site short does not degrade - it silently skips a heal
    /// that had become possible. Deriving the answer from the state cannot be one
    /// call site short. The scan is `O(sections + merges + closures + datums)` and
    /// runs only when a beyond-invariant residual already exists, next to a
    /// `merge_alignment_beyond` pass that is `O(total shots)` and a re-solve that
    /// is more expensive still.
    fn realign_structure_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.sections.len().hash(&mut hasher);
        for section in &self.sections {
            section.id.hash(&mut hasher);
            section.shots.len().hash(&mut hasher);
        }
        self.station_merges.len().hash(&mut hasher);
        for merge in &self.station_merges {
            merge.station_a.hash(&mut hasher);
            merge.station_b.hash(&mut hasher);
        }
        self.loop_closures.len().hash(&mut hasher);
        for closure in &self.loop_closures {
            closure.id.hash(&mut hasher);
            closure.station_a.hash(&mut hasher);
            closure.station_b.hash(&mut hasher);
            closure.is_aligned.hash(&mut hasher);
            std::mem::discriminant(&closure.strategy).hash(&mut hasher);
        }
        self.zero_datums.len().hash(&mut hasher);
        for datum in &self.zero_datums {
            datum.station_ref.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Recompute section offsets so every merge's endpoints coincide again,
    /// anchoring each connected group at its zero datum (or, failing that, an
    /// arbitrary section). Returns the number of merges that were misaligned
    /// beyond the REPORTING tolerance before the realign.
    ///
    /// This is the per-command self-heal (via `reconcile_geometry`): it keeps the
    /// fast no-drift early-return because the in-memory offsets it sees were just
    /// computed by the command/cascade, not read from an untrusted source, and an
    /// unconditional re-solve after every command would override the exact state
    /// an undo/redo snapshot just restored. The *load*/repair paths use
    /// [`Self::resolve_offsets`] instead, which never trusts a persisted value.
    ///
    /// The gate is [`Self::MERGE_ALIGNMENT_INVARIANT_M`], NOT the 5 cm reporting
    /// tolerance the return value counts against. Gating the repair on the
    /// reporting number left the entire 1-5 cm band unhealed - a state the app's
    /// own `debug_assert_merge_alignment` calls a violation and panics a test
    /// build on, silently saved to the `.stalx` in a release one (#413 finding
    /// 20: a shot edit in an unanchored merge group produced exactly that, and
    /// the drift equalled the edit). The heal gate must stay at or below the
    /// assert; it may never be loosened back onto the reporting threshold.
    ///
    /// ## The fixpoint skip
    ///
    /// Not every beyond-invariant gap is healable. An over-anchored component
    /// (two zero datums in one rigid merge group) and a cyclic merge misclosure
    /// both leave a residual the cascade cannot reduce: the solve runs, changes
    /// nothing, and the gate is non-empty again on the next command - so this
    /// function used to re-solve the WHOLE project after every keystroke-scale
    /// edit, forever, and each of those re-solves also overrode the exact offsets
    /// an undo/redo snapshot had just restored (the thing the drift gate exists
    /// to avoid). When a solve leaves the residual and the offsets untouched, the
    /// state is a solver fixpoint and is recorded as a [`RealignFixpoint`]; while
    /// the live state still fingerprints identically, the re-solve is skipped.
    ///
    /// The invariant is not weakened by that, but the reason is narrower than
    /// the one this comment used to give ("every geometry edit that could change
    /// the outcome changes a gap"), which is simply false - the test below that
    /// drifts a bystander by 3 mm is a geometry edit the solve WOULD act on and
    /// no gap records. Two properties carry the guarantee instead:
    ///
    /// * any drift that CROSSES the invariant enters the residual set, and that
    ///   set is compared by identity - a pair appearing, a pair going, or one
    ///   gap changing by a single ULP all miss the fingerprint and get the full
    ///   solve. The state the heal gate and `debug_assert_merge_alignment` are
    ///   written in terms of - "a merge is at least
    ///   [`Self::MERGE_ALIGNMENT_INVARIANT_M`] out" - therefore cannot be
    ///   reached while the skip is armed;
    /// * any structural edit that could turn an irreducible residual into a
    ///   reducible one without moving anything (a merge, closure, datum or
    ///   section added, removed or converted) changes the structure hash and
    ///   misses too.
    ///
    /// What the skip does leave alone is a SUB-invariant offset change inside a
    /// component that already holds a residual: the solve would pull it back and
    /// the skip does not. That is bounded by the first bullet - it is drift the
    /// heal gate itself declines to act on until it crosses, at which point the
    /// solve runs - and it is the optimization, not a leak: without it, one
    /// irreducible gap anywhere in the project makes every keystroke-scale edit
    /// pay a whole-project re-solve again.
    ///
    /// A whole-rigid-group translation is NOT in that category, despite looking
    /// like it should be (it leaves every gap and the structure bit-identical).
    /// The cascade derives each non-authoritative section from an authoritative
    /// one's STORED offset and never re-derives an authoritative section from
    /// anything, so it is translation-invariant: after a group move the full
    /// solve provably changes nothing, and a skip that hides nothing hides
    /// nothing (`a_whole_rigid_group_translation_is_a_solver_no_op` below runs
    /// the move through `adjust_rigid_group_offsets` and compares).
    pub fn realign_merges(&mut self) -> usize {
        let residual = self.residual_merge_gaps();
        if residual.is_empty() {
            return 0;
        }
        let fingerprint = RealignFixpoint {
            structure: self.realign_structure_hash(),
            residual,
        };
        if self.realign_fixpoint.as_ref() == Some(&fingerprint) {
            // Observationally identical to running the solve: a recorded fixpoint
            // is a state the cascade left byte-for-byte alone, so `resolve_offsets`
            // would have counted the same misalignments before and after and
            // returned the same 0 healed merges the early-return above does.
            return 0;
        }
        let (healed, changed) = self.resolve_offsets_tracked();
        self.realign_fixpoint = if changed {
            // The solve moved something, so it is not (yet) a fixpoint - and the
            // residual it was fingerprinted from is stale. A later call re-derives.
            None
        } else {
            // Nothing moved: `resolve_offsets` only ever writes `section_offsets`,
            // so with those identical the residual and the structure are identical
            // too, and `fingerprint` IS the post-solve fingerprint.
            Some(fingerprint)
        };
        healed
    }

    /// Zero-trust re-solve of section offsets, used on LOAD and explicit REPAIR.
    /// Unlike [`Self::realign_merges`] it ALWAYS re-runs the cascade, even when no
    /// merge is drifted past the 5 cm reporting tolerance: section offsets are
    /// derived state, so a *sub-tolerance* stale/wrong cached offset (which
    /// `check_merge_alignment` can't see) must never be trusted across a reload.
    /// The cascade re-derives every offset reachable from an anchor; only the
    /// genuinely non-derivable anchors (datum sections and free-placed floating
    /// components) keep their stored offset, because the cascade never
    /// seed-adjusts them. Returns the number of merges that were misaligned beyond
    /// the reporting tolerance before the re-solve.
    pub fn resolve_offsets(&mut self) -> usize {
        self.resolve_offsets_tracked().0
    }

    /// [`Self::resolve_offsets`], also reporting whether the re-solve MOVED an
    /// offset. That flag is what lets [`Self::realign_merges`] tell a solve which
    /// healed something from one which provably cannot: the healed COUNT cannot,
    /// being blind to every sub-reporting-tolerance change - which is the whole
    /// reason this function re-cascades unconditionally.
    fn resolve_offsets_tracked(&mut self) -> (usize, bool) {
        let before = self.check_merge_alignment().len();
        // Snapshot the derived offsets so we can tell whether the re-solve actually
        // moved anything - including SUB-tolerance changes the `before`/`after`
        // misalignment count can't see (that is the whole reason this re-cascades
        // unconditionally). Comparing the count alone would miss them.
        let offsets_before = self.section_offsets.clone();
        let seeds = self.realign_seeds();
        let was_deferred = self.cascade_deferred;
        self.cascade_deferred = false;
        self.cascade_connection_offsets(&seeds);
        self.cascade_deferred = was_deferred;
        // Only stamp a modification when the re-solve genuinely changed an offset.
        // Opening a byte-clean project re-solves to the identical offsets, and
        // touch()ing there would bump `modified_at` for a project the user never
        // edited - a later no-edit save would then persist a bogus timestamp, and
        // it would make a no-op integrity Repair look like a real change.
        let changed = self.section_offsets != offsets_before;
        if changed {
            self.touch();
        }
        (
            before.saturating_sub(self.check_merge_alignment().len()),
            changed,
        )
    }
}
