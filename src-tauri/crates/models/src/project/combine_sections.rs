//! Combine Sections: join two sections that meet end to end into one; other
//! sections there stay attached as branches. The inverse of Split Section
//! Here (a split at a junction copies its merges to both halves, and combining
//! them back leaves the other sections attached to the joined station).
//!
//! First, what has to give (`combine_plan.rs`, listed in the dialog):
//! - the other section's stored bearings turn to read against the kept
//!   section's north reference, when the two differ;
//! - the other section's shots that share an identity (uuid) with the kept
//!   section's get new ones (`renew_shared_shot_uuids`, a heal the command
//!   runs before it records the undo step);
//! - rigid ties that would tie the combined section to itself, or hold the
//!   moving side, become ordinary loop closures;
//! - the moving side (the other section and what is still merged to it, or
//!   the kept side when a zero datum or the closure-adjusted view's anchor
//!   holds the other; see `combine_plan.rs`) moves by the gap at the
//!   join: a loop closure's misclosure, or a co-located or merged pair's
//!   residual; what is merged at its join station moves with the joined
//!   station instead, when the two join stations were moved by hand by
//!   different amounts (#755).
//!
//! Then the join. The KEPT section keeps its id, its shot ids, its direction,
//! its station 0 orientation and (unless it is the side that moves) its exact
//! position. The OTHER section is reversed first when the two meet end-to-end
//! or start-to-start (`reverse_section`, which reciprocates bearings, swaps
//! depths, negates pitch and optionally swaps left and right), then its shots
//! are appended to the kept section (kept end meets other start) or prepended
//! (kept start meets other end). Every per-shot field and uuid travels; the
//! other section's shots get new ids from the kept section's counter.
//!
//! The two join stations become ONE station:
//! - the join merge, and any loop closure across the join point (it would tie
//!   the joined station to itself), go;
//! - a comment or name both carry is kept once if identical, otherwise per
//!   the dialog's choice (kept, other, or both joined);
//! - the fixed side's manual offset (or none) wins, the other's is dropped
//!   (and what is merged to the other moved onto the joined station, above);
//! - a zero datum on either (never both) and closures to other stations move
//!   onto the joined station.
//!
//! Every other stored station reference moves with its station, the gauge
//! stays on the same physical station (the lower section id's gauge, which is
//! the one the component used), and when the two sections' reverse parities
//! differ the other section's shots keep their own record parity for dive-plan
//! pins (`Project::counter_parity_shots`). The closures the combine made or
//! whose ends it moved apart are measured again off the final geometry.
//!
//! Credit: survey credit (surveyors, explorers, date) lives on every shot and
//! travels with it, through the reverse too, so every shot keeps its own
//! exactly and the combined section's credit is simply the union.

use std::collections::{HashMap, HashSet};

use crate::angle::normalize_azimuth;
use crate::{
    ClosurePair, ClosuresBeforeEdit, CombineJoinEnds, CombineOutcome, CombinePick,
    CombineRemovedClosure, CombineRemovedKind, CombineReplacedClosure, CombineSectionsOptions,
    LoopClosure, SectionEnd, SectionId, Shot, ShotData, StationKey, StationRef,
};

use super::combine_plan::{JoinPoint, Tie, TieId, TiePlan};
use super::gauge::captured_gauge;
use super::reverse::shot_run_displacement;
use super::station_rekey::take_naming;
use super::uuid_unique::{reminted_shot_uuid_avoiding, ShotUuidRemap};
use super::Project;

/// Everything `combine_sections` decided before it changes anything.
struct CombineSetup {
    kept_end: SectionEnd,
    other_end: SectionEnd,
    join: JoinPoint,
    ties: TiePlan,
    rotation: Option<f64>,
    /// Closures across the join point: they would tie the joined station to
    /// itself.
    join_closures: HashSet<u64>,
}

