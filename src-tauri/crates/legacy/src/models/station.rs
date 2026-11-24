use serde::{Deserialize, Serialize};

use super::LineId;

/// A station's project-unique, never-reused integer identity: the Station ID
/// the user sees (the Station detail field, the Data table column, the
/// "Station IDs" map and image-export label). Allocated from
/// [`crate::models::Project::next_station_id`] in line order, station order, by
/// [`crate::models::Project::ensure_station_identities`]; carried with its physical
/// station through every structural edit by the same `StationKey` walkers that
/// carry `station_names`. Never shown as the positional `line:index` handle,
/// which stays an internal session address.
pub type StationId = u32;

/// Every valid Station ID is below this (2^31). A stored id at or above it
/// (a crafted or corrupt file, a snapshot, an Add that would overflow) is
/// invalid: the normalization re-mints it, and a counter past it is brought
/// back ([`crate::models::Project::ensure_station_identities`]). The headroom above
/// keeps `next_station_id` arithmetic far from `u32::MAX`, where saturating
/// adds handed out one id twice.
pub const MAX_STATION_ID: StationId = 1 << 31;

/// Reference to a station using line_id:station_index format
///
/// Ordered lexicographically by `(line_id, station_index)` - the field order
/// the derive uses. The order is load-bearing, not cosmetic: the closure solve
/// numbers its unknowns from a SORTED sequence of station refs so the adjusted
/// geometry is a function of the project rather than of `HashMap` iteration
/// order (see `Project::adjust_least_squares`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
// `export_to` without `export` (here and on `GpsCoordinates` / `Coordinates`):
// no export test writes the file - it must only carry the member-crate path
// base, because `ImportPlacement` (same base, see its doc comment) lists these
// as ts-rs dependencies and mixed bases make the import diff unrepresentable.
pub struct StationRef {
    pub line_id: LineId,
    pub station_index: u32,
}

impl StationRef {
    pub fn new(line_id: LineId, station_index: u32) -> Self {
        Self {
            line_id,
            station_index,
        }
    }
}

impl std::fmt::Display for StationRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line_id, self.station_index)
    }
}

/// Position-independent identity of a station within a line.
///
/// A station is the boundary between two shots; `LineStart` is the boundary
/// before the first shot (station index 0) and `AfterShot(uuid)` is the boundary
/// after the shot with that stable UUID (station index = that shot's position + 1).
/// Unlike a positional `station_index`, this survives inserting/deleting other
/// shots in the line, so it is the *stored* form of a station reference
/// (see [`StationKey`]). It is resolved to a transient positional [`StationRef`]
/// at the geometry/IPC boundary.
// Internal identity types - NOT crossing IPC (the wire ref stays positional
// `StationRef`), so no `TS` derive; serde is for in-memory project (de)serialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StationAnchor {
    LineStart,
    AfterShot(String),
    /// Positional fallback: a raw station index. Used ONLY transitionally - when
    /// loading a pre-anchor (v6) file's positional refs, and for test/construction
    /// convenience - then canonicalized to `LineStart`/`AfterShot` by the load
    /// re-anchor pass. The library never compares or looks up with an `Index` key
    /// against a stored (canonical) key; it builds keys via `Project::key_for_ref`
    /// and compares via `Project::resolve_key`. A stray `Index` therefore can't
    /// silently mismatch a canonical key at runtime.
    Index(u32),
}

/// Stored, position-independent station reference: a line plus a
/// [`StationAnchor`]. This is what cross-references (merges, closures, datums,
/// station offsets/comments) persist and hold in memory, so an insert/delete of
/// an unrelated shot never has to rewrite them and a reference can never silently
/// point at the wrong in-range station. The solver/IPC use the transient
/// positional [`StationRef`], obtained via `Project::resolve_key`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct StationKey {
    pub line_id: LineId,
    pub anchor: StationAnchor,
}

