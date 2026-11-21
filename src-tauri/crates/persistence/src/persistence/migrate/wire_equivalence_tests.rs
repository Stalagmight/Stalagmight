//! The wire projection (`models::survey::wire`, plan 9.2) against the legacy
//! model's own payloads, on legacy files: each generated legacy project (and
//! each tracked fixture) is opened by the frozen 0.6.0 reader, which gives
//! the payloads 0.6.0 sent (`compute_station_coordinates`, the IPC
//! `Project`, the Computed View stations, the previews), and migrated to the
//! station graph, whose wire projection must send the same thing field by
//! field except for the declared differences.
//!
//! Every difference is classified; a class is a declared change (plan 12.2,
//! 9.2, 9.3 and the stream deviations) and is counted, anything else fails
//! the test with the first few examples. `Tally::classes` names them; the
//! sweep (`STALAGMIGHT_RUST_WIRE_SEEDS=<n>`, ignored) prints the counts.
//!
//! Positions are waived per component and checked per slot, never per case
//! (J-docs-tests-truth-5): a slot may sit elsewhere than 0.6.0 sent it only
//! in a component where the migration moved a slot (the migrated model's
//! position against 0.6.0's, which `verify.rs` classified: its verification
//! of that view must be clean), and only to exactly the migrated model's own
//! position, with 0.6.0's payload exactly 0.6.0's own position. So a
//! regression of the wire projection (a hand-move z applied twice, a slot
//! mapped to the wrong station) fails here even in a case the migration
//! moved, and so does any move outside a moved component. The derived
//! figures that read positions (a GPS fix's gap, a far fix, a closure
//! preview's misclosure, a merge's datum shift) are waived per component the
//! same way.
//!
//! Synthetic data only (the generator's), plus the two tracked fixtures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use stalagmight_legacy::models as m;
use stalagmight_legacy::{LegacySurvey, OpenOptions};

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::{AdjustedPositions, ClosureStrategy, Project, RawPositions};
use crate::models::{StationId, StationRef};

use super::merges::UnionFind;
use super::test_support::{random_case, Rng};
use super::{migrate, Migrated};

/// Seeds of the legacy generator the gate runs.
const SEEDS: u64 = 48;
/// Raw and Computed View XY agree within this (the migration verifies the
/// tighter per-slot bound; this only has to tell a match from a move).
const XY_TOL: f64 = 1e-5;
const Z_TOL: f64 = 1e-9;
/// A wire coordinate is the model position it projects to within this (the
/// JSON round trip of an f64 is exact; this only absorbs nothing).
const SAME_POSITION_TOL: f64 = 1e-12;
/// Closure metrics and gaps are compared within this.
const METRIC_TOL: f64 = 1e-6;

/// The legacy IPC view of a legacy project (`serialize_project_ipc`).
struct Ipc<'a>(&'a m::Project);

impl Serialize for Ipc<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        m::serialize_project_ipc(self.0, s)
    }
}

fn json(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn close(a: &Value, b: &Value, tol: f64) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= tol || (x.is_nan() && y.is_nan()),
        _ => a == b,
    }
}

fn slot_of(value: &Value) -> StationRef {
    serde_json::from_value(value.clone()).unwrap()
}

/// Declared difference classes (counted) and unexplained ones (failures).
#[derive(Debug, Default)]
pub(super) struct Tally {
    pub(super) classes: BTreeMap<&'static str, usize>,
    pub(super) unexplained: Vec<String>,
    cases: usize,
}

impl Tally {
    fn note(&mut self, class: &'static str) {
        *self.classes.entry(class).or_default() += 1;
    }

    fn fail(&mut self, what: String) {
        self.unexplained.push(what);
    }

    fn absorb(&mut self, other: Tally) {
        for (class, n) in other.classes {
            *self.classes.entry(class).or_default() += n;
        }
        self.unexplained.extend(other.unexplained);
        self.cases += other.cases;
    }

    fn assert_clean(&self) {
        assert!(
            self.unexplained.is_empty(),
            "{} unexplained wire differences in {} cases, first: {:#?}\nclasses: {:#?}",
            self.unexplained.len(),
            self.cases,
            &self.unexplained[..self.unexplained.len().min(12)],
            self.classes
        );
    }
}

/// The station graph's delete preview, with what the comparison needs.
struct NewDelete {
    impact: Value,
    /// The delete removes a datum (its piece may lose its placement, and a
    /// closure to another placed piece then promotes).
    unplaces: bool,
    /// Closures attached to the removed slot (on its station, `via` its
    /// line) that the sweep promotes: today's `closure_to_merge` scope.
    member_promoted: BTreeSet<u64>,
    /// The `Tie`s on the removed slot that left with it (S13).
    member_ties: BTreeSet<u64>,
    /// The sweep's other promotions.
    other_promotions: usize,
    /// The removed station holds a datum.
    datum_on_station: bool,
}

impl NewDelete {
    fn of(p: &Project, preview: &crate::models::survey::DeleteShotPreview) -> Self {
        let on_slot = |c: &crate::models::survey::LoopClosure| {
            (c.a == preview.removed_station && c.a_via == Some(preview.line))
                || (c.b == preview.removed_station && c.b_via == Some(preview.line))
        };
        let original = |id: u64| p.closures.iter().find(|c| c.id == id);
        let member_promoted: BTreeSet<u64> = preview
            .promoted_closures
            .iter()
            .map(|promotion| promotion.closure.id)
            .filter(|&id| original(id).is_some_and(on_slot))
            .collect();
        let member_ties = preview
            .removed_closures
            .iter()
            .filter(|c| c.strategy == ClosureStrategy::Tie && on_slot(c))
            .map(|c| c.id)
            .collect();
        Self {
            impact: json(p.delete_shot_impact(preview)),
            unplaces: !preview.removed_datums.is_empty(),
            other_promotions: preview.promoted_closures.len() - member_promoted.len(),
            member_promoted,
            member_ties,
            datum_on_station: p
                .datums
                .iter()
                .any(|d| d.station == preview.removed_station),
        }
    }
}

/// One legacy file: its two models and what the comparison needs to know
/// about the migration.
struct Case<'a> {
    what: String,
    lp: &'a m::Project,
    legacy: &'a LegacySurvey,
    p: &'a Project,
    migrated: &'a Migrated,
    /// The migrated project's raw layout and Closures Computed View.
    raw: RawPositions,
    adjusted: AdjustedPositions,
    /// Per view (`[raw, computed view]`), the components where the migrated
    /// model's position of some slot differs from 0.6.0's: the only ones
    /// whose positions may differ on the wire. Raw: shot components;
    /// computed view: least-squares components (shot components joined by
    /// closures, as `verify.rs` groups them).
    moved_components: [BTreeSet<usize>; 2],
    /// Per shot component, its least-squares component.
    solve_of: Vec<usize>,
    /// Stations tied by `Tie` closures, grouped (a demoted merge's two
    /// stations are in one group).
    tie_group: BTreeMap<StationId, StationId>,
    /// Slots whose legacy comment is the empty string (no comment now).
    empty_comments: BTreeSet<StationRef>,
    /// Closure ids the legacy file did not have (`Tie`s the migration made
    /// from demoted merges, reminted ids).
    new_closure_ids: BTreeSet<u64>,
    tally: Tally,
}

