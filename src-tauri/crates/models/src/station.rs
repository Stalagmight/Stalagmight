use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::SectionId;

/// Reference to a station using section_id:station_index format
///
/// Ordered lexicographically by `(section_id, station_index)` - the field order
/// the derive uses. The order is load-bearing, not cosmetic: the closure solve
/// numbers its unknowns from a SORTED sequence of station refs so the adjusted
/// geometry is a function of the project rather than of `HashMap` iteration
/// order (see `Project::adjust_least_squares`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
// `export_to` without `export` (here and on `GpsCoordinates` / `Coordinates`):
// no export test writes the file - it must only carry the member-crate path
// base, because `ImportPlacement` (same base, see its doc comment) lists these
// as ts-rs dependencies and mixed bases make the import diff unrepresentable.
#[ts(export_to = "../../bindings/StationRef.ts")]
pub struct StationRef {
    pub section_id: SectionId,
    pub station_index: u32,
}

impl StationRef {
    pub fn new(section_id: SectionId, station_index: u32) -> Self {
        Self {
            section_id,
            station_index,
        }
    }
}

impl std::fmt::Display for StationRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.section_id, self.station_index)
    }
}

/// Position-independent identity of a station within a section.
///
/// A station is the boundary between two shots; `SectionStart` is the boundary
/// before the first shot (station index 0) and `AfterShot(uuid)` is the boundary
/// after the shot with that stable UUID (station index = that shot's position + 1).
/// Unlike a positional `station_index`, this survives inserting/deleting other
/// shots in the section, so it is the *stored* form of a station reference
/// (see [`StationKey`]). It is resolved to a transient positional [`StationRef`]
/// at the geometry/IPC boundary.
// Internal identity types - NOT crossing IPC (the wire ref stays positional
// `StationRef`), so no `TS` derive; serde is for in-memory project (de)serialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StationAnchor {
    SectionStart,
    AfterShot(String),
    /// Positional fallback: a raw station index. Used ONLY transitionally - when
    /// loading a pre-anchor (v6) file's positional refs, and for test/construction
    /// convenience - then canonicalized to `SectionStart`/`AfterShot` by the load
    /// re-anchor pass. The library never compares or looks up with an `Index` key
    /// against a stored (canonical) key; it builds keys via `Project::key_for_ref`
    /// and compares via `Project::resolve_key`. A stray `Index` therefore can't
    /// silently mismatch a canonical key at runtime.
    Index(u32),
}

/// Stored, position-independent station reference: a section plus a
/// [`StationAnchor`]. This is what cross-references (merges, closures, datums,
/// station offsets/comments) persist and hold in memory, so an insert/delete of
/// an unrelated shot never has to rewrite them and a reference can never silently
/// point at the wrong in-range station. The solver/IPC use the transient
/// positional [`StationRef`], obtained via `Project::resolve_key`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct StationKey {
    pub section_id: SectionId,
    pub anchor: StationAnchor,
}

// Backward-compatible deserialization: accept BOTH the current
// `{section_id, anchor}` shape AND the legacy positional `{section_id,
// station_index}` shape written by pre-anchor builds. The legacy shape appears
// in persisted data that embeds a serialized `Project` or `StationMerge`/
// `LoopClosure`/`ZeroDatum`/offset/comment key - most importantly the
// `undo_stack` snapshot blobs, which are NOT re-encoded through the positional
// wire form. Without this, opening an old project after upgrading would fail to
// deserialize every legacy undo entry and silently drop the undo history. A
// legacy positional ref maps to a transitional `SectionStart`/`Index` anchor,
// which the resolve layer tolerates and the load re-anchor pass canonicalizes.
//
// NOTE: this `Compat` shim distinguishes the two shapes by FIELD NAME, so it only
// works for self-describing formats (JSON - what the undo blobs and offset/comment
// maps use). If a future code path serializes a `StationKey` through a
// non-self-describing format (bincode/postcard/positional msgpack), this manual
// impl must be revisited - there is no such path today (the binary IPC/SQLite
// routes use the positional `StationRef` wire types, never `StationKey`).
impl<'de> Deserialize<'de> for StationKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Compat {
            section_id: SectionId,
            #[serde(default)]
            anchor: Option<StationAnchor>,
            #[serde(default)]
            station_index: Option<u32>,
        }
        let c = Compat::deserialize(deserializer)?;
        match (c.anchor, c.station_index) {
            // Current shape - the anchor is authoritative.
            (Some(anchor), _) => Ok(StationKey {
                section_id: c.section_id,
                anchor,
            }),
            // Legacy positional shape - map index to a transitional anchor.
            (None, Some(index)) => Ok(StationKey::positional(c.section_id, index)),
            (None, None) => Err(<D::Error as serde::de::Error>::missing_field("anchor")),
        }
    }
}