/// The joined station's text, from the kept and the other join station's.
fn pick_text(
    kept: Option<String>,
    other: Option<String>,
    pick: CombinePick,
    separator: &str,
) -> Option<String> {
    match (kept, other) {
        (Some(kept), Some(other)) if kept == other => Some(kept),
        (Some(kept), Some(other)) => Some(match pick {
            CombinePick::Kept => kept,
            CombinePick::Other => other,
            CombinePick::Both => format!("{kept}{separator}{other}"),
        }),
        (kept, other) => kept.or(other),
    }
}

impl Project {
    /// A shot's RECORD parity: its section's `reverse_parity`, flipped when
    /// the shot came from a section whose parity differed (see
    /// [`Project::counter_parity_shots`]). Flips with every reverse of the
    /// record, wherever the record lives.
    pub fn shot_record_parity(&self, section: &crate::Section, shot_uuid: Option<&str>) -> bool {
        section.reverse_parity
            ^ shot_uuid.is_some_and(|uuid| self.counter_parity_shots.contains(uuid))
    }

    /// Decide the join and the plan for combining `other` into `kept`, or why
    /// not.
    fn combine_setup(
        &self,
        kept: SectionId,
        other: SectionId,
        join: Option<CombineJoinEnds>,
    ) -> Result<CombineSetup, String> {
        if let Some(reason) = self.combine_refusal(kept, other, join) {
            return Err(reason);
        }
        let fail = || format!("Sections {kept} and {other} cannot be combined");
        let candidate = self
            .combine_candidate_joining(kept, other, join)
            .ok_or_else(fail)?;
        let (kept_end, other_end) = if candidate.section_a == kept {
            (candidate.end_a, candidate.end_b)
        } else {
            (candidate.end_b, candidate.end_a)
        };
        let pairing = super::combine_candidate::Pairing {
            a: kept,
            end_a: kept_end,
            b: other,
            end_b: other_end,
        };
        let table = self.end_table();
        let touch = self.pairing_touch(pairing, &table).ok_or_else(fail)?;
        let (_, Some((join, ties))) = self.plan_keeping(pairing, &touch, kept, &table) else {
            return Err(fail());
        };
        let (k, o) = (
            self.get_section(kept).ok_or_else(fail)?,
            self.get_section(other).ok_or_else(fail)?,
        );
        let join_closures = self
            .loop_closures
            .iter()
            .filter(|c| {
                match (
                    self.resolve_key(&c.station_a),
                    self.resolve_key(&c.station_b),
                ) {
                    (Some(a), Some(b)) => join.is_join_tie(a, b),
                    _ => false,
                }
            })
            .map(|c| c.id)
            .collect();
        Ok(CombineSetup {
            kept_end,
            other_end,
            rotation: self.azimuth_rotation(k, o),
            join,
            ties,
            join_closures,
        })
    }

    /// Give `other`'s shots that share an identity (uuid) with a shot of
    /// `kept` new ones, derived as the load-time heal derives them
    /// (`reminted_shot_uuid`), so a row stored against the old id in `other`
    /// is still recognised. Returns how many it renamed.
    ///
    /// A heal, not part of the combine's undo step: `execute_combine_sections`
    /// runs it before it pins position-keyed ignores and records the "before"
    /// snapshot, so the pins, the held component gauge and the snapshot all
    /// name each shot by the identity it keeps. Two sections sharing a shot
    /// identity is a state a reload heals the same way.
    pub fn renew_shared_shot_uuids(&mut self, kept: SectionId, other: SectionId) -> usize {
        let (Some(k), Some(o)) = (self.get_section(kept), self.get_section(other)) else {
            return 0;
        };
        let shared = Self::shared_shot_uuids(k, o);
        if shared.is_empty() {
            return 0;
        }
        let mut taken: HashSet<String> = self
            .sections
            .iter()
            .flat_map(|s| s.shots.iter().filter_map(|shot| shot.uuid.clone()))
            .collect();
        let mut remap = ShotUuidRemap::default();
        for old in &shared {
            let new = reminted_shot_uuid_avoiding(other, old, |c| taken.contains(c));
            taken.insert(new.clone());
            remap.insert(other, old.clone(), new);
        }
        self.apply_shot_uuid_remap(&remap);
        self.follow_counter_parity(&remap);
        shared.len()
    }