impl<'a> Case<'a> {
    fn new(what: String, legacy: &'a LegacySurvey, migrated: &'a Migrated) -> Self {
        let p = &migrated.project;
        let mut parent: BTreeMap<StationId, StationId> = BTreeMap::new();
        fn root(parent: &BTreeMap<StationId, StationId>, mut s: StationId) -> StationId {
            while let Some(&up) = parent.get(&s) {
                if up == s {
                    break;
                }
                s = up;
            }
            s
        }
        for closure in p
            .closures
            .iter()
            .filter(|c| c.strategy == ClosureStrategy::Tie)
        {
            let (ra, rb) = (root(&parent, closure.a), root(&parent, closure.b));
            if ra != rb {
                parent.insert(ra.max(rb), ra.min(rb));
            }
        }
        let tie_group = p.stations.keys().map(|&s| (s, root(&parent, s))).collect();
        let old_ids: BTreeSet<u64> = legacy.project.loop_closures.iter().map(|c| c.id).collect();
        let raw = p.raw_positions();
        let adjusted = p.adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default());
        let topology = p.topology();
        let mut uf = UnionFind::new(topology.components().len());
        for c in &p.closures {
            if let (Some(a), Some(b)) = (topology.component_of(c.a), topology.component_of(c.b)) {
                uf.union(a, b);
            }
        }
        let solve_of = (0..topology.components().len())
            .map(|c| uf.find(c))
            .collect();
        let mut case = Self {
            what,
            lp: &legacy.project,
            legacy,
            p,
            raw,
            adjusted,
            moved_components: [BTreeSet::new(), BTreeSet::new()],
            solve_of,
            empty_comments: BTreeSet::new(),
            migrated,
            tie_group,
            new_closure_ids: p
                .closures
                .iter()
                .map(|c| c.id)
                .filter(|id| !old_ids.contains(id))
                .collect(),
            tally: Tally {
                cases: 1,
                ..Tally::default()
            },
        };
        for adjusted in [false, true] {
            let mut moved = BTreeSet::new();
            for line in p.lines() {
                for index in 0..line.station_count() {
                    let slot = StationRef::new(line.id, index as u32);
                    let (new, old) = case.model_positions(slot, adjusted);
                    let same = match (new, old) {
                        (Some(n), Some(o)) => {
                            (n[0] - o[0]).abs() <= XY_TOL
                                && (n[1] - o[1]).abs() <= XY_TOL
                                && (n[2] - o[2]).abs() <= Z_TOL
                        }
                        _ => false,
                    };
                    if let (false, Some(component)) = (same, case.component(slot, adjusted)) {
                        moved.insert(component);
                    }
                }
            }
            case.moved_components[usize::from(adjusted)] = moved;
        }
        case
    }

    /// The component of `slot` in a view (module docs).
    fn component(&self, slot: StationRef, adjusted: bool) -> Option<usize> {
        let component = self.p.topology().component_of(self.station(slot)?)?;
        if adjusted {
            self.solve_of.get(component).copied()
        } else {
            Some(component)
        }
    }

    /// The migrated model's own position of `slot` in a view, and 0.6.0's
    /// (what `verify.rs` compares).
    fn model_positions(
        &self,
        slot: StationRef,
        adjusted: bool,
    ) -> (Option<[f64; 3]>, Option<[f64; 3]>) {
        let legacy_slot = stalagmight_legacy::Slot {
            line_id: slot.line_id,
            station_index: slot.station_index,
        };
        if adjusted {
            (
                self.adjusted.slot(self.p, slot),
                self.legacy
                    .adjusted_positions
                    .as_ref()
                    .and_then(|a| a.get(&legacy_slot))
                    .copied(),
            )
        } else {
            (
                self.raw.slot(self.p, slot),
                self.legacy.raw_positions.get(&legacy_slot).copied(),
            )
        }
    }

    /// Whether the migration moved positions in the component of `slot` in a
    /// view (the raw one for the derived figures).
    fn moved_component(&self, slot: StationRef, adjusted: bool) -> bool {
        self.component(slot, adjusted)
            .is_some_and(|c| self.moved_components[usize::from(adjusted)].contains(&c))
    }

    /// Why a slot whose wire position differs from 0.6.0's is not explained
    /// by the migration, if it is not (module docs): the move must be in a
    /// moved component of a view whose verification is clean, and be exactly
    /// the model's move.
    fn unexplained_move(
        &self,
        slot: StationRef,
        o: &Value,
        n: &Value,
        adjusted: bool,
    ) -> Option<String> {
        let v = &self.migrated.report.verification;
        let verified = if adjusted {
            matches!(&v.adjusted, super::AdjustedCheck::Compared(check) if check.mismatches.is_empty())
        } else {
            v.mismatches.is_empty()
        };
        if !verified {
            return Some("the migration's own verification of this view is not clean".to_string());
        }
        if !self.moved_component(slot, adjusted) {
            return Some("the migration moved nothing in this component".to_string());
        }
        let (Some(model), Some(legacy)) = self.model_positions(slot, adjusted) else {
            return Some("no model position to compare".to_string());
        };
        let shows = |wire: &Value, at: [f64; 3]| {
            ["x", "y", "z"]
                .iter()
                .zip(at)
                .all(|(key, want)| match wire[*key].as_f64() {
                    Some(got) => (got - want).abs() <= SAME_POSITION_TOL,
                    // JSON has no NaN: 0.6.0's payload writes a non-finite one as null.
                    None => wire[*key].is_null() && !want.is_finite(),
                })
        };
        if !shows(n, model) {
            return Some(format!(
                "the wire does not show the migrated model's position {model:?}"
            ));
        }
        if !shows(o, legacy) {
            return Some(format!(
                "0.6.0's payload does not show 0.6.0's position {legacy:?}"
            ));
        }
        None
    }

    fn fail(&mut self, what: String) {
        let what = format!("{}: {what}", self.what);
        self.tally.fail(what);
    }

    fn station(&self, slot: StationRef) -> Option<StationId> {
        self.p.station_at(slot)
    }

    fn junction(&self, slot: StationRef) -> bool {
        self.station(slot)
            .is_some_and(|s| self.p.topology().is_junction(s))
    }

    fn tied(&self, a: StationRef, b: StationRef) -> bool {
        match (self.station(a), self.station(b)) {
            (Some(a), Some(b)) => self.tie_group.get(&a) == self.tie_group.get(&b),
            _ => false,
        }
    }

    fn demotions(&self) -> bool {
        let r = &self.migrated.report;
        r.merges_demoted_cycle + r.merges_demoted_same_line > 0
    }

    // --- the per-slot station list -------------------------------------

    fn stations(&mut self, view: &'static str, old: &Value, new: &Value, adjusted: bool) {
        let (Some(old), Some(new)) = (old.as_array(), new.as_array()) else {
            return self.fail(format!("{view}: not lists"));
        };
        if old.len() != new.len() {
            return self.fail(format!("{view}: {} slots, now {}", old.len(), new.len()));
        }
        for (o, n) in old.iter().zip(new) {
            if o["station_ref"] != n["station_ref"] {
                return self.fail(format!("{view}: slot order at {}", o["station_ref"]));
            }
            let slot = slot_of(&o["station_ref"]);
            self.slot_station(view, slot, o, n, adjusted);
        }
    }

    fn slot_station(&mut self, view: &str, slot: StationRef, o: &Value, n: &Value, adjusted: bool) {
        let (oc, nc) = (&o["coordinates"], &n["coordinates"]);
        let moved = !close(&oc["x"], &nc["x"], XY_TOL)
            || !close(&oc["y"], &nc["y"], XY_TOL)
            || !close(&oc["z"], &nc["z"], Z_TOL);
        if moved {
            match self.unexplained_move(slot, oc, nc, adjusted) {
                None => self.tally.note(if adjusted {
                    "computed view: slot moved by a migration class (TieSolved / TieNoLongerFails / MergeTieSkipped / LegacyWeightsDegenerate / LegacyRewriteFailed / LegacySolvePrecision), to the migrated model's position"
                } else {
                    "raw: slot moved by a migration class (torn merge collapsed), to the migrated model's position"
                }),
                Some(why) => self.fail(format!("{view} {slot}: coordinates {oc} -> {nc}: {why}")),
            }
        }
        if adjusted {
            return;
        }
        let junction = self.junction(slot);
        for (key, class) in [
            (
                "id",
                "identity: a junction shares the survivor's Station ID",
            ),
            ("name", "identity: a junction shares the survivor's name"),
            (
                "comment",
                "identity: a junction shares one (joined) comment",
            ),
        ] {
            if o.get(key) != n.get(key) {
                if key == "comment"
                    && o.get(key) == Some(&Value::String(String::new()))
                    && n.get(key).is_none()
                {
                    self.empty_comments.insert(slot);
                    self.tally
                        .note("identity: an empty legacy comment is no comment");
                } else if junction {
                    self.tally.note(class);
                } else if key == "name" && self.migrated.report.stations_renamed > 0 {
                    self.tally.note(
                        "identity: a name letter-suffixed by the migration (unique per line)",
                    );
                } else {
                    self.fail(format!(
                        "{view} {slot}: {key} {:?} -> {:?}",
                        o.get(key),
                        n.get(key)
                    ));
                }
            }
        }
        self.merged_with(slot, o, n);
        self.closures_with(slot, o, n);
        if o["is_zero_datum"] != n["is_zero_datum"] || o["gps_coordinates"] != n["gps_coordinates"]
        {
            if junction && n["is_zero_datum"] == Value::Bool(true) {
                self.tally
                    .note("datum: every slot of a datum station shows the datum its dialog edits");
            } else {
                self.fail(format!(
                    "{view} {slot}: datum {} {} -> {} {}",
                    o["is_zero_datum"],
                    o["gps_coordinates"],
                    n["is_zero_datum"],
                    n["gps_coordinates"]
                ));
            }
        }
    }

    fn merged_with(&mut self, slot: StationRef, o: &Value, n: &Value) {
        let set = |v: &Value| -> BTreeSet<StationRef> {
            v.as_array()
                .map(|a| a.iter().map(slot_of).collect())
                .unwrap_or_default()
        };
        let (old, new) = (set(&o["merged_with"]), set(&n["merged_with"]));
        if old == new {
            return;
        }
        let gone: Vec<StationRef> = old.difference(&new).copied().collect();
        let added: Vec<StationRef> = new.difference(&old).copied().collect();
        if added.is_empty() && gone.iter().all(|&g| self.tied(slot, g)) {
            self.tally.note(
                "merged_with: a demoted (cycle / same-line) merge is a Tie closure, not a junction",
            );
        } else {
            self.fail(format!(
                "{slot}: merged_with lost {gone:?}, gained {added:?}"
            ));
        }
    }

    fn closures_with(&mut self, slot: StationRef, o: &Value, n: &Value) {
        type Entry = (u64, StationRef, Option<StationRef>, bool);
        let set = |v: &Value| -> BTreeSet<Entry> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| {
                            (
                                r["closure_id"].as_u64().unwrap_or(0),
                                slot_of(&r["station_ref"]),
                                (!r["via_station"].is_null()).then(|| slot_of(&r["via_station"])),
                                r["is_transitive"] == Value::Bool(true),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (old, new) = (set(&o["closures_with"]), set(&n["closures_with"]));
        if old == new {
            return;
        }
        for entry in old.symmetric_difference(&new) {
            if self.new_closure_ids.contains(&entry.0) {
                self.tally
                    .note("closures_with: entries of a Tie closure the migration made from a demoted merge");
            } else if self.demotions() {
                self.tally
                    .note("closures_with: transitive entries over a merge group a demotion split");
            } else {
                self.fail(format!("{slot}: closures_with differs at {entry:?}"));
            }
        }
    }

    // --- the IPC Project ----------------------------------------------

    fn project(&mut self, old: &Value, new: &Value) {
        let mut old_keys: BTreeSet<&String> = old
            .as_object()
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        if old_keys.remove(&"line_offsets".to_string()) {
            self.tally
                .note("project: line_offsets dropped from the wire");
        }
        let new_keys: BTreeSet<&String> = new
            .as_object()
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        if old_keys != new_keys {
            return self.fail(format!("project keys {old_keys:?} -> {new_keys:?}"));
        }
        for key in [
            "name",
            "location",
            "created_at",
            "modified_at",
            "file_path",
            "next_line_id",
            "imported_file_hashes",
            "imported_line_hashes",
            "validation",
            "surface_elevation",
            "default_azimuth_reference",
            "cave_sections",
            "next_cave_section_id",
        ] {
            if old[key] != new[key] {
                self.fail(format!("project.{key}: {} -> {}", old[key], new[key]));
            }
        }
        if old["next_closure_id"] != new["next_closure_id"] {
            if self.new_closure_ids.is_empty() {
                self.fail(format!(
                    "next_closure_id {} -> {}",
                    old["next_closure_id"], new["next_closure_id"]
                ));
            } else {
                self.tally
                    .note("project: next_closure_id past the Tie closures the migration made");
            }
        }
        self.lines(&old["lines"], &new["lines"]);
        self.station_merges(&old["station_merges"], &new["station_merges"]);
        self.zero_datums(&old["zero_datums"], &new["zero_datums"]);
        self.loop_closures(&old["loop_closures"], &new["loop_closures"]);
    }

    fn lines(&mut self, old: &Value, new: &Value) {
        let (Some(old), Some(new)) = (old.as_array(), new.as_array()) else {
            return self.fail("lines: not lists".to_string());
        };
        if old.len() != new.len() {
            return self.fail("line count".to_string());
        }
        for (o, n) in old.iter().zip(new) {
            let id = &o["id"];
            for key in [
                "id",
                "name",
                "color",
                "direction",
                "quality",
                "next_shot_id",
                "azimuth_reference",
                "cave_section_id",
                "surveyor",
                "date",
            ] {
                if key == "date" && self.migrated.report.dates_clamped > 0 {
                    if o.get(key) != n.get(key) {
                        self.tally
                            .note("line: a date outside 0001-9999 clamped (F10) by the migration");
                    }
                } else if o.get(key) != n.get(key) {
                    self.fail(format!(
                        "line {id}.{key}: {:?} -> {:?}",
                        o.get(key),
                        n.get(key)
                    ));
                }
            }
            for key in ["reverse_parity", "gauge_shot_uuid", "gauge_neighbour_uuid"] {
                if o[key] != n[key] {
                    self.tally
                        .note("line: legacy gauge bookkeeping (reverse_parity, gauge pair) is false / null");
                }
            }
            if o["shots"] != n["shots"] {
                if self.migrated.report.numbers_sanitized > 0
                    || self.migrated.report.shot_uuids_minted > 0
                    || self.migrated.report.dates_clamped > 0
                {
                    self.tally.note(
                        "line: shots sanitized, uuids minted or dates clamped (F10) by the migration",
                    );
                } else {
                    let first = o["shots"]
                        .as_array()
                        .zip(n["shots"].as_array())
                        .and_then(|(a, b)| {
                            a.iter().zip(b).enumerate().find_map(|(i, (x, y))| {
                                let keys = x.as_object()?.keys();
                                keys.filter(|k| x.get(*k) != y.get(*k))
                                    .map(|k| format!("shot {i}.{k}: {:?} -> {:?}", x[k], y.get(k)))
                                    .next()
                            })
                        })
                        .unwrap_or_default();
                    self.fail(format!("line {id}: shots differ ({first})"));
                }
            }
        }
    }

    /// The merge pairs as a partition of slots: today's groups (union of the
    /// stored merges) against the junctions (one station's slots).
    fn station_merges(&mut self, old: &Value, new: &Value) {
        fn groups(pairs: &Value) -> BTreeSet<BTreeSet<StationRef>> {
            let mut parent: BTreeMap<StationRef, StationRef> = BTreeMap::new();
            fn root(parent: &BTreeMap<StationRef, StationRef>, mut s: StationRef) -> StationRef {
                while let Some(&up) = parent.get(&s) {
                    if up == s {
                        break;
                    }
                    s = up;
                }
                s
            }
            for pair in pairs.as_array().into_iter().flatten() {
                let (a, b) = (slot_of(&pair["station_a"]), slot_of(&pair["station_b"]));
                if a == b {
                    continue;
                }
                parent.entry(a).or_insert(a);
                parent.entry(b).or_insert(b);
                let (ra, rb) = (root(&parent, a), root(&parent, b));
                if ra != rb {
                    parent.insert(ra.max(rb), ra.min(rb));
                }
            }
            let mut out: BTreeMap<StationRef, BTreeSet<StationRef>> = BTreeMap::new();
            for &s in parent.keys() {
                out.entry(root(&parent, s)).or_default().insert(s);
            }
            out.into_values().collect()
        }
        let pairs = |v: &Value| -> BTreeSet<(StationRef, StationRef)> {
            v.as_array()
                .into_iter()
                .flatten()
                .map(|m| {
                    let (a, b) = (slot_of(&m["station_a"]), slot_of(&m["station_b"]));
                    (a.min(b), a.max(b))
                })
                .collect()
        };
        if pairs(old) != pairs(new) {
            self.tally
                .note("station_merges: derived star pairs (first slot to each other slot)");
        }
        let (old_groups, new_groups) = (groups(old), groups(new));
        if old_groups == new_groups {
            return;
        }
        // Each new junction lies inside one old group; an old group is the
        // union of junctions joined by demoted merges (Ties).
        for group in &old_groups {
            let inside: Vec<&BTreeSet<StationRef>> =
                new_groups.iter().filter(|g| g.is_subset(group)).collect();
            let covered: BTreeSet<StationRef> =
                inside.iter().flat_map(|g| g.iter().copied()).collect();
            let first = *group.iter().next().unwrap_or(&StationRef::new(0, 0));
            let tied = group.iter().all(|&s| self.tied(first, s));
            if (covered.len() == group.len() || tied) && self.demotions() {
                self.tally
                    .note("station_merges: a merge group split by a demoted merge (now a Tie)");
            } else if covered.len() != group.len() || !tied {
                self.fail(format!("merge group {group:?} -> junctions {inside:?}"));
            }
        }
    }

    fn zero_datums(&mut self, old: &Value, new: &Value) {
        let (Some(old), Some(new)) = (old.as_array(), new.as_array()) else {
            return;
        };
        if old.len() != new.len() {
            let r = &self.migrated.report;
            if r.datums_dropped_dangling + r.datums_dropped_invalid > 0 {
                self.tally
                    .note("zero_datums: a dangling / invalid datum dropped by the migration");
            } else {
                self.fail(format!("datum count {} -> {}", old.len(), new.len()));
            }
            return;
        }
        for (i, (o, n)) in old.iter().zip(new).enumerate() {
            if o["station_ref"] != n["station_ref"] {
                let (a, b) = (slot_of(&o["station_ref"]), slot_of(&n["station_ref"]));
                if self.station(a) == self.station(b) {
                    self.tally
                        .note("zero_datums: shown on another slot of its junction");
                } else {
                    self.fail(format!("datum {i}: slot {a} -> {b}"));
                }
            }
            for key in ["gps", "accuracy_m", "is_entrance"] {
                if o.get(key) != n.get(key) {
                    self.fail(format!(
                        "datum {i}.{key}: {:?} -> {:?}",
                        o.get(key),
                        n.get(key)
                    ));
                }
            }
            if o["role"] != n["role"] {
                if self.migrated.report.datum_roles_changed > 0 {
                    self.tally
                        .note("zero_datums: role changed by the migration (reported)");
                } else {
                    self.fail(format!("datum {i}.role {} -> {}", o["role"], n["role"]));
                }
            }
            let (og, ng) = (&o["gap"], &n["gap"]);
            let gap_same = og.is_null() == ng.is_null()
                && ["gap_m", "bearing_deg", "threshold_m", "survey_distance_m"]
                    .iter()
                    .all(|k| close(&og[*k], &ng[*k], METRIC_TOL));
            if !gap_same {
                let slot = slot_of(&n["station_ref"]);
                if self.demotions() || self.moved_component(slot, false) {
                    self.tally.note(
                        "zero_datums: GPS-fix gap over a demoted merge / in a moved component",
                    );
                } else {
                    self.fail(format!("datum {i}.gap {og} -> {ng}"));
                }
            }
        }
    }

    fn loop_closures(&mut self, old: &Value, new: &Value) {
        let by_id = |v: &Value| -> BTreeMap<u64, Value> {
            v.as_array()
                .into_iter()
                .flatten()
                .map(|c| (c["id"].as_u64().unwrap_or(0), c.clone()))
                .collect()
        };
        let (old, new) = (by_id(old), by_id(new));
        for (id, o) in &old {
            let Some(n) = new.get(id) else {
                let r = &self.migrated.report;
                if r.closures_dropped_dangling
                    + r.closures_dropped_one_station
                    + r.closures_dropped_unanchored
                    + r.closure_ids_reminted
                    > 0
                {
                    self.tally
                        .note("loop_closures: dropped or reminted by the migration (reported)");
                } else {
                    self.fail(format!("closure {id} missing"));
                }
                continue;
            };
            for end in ["station_a", "station_b"] {
                if o[end] != n[end] {
                    let (a, b) = (slot_of(&o[end]), slot_of(&n[end]));
                    if self.station(a) == self.station(b) {
                        self.tally
                            .note("loop_closures: end shown on another slot of its junction");
                    } else {
                        self.fail(format!("closure {id}.{end}: {a} -> {b}"));
                    }
                }
            }
            if o["strategy"] != n["strategy"] {
                self.tally
                    .note("loop_closures: a legacy aligned closure (now Tie) is shown as Batch");
            }
            if o["is_aligned"] != n["is_aligned"] {
                self.tally
                    .note("loop_closures: is_aligned recomputed (Tie always, Move re-measured)");
            }
            let metrics_same = [
                "path_length",
                "misclosure_distance",
                "error_percentage",
                "horizontal_misclosure",
                "vertical_misclosure",
            ]
            .iter()
            .all(|k| close(&o[*k], &n[*k], METRIC_TOL));
            if !metrics_same {
                if close(
                    &o["misclosure_distance"],
                    &n["misclosure_distance"],
                    METRIC_TOL,
                ) {
                    self.tally.note(
                        "loop_closures: path_length recomputed along shots only (error % with it)",
                    );
                } else {
                    self.tally
                        .note("loop_closures: metrics recomputed from the current layout (stored at creation before)");
                }
            }
        }
        for id in new.keys().filter(|id| !old.contains_key(id)) {
            if self.new_closure_ids.contains(id) {
                self.tally
                    .note("loop_closures: a Tie the migration made from a demoted merge (aligned Batch on the wire)");
            } else {
                self.fail(format!("closure {id} is new"));
            }
        }
    }

    // --- previews -----------------------------------------------------

    fn line_relationships(&mut self) {
        for line in self.lp.lines.iter().map(|l| l.id).collect::<Vec<_>>() {
            let old = json(self.lp.get_line_relationships(line));
            let new = json(self.p.line_relationships(line));
            if old["station_offset_count"] != new["station_offset_count"] {
                self.fail(format!(
                    "line {line} relationships: hand moves {} -> {}",
                    old["station_offset_count"], new["station_offset_count"]
                ));
            }
            for (key, class) in [
                ("merges", "line relationships: merges are the derived pairs on the line"),
                (
                    "closures",
                    "line relationships: closures are the ones the delete removes (a junction's stay)",
                ),
                (
                    "has_line_offset",
                    "line relationships: has_line_offset is a non-trivial Placement the line holds alone",
                ),
                (
                    "zero_datum_count",
                    "line relationships: datums the delete removes (a junction's stay)",
                ),
            ] {
                let differs = match (old[key].as_array(), new[key].as_array()) {
                    (Some(a), Some(b)) => a.len() != b.len(),
                    _ => old[key] != new[key],
                };
                if differs {
                    self.tally.note(class);
                }
            }
        }
    }

    fn delete_previews(&mut self) {
        let lines: Vec<(u32, Vec<u32>)> = self
            .lp
            .lines
            .iter()
            .map(|l| (l.id, l.shots.iter().map(|s| s.id).collect()))
            .collect();
        for (line, shots) in lines {
            let mut picks: Vec<u32> = [shots.first(), shots.get(shots.len() / 2), shots.last()]
                .into_iter()
                .flatten()
                .copied()
                .collect();
            picks.dedup();
            for shot in picks {
                let old = self.lp.preview_delete_shot(line, shot).map(json);
                let new = self
                    .p
                    .preview_delete_shot(line, shot)
                    .map(|preview| NewDelete::of(self.p, &preview));
                match (old, new) {
                    (Ok(old), Ok(new)) => self.delete_impact(line, shot, &old, &new),
                    (Err(_), Err(_)) => {}
                    (old, new) => self.fail(format!(
                        "delete {line}/{shot}: {:?} -> {:?}",
                        old.err(),
                        new.err().map(|e| e.to_string())
                    )),
                }
            }
        }
    }

    fn delete_impact(&mut self, line: u32, shot: u32, old: &Value, delete: &NewDelete) {
        let (new, unplaces) = (&delete.impact, delete.unplaces);
        if old["removed_station"] != new["removed_station"] {
            return self.fail(format!("delete {line}/{shot}: removed station"));
        }
        let removed = slot_of(&old["removed_station"]);
        // A junction stays for its other lines; only this line leaves it.
        let junction = self.junction(removed);
        let tied = self.demotions();
        if old["shift_blocked"] != new["shift_blocked"] {
            if junction || tied {
                self.tally
                    .note("delete: shift verdict at a junction / over a demoted merge");
            } else {
                self.fail(format!(
                    "delete {line}/{shot}: shift_blocked {} -> {}",
                    old["shift_blocked"], new["shift_blocked"]
                ));
            }
        }
        let ids = |v: &Value| -> BTreeSet<u64> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["closure_id"].as_u64())
                .collect()
        };
        let (old_removed, mut new_removed) =
            (ids(&old["removed_closures"]), ids(&new["removed_closures"]));
        // A demoted merge (`Tie`) on the removed slot leaves with it, as
        // today's merge did (today's preview listed it in `removed_merges`).
        let demoted: Vec<u64> = delete
            .member_ties
            .iter()
            .copied()
            .filter(|id| self.new_closure_ids.contains(id))
            .collect();
        if !demoted.is_empty() {
            self.tally.note(
                "delete: a demoted merge (Tie) on the removed slot leaves with it, as today's merge did (listed as a closure; S13 at a junction)",
            );
            new_removed.retain(|id| !demoted.contains(id));
        }
        if old_removed != new_removed {
            if junction {
                self.tally
                    .note("delete: a closure on a junction stays (today it left with the member)");
            } else if old_removed.is_subset(&new_removed) {
                self.tally.note(
                    "delete: the closures the delete leaves across one shot are listed (#737)",
                );
            } else {
                self.fail(format!(
                    "delete {line}/{shot}: removed closures {old_removed:?} -> {new_removed:?}"
                ));
            }
        }
        let count = |v: &Value| v.as_array().map_or(0, Vec::len);
        // A legacy self-merge (one slot with itself) is dropped by the
        // migration (G1); today's preview listed it.
        let merges = |v: &Value| {
            v.as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["station_a"] != m["station_b"])
                .count()
        };
        if count(&old["removed_merges"]) != merges(&old["removed_merges"]) {
            self.tally
                .note("delete: a legacy self-merge (dropped by the migration) is not listed");
        }
        if count(&old["replacement_merges"]) + count(&old["replacement_closure_updates"]) > 0 {
            self.tally
                .note("delete: replacement merges / rewired closures no longer happen (declared)");
        }
        if merges(&old["removed_merges"]) != count(&new["removed_merges"]) {
            if junction || tied {
                self.tally
                    .note("delete: removed merges are the slot leaving its junction");
            } else {
                self.fail(format!(
                    "delete {line}/{shot}: removed merges {} -> {}",
                    old["removed_merges"], new["removed_merges"]
                ));
            }
        }
        // Today's preview lists the promotions of the closures on the removed
        // member only (its sweep's others ran unlisted): compare those.
        let old_promoted = count(&old["closure_to_merge"]);
        let new_promoted = delete.member_promoted.len();
        if old_promoted != new_promoted {
            let rewired: BTreeSet<u64> = old["replacement_closure_updates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|u| u["closure_id"].as_u64())
                .collect();
            let legacy_closure_ties = delete
                .member_ties
                .iter()
                .any(|id| !self.new_closure_ids.contains(id));
            if unplaces && !junction {
                self.tally.note(
                    "delete: a closure to another placed piece is promoted when the delete takes the piece's datum (today's preview missed the sweep)",
                );
            } else if new_promoted > old_promoted
                && delete.member_promoted.iter().all(|id| rewired.contains(id))
            {
                self.tally.note(
                    "delete: a closure on the removed member left as the only bridge is promoted where today's preview listed it re-anchored (its execution then promoted the orphan)",
                );
            } else if old_promoted > new_promoted && legacy_closure_ties {
                self.tally.note(
                    "delete: a legacy aligned closure (now Tie) on the removed member leaves with it like a demoted merge (S13: the two Ties look alike)",
                );
            } else if delete.datum_on_station {
                self.tally.note(
                    "delete: a promotion on the removed member differs because the junction keeps its datum (declared)",
                );
            } else if old_promoted > new_promoted && self.kept_between_placed(line, shot) {
                self.tally.note(
                    "delete: a closure on the removed member stays between two placed pieces (a GPS fix the delete promotes places the junction's piece; today promoted it)",
                );
            } else if tied {
                self.tally.note(
                    "delete: a promotion on the removed member differs over a demoted merge (a Tie does not join pieces)",
                );
            } else {
                self.fail(format!(
                    "delete {line}/{shot}: promoted {} -> {}",
                    old["closure_to_merge"], new["closure_to_merge"]
                ));
            }
        }
        if delete.other_promotions > 0 {
            self.tally.note(
                "delete: the sweep's promotions away from the removed member are listed (today's preview left them out; its execution ran them)",
            );
        }
    }

    /// Whether the delete keeps a closure between two placed pieces.
    fn kept_between_placed(&self, line: u32, shot: u32) -> bool {
        let mut copy = self.p.clone();
        copy.delete_shot(line, shot)
            .is_ok_and(|done| !done.report.sweep.kept_between_placed.is_empty())
    }

    fn range_previews(&mut self) {
        let lines: Vec<(u32, Vec<u32>)> = self
            .lp
            .lines
            .iter()
            .filter(|l| l.shots.len() >= 3)
            .map(|l| (l.id, l.shots.iter().map(|s| s.id).collect()))
            .collect();
        for (line, shots) in lines {
            let n = shots.len();
            for (first, last) in [(0, n / 2), (1, n - 2), (n / 2, n - 1)] {
                let (a, b) = (shots[first], shots[last]);
                let old = self.lp.preview_delete_shot_range(line, a, b).map(json);
                let new = self.p.preview_delete_shot_range(line, a, b).map(|preview| {
                    let swept: BTreeSet<u64> =
                        preview.swept_closures.iter().map(|c| c.id).collect();
                    (json(self.p.delete_range_impact(&preview)), swept)
                });
                match (old, new) {
                    (Ok(old), Ok((new, swept))) => self.range_impact(line, &old, &new, &swept),
                    (Err(_), Err(_)) => {}
                    (old, new) => self.fail(format!(
                        "range {line}/{a}..{b}: {:?} -> {:?}",
                        old.err(),
                        new.err().map(|e| e.to_string())
                    )),
                }
            }
        }
    }

    /// Whether closure `id` is a `Move` closure the survey before the edit
    /// does not hold aligned (so it did not tie in the solve then).
    fn unaligned_move_before(&self, id: u64) -> bool {
        let raw = self.p.raw_positions();
        self.p.closures.iter().any(|c| {
            c.id == id && c.strategy == ClosureStrategy::Move && !self.p.closure_is_aligned(&raw, c)
        })
    }

    /// `swept`: the closures the port's preview says its sweep removes.
    fn range_impact(&mut self, line: u32, old: &Value, new: &Value, swept: &BTreeSet<u64>) {
        for key in ["line_id", "start_pos", "end_pos", "kind"] {
            if old[key] != new[key] {
                return self.fail(format!(
                    "range on {line}: {key} {} -> {}",
                    old[key], new[key]
                ));
            }
        }
        let slots = |v: &Value| -> Vec<StationRef> {
            v.as_array().into_iter().flatten().map(slot_of).collect()
        };
        let (old_removed, new_removed) = (
            slots(&old["removed_stations"]),
            slots(&new["removed_stations"]),
        );
        let junction = old_removed
            .iter()
            .chain(&new_removed)
            .any(|&s| self.junction(s));
        let tied = self.demotions();
        if old["shift_blocked"] != new["shift_blocked"] {
            // The removed set follows the verdict (the split's interior set
            // when only the split is offered, else the gap-close set).
            if junction || tied {
                self.tally.note(
                    "range delete: shift verdict at a junction / over a demoted merge (the removed set follows it)",
                );
                return;
            }
            return self.fail(format!(
                "range on {line}: shift_blocked {} {} -> {} {}",
                old["shift_blocked"],
                old["shift_blocked_reasons"],
                new["shift_blocked"],
                new["shift_blocked_reasons"]
            ));
        }
        if old_removed != new_removed {
            return self.fail(format!(
                "range on {line}: removed stations {old_removed:?} -> {new_removed:?}"
            ));
        }
        let scoped = |case: &mut Self, what: &str, class: &'static str| {
            if junction || tied {
                case.tally.note(class);
            } else {
                case.fail(format!(
                    "range on {line}: {what} {} -> {}",
                    old[what], new[what]
                ));
            }
        };
        let ids = |v: &Value| -> BTreeSet<u64> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["closure_id"].as_u64())
                .collect()
        };
        let (old_removed, new_removed) =
            (ids(&old["removed_closures"]), ids(&new["removed_closures"]));
        if old_removed != new_removed {
            if old_removed.is_subset(&new_removed)
                && new_removed
                    .difference(&old_removed)
                    .all(|id| swept.contains(id))
            {
                self.tally.note(
                    "range delete: the closures the sweep removes are listed (W6, N2 #738; today's range preview left the sweep out)",
                );
            } else {
                scoped(
                    self,
                    "removed_closures",
                    "range delete: a closure on a junction stays",
                );
            }
        }
        let (old_across, new_across) = (
            ids(&old["closures_removed_across_one_shot"]),
            ids(&new["closures_removed_across_one_shot"]),
        );
        if old_across != new_across {
            if old_across.is_subset(&new_across)
                && new_across
                    .difference(&old_across)
                    .all(|&id| self.unaligned_move_before(id))
            {
                self.tally.note(
                    "range delete: a Move closure the gap-close brings within 1 cm is removed across one shot (alignment derived, plan 3.6; today judged by its flag from the open)",
                );
            } else {
                scoped(
                    self,
                    "closures_removed_across_one_shot",
                    "range delete: across-one-shot removals over a junction / demoted merge",
                );
            }
        }
        for key in ["removed_datums", "removed_comments"] {
            let (mut o, n): (BTreeSet<StationRef>, BTreeSet<StationRef>) = (
                slots(&old[key]).into_iter().collect(),
                slots(&new[key]).into_iter().collect(),
            );
            if key == "removed_comments" {
                let before = o.len();
                o.retain(|s| !self.empty_comments.contains(s));
                if o.len() != before {
                    self.tally
                        .note("range delete: an empty legacy comment is not listed");
                }
            }
            let at_junction = o.symmetric_difference(&n).all(|&s| self.junction(s));
            if o != n {
                if at_junction {
                    self.tally
                        .note("range delete: a junction keeps its datum and comment");
                } else {
                    scoped(
                        self,
                        key,
                        "range delete: datum / comment over a demoted merge",
                    );
                }
            }
        }
        let count = |v: &Value| {
            v.as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["station_a"] != m["station_b"])
                .count()
        };
        if count(&old["removed_merges"]) != count(&new["removed_merges"]) {
            scoped(
                self,
                "removed_merges",
                "range delete: removed merges are the slots leaving their junctions",
            );
        }
        if count(&old["replacement_merges"])
            + old["replacement_closure_updates"]
                .as_array()
                .map_or(0, Vec::len)
            > 0
        {
            self.tally.note(
                "range delete: replacement merges / rewired closures no longer happen (declared)",
            );
        }
        if count(&old["closure_to_merge"]) != count(&new["closure_to_merge"]) {
            self.tally.note(
                "range delete: closures promoted by the sweep differ (junction left, or a datum taken)",
            );
        }
    }

    fn connectivity(&mut self, seed: u64) {
        let slots: Vec<StationRef> = self
            .lp
            .lines
            .iter()
            .flat_map(|l| (0..=l.shots.len() as u32).map(move |i| StationRef::new(l.id, i)))
            .collect();
        if slots.len() < 2 {
            return;
        }
        let mut rng = Rng::new(seed ^ 0x5eed);
        for _ in 0..12 {
            let a = slots[rng.below(slots.len())];
            let b = slots[rng.below(slots.len())];
            let old = old_connectivity(self.lp, a, b);
            let new = self.p.check_merge_connectivity_wire(a, b);
            match (old, new) {
                (Ok((connected, preview)), Ok(new)) => {
                    let old_shift = (!connected)
                        .then(|| {
                            self.lp.merge_datum_shift(
                                m::StationRef::new(a.line_id, a.station_index),
                                m::StationRef::new(b.line_id, b.station_index),
                            )
                        })
                        .flatten();
                    self.datum_shift(
                        a,
                        b,
                        old_shift.map(json),
                        new.datum_shift.as_ref().map(json),
                    );
                    if connected != new.connected {
                        if self.demotions() {
                            self.tally.note(
                                "connectivity: a demoted merge is a closure (still connected)",
                            );
                        } else {
                            self.fail(format!(
                                "connectivity {a}-{b}: {connected} -> {}",
                                new.connected
                            ));
                        }
                    }
                    if let (Some(o), Some(n)) = (preview, new.preview) {
                        if !close(
                            &json(o.misclosure_distance),
                            &json(n.misclosure_distance),
                            METRIC_TOL,
                        ) && !self.moved_component(a, false)
                            && !self.moved_component(b, false)
                        {
                            self.fail(format!("closure preview {a}-{b}: misclosure"));
                        }
                        if !close(&json(o.path_length), &json(n.path_length), METRIC_TOL) {
                            self.tally
                                .note("closure preview: path_length along shots only");
                        }
                    }
                }
                (Err(o), Err(n)) if o == n => {}
                (Err(_), _) | (_, Err(_)) if self.junction(a) || self.junction(b) => {
                    self.tally.note(
                        "connectivity: a closure or merge on any slot of a station counts for all its slots",
                    );
                }
                (old, new) => {
                    if self.demotions() {
                        self.tally
                            .note("connectivity: a demoted merge (Tie) refuses like a closure");
                    } else {
                        self.fail(format!("connectivity {a}-{b}: {old:?} -> {new:?}"));
                    }
                }
            }
        }
    }

    fn datum_shift(
        &mut self,
        a: StationRef,
        b: StationRef,
        old: Option<Value>,
        new: Option<Value>,
    ) {
        match (old, new) {
            (None, None) => {}
            (Some(o), Some(n)) => {
                let same_station = self.station(slot_of(&o["datum_station"]))
                    == self.station(slot_of(&n["datum_station"]));
                if o["moving_line_ids"] != n["moving_line_ids"] || !same_station {
                    self.fail(format!("datum shift {a}-{b}: {o} -> {n}"));
                } else if !close(&o["shift_m"], &n["shift_m"], XY_TOL) {
                    if self.moved_component(a, false) || self.moved_component(b, false) {
                        self.tally
                            .note("merge datum shift: across a component the migration moved");
                    } else {
                        self.fail(format!("datum shift {a}-{b}: {o} -> {n}"));
                    }
                }
            }
            (old, new) => {
                if self.demotions() || self.junction(a) || self.junction(b) {
                    self.tally.note(
                        "merge datum shift: piece membership over a junction / demoted merge",
                    );
                } else {
                    self.fail(format!("datum shift {a}-{b}: {old:?} -> {new:?}"));
                }
            }
        }
    }

    fn adjustment_report(&mut self, legacy: &LegacySurvey) {
        let config = ClosureAdjustmentConfig::default();
        let old = json(&legacy.adjustment_report);
        let new = json(self.p.adjustment_report_wire(&config));
        let skipped = |v: &Value| -> BTreeSet<u64> {
            v["skipped_closures"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["closure_id"].as_u64())
                .collect()
        };
        if skipped(&old) != skipped(&new) {
            if self.new_closure_ids.is_empty() && !self.demotions() {
                self.fail(format!(
                    "skipped closures {:?} -> {:?}",
                    skipped(&old),
                    skipped(&new)
                ));
            } else {
                self.tally
                    .note("adjustment report: a demoted merge (Tie) goes through the across-one-shot check (D11)");
            }
        }
        let failures = |v: &Value| v["failures"].as_array().map_or(0, Vec::len);
        // The verification compared the components each solve failed and
        // explained every difference (a D11 failure can move nothing beyond
        // the tolerance, so it leaves no slot in a class).
        let failures_explained = matches!(
            &self.migrated.report.verification.adjusted,
            super::AdjustedCheck::Compared(a) if a.failure_mismatches.is_empty()
        );
        if failures(&old) != failures(&new) {
            if failures_explained {
                self.tally.note(
                    "adjustment report: failures differ by a Computed View class (D10 / D11)",
                );
            } else {
                self.fail(format!(
                    "failures {} -> {}",
                    old["failures"], new["failures"]
                ));
            }
        }
        let far = |v: &Value| -> BTreeSet<StationRef> {
            v["far_gps_fixes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| slot_of(&f["station"]))
                .collect()
        };
        let (old_far, new_far) = (far(&old), far(&new));
        if old_far.len() != new_far.len() {
            let moved = old_far
                .symmetric_difference(&new_far)
                .all(|&slot| self.moved_component(slot, false));
            if self.demotions() || moved {
                self.tally.note(
                    "adjustment report: far GPS fixes over a demoted merge / in a moved component",
                );
            } else {
                self.fail(format!(
                    "far fixes {} -> {}",
                    old["far_gps_fixes"], new["far_gps_fixes"]
                ));
            }
        }
    }

    fn combine_candidates(&mut self) {
        let ids: Vec<u32> = self.lp.lines.iter().map(|l| l.id).collect();
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                let old = self.lp.combine_candidate_for_lines(a, b).map(json);
                let new = self
                    .p
                    .combine_candidate_for_lines(a, b)
                    .map(|c| json(self.p.combine_candidate_wire(&c)));
                match (old, new) {
                    (None, None) => {}
                    (Some(old), Some(new)) => self.combine_candidate(a, b, &old, &new),
                    (old, _) => {
                        if old.is_some() {
                            self.tally
                                .note("combine (M1 semantics): a pair today offers that the station graph does not");
                        } else {
                            self.tally.note("combine (M1 semantics): a pair the station graph offers that today does not");
                        }
                    }
                }
            }
        }
    }

    fn combine_candidate(&mut self, a: u32, b: u32, old: &Value, new: &Value) {
        let old_keys: BTreeSet<&String> = old
            .as_object()
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        let new_keys: BTreeSet<&String> = new
            .as_object()
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        if old_keys != new_keys {
            return self.fail(format!(
                "combine {a}+{b}: keys {old_keys:?} -> {new_keys:?}"
            ));
        }
        for key in [
            "line_a",
            "line_b",
            "end_a",
            "end_b",
            "reverses",
            "shot_count_a",
            "shot_count_b",
        ] {
            if old[key] != new[key] {
                self.tally.note("combine (M1 semantics): join ends differ");
            }
        }
        if old["join_kind"] != new["join_kind"] {
            self.tally.note("combine (M1 semantics): join kind differs");
        }
        for key in ["station_a", "station_b"] {
            if old[key] != new[key] {
                self.tally.note("combine (M1 semantics): join slot differs");
            }
        }
        if old["blocked_reason"].is_null() != new["blocked_reason"].is_null() {
            self.tally
                .note("combine (M1 semantics): blocked verdict differs");
        }
        for plan in ["plan_keeping_a", "plan_keeping_b"] {
            let (o, n) = (&old[plan], &new[plan]);
            if o["moving_line"] != n["moving_line"] {
                self.tally
                    .note("combine (M1 semantics): plan moving line differs (S3, S10)");
            }
            if !close(&o["shift_m"], &n["shift_m"], METRIC_TOL) {
                self.tally
                    .note("combine (M1 semantics): plan shift differs");
            }
            let count = |v: &Value| v.as_array().map_or(0, Vec::len);
            if count(&o["torn_ties"]) != count(&n["torn_ties"]) {
                self.tally
                    .note("combine (M1 semantics): plan torn ties differ (S1, S2, S5)");
            }
        }
    }
}