impl StationKey {
    pub fn new(section_id: SectionId, anchor: StationAnchor) -> Self {
        Self { section_id, anchor }
    }

    /// A positional key (index 0 → `SectionStart`, else `Index`). Transitional /
    /// test convenience; the library canonicalizes to `AfterShot` via
    /// [`crate::Project::key_for_ref`].
    pub fn positional(section_id: SectionId, station_index: u32) -> Self {
        let anchor = if station_index == 0 {
            StationAnchor::SectionStart
        } else {
            StationAnchor::Index(station_index)
        };
        Self { section_id, anchor }
    }
}

impl From<StationRef> for StationKey {
    fn from(r: StationRef) -> Self {
        StationKey::positional(r.section_id, r.station_index)
    }
}

impl std::fmt::Display for StationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.anchor {
            StationAnchor::SectionStart => write!(f, "{}:start", self.section_id),
            StationAnchor::AfterShot(uuid) => write!(f, "{}:after({uuid})", self.section_id),
            StationAnchor::Index(i) => write!(f, "{}:{i}", self.section_id),
        }
    }
}

/// GPS coordinates in WGS84 format
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
// Member-crate path base only - see the rationale on `StationRef`.
#[ts(export_to = "../../bindings/GpsCoordinates.ts")]
pub struct GpsCoordinates {
    pub latitude: f64,
    pub longitude: f64,
    /// Depth reference (meters below surface)
    pub depth_reference: f64,
}

/// Largest magnitude a zero datum's depth reference may carry, in meters.
///
/// Finiteness alone is not a range: `1e300` is finite, and it reached the
/// project end to end - through the zero-datum dialog, into the geometry, and
/// into every distance derived from it. The bound is deliberately generous
/// rather than diving-realistic (Challenger Deep is ~10 935 m below the surface
/// and Everest ~8 849 m above it, and the field is signed because a datum can
/// sit above the surface), so it refuses only values that are meaningless for
/// any point on Earth.
///
/// Mirrored by `MAX_DEPTH_REFERENCE_M` in
/// src/components/Dialogs/ZeroDatumConfigDialog.tsx; the pair is pinned by
/// src/test/__tests__/configBoundsMirror.test.ts, which parses this constant.
pub const MAX_DEPTH_REFERENCE_M: f64 = 11_000.0;

impl GpsCoordinates {
    /// Creates new GPS coordinates after validation
    /// Returns Err if values are non-finite or out of range
    pub fn new(latitude: f64, longitude: f64, depth_reference: f64) -> Result<Self, String> {
        Self::validate(latitude, longitude, depth_reference)?;
        Ok(Self {
            latitude,
            longitude: Self::normalize_longitude(longitude),
            depth_reference,
        })
    }