    /// Combine `other` into `kept` at `options.join` (or the join the
    /// two-section query prefers). Re-runs the candidate check first, so a
    /// refusal leaves the project exactly as it was. Past the check, the
    /// failures left (a section vanishing mid-way) are unreachable, but this
    /// does not roll back: `execute_combine_sections` restores its snapshot
    /// on any `Err`.
    ///
    /// Returns the ties it removed because, combined, the loop closure each
    /// is or would become joins the two ends of one shot (#746): the plan's
    /// `dropped` list, the one the dialog showed, named by the stations they
    /// had before the combine. [`Self::combine_sections_reporting`] also
    /// returns the closures it replaced (#749).
    pub fn combine_sections(
        &mut self,
        kept: SectionId,
        other: SectionId,
        options: &CombineSectionsOptions,
    ) -> Result<Vec<CombineRemovedClosure>, String> {
        self.combine_sections_reporting(kept, other, options)
            .map(|outcome| outcome.removed_across_one_shot)
    }

    /// [`Self::combine_sections`], returning both lists the dialog showed:
    /// the ties removed across one shot (#746), and the closures the
    /// combine's dedupe dropped for a closure the solve takes on the same two
    /// stations while not one itself (#749), as the plan's `replaced` list
    /// names them. What the dedupe really dropped is checked against the
    /// plan: a miss is a `debug_assert!` failure in a debug build, and in a
    /// release build an error in the log, with what really happened
    /// returned.
    pub fn combine_sections_reporting(
        &mut self,
        kept: SectionId,
        other: SectionId,
        options: &CombineSectionsOptions,
    ) -> Result<CombineOutcome, String> {
        // The steps below find closures by id (the join's own, the ties
        // `tear_ties` removes, the names of what the dedupe replaces), as
        // the plan finds them by position: unique ids make the two agree,
        // and the load gives a repeated one a fresh id (`assign_closure_ids`).
        debug_assert!(
            {
                let mut ids = HashSet::with_capacity(self.loop_closures.len());
                self.loop_closures.iter().all(|c| ids.insert(c.id))
            },
            "combine sections: two loop closures share an id"
        );
        let setup = self.combine_setup(kept, other, options.join)?;
        // For the check after the join that the plan's prediction left no
        // closure across one shot behind.
        let closures_before = self.closures_before_edit();
        // Every closure's stations before the combine, by id (unique: the
        // load gives a repeated one a fresh id, `assign_closure_ids`), to name
        // what the dedupe replaces; only where it can replace anything.
        let stations_before: HashMap<u64, (StationRef, StationRef)> =
            if self.combine_may_replace(kept, other) {
                let mut stations = HashMap::new();
                for c in &self.loop_closures {
                    if let (Some(a), Some(b)) = (
                        self.resolve_key(&c.station_a),
                        self.resolve_key(&c.station_b),
                    ) {
                        stations.entry(c.id).or_insert((a, b));
                    }
                }
                stations
            } else {
                HashMap::new()
            };
        // Shared shot identities first (a no-op when the command already
        // renewed them), so the held gauge below names shots by the identity
        // they keep.
        self.renew_shared_shot_uuids(kept, other);
        // Before anything moves, including the reverse below.
        let component_gauges = self.hold_component_gauges(&[kept, other]);
        let (remeasure, torn_into) = self.prepare_combine(&setup);
        let deduped = self.join_sections(kept, other, &setup, options)?;
        let replaced = Self::name_replaced(&deduped, &stations_before, &torn_into);
        Self::check_replaced_as_planned(&replaced, &setup.ties.replaced);
        // The component's lowest id may now be a third section's (the other
        // section's id is retired): hold the component at the station the
        // adjustment held before. After the parity toggle, which a held
        // station named by one shot reads.
        self.restore_component_gauges(&component_gauges);
        self.remeasure_closures(&remeasure);
        let mut removed: Vec<CombineRemovedClosure> = setup
            .ties
            .dropped
            .iter()
            .map(|(tie, kind)| CombineRemovedClosure {
                station_a: tie.a,
                station_b: tie.b,
                kind: *kind,
            })
            .collect();
        removed
            .extend(self.remove_unpredicted_closures_across_one_shot(&closures_before, &torn_into));
        self.touch();
        Ok(CombineOutcome {
            removed_across_one_shot: removed,
            replaced,
        })
    }