/// Today's `check_merge_connectivity` (crates/core `command_query.rs`) on
/// the frozen model: `Ok((connected, preview))`, or today's refusal text.
fn old_connectivity(
    lp: &m::Project,
    a: StationRef,
    b: StationRef,
) -> Result<(bool, Option<m::ClosurePreview>), String> {
    let (a, b) = (
        m::StationRef::new(a.line_id, a.station_index),
        m::StationRef::new(b.line_id, b.station_index),
    );
    if lp.are_stations_merged(a, b) {
        return Err("These stations are already merged.".to_string());
    }
    if lp.are_stations_in_closure(a, b) {
        return Err("A loop closure already exists between these stations.".to_string());
    }
    let connected = lp.get_connected_line_ids(a.line_id).contains(&b.line_id);
    let preview = if connected {
        Some(lp.compute_closure_metrics(a, b)?)
    } else {
        None
    };
    Ok((connected, preview))
}

/// Compare everything for one legacy file.
pub(super) fn compare(
    what: String,
    seed: u64,
    legacy: &LegacySurvey,
    migrated: &Migrated,
) -> Tally {
    let mut case = Case::new(what, legacy, migrated);
    let lp = &legacy.project;
    let p = &migrated.project;

    let raw = p.raw_positions();
    case.stations(
        "raw stations",
        &json(lp.compute_station_coordinates()),
        &json(p.stations_wire(&raw)),
        false,
    );
    case.project(&json(Ipc(lp)), &json(p.project_wire_with(&raw)));

    let config = ClosureAdjustmentConfig::default();
    let old_adjusted = lp.compute_adjusted_stations(&m::config::ClosureAdjustmentConfig::default());
    let adjusted = p.adjusted_positions_from(&raw, &config);
    case.stations(
        "computed view stations",
        &json(&old_adjusted.stations),
        &json(p.adjusted_stations_wire(&adjusted)),
        true,
    );

    let state = p.project_state_wire(false);
    // 0.6.0's station count was the slot count; the wire's is the distinct
    // stations, as the file summary counts them (review finding UX-8).
    if p.survey_stats(&raw, false).slot_count
        != lp.lines.iter().map(|l| l.shots.len() + 1).sum::<usize>()
        || state.stats.station_count != p.stations.len()
        || state.stats.line_count != lp.lines.len()
    {
        case.fail("stats: line / slot / station counts".to_string());
    }

    case.line_relationships();
    case.delete_previews();
    case.range_previews();
    case.connectivity(seed);
    case.adjustment_report(legacy);
    case.combine_candidates();
    case.tally
}