// Backward-compatible deserialization: accept BOTH the current
// `{line_id, anchor}` shape AND the legacy positional `{line_id,
// station_index}` shape written by pre-anchor builds. The legacy shape appears
// in persisted data that embeds a serialized `Project` or `StationMerge`/
// `LoopClosure`/`ZeroDatum`/offset/comment key - most importantly the
// `undo_stack` snapshot blobs, which are NOT re-encoded through the positional
// wire form. Without this, opening an old project after upgrading would fail to
// deserialize every legacy undo entry and silently drop the undo history. A
// legacy positional ref maps to a transitional `LineStart`/`Index` anchor,
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
            line_id: LineId,
            #[serde(default)]
            anchor: Option<StationAnchor>,
            #[serde(default)]
            station_index: Option<u32>,
        }
        let c = Compat::deserialize(deserializer)?;
        match (c.anchor, c.station_index) {
            // Current shape - the anchor is authoritative.
            (Some(anchor), _) => Ok(StationKey {
                line_id: c.line_id,
                anchor,
            }),
            // Legacy positional shape - map index to a transitional anchor.
            (None, Some(index)) => Ok(StationKey::positional(c.line_id, index)),
            (None, None) => Err(<D::Error as serde::de::Error>::missing_field("anchor")),
        }
    }
}

impl StationKey {
    pub fn new(line_id: LineId, anchor: StationAnchor) -> Self {
        Self { line_id, anchor }
    }

    /// A positional key (index 0 → `LineStart`, else `Index`). Transitional /
    /// test convenience; the library canonicalizes to `AfterShot` via
    /// [`crate::models::Project::key_for_ref`].
    pub fn positional(line_id: LineId, station_index: u32) -> Self {
        let anchor = if station_index == 0 {
            StationAnchor::LineStart
        } else {
            StationAnchor::Index(station_index)
        };
        Self { line_id, anchor }
    }
}

impl From<StationRef> for StationKey {
    fn from(r: StationRef) -> Self {
        StationKey::positional(r.line_id, r.station_index)
    }
}

impl std::fmt::Display for StationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.anchor {
            StationAnchor::LineStart => write!(f, "{}:start", self.line_id),
            StationAnchor::AfterShot(uuid) => write!(f, "{}:after({uuid})", self.line_id),
            StationAnchor::Index(i) => write!(f, "{}:{i}", self.line_id),
        }
    }
}

/// GPS coordinates in WGS84 format
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
// Member-crate path base only - see the rationale on `StationRef`.
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

/// Internal XYZ coordinates (meters)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
// Member-crate path base only - see the rationale on `StationRef`.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// The skip stays (unlike `CommandResult::line_ids` in the core crate,
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
    pub closures_with: Option<Vec<ClosureRef>>,
    /// User comment for this station
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The station's project-unique Station ID ([`StationId`]).
    #[serde(default)]
    pub id: StationId,
    /// The station's name, unique within its line (e.g. `"17"`, `"A3"`, or
    /// the MNemo station number a DMP import recorded). Every station has one
    /// once the project is normalized ([`crate::models::Project::ensure_station_identities`]);
    /// empty only for a station the normalization could not key (a shot
    /// without a uuid), which a loaded project never has.
    #[serde(default)]
    pub name: String,
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
            id: 0,
            name: String::new(),
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
            id: 0,
            name: String::new(),
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
///
/// One list holds every datum. Its role is derived, never stored
/// ([`DatumRole`], `Project::datum_roles`): the first resolvable datum of each
/// merge-connected component PLACES that rigid group, and every later one in
/// the same component is a GPS FIX that only the Closures Computed View solve
/// reads (#741).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZeroDatum {
    pub station_ref: StationKey,
    pub gps: GpsCoordinates,
    /// Horizontal accuracy of the GPS position in metres. `None` means
    /// [`DEFAULT_GPS_ACCURACY_M`]; `Some(0.0)` is a hard pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f64>,
    /// The user's answer to "is this station a cave entrance?", or `None` for
    /// the default rule (`Project::datum_entrances`): a placement datum is
    /// always an entrance, a GPS fix is one when its station is the first or
    /// last station of its line (#741).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrance: Option<bool>,
}

impl ZeroDatum {
    /// A datum with the default accuracy and the default entrance rule.
    pub fn new(station_ref: StationKey, gps: GpsCoordinates) -> Self {
        Self {
            station_ref,
            gps,
            accuracy_m: None,
            entrance: None,
        }
    }

    /// The accuracy the solve uses: the stored one, or the default.
    pub fn effective_accuracy_m(&self) -> f64 {
        self.accuracy_m.unwrap_or(DEFAULT_GPS_ACCURACY_M)
    }
}