    /// The backstop to the plan's `replaced` list (#749): the closures the
    /// dedupe dropped for one the solve takes are the ones the dialog listed.
    /// Both come from the one rule (`closure_dedupe`), so a difference is the
    /// plan's model of the combined closure list missing something: a
    /// `debug_assert!` failure in a debug build, an error in the log in a
    /// release build (the combine returns what really happened).
    fn check_replaced_as_planned(
        replaced: &[CombineReplacedClosure],
        planned: &[CombineReplacedClosure],
    ) {
        debug_assert_eq!(
            replaced, planned,
            "combine replaced closures its plan did not predict"
        );
        if replaced != planned {
            tracing::error!(
                replaced = replaced.len(),
                planned = planned.len(),
                purpose = "list exactly the closures a combine replaces",
                "Combine replaced closures its plan did not predict"
            );
        }
    }

    /// The backstop to the plan's prediction (#746): any closure the combined
    /// project skips now but did not before the combine is one the plan's
    /// model of the joined survey missed. That is the preview/execution drift
    /// the dialog listing exists to prevent, so in a debug build (every test,
    /// fuzzer and debug app build) it is a `debug_assert!` failure. In a
    /// release build the closure is removed all the same, so a combine never
    /// leaves a NEW closure across one shot, with an error in the log. Each is
    /// named by its stations before the combine and as what it was then: a
    /// closure a tie of this combine became (`torn_into`, from `tear_ties`)
    /// by that tie, a merge or snapped closure; any other by its own stations
    /// (`before`), as a loop closure.
    fn remove_unpredicted_closures_across_one_shot(
        &mut self,
        before: &ClosuresBeforeEdit,
        torn_into: &HashMap<u64, Tie>,
    ) -> Vec<CombineRemovedClosure> {
        let missed = self.closures_newly_across_one_shot(before);
        debug_assert!(
            missed.is_empty(),
            "combine left closures across one shot its plan did not predict: {missed:?}"
        );
        if missed.is_empty() {
            return Vec::new();
        }
        tracing::error!(
            missed = missed.len(),
            purpose = "never leave a closure across one shot behind a combine",
            "Combine left closures across one shot its plan did not predict; removing them"
        );
        let ids: HashSet<u64> = missed.iter().map(|c| c.closure_id).collect();
        self.loop_closures.retain(|c| !ids.contains(&c.id));
        Self::name_missed(&missed, torn_into)
    }

    /// Name each closure the backstop removes as the user knew it before the
    /// combine (see [`Self::remove_unpredicted_closures_across_one_shot`]).
    pub(super) fn name_missed(
        missed: &[ClosurePair],
        torn_into: &HashMap<u64, Tie>,
    ) -> Vec<CombineRemovedClosure> {
        missed
            .iter()
            .map(|c| match torn_into.get(&c.closure_id) {
                Some(tie) => CombineRemovedClosure {
                    station_a: tie.a,
                    station_b: tie.b,
                    kind: match tie.id {
                        TieId::Merge(_) => CombineRemovedKind::Merge,
                        TieId::Closure(_) => CombineRemovedKind::SnappedClosure,
                    },
                },
                None => CombineRemovedClosure {
                    station_a: c.station_a,
                    station_b: c.station_b,
                    kind: CombineRemovedKind::LoopClosure,
                },
            })
            .collect()
    }