fn compare_bytes(what: String, seed: u64, bytes: &[u8]) -> Tally {
    let legacy = stalagmight_legacy::open_bytes(bytes, &OpenOptions::default()).unwrap();
    let migrated = migrate(&legacy, &ClosureAdjustmentConfig::default()).unwrap();
    compare(what, seed, &legacy, &migrated)
}

fn seeds(first: u64, count: u64) -> Tally {
    let mut tally = Tally::default();
    for seed in first..first + count {
        tally.absorb(compare_bytes(
            format!("seed {seed}"),
            seed,
            &random_case(seed).write(),
        ));
    }
    tally
}

#[test]
fn legacy_surveys_project_to_todays_payloads_up_to_the_declared_differences() {
    let tally = seeds(1, SEEDS);
    tally.assert_clean();
    // The generator exercises junctions, so the declared junction classes
    // must actually be seen (a vacuous pass would hide a broken projection).
    for class in [
        "identity: a junction shares the survivor's Station ID",
        "project: line_offsets dropped from the wire",
        "station_merges: derived star pairs (first slot to each other slot)",
    ] {
        assert!(
            tally.classes.contains_key(class),
            "never saw {class}: {:#?}",
            tally.classes
        );
    }
}

#[test]
fn the_tracked_fixtures_project_to_todays_payloads() {
    let mut tally = Tally::default();
    for name in ["fractal_plant_iter3.stalx", "naharon.stalx"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../e2e/fixtures")
            .join(name);
        tally.absorb(compare_bytes(
            name.to_string(),
            7,
            &std::fs::read(path).unwrap(),
        ));
    }
    tally.assert_clean();
}