    /// Validates GPS coordinate values
    /// Latitude must be in [-90, 90], longitude in [-180, 180]
    /// All values must be finite (not NaN or ±infinity)
    pub fn validate(latitude: f64, longitude: f64, depth_reference: f64) -> Result<(), String> {
        if !latitude.is_finite() {
            return Err(format!("Latitude must be finite, got {latitude}"));
        }
        if !longitude.is_finite() {
            return Err(format!("Longitude must be finite, got {longitude}"));
        }
        if !depth_reference.is_finite() {
            return Err(format!(
                "Depth reference must be finite, got {depth_reference}"
            ));
        }
        if !(-MAX_DEPTH_REFERENCE_M..=MAX_DEPTH_REFERENCE_M).contains(&depth_reference) {
            return Err(format!(
                "Depth reference must be in [-{MAX_DEPTH_REFERENCE_M}, {MAX_DEPTH_REFERENCE_M}] meters, got {depth_reference}"
            ));
        }
        if !(-90.0..=90.0).contains(&latitude) {
            return Err(format!("Latitude must be in [-90, 90], got {latitude}"));
        }
        if !(-180.0..=180.0).contains(&longitude) {
            return Err(format!("Longitude must be in [-180, 180], got {longitude}"));
        }
        Ok(())
    }

    /// Checks if coordinates are valid (finite and in range)
    pub fn is_valid(&self) -> bool {
        Self::validate(self.latitude, self.longitude, self.depth_reference).is_ok()
    }

    /// Normalizes longitude to [-180, 180) range
    fn normalize_longitude(mut lon: f64) -> f64 {
        while lon >= 180.0 {
            lon -= 360.0;
        }
        while lon < -180.0 {
            lon += 360.0;
        }
        lon
    }
}

/// GPS bounding box in WGS84 format
/// Uses cardinal directions: north/south for latitude, east/west for longitude
/// Note: When bounds cross the international dateline, `east < west`.
/// For example, bounds spanning from 170°E to 170°W would have west=170, east=-170.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct GpsBounds {
    /// Northern boundary (max latitude)
    pub north: f64,
    /// Southern boundary (min latitude)
    pub south: f64,
    /// Eastern boundary longitude. If `east < west`, bounds cross the dateline.
    pub east: f64,
    /// Western boundary longitude
    pub west: f64,
}

/// Internal XYZ coordinates (meters)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
// Member-crate path base only - see the rationale on `StationRef`.
#[ts(export_to = "../../bindings/Coordinates.ts")]
pub struct Coordinates {
    /// X = East (meters)
    pub x: f64,
    /// Y = North (meters)
    pub y: f64,
    /// Z = Depth (meters, positive = below surface)
    pub z: f64,
}

impl Coordinates {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// True if all three components are finite (no NaN/±inf).
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Construct only if every component is finite. Use at ingest boundaries
    /// (offsets/moves) so a non-finite value can't enter coordinate computation,
    /// where it silently poisons every downstream distance/alignment comparison.
    pub fn new_checked(x: f64, y: f64, z: f64) -> Result<Self, String> {
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return Err(format!("Coordinates must be finite, got ({x}, {y}, {z})"));
        }
        Ok(Self { x, y, z })
    }

    /// Calculate distance to another coordinate
    pub fn distance_to(&self, other: &Coordinates) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        let dz = other.z - self.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Calculate 2D horizontal distance to another coordinate
    pub fn horizontal_distance_to(&self, other: &Coordinates) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Format this offset as a human-readable string with 16-point compass direction.
    ///
    /// x = East, y = North, z = Depth (positive = below surface).
    /// Examples: "5.3m NNW", "5.3m NNW, 1.2m down", "1.2m up", "no movement"
    pub fn display_offset(&self) -> String {
        const COMPASS: [&str; 16] = [
            "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
            "NW", "NNW",
        ];

        let horiz_dist = (self.x * self.x + self.y * self.y).sqrt();
        let has_horiz = horiz_dist >= 0.01;
        let vert_abs = self.z.abs();
        let has_vert = vert_abs >= 0.01;

        if !has_horiz && !has_vert {
            return "no movement".to_string();
        }

        let mut parts = Vec::with_capacity(2);

        if has_horiz {
            // Bearing from North: atan2(East, North) in degrees [0, 360)
            let bearing_rad = self.x.atan2(self.y);
            let bearing_deg = bearing_rad.to_degrees().rem_euclid(360.0);
            let sector = ((bearing_deg + 11.25) / 22.5).floor() as usize % 16;
            parts.push(format!("{:.1}m {}", horiz_dist, COMPASS[sector]));
        }

        if has_vert {
            let direction = if self.z > 0.0 { "down" } else { "up" };
            parts.push(format!("{vert_abs:.1}m {direction}"));
        }

        parts.join(", ")
    }
}