    /// Everything that has to give before the join (see the module docs).
    /// Returns the closures to measure again once the geometry is final, and
    /// the tie each closure a torn tie became came from.
    fn prepare_combine(&mut self, setup: &CombineSetup) -> (HashSet<u64>, HashMap<u64, Tie>) {
        let (kept, other) = (setup.join.kept, setup.join.other);
        if let Some(rotation) = setup.rotation {
            let reference = self.get_section(kept).and_then(|k| k.azimuth_reference);
            if let Some(section) = self.get_section_mut(other) {
                if rotation.abs() > 0.0 {
                    for shot in &mut section.shots {
                        shot.azimuth_in = normalize_azimuth(shot.azimuth_in + rotation);
                        shot.azimuth_out = normalize_azimuth(shot.azimuth_out + rotation);
                    }
                }
                section.azimuth_reference = reference;
            }
        }
        self.loop_closures
            .retain(|c| !setup.join_closures.contains(&c.id));
        let dropped: Vec<Tie> = setup.ties.dropped.iter().map(|(tie, _)| *tie).collect();
        let torn_into: HashMap<u64, Tie> = self
            .tear_ties(&setup.ties.torn, &dropped)
            .into_iter()
            .collect();
        let mut remeasure: HashSet<u64> = torn_into.keys().copied().collect();
        // Each section that moves, by how much: the moving side by the shift,
        // what is merged at the join station whose hand move is dropped by
        // that station's move onto the joined station (#755).
        let mut moves: HashMap<SectionId, (f64, f64)> = setup
            .ties
            .moved
            .iter()
            .map(|id| (*id, setup.ties.shift))
            .collect();
        if let Some((_, toward)) = setup.ties.dropped_hand_move {
            moves.extend(setup.ties.joined_moved.iter().map(|id| (*id, toward)));
        }
        if !moves.is_empty() {
            // A closure whose two ends move apart changes length.
            let by = |id: &SectionId| moves.get(id).copied().unwrap_or((0.0, 0.0));
            remeasure.extend(
                self.loop_closures
                    .iter()
                    .filter(|c| by(&c.station_a.section_id) != by(&c.station_b.section_id))
                    .map(|c| c.id),
            );
            let mut ids: Vec<SectionId> = moves.keys().copied().collect();
            ids.sort_unstable();
            for id in ids {
                let (dx, dy) = moves[&id];
                self.adjust_section_offset(id, dx, dy);
            }
        }
        (remeasure, torn_into)
    }