/// `STALAGMIGHT_RUST_WIRE_SEEDS=<n> [STALAGMIGHT_RUST_WIRE_FIRST_SEED=<k>]`:
/// run `n` seeds and print every class with its count.
#[test]
#[ignore = "sweep: set STALAGMIGHT_RUST_WIRE_SEEDS"]
fn wire_equivalence_sweep() {
    let env = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse().ok());
    let count: u64 = env("STALAGMIGHT_RUST_WIRE_SEEDS").unwrap_or(200);
    let first: u64 = env("STALAGMIGHT_RUST_WIRE_FIRST_SEED").unwrap_or(1);
    let tally = seeds(first, count);
    eprintln!("wire equivalence: {} cases", tally.cases);
    for (class, n) in &tally.classes {
        eprintln!("  {n:>7}  {class}");
    }
    tally.assert_clean();
}

/// Run the delete on both models: 0.6.0's (the frozen model's own removal
/// with its rewire and orphan conversion) and the station graph's. Every
/// slot either has must sit where 0.6.0 puts it.
fn assert_delete_moves_what_today_moves(seed: u64, line: u32, pick: impl Fn(usize) -> usize) {
    let legacy =
        stalagmight_legacy::open_bytes(&random_case(seed).write(), &OpenOptions::default())
            .unwrap();
    let migrated = migrate(&legacy, &ClosureAdjustmentConfig::default()).unwrap();
    let mut old = legacy.project;
    let mut new = migrated.project;
    let shots = &old.lines.iter().find(|l| l.id == line).unwrap().shots;
    let shot = shots[pick(shots.len())].id;
    old.remove_shot_with_rewire_reporting(line, shot).unwrap();
    new.delete_shot(line, shot).unwrap();
    let today: BTreeMap<StationRef, [f64; 2]> = old
        .compute_station_coordinates()
        .into_iter()
        .map(|s| {
            let slot = StationRef::new(s.station_ref.line_id, s.station_ref.station_index);
            (slot, [s.coordinates.x, s.coordinates.y])
        })
        .collect();
    let slots = new.raw_positions().slots(&new);
    assert_eq!(slots.len(), today.len(), "seed {seed}: slot count");
    for slot in slots {
        let [x, y] = today[&slot.slot];
        assert!(
            (slot.x - x).hypot(slot.y - y) <= XY_TOL,
            "seed {seed}, delete {line}/{shot}: {:?} at ({}, {}), today ({x}, {y})",
            slot.slot,
            slot.x,
            slot.y
        );
    }
}

