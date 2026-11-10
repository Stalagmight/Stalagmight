//! Named dive-plan snapshots (#244), persisted in a sibling `.plnx` SQLite file.
//!
//! A plan captures exactly the input tuple of `compute_gas_reach_field`, so loading
//! a plan is "set the gas inputs and recompute": the reachability field itself is
//! never stored (it is always derived from live geometry, so a stale overlay can
//! never be shown). See `git show de8eef91:docs/plans/2026-07-06_dive-plan-persistence.md` (doc removed from master in #229).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::GasConfig;
use crate::{Cylinder, Diver, PlanSegment, Project, StationKey, StationRef};

/// A station identified by the shots it SITS BETWEEN, rather than by its position or by which
/// side of one shot it is on (#24).
///
/// This is the sidecar's own storage form. The project's internal cross-references keep using
/// [`StationKey`] - they are rewritten in place by the edits that move them, which a file
/// outside the project cannot be.
///
/// The point is that a station's neighbours do not care which way you walk the section. Station
/// `i` sits between `shots[i-1]` and `shots[i]`; reverse the section and it sits between the
/// same two shots, so the same record still names it. `AfterShot(uuid)` cannot say that: "after"
/// is defined by traversal order, so a reverse silently slides it onto the neighbouring station
/// while it carries on resolving.
///
/// `a`/`b` are the two neighbours as they stood at save time, `None` for the free end of a
/// terminal station. They are NOT ordered - resolution derives the side from the current
/// positions, so it reads the same after a reverse.
///
/// `pub` only so it can appear inside [`StoredAnchor`] on the IPC boundary (#310); every field
/// stays crate-private (`pub(crate)`, built and placed by `core::state::dive_plan`), which is
/// what makes an anchor opaque to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanAnchor {
    pub section_id: crate::SectionId,
    #[serde(default)]
    pub a: Option<String>,
    #[serde(default)]
    pub b: Option<String>,
    /// Which side of `a` the station sat on when it was saved, in TRAVERSAL terms.
    ///
    /// Only consulted when the pair cannot place the station on its own - a terminal station has
    /// no second neighbour, and a deleted partner leaves only one. On its own this bit is
    /// reverse-UNSAFE, which is what `parity` below repairs.
    #[serde(default)]
    pub after_a: bool,
    /// The section's [`Section::reverse_parity`] when this anchor was written.
    ///
    /// This is what makes the single-neighbour case correct. `after_a` is expressed in traversal
    /// terms, and a reverse flips traversal - so the same observable state ("`a` is at position
    /// 0 and something follows it") is produced by two different histories that need OPPOSITE
    /// answers:
    ///
    /// ```text
    ///   [s0,s1,s2] reversed          -> a at 0, follower present -> station is index 0
    ///   [s0,s1] delete s0, append s2 -> a at 0, follower present -> station is index 1
    /// ```
    ///
    /// The pair alone cannot tell those apart. Comparing this against the section's parity now
    /// can: differing parity means a reverse happened and the side flips; equal parity means it
    /// did not and the saved side is read literally.
    ///
    /// Absent (`false`) on plans written before this field existed, which is correct for them -
    /// no reverse had been recorded against their sections either.
    #[serde(default)]
    pub parity: bool,
    /// `length.to_bits()` of the sole neighbouring shot, recorded only for a TERMINAL station.
    ///
    /// Turns the one case the pair genuinely cannot survive - splitting that shot - into a
    /// VISIBLE stale reference instead of a silent one-station error. The split keeps the
    /// original UUID on the shortened tail, so afterwards no shot in the file has this station
    /// as an endpoint and nothing can name it; the length necessarily changed, which is what
    /// gives us a detector.
    ///
    /// Read only when the resolved side is no longer at an end of the section - so a plain
    /// prepend or append, which leaves the shot's length alone, keeps resolving exactly as
    /// before, and a reverse is untouched (length is invariant, and `parity` handles the side).
    /// The one false positive is editing this shot's length AND extending that same end before
    /// the plan is next loaded, which yields a "re-pin this" warning rather than a wrong route.
    ///
    /// ⚠ Compared as a LENGTH within a relative tolerance, never bit-for-bit - see
    /// `terminal_length_matches` in `core::state::dive_plan`. Bit equality is not survivable now
    /// that an anchor crosses the IPC boundary (#310): this is a full 64-bit integer, JavaScript
    /// has only doubles, and the shortest-round-trip decimal `JSON.stringify` writes for one
    /// differs from the original in its low bits. Those are the low MANTISSA bits of the encoded
    /// length, so the length itself survives to ~1e-13 relative - orders of magnitude finer than
    /// the thing this field exists to detect.
    #[serde(default)]
    pub terminal_shot_length_bits: Option<u64>,
}