/// `skip_serializing_if` for [`Station::closures_with`]: absent and empty are
/// the same fact, so both leave the key off the wire.
///
/// `Option::is_none` alone would not be enough. `Station::closures_mut` returns
/// a `&mut Vec`, so `Some(vec![])` is reachable without ever pushing - and a
/// future `clear()`/`retain()` can empty an existing list - after which every
/// station carries `"closures_with":[]` for the life of the value, which is
/// exactly the per-station cost the skip exists to avoid. It also normalizes a
/// stored empty array back to nothing on re-serialize.
#[expect(
    clippy::ref_option,
    reason = "serde calls skip_serializing_if with &FieldType; Option<&Vec<_>> does not typecheck there"
)]
fn closures_absent_or_empty(closures: &Option<Vec<ClosureRef>>) -> bool {
    closures.as_ref().is_none_or(Vec::is_empty)
}

/// A computed station in the cave survey
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Station {
    /// Reference to this station
    pub station_ref: StationRef,
    /// Computed coordinates (or GPS-assigned)
    pub coordinates: Coordinates,
    /// Whether this station has GPS coordinates assigned
    pub is_zero_datum: bool,
    /// GPS coordinates if this is a zero datum
    pub gps_coordinates: Option<GpsCoordinates>,
    /// Other stations merged with this one
    pub merged_with: Vec<StationRef>,
    /// Loop closures involving this station; `None` when there are none.
    ///
    /// `Option<Vec<_>>` rather than `Vec<_>` for the sake of the GENERATED TYPE
    /// (#441). ts-rs 7 emits `field?:` only for an `Option<T>` - `optional` is
    /// rejected on any other type - so a `skip_serializing_if` `Vec` would
    /// generate as a required `Array<ClosureRef>` that the wire does not always
    /// carry, and `tsc` would be blind to every bare `station.closures_with.map`.
    ///
    /// The skip stays (unlike `CommandResult::section_ids` in the core crate,
    /// which drops it): this one is paid PER STATION - `get_project_state`
    /// re-serializes every station as JSON on each refresh, and that payload is
    /// already ~25 MB for the 89k-station projects the binary loader exists for.
    /// Always sending an empty array there is not free; sending it once per
    /// command result is.
    ///
    /// `None` and `Some(vec![])` mean the same thing, and
    /// [`closures_absent_or_empty`] keeps BOTH off the wire - `closures_mut`
    /// hands out a `&mut Vec`, so an empty `Some` is reachable without a push.
    ///
    /// `#[ts(optional)]` is explicit because ts-rs only infers optionality from
    /// the literal predicate `"Option::is_none"` (`attr/field.rs:70`); a named
    /// predicate would silently regenerate this as a required array.
    #[serde(default, skip_serializing_if = "closures_absent_or_empty")]
    #[ts(optional)]
    pub closures_with: Option<Vec<ClosureRef>>,
    /// User comment for this station
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Station name (e.g. the mNemo station number from a DMP import). Not
    /// unique - shown next to the positional id, except in the opt-in Station
    /// names map label and image-export option, which are exclusive with the
    /// id labels. Off the wire when absent: `get_project_state` serializes
    /// every station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Station {
    pub fn new(station_ref: StationRef) -> Self {
        Self {
            station_ref,
            coordinates: Coordinates::default(),
            is_zero_datum: false,
            gps_coordinates: None,
            merged_with: Vec::new(),
            closures_with: None,
            comment: None,
            name: None,
        }
    }

    pub fn with_coordinates(station_ref: StationRef, coordinates: Coordinates) -> Self {
        Self {
            station_ref,
            coordinates,
            is_zero_datum: false,
            gps_coordinates: None,
            merged_with: Vec::new(),
            closures_with: None,
            comment: None,
            name: None,
        }
    }

    /// The loop closures involving this station; empty when the field is absent.
    /// Read through this rather than matching the `Option`: `None` and an empty
    /// `Vec` are the same fact (see the field's doc comment).
    pub fn closures(&self) -> &[ClosureRef] {
        self.closures_with.as_deref().unwrap_or_default()
    }

    /// The closure list for writing, created on first use. Going through this
    /// keeps the "`None` when empty" invariant that keeps the key off the wire.
    pub fn closures_mut(&mut self) -> &mut Vec<ClosureRef> {
        self.closures_with.get_or_insert_with(Vec::new)
    }
}