#[test]
fn deleting_a_first_shot_at_a_junction_moves_nothing_today_keeps_still() {
    // Seed 5: line 0's station 0 is a junction with the shot-less line 3,
    // and a 0.6.0 same-line merge on line 0's member there is a `Tie` now.
    // Today the delete took that merge with the member: line 3 stays where
    // it was. The junction stays for line 3 (S13).
    assert_delete_moves_what_today_moves(5, 0, |_| 0);
    let legacy =
        stalagmight_legacy::open_bytes(&random_case(5).write(), &OpenOptions::default()).unwrap();
    let mut p = migrate(&legacy, &ClosureAdjustmentConfig::default())
        .unwrap()
        .project;
    let junction = p.station_at(StationRef::new(0, 0)).unwrap();
    let shot = p.line(0).unwrap().shots[0].id;
    let done = p.delete_shot(0, shot).unwrap();
    assert!(!done.value.station_deleted);
    assert_eq!(p.station_at(StationRef::new(3, 0)), Some(junction));
    assert!(done.report.sweep.promoted.is_empty());
    assert_eq!(done.value.closures_removed.len(), 1, "the member's Tie");
}

#[test]
fn a_closure_on_the_member_a_delete_takes_promotes_as_today() {
    // Seed 23: a closure on line 2's last station (a junction) is the only
    // bridge once the line leaves it; today the junction's side moved onto
    // the closure's far end. Seed 215: two closures could promote; today's
    // delete promoted the one on the removed member first.
    assert_delete_moves_what_today_moves(23, 2, |n| n - 1);
    assert_delete_moves_what_today_moves(215, 0, |_| 0);
}