/// Default horizontal accuracy of a zero datum's GPS, in metres: 0, a hard pin.
///
/// The survey bends to go exactly through the GPS, the way Ariane treats every
/// entrance. A soft default (5 m was measured) barely moves the Closures
/// Computed View, because the solve trusts a 500 m traverse to about a metre;
/// a user who wants a softer fix types an accuracy (#741).
pub const DEFAULT_GPS_ACCURACY_M: f64 = 0.0;

/// Largest horizontal accuracy a zero datum may claim, in metres. A fix worse
/// than this carries no position worth pulling a survey toward; refusing it
/// keeps a typo (an extra digit) from entering the solve as a weight.
pub const MAX_GPS_ACCURACY_M: f64 = 1000.0;

/// Validate a zero datum's accuracy: absent (the default), or a finite number
/// of metres in `0 ..= MAX_GPS_ACCURACY_M`. `0` is a hard pin.
pub fn validate_gps_accuracy(accuracy_m: Option<f64>) -> Result<(), String> {
    match accuracy_m {
        None => Ok(()),
        Some(a) if a.is_finite() && (0.0..=MAX_GPS_ACCURACY_M).contains(&a) => Ok(()),
        Some(a) => Err(format!(
            "GPS accuracy must be between 0 and {MAX_GPS_ACCURACY_M} m (got {a})"
        )),
    }
}

/// What a zero datum does, derived per merge-connected component (#741).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatumRole {
    /// The first resolvable datum of its merge component: it places that rigid
    /// group in Raw Data View, exactly as a datum always has. The first
    /// placement datum of the project also defines the frame.
    Placement,
    /// A later datum in a component that is already placed. It never moves raw
    /// geometry; the Closures Computed View solve pulls the survey toward it.
    GpsFix,
}

/// Positional wire form of [`StationMerge`] for the frontend (which reasons by
/// `station_index`). ts-rs exports it as `StationMerge` so the generated TS and
/// the frontend stay positional; the backend resolves the stored keys when encoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationMergeWire {
    pub station_a: StationRef,
    pub station_b: StationRef,
    pub merged_at: chrono::DateTime<chrono::Utc>,
}

/// Positional wire form of [`ZeroDatum`]; ts-rs exports it as `ZeroDatum`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZeroDatumWire {
    pub station_ref: StationRef,
    pub gps: GpsCoordinates,
    /// Stored accuracy in metres; absent means [`DEFAULT_GPS_ACCURACY_M`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f64>,
    /// Derived role in its merge component.
    pub role: DatumRole,
    /// Whether the station counts as a cave entrance (Plan mode's "Start
    /// from entrance", penetration), resolved by the backend from the role,
    /// the stored override and the default rule, so the frontend never
    /// re-derives it (#741).
    pub is_entrance: bool,
    /// For a GPS fix, how far its GPS is from where the raw survey puts its
    /// station (`Project::gps_fix_gaps`), so the Details panel shows the
    /// backend's number. Absent for a placement datum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<crate::models::DatumGap>,
}

/// Strategy for distributing loop closure errors
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosurePreview {
    pub path_length: f64,
    pub misclosure_distance: f64,
    pub error_percentage: f64,
    pub horizontal_misclosure: f64,
    pub vertical_misclosure: f64,
}

/// Result of a connectivity check between two stations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivityResult {
    pub connected: bool,
    pub preview: Option<ClosurePreview>,
    /// Whether merge is allowed for the given station pair
    #[serde(default)]
    pub can_merge: bool,
    /// Explanation when merge is not allowed
    #[serde(default)]
    pub reason: Option<String>,
    /// Why a loop closure between the pair would be refused (#737): it would
    /// tie the two ends of one shot together, directly or through merges and
    /// other closures. The closure dialog shows it and
    /// disables Create; `create_loop_closure` refuses the same pair with the
    /// same words regardless. Absent when a closure would close a real loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure_refusal: Option<String>,
    /// When both stations' pieces are placed by their own zero datum (#741):
    /// the piece the merge moves, by how much, and the datum that becomes a
    /// GPS fix. The Merge dialog says so before the user confirms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum_shift: Option<crate::models::MergeDatumShift>,
}