/// Record of a station merge. Endpoints stored as position-independent
/// [`StationKey`]s. Backend-only (no `TS`); the frontend/wire form is
/// [`StationMergeWire`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationMerge {
    pub station_a: StationKey,
    pub station_b: StationKey,
    pub merged_at: chrono::DateTime<chrono::Utc>,
}

/// Zero datum assignment (backend; wire form is [`ZeroDatumWire`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZeroDatum {
    pub station_ref: StationKey,
    pub gps: GpsCoordinates,
}

/// Positional wire form of [`StationMerge`] for the frontend (which reasons by
/// `station_index`). ts-rs exports it as `StationMerge` so the generated TS and
/// the frontend stay positional; the backend resolves the stored keys when encoding.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(rename = "StationMerge", export_to = "bindings/StationMerge.ts")]
pub struct StationMergeWire {
    pub station_a: StationRef,
    pub station_b: StationRef,
    #[ts(type = "string")]
    pub merged_at: chrono::DateTime<chrono::Utc>,
}

/// Positional wire form of [`ZeroDatum`]; ts-rs exports it as `ZeroDatum`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(rename = "ZeroDatum", export_to = "bindings/ZeroDatum.ts")]
pub struct ZeroDatumWire {
    pub station_ref: StationRef,
    pub gps: GpsCoordinates,
}

/// Strategy for distributing loop closure errors
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, TS)]
pub enum ClosureStrategy {
    /// Distribute proportionally by shot length (Bowditch method).
    /// Deferred for now; retained for legacy data and potential future use.
    ProportionalByLength,
    /// Distribute equally across all shots.
    /// Deferred for now; retained for legacy data and potential future use.
    EqualDistribution,
    /// Defer adjustment for batch processing
    #[serde(alias = "KeepRaw", alias = "ManualAdjustment")]
    #[default]
    Batch,
    /// Weighted least-squares adjustment (batch-adjusted)
    #[serde(alias = "Sequential")]
    LeastSquaresWeighted,
    /// Created by snap-to-merge move (station was physically moved to align)
    Move,
}