/// Bend one wire slot of `bytes` in a view (1 km in x, then 0.5 m in depth,
/// a hand-move z applied twice), in a component the migration moved, and
/// assert the comparison calls it unexplained while the untouched payload
/// passes. `false` when the migration moved nothing in that view.
fn a_bent_wire_slot_is_unexplained(what: &str, bytes: &[u8], adjusted: bool) -> bool {
    let config = ClosureAdjustmentConfig::default();
    let legacy = stalagmight_legacy::open_bytes(bytes, &OpenOptions::default()).unwrap();
    let migrated = migrate(&legacy, &config).unwrap();
    let probe = Case::new(what.to_string(), &legacy, &migrated);
    if probe.moved_components[usize::from(adjusted)].is_empty() {
        return false;
    }
    let (lp, p) = (&legacy.project, &migrated.project);
    let (old, new) = if adjusted {
        (
            json(
                &lp.compute_adjusted_stations(&m::config::ClosureAdjustmentConfig::default())
                    .stations,
            ),
            json(p.adjusted_stations_wire(&probe.adjusted)),
        )
    } else {
        (
            json(lp.compute_station_coordinates()),
            json(p.stations_wire(&probe.raw)),
        )
    };
    let mut case = Case::new(what.to_string(), &legacy, &migrated);
    case.stations("stations", &old, &new, adjusted);
    assert!(
        case.tally.unexplained.is_empty(),
        "{what}: {:#?}",
        case.tally.unexplained
    );
    // A slot of a component the migration moved: exactly where a whole-case
    // waiver looked away.
    let at = new
        .as_array()
        .unwrap()
        .iter()
        .position(|s| probe.moved_component(slot_of(&s["station_ref"]), adjusted))
        .unwrap();
    for (axis, by) in [("x", 1000.0), ("z", 0.5)] {
        let mut bent = new.clone();
        let c = &mut bent[at]["coordinates"][axis];
        *c = Value::from(c.as_f64().unwrap() + by);
        let mut case = Case::new(what.to_string(), &legacy, &migrated);
        case.stations("stations", &old, &bent, adjusted);
        assert!(
            !case.tally.unexplained.is_empty(),
            "{what}, adjusted {adjusted}: a {by} m {axis} move passed"
        );
    }
    true
}