    /// Join the shots and the two join stations (see the module docs).
    ///
    /// Returns what the closure dedupe dropped for a closure the solve takes
    /// (`Project::dedupe_closures_in`).
    fn join_sections(
        &mut self,
        kept: SectionId,
        other: SectionId,
        setup: &CombineSetup,
        options: &CombineSectionsOptions,
    ) -> Result<Vec<(LoopClosure, LoopClosure)>, String> {
        let (kept_end, other_end) = (setup.kept_end, setup.other_end);
        if kept_end == other_end && !self.reverse_section(other, options.swap_lr) {
            return Err(format!("Section {other} not found"));
        }
        // After the reverse the other section meets the kept one start to end.
        let append = kept_end == SectionEnd::End;

        let (Some(k), Some(o)) = (self.get_section(kept), self.get_section(other)) else {
            return Err(format!("Sections {kept} and {other} not found"));
        };
        let (nk, no) = (k.shots.len() as u32, o.shots.len() as u32);
        let join_k = StationRef::new(kept, if append { nk } else { 0 });
        let join_o = StationRef::new(other, if append { 0 } else { no });
        let other_shots: Vec<ShotData> = o.shots.iter().map(Shot::to_shot_data).collect();
        let parity_flip = k.reverse_parity != o.reverse_parity;
        let reference = self.azimuth_reference_for(k);
        // The kept section's own gauge: the lower section id's, which is the
        // one the pair's component used when neither is held elsewhere.
        // `restore_component_gauges` settles the component's anchor.
        let source = if kept < other { k } else { o };
        let gauge = Self::gauge_index(source) as u32;
        let gauge = match (source.id == kept, append) {
            (true, true) | (false, false) => gauge,
            (true, false) => gauge + no,
            (false, true) => nk + gauge,
        };
        let gauge_capture = captured_gauge(source);

        // Settle the join first (it touches keyed entries only, never the
        // structure), then resolve every stored key against the structure as
        // it still is, so the kept join station's freshly written key is
        // re-keyed with the rest.
        self.resolve_combine_join(join_k, join_o, options, setup.ties.kept_offset_wins);
        let stored = self.resolve_stored_keys_in(&[kept, other]);

        // ---- The shots ------------------------------------------------------
        self.remove_section(other);
        self.remove_section_offset(other);
        self.rebuild_section_index();
        let section = self
            .get_section_mut(kept)
            .ok_or_else(|| format!("Section {kept} not found"))?;
        for data in other_shots.iter().cloned() {
            section.add_shot(data);
        }
        if !append {
            section.shots.rotate_right(no as usize);
        }
        // An empty name leaves the section unnamed, as it may have been.
        section.name = Some(options.name.clone()).filter(|name| !name.trim().is_empty());
        section.color.clone_from(&options.color);
        self.reindex_section_shot_uuids(kept);
        self.reindex_section_shot_uuids(other);
        if !append {
            // Station 0 is now the other section's start: move the anchor back
            // by the other section's span so the kept stations stay put.
            let (dx, dy) = shot_run_displacement(&other_shots, reference);
            if dx.is_finite() && dy.is_finite() {
                self.adjust_section_offset(kept, -dx, -dy);
            }
        }

        // ---- Every stored reference follows its station --------------------
        let homes: HashMap<StationKey, StationRef> = stored
            .into_iter()
            .filter_map(|(key, at)| {
                let index = if at.section_id == other {
                    if append {
                        nk + at.station_index
                    } else {
                        at.station_index
                    }
                } else if append {
                    return None;
                } else {
                    at.station_index + no
                };
                Some((key, StationRef::new(kept, index)))
            })
            .collect();
        let unmoved = self.rekey_station_refs(&homes, "combine sections");
        let deduped = self.dedupe_closures_in(kept);
        self.dedupe_merges_in(kept);
        self.keep_gauge_on_station(kept, gauge as usize, gauge_capture.as_ref());
        if parity_flip {
            // The other section's shots now live under the kept section's
            // parity: toggle them so each keeps its own record parity.
            for uuid in other_shots.iter().filter_map(|data| data.uuid.as_ref()) {
                if !self.counter_parity_shots.remove(uuid) {
                    self.counter_parity_shots.insert(uuid.clone());
                }
            }
        }
        if cfg!(debug_assertions) {
            self.for_each_station_ref(|label, key| {
                debug_assert!(
                    key.section_id != other || unmoved.contains(&key),
                    "combine sections: a {label} still names the retired section {other}"
                );
            });
        }
        Ok(deduped)
    }