impl ClosureStrategy {
    /// Convert to a string suitable for SQLite storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProportionalByLength => "ProportionalByLength",
            Self::EqualDistribution => "EqualDistribution",
            Self::Batch => "Batch",
            Self::LeastSquaresWeighted => "LeastSquaresWeighted",
            Self::Move => "Move",
        }
    }

    /// Parse from a SQLite string value, defaulting to Batch
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "ProportionalByLength" => Self::ProportionalByLength,
            "EqualDistribution" => Self::EqualDistribution,
            "Batch" => Self::Batch,
            "LeastSquaresWeighted" => Self::LeastSquaresWeighted,
            "Sequential" => Self::LeastSquaresWeighted,
            "KeepRaw" => Self::Batch,
            "ManualAdjustment" => Self::Batch,
            "Move" => Self::Move,
            _ => Self::default(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn station_ref_display_formats_as_section_and_index() {
        let station = StationRef::new(2, 3);
        assert_eq!(station.to_string(), "2:3");
    }

    /// #524 review: `is_finite` was the ONLY check on the depth reference, and
    /// finiteness is not a range - `1e300` is finite, and it reached the
    /// geometry and every distance derived from it.
    #[test]
    fn a_depth_reference_outside_any_earthly_range_is_refused() {
        assert!(
            1e300_f64.is_finite(),
            "premise: the old finiteness check accepts this"
        );

        for depth in [1e300, -1e300, MAX_DEPTH_REFERENCE_M + 1.0] {
            assert!(
                GpsCoordinates::new(10.0, 20.0, depth).is_err(),
                "{depth} m must be refused"
            );
        }

        // The bound itself, and ordinary values on both sides of the surface,
        // still pass - a datum ABOVE the surface is a negative depth.
        for depth in [
            0.0,
            -MAX_DEPTH_REFERENCE_M,
            MAX_DEPTH_REFERENCE_M,
            -120.0,
            85.0,
        ] {
            assert!(
                GpsCoordinates::new(10.0, 20.0, depth).is_ok(),
                "{depth} m must still be accepted"
            );
        }
    }

    /// #441: a field the wire can omit must be DECLARED omittable.
    ///
    /// ts-rs 7 emits `foo?:` only for an `Option<T>`, so `closures_with` has to be
    /// `Option<Vec<_>>` to keep its `skip_serializing_if` (worth keeping: the skip
    /// is paid per station on every `get_project_state`). `merged_with` is the
    /// control - same element shape, no skip, so it must stay required.
    ///
    /// Turns red if `closures_with` goes back to a plain `Vec` (declaration loses
    /// the `?` while the key still vanishes) or if the skip is dropped without
    /// retyping (the empty station starts carrying `"closures_with": []`).
    #[test]
    fn closures_with_is_omitted_when_empty_and_declared_optional() {
        use ts_rs::TS;

        let empty = serde_json::to_value(Station::new(StationRef::new(1, 0))).unwrap();
        assert!(
            !empty.as_object().unwrap().contains_key("closures_with"),
            "a station with no closures must omit the key, not send []: {empty}"
        );

        let mut linked = Station::new(StationRef::new(1, 0));
        linked.closures_mut().push(ClosureRef {
            closure_id: 7,
            station_ref: StationRef::new(2, 0),
            via_station: None,
            is_transitive: false,
        });
        let linked = serde_json::to_value(linked).unwrap();
        assert!(
            linked.as_object().unwrap().contains_key("closures_with"),
            "a station WITH closures must carry them: {linked}"
        );

        let decl = Station::decl();
        assert!(
            decl.contains("closures_with?:"),
            "closures_with leaves the wire when empty, so its declaration must carry \
             the `?`: {decl}"
        );
        assert!(
            decl.contains("merged_with: Array<StationRef>"),
            "merged_with is always serialized, so it must stay required: {decl}"
        );
    }

    /// #441 follow-up: EMPTY must leave the wire too, not just `None`.
    ///
    /// `closures_mut()` hands out a `&mut Vec`, so `Some(vec![])` is reachable
    /// without a push - by taking the list and not filling it, and by any future
    /// `clear()`/`retain()` that empties one. Under a bare
    /// `skip_serializing_if = "Option::is_none"` that value carries
    /// `"closures_with":[]` on every station for the rest of its life, which is
    /// precisely the per-station cost the skip exists to avoid.
    ///
    /// It also normalizes legacy input: a stored `"closures_with": []`
    /// deserializes to `Some(vec![])` and must not be re-serialized.
    #[test]
    fn an_emptied_closure_list_leaves_the_wire_like_an_absent_one() {
        let mut taken = Station::new(StationRef::new(1, 0));
        let _ = taken.closures_mut();
        let taken = serde_json::to_value(taken).unwrap();
        assert!(
            !taken.as_object().unwrap().contains_key("closures_with"),
            "an empty closure list must be omitted exactly like an absent one: {taken}"
        );

        let legacy: Station = serde_json::from_value(serde_json::json!({
            "station_ref": { "section_id": 1, "station_index": 0 },
            "coordinates": { "x": 0.0, "y": 0.0, "z": 0.0 },
            "is_zero_datum": false,
            "gps_coordinates": null,
            "merged_with": [],
            "closures_with": [],
        }))
        .unwrap();
        let legacy = serde_json::to_value(legacy).unwrap();
        assert!(
            !legacy.as_object().unwrap().contains_key("closures_with"),
            "a stored empty array must not survive a re-serialize: {legacy}"
        );
    }

    #[test]
    fn gps_coordinates_normalizes_longitude() {
        let gps = GpsCoordinates::new(10.0, 180.0, 5.0).unwrap();
        assert!((gps.longitude + 180.0).abs() < 1e-6);
    }

    #[test]
    fn gps_coordinates_validation_rejects_invalid_values() {
        assert!(GpsCoordinates::new(91.0, 0.0, 0.0).is_err());
        assert!(GpsCoordinates::new(-91.0, 0.0, 0.0).is_err());
        assert!(GpsCoordinates::new(0.0, 181.0, 0.0).is_err());
        assert!(GpsCoordinates::new(0.0, -181.0, 0.0).is_err());
        assert!(GpsCoordinates::new(f64::NAN, 0.0, 0.0).is_err());
        assert!(GpsCoordinates::new(0.0, f64::NAN, 0.0).is_err());
        assert!(GpsCoordinates::new(0.0, 0.0, f64::NAN).is_err());
    }

    #[test]
    fn gps_coordinates_is_valid_checks_ranges() {
        let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();
        assert!(gps.is_valid());

        let invalid = GpsCoordinates {
            latitude: 120.0,
            longitude: 0.0,
            depth_reference: 0.0,
        };
        assert!(!invalid.is_valid());
    }

    #[test]
    fn coordinates_distance_calculations() {
        let a = Coordinates::new(0.0, 0.0, 0.0);
        let b = Coordinates::new(3.0, 4.0, 12.0);

        let distance = a.distance_to(&b);
        assert!((distance - 13.0).abs() < 1e-6);

        let horizontal = a.horizontal_distance_to(&b);
        assert!((horizontal - 5.0).abs() < 1e-6);
    }

    #[test]
    fn display_offset_no_movement() {
        assert_eq!(
            Coordinates::new(0.0, 0.0, 0.0).display_offset(),
            "no movement"
        );
        assert_eq!(
            Coordinates::new(0.005, 0.005, 0.005).display_offset(),
            "no movement"
        );
    }

    #[test]
    fn display_offset_cardinal_directions() {
        // Pure North
        assert_eq!(Coordinates::new(0.0, 5.0, 0.0).display_offset(), "5.0m N");
        // Pure East
        assert_eq!(Coordinates::new(5.0, 0.0, 0.0).display_offset(), "5.0m E");
        // Pure South
        assert_eq!(Coordinates::new(0.0, -5.0, 0.0).display_offset(), "5.0m S");
        // Pure West
        assert_eq!(Coordinates::new(-5.0, 0.0, 0.0).display_offset(), "5.0m W");
    }

    #[test]
    fn display_offset_intercardinal_directions() {
        // NE (equal x and y positive)
        let d = Coordinates::new(5.0, 5.0, 0.0).display_offset();
        assert!(d.contains("NE"), "expected NE in '{d}'");
        // SW (equal x and y negative)
        let d = Coordinates::new(-5.0, -5.0, 0.0).display_offset();
        assert!(d.contains("SW"), "expected SW in '{d}'");
    }

    #[test]
    fn display_offset_16_point_compass() {
        // NNE: bearing ~22.5° (small east, large north)
        let d = Coordinates::new(1.0, 2.5, 0.0).display_offset();
        assert!(d.contains("NNE"), "expected NNE in '{d}'");
        // SSW: bearing ~202.5° (small west, large south)
        let d = Coordinates::new(-1.0, -2.5, 0.0).display_offset();
        assert!(d.contains("SSW"), "expected SSW in '{d}'");
    }

    #[test]
    fn display_offset_vertical_only() {
        assert_eq!(
            Coordinates::new(0.0, 0.0, 3.0).display_offset(),
            "3.0m down"
        );
        assert_eq!(Coordinates::new(0.0, 0.0, -2.5).display_offset(), "2.5m up");
    }

    #[test]
    fn display_offset_combined_horizontal_and_vertical() {
        let d = Coordinates::new(0.0, 5.0, -1.2).display_offset();
        assert_eq!(d, "5.0m N, 1.2m up");
    }
}