/// The oracle itself (J-docs-tests-truth-5): in a case where the migration
/// did move positions (so a whole-case waiver would have let anything
/// through), a wire slot moved off the migrated model's position is
/// unexplained, in the Closures Computed View (generated cases: `TieSolved`
/// and the other Computed View classes) and in the raw layout (a 0.6.0 merge
/// torn by 5 m that the migration collapses, which the generator's default
/// draw does not make); the same payloads untouched pass.
#[test]
fn a_wire_move_the_migration_did_not_make_fails_even_where_it_moved_positions() {
    let mut computed = 0;
    for seed in 1..=SEEDS {
        if computed < 3
            && a_bent_wire_slot_is_unexplained(
                &format!("seed {seed}"),
                &random_case(seed).write(),
                true,
            )
        {
            computed += 1;
        }
    }
    assert_eq!(
        computed, 3,
        "too few generated cases with Computed View moves"
    );

    // C hangs off B; C's stored offset drifted 5 m, and a merge row that does
    // not resolve keeps the 0.6.0 cascade from healing it (migrate_tests).
    let leg = |az: f64, n: u32| m::ShotData {
        azimuth_in: az,
        azimuth_out: az,
        length: 10.0,
        uuid: Some(format!("{n:04}-torn-uuid")),
        ..m::ShotData::default()
    };
    let mut p = m::Project::new("Torn".to_string(), None);
    let mut add = |shots: Vec<m::ShotData>| {
        p.add_line(m::LineData {
            color: "#336699".to_string(),
            quality: 1.0,
            shots,
            ..m::LineData::default()
        })
        .id
    };
    let a = add(vec![leg(90.0, 0)]);
    let b = add(vec![leg(0.0, 1), leg(0.0, 2)]);
    let c = add(vec![leg(90.0, 3)]);
    p.merge_stations(m::StationRef::new(c, 0), m::StationRef::new(b, 1))
        .unwrap();
    p.adjust_line_offset(c, 5.0, 0.0);
    let bytes = super::test_support::edit_sqlite(
        &stalagmight_legacy::write_v18(&p, &stalagmight_legacy::LegacyExtras::default()).unwrap(),
        |conn| {
            conn.execute(
                "INSERT INTO station_merges (station_a_section, station_a_index, \
                 station_b_section, station_b_index, merged_at) \
                 VALUES (?1, 999, ?2, 0, '2023-11-14T22:13:20+00:00')",
                [a, b],
            )
            .unwrap();
        },
    );
    assert!(
        a_bent_wire_slot_is_unexplained("torn merge", &bytes, false),
        "the torn merge moves the raw layout"
    );
}