impl PlanAnchor {
    /// Position of a neighbour in the section as it stands now.
    /// Position of a neighbour in the section as it stands now.
    ///
    /// Takes the project's shot-UUID index first, the way `Project::resolve_key` does, and only
    /// falls back to a scan when the index is cold or stale. Without it every anchor cost a full
    /// pass per neighbour - two per station - and a hand-edited `.plnx` full of UUIDs that match
    /// nothing would scan the whole section every time.
    pub fn position_of(
        project: &Project,
        section: &crate::Section,
        uuid: &Option<String>,
    ) -> Option<usize> {
        let uuid = uuid.as_deref()?;
        if let Some(&i) = project.shot_uuid_index.get(&(section.id, uuid.to_string())) {
            // Re-check before trusting it, exactly as `Project::resolve_key` does: a stale
            // entry then costs only the scan below and can never resolve to the wrong shot.
            if section
                .shots
                .get(i as usize)
                .and_then(|sh| sh.uuid.as_deref())
                == Some(uuid)
            {
                return Some(i as usize);
            }
        }
        section
            .shots
            .iter()
            .position(|s| s.uuid.as_deref() == Some(uuid))
            .or_else(|| {
                // A plan saved against an id the load renamed (a cross-section
                // repeat in a file saved before shot UUIDs were project-wide
                // unique) still finds its shot through the derived id.
                let renamed = crate::reminted_shot_uuid(section.id, uuid);
                section
                    .shots
                    .iter()
                    .position(|s| s.uuid.as_deref() == Some(renamed.as_str()))
            })
    }
}

/// What a `.plnx` row can hold for one station.
///
/// Plans written before #24 store a [`StationKey`] - `{section_id, anchor}`. Those are still
/// read, and still resolve exactly as they always did (including the reverse blind spot), so
/// upgrading cannot lose anyone's route. They are rewritten in the pair form the next time the
/// user saves that plan, which is also when they stop being vulnerable to it.
///
/// `Legacy` is listed FIRST deliberately: `StationKey` REQUIRES an `anchor` or `station_index`
/// field, so only a pre-#24 blob can match it and a pair-form blob falls through. Reversing the
/// order would make `PlanAnchor` - whose fields all default - swallow legacy rows and quietly
/// blank every reference in them.
///
/// `pub` since #310, when it started crossing the IPC boundary so the frontend can hold a live
/// plan pin as an anchor rather than an index. It is `crate::StoredAnchor` (re-exported
/// from `core::state::dive_plan`, the code that builds and places it) and is OPAQUE to the
/// frontend: no public constructor, no public fields, and `unknown` in TS.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum StoredAnchor {
    Legacy(StationKey),
    Pair(PlanAnchor),
}

/// Payload schema version stamped into every saved snapshot. Bump only on a
/// breaking change to `DivePlanSnapshot`; additive changes need no bump because
/// unknown fields are ignored and missing fields default (serde, config-style).
pub const DIVE_PLAN_SCHEMA_VERSION: u32 = 2;

/// A missing diver (only reachable from a malformed or truncated blob) defaults to
/// the canonical 20 L/min rather than a zero RMV the engine would reject.
fn default_diver() -> Diver {
    Diver {
        rmv_l_per_min: 20.0,
        label: None,
    }
}

/// An arbitrary spot the diver marked on the plan (#298) - a photo stop, a lead to
/// look at, a restriction to rig. Purely an annotation: it is saved with the plan
/// but never reaches the gas engine, so marking a station cannot change the route
/// or the budget. The label is optional (a bare pin is a valid mark) and capped on
/// the write path; see `MAX_MARKED_LABEL_CHARS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MarkedPoint {
    pub station: StationRef,
    #[serde(default)]
    pub label: Option<String>,
}