/// Reference to a frozen shot (part of an adjusted survey network).
/// Frozen shots cannot have their geometry fields edited.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FrozenShotRef {
    pub section_id: SectionId,
    pub shot_index: u32,
}

/// A detected loop closure in the survey. Endpoints stored as position-independent
/// [`StationKey`]s. Backend-only (no `TS`); the wire form is [`LoopClosureWire`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopClosure {
    /// Unique identifier for this closure
    #[serde(default)]
    pub id: u64,
    /// First station in the closure (merge point A)
    pub station_a: StationKey,
    /// Second station in the closure (merge point B)
    pub station_b: StationKey,
    /// Total path length along the loop (meters)
    pub path_length: f64,
    /// Misclosure distance (meters) - how far apart the two stations are
    pub misclosure_distance: f64,
    /// Closure error percentage (misclosure / path_length * 100)
    pub error_percentage: f64,
    /// Horizontal misclosure component (meters)
    pub horizontal_misclosure: f64,
    /// Vertical misclosure component (meters)
    pub vertical_misclosure: f64,
    /// Strategy for distributing the closure error
    #[serde(default)]
    pub strategy: ClosureStrategy,
    /// Whether the closure endpoints have been aligned (post-adjustment or snap-to-merge)
    #[serde(default)]
    pub is_aligned: bool,
}