    /// Make the two join stations one, while the structure is still the old
    /// one: drop the ties between them and settle the keyed texts and offsets
    /// onto the kept join station's key (the re-key afterwards moves that key
    /// when the kept station's anchor changes).
    fn resolve_combine_join(
        &mut self,
        join_k: StationRef,
        join_o: StationRef,
        options: &CombineSectionsOptions,
        kept_offset_wins: bool,
    ) {
        let between = |project: &Project, x: &StationKey, y: &StationKey| {
            let (x, y) = (project.resolve_key(x), project.resolve_key(y));
            (x == Some(join_k) && y == Some(join_o)) || (x == Some(join_o) && y == Some(join_k))
        };
        let merges: HashSet<usize> = self
            .station_merges
            .iter()
            .enumerate()
            .filter(|(_, m)| between(self, &m.station_a, &m.station_b))
            .map(|(i, _)| i)
            .collect();
        let mut index = 0;
        self.station_merges.retain(|_| {
            index += 1;
            !merges.contains(&(index - 1))
        });
        let closures: HashSet<u64> = self
            .loop_closures
            .iter()
            .filter(|c| between(self, &c.station_a, &c.station_b))
            .map(|c| c.id)
            .collect();
        self.loop_closures.retain(|c| !closures.contains(&c.id));

        let Some(key) = self.key_for_ref(join_k) else {
            return;
        };
        let mut comments = std::mem::take(&mut self.station_comments);
        let comment_k = take_naming(self, &mut comments, join_k);
        let comment_o = take_naming(self, &mut comments, join_o);
        if let Some(text) = pick_text(comment_k, comment_o, options.comment_pick, "\n") {
            comments.insert(key.clone(), text);
        }
        self.station_comments = comments;
        let mut names = std::mem::take(&mut self.station_names);
        let name_k = take_naming(self, &mut names, join_k);
        let name_o = take_naming(self, &mut names, join_o);
        if let Some(text) = pick_text(name_k, name_o, options.name_pick, " / ") {
            names.insert(key.clone(), text);
        }
        self.station_names = names;
        // The fixed side's own offset (or none) wins, so that side stays
        // exactly in place; the plan reports how far the other one moves.
        let mut offsets = std::mem::take(&mut self.station_offsets);
        let offset_k = take_naming(self, &mut offsets, join_k);
        let offset_o = take_naming(self, &mut offsets, join_o);
        if let Some(offset) = if kept_offset_wins { offset_k } else { offset_o } {
            offsets.insert(key, offset);
        }
        self.station_offsets = offsets;
    }

    /// Drop merges onto `section_id` that the combine made redundant: one
    /// that now ties a station to itself, or ties two stations that other
    /// merges already make one point. At a junction a branch merged to both
    /// join stations (directly, or one of them through the other) ends up
    /// merged twice to the joined station, or merged to it and to a third
    /// branch that is merged to it too. The merges elsewhere are read first,
    /// so only a merge onto the combined section is ever dropped.
    fn dedupe_merges_in(&mut self, section_id: SectionId) {
        let resolved: Vec<Option<(StationRef, StationRef)>> = self
            .station_merges
            .iter()
            .map(|m| {
                Some((
                    self.resolve_key(&m.station_a)?,
                    self.resolve_key(&m.station_b)?,
                ))
            })
            .collect();
        let mut parent: HashMap<StationRef, StationRef> = HashMap::new();
        fn root(parent: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
            let mut at = x;
            while let Some(&up) = parent.get(&at) {
                if up == at {
                    break;
                }
                at = up;
            }
            parent.insert(x, at);
            at
        }
        let onto = |m: &crate::StationMerge| {
            m.station_a.section_id == section_id || m.station_b.section_id == section_id
        };
        for (merge, ends) in self.station_merges.iter().zip(&resolved) {
            if let (false, Some((a, b))) = (onto(merge), ends) {
                let (ra, rb) = (root(&mut parent, *a), root(&mut parent, *b));
                parent.insert(ra, rb);
            }
        }
        let keep: Vec<bool> = self
            .station_merges
            .iter()
            .zip(&resolved)
            .map(|(merge, ends)| {
                let (true, Some((a, b))) = (onto(merge), ends) else {
                    return true;
                };
                let (ra, rb) = (root(&mut parent, *a), root(&mut parent, *b));
                if ra == rb {
                    return false;
                }
                parent.insert(ra, rb);
                true
            })
            .collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        let mut keep = keep.into_iter();
        self.station_merges.retain(|_| keep.next().unwrap_or(true));
    }
}