/// The persisted gas-plan inputs: the argument set of `compute_gas_reach_field`,
/// plus the plan-only marked points. Every field is serde-default tolerant so a plan
/// written by an older or newer build still loads - which is why adding `waypoints`
/// and `marked_points` needed no schema-version bump. No `PartialEq` (GasConfig has
/// none); tests compare via JSON.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DivePlanSnapshot {
    #[serde(default)]
    pub origin: Option<StationRef>,
    #[serde(default)]
    pub endpoint: Option<StationRef>,
    /// Ordered intermediate points the dive must visit between origin and endpoint (#298).
    #[serde(default)]
    pub waypoints: Vec<StationRef>,
    #[serde(default = "default_diver")]
    pub diver: Diver,
    #[serde(default)]
    pub cylinders: Vec<Cylinder>,
    #[serde(default)]
    pub gas_config: GasConfig,
    #[serde(default)]
    pub stage_drop_overrides: Vec<Option<StationRef>>,
    /// Map annotations saved with this plan (#298); never an input to the gas engine.
    #[serde(default)]
    pub marked_points: Vec<MarkedPoint>,
    /// Per-span pace and propulsion (#317). Positional against `waypoints`, NOT a
    /// `StationKey`-anchored reference: segment `k` covers the span from way point
    /// `k-1` to way point `k` (with `k = 0` running from origin and the last segment
    /// covering the open reach field past the final way point), so there are always
    /// `waypoints.len() + 1` of them. Because a segment names a SPAN rather than a
    /// station, it carries no station of its own to canonicalize - unlike
    /// origin/endpoint/waypoints it needs no `StationKey` round trip on save/load; a
    /// dropped way point instead triggers a segment MERGE (see
    /// `core::state::dive_plan::resolve_persisted`).
    #[serde(default)]
    pub segments: Vec<PlanSegment>,
}

/// A dive plan's dropdown row: identity + name + timestamps (RFC3339). Carries no
/// heavy payload, so listing many plans is cheap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DivePlanMeta {
    /// SQLite rowid. `number` in TS (well under 2^53), not a `bigint`.
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The anchors behind a plan's station references (#310), index-aligned with the lists in
/// [`DivePlanSnapshot`].
///
/// A `StationRef` is POSITIONAL, so it silently rebinds to whichever station slid into that
/// index when the survey is edited. The plan file has never had that problem - it stores
/// anchors and re-places them on load - but a plan being edited in Plan mode lived only as
/// refs. Handing the anchors out lets the frontend hold each live pin the way the file does.
///
/// ⚠ **An anchor is OPAQUE to the frontend**: TS sees `unknown`, the fields are private to
/// `core::state::dive_plan`, and the only legal things to do with one are store it and hand it
/// back to `plan_refs_for_anchors`. Nothing outside this crate should build, inspect or
/// interpolate one - the shape is a persistence detail that has already changed once (#24).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct PlanAnchorSet {
    #[ts(type = "unknown | null")]
    pub origin: Option<StoredAnchor>,
    #[ts(type = "unknown | null")]
    pub endpoint: Option<StoredAnchor>,
    /// Index-aligned with the RESOLVED `waypoints`: a dangling way point leaves both lists
    /// together, so every entry here is populated. The `Option` is for shape, not for absence.
    #[ts(type = "Array<unknown | null>")]
    pub waypoints: Vec<Option<StoredAnchor>>,
    /// Index-aligned with the resolved `marked_points`, same as `waypoints`.
    #[ts(type = "Array<unknown | null>")]
    pub marked_points: Vec<Option<StoredAnchor>>,
    /// Index-aligned with `stage_drop_overrides`, which KEEPS its slots (an override that no
    /// longer places is nulled rather than dropped, because its position names a stage), so a
    /// `None` here is a genuinely absent or dangling override.
    #[ts(type = "Array<unknown | null>")]
    pub stage_drop_overrides: Vec<Option<StoredAnchor>>,
}