/// Positional wire form of [`LoopClosure`]; ts-rs exports it as `LoopClosure`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(rename = "LoopClosure", export_to = "bindings/LoopClosure.ts")]
pub struct LoopClosureWire {
    #[serde(default)]
    pub id: u64,
    pub station_a: StationRef,
    pub station_b: StationRef,
    pub path_length: f64,
    pub misclosure_distance: f64,
    pub error_percentage: f64,
    pub horizontal_misclosure: f64,
    pub vertical_misclosure: f64,
    #[serde(default)]
    pub strategy: ClosureStrategy,
    #[serde(default)]
    pub is_aligned: bool,
}

/// Reference to a loop closure from a station's perspective
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClosureRef {
    /// References LoopClosure.id
    pub closure_id: u64,
    /// Target station in the closure
    pub station_ref: StationRef,
    /// Intermediate station for transitive closures
    pub via_station: Option<StationRef>,
    /// Whether this is a transitive closure
    pub is_transitive: bool,
}

/// Preview data for a potential loop closure (read-only metrics, no mutation)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClosurePreview {
    pub path_length: f64,
    pub misclosure_distance: f64,
    pub error_percentage: f64,
    pub horizontal_misclosure: f64,
    pub vertical_misclosure: f64,
}

/// Result of a connectivity check between two stations
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConnectivityResult {
    pub connected: bool,
    pub preview: Option<ClosurePreview>,
    /// Whether merge is allowed for the given station pair
    #[serde(default)]
    pub can_merge: bool,
    /// Explanation when merge is not allowed
    #[serde(default)]
    pub reason: Option<String>,
}