/// A loaded dive plan: its metadata, the snapshot to apply, and how many station
/// references no longer resolve against the current survey (nulled on load; the UI
/// warns when > 0). See section 6 of the design doc.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DivePlan {
    pub meta: DivePlanMeta,
    pub snapshot: DivePlanSnapshot,
    pub stale_ref_count: u32,
    /// The anchors the resolved snapshot came from, so loading a plan seeds the live pins
    /// without a second round trip (#310). Defaults when absent so an older blob - or a test
    /// fixture - still deserializes.
    #[serde(default)]
    pub anchors: PlanAnchorSet,
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::float_cmp,
        reason = "tests build from known-valid literals; a failed unwrap should fail the test"
    )]
    use super::*;

    fn sample_json() -> String {
        // A full v1 snapshot as it would be written by the app.
        serde_json::json!({
            "origin": { "section_id": 1, "station_index": 0 },
            "endpoint": { "section_id": 2, "station_index": 5 },
            "diver": { "rmv_l_per_min": 18.0, "label": "lead" },
            "cylinders": [],
            "gas_config": serde_json::to_value(GasConfig::default()).unwrap(),
            "stage_drop_overrides": [null, { "section_id": 1, "station_index": 3 }],
            "waypoints": [{ "section_id": 1, "station_index": 2 }],
            "marked_points": [
                { "station": { "section_id": 2, "station_index": 1 }, "label": "photo" },
                { "station": { "section_id": 2, "station_index": 4 } }
            ],
            "segments": [
                { "speed_m_per_min": 10.0, "on_scooter": true },
                { "speed_m_per_min": 15.0, "on_scooter": false }
            ]
        })
        .to_string()
    }

    #[test]
    fn round_trips_through_serde() {
        let snap: DivePlanSnapshot = serde_json::from_str(&sample_json()).unwrap();
        assert_eq!(snap.origin.unwrap().station_index, 0);
        assert_eq!(snap.endpoint.unwrap().section_id, 2);
        assert_eq!(snap.diver.rmv_l_per_min, 18.0);
        assert_eq!(snap.stage_drop_overrides.len(), 2);
        assert!(snap.stage_drop_overrides[0].is_none());
        assert_eq!(snap.stage_drop_overrides[1].unwrap().station_index, 3);
        assert_eq!(snap.waypoints, vec![StationRef::new(1, 2)]);
        assert_eq!(snap.marked_points.len(), 2);
        assert_eq!(snap.marked_points[0].label.as_deref(), Some("photo"));
        // A bare pin carries no label - the field defaults rather than failing the parse.
        assert!(snap.marked_points[1].label.is_none());
        assert_eq!(snap.segments.len(), 2);
        assert_eq!(snap.segments[0].speed_m_per_min, Some(10.0));
        assert!(snap.segments[0].on_scooter);
        assert!(!snap.segments[1].on_scooter);
        // Re-serialize and reload: the round trip is lossless for the known fields.
        let again: DivePlanSnapshot =
            serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        assert_eq!(again.endpoint.unwrap().station_index, 5);
        assert_eq!(again.waypoints, snap.waypoints);
        assert_eq!(again.marked_points, snap.marked_points);
        assert_eq!(again.segments, snap.segments);
    }

    #[test]
    fn tolerates_unknown_fields() {
        // A field from a hypothetical newer build must be ignored, not rejected.
        let json = serde_json::json!({
            "origin": null,
            "diver": { "rmv_l_per_min": 20.0, "label": null },
            "future_field_from_v2": { "anything": 42 }
        })
        .to_string();
        let snap: DivePlanSnapshot = serde_json::from_str(&json).unwrap();
        assert!(snap.origin.is_none());
        assert_eq!(snap.diver.rmv_l_per_min, 20.0);
    }

    #[test]
    fn defaults_missing_fields() {
        // An almost-empty blob loads: origin/endpoint None, empty vecs, default
        // config, and the diver falls back to the canonical rate.
        let snap: DivePlanSnapshot = serde_json::from_str("{}").unwrap();
        assert!(snap.origin.is_none());
        assert!(snap.endpoint.is_none());
        assert!(snap.cylinders.is_empty());
        assert!(snap.stage_drop_overrides.is_empty());
        assert_eq!(snap.diver.rmv_l_per_min, 20.0);
        // A plan written before #298 has neither list; both default to empty rather than
        // failing the parse, which is why the schema version did not need a bump.
        assert!(snap.waypoints.is_empty());
        assert!(snap.marked_points.is_empty());
        // Same for a plan written before #317: no segments, no bump needed either.
        assert!(snap.segments.is_empty());
    }
}
