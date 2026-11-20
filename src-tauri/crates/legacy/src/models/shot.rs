use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

/// Unique identifier for a shot within a line
pub type ShotId = u32;

use super::{
    normalize_azimuth, AzimuthReference, LineId, SurveyCredit, EMPTY_CREDIT, FEATURE_ID_STRIDE,
};

/// Convert a raw ShotId to a 1-indexed display position.
///
/// After database reload, shot IDs have the form `line_id * FEATURE_ID_STRIDE + sequence_order`
/// where `sequence_order` is 0-indexed. Before first save, shot IDs are plain
/// sequential integers (0, 1, 2, …).
///
/// Returns a 1-indexed position suitable for user display (e.g., "13:5").
///
/// Total for every value the type admits: `Command::description()` calls this
/// on every command path BEFORE the id is validated against the line, so an
/// out-of-range id (e.g. `ShotId::MAX`) must saturate rather than overflow
/// (#413 finding 24).
pub fn shot_display_position(shot_id: ShotId, line_id: LineId) -> u32 {
    let base = line_id.saturating_mul(FEATURE_ID_STRIDE);
    if shot_id >= base {
        (shot_id - base).saturating_add(1)
    } else {
        shot_id.saturating_add(1)
    }
}

/// Convert slope length + vertical delta into horizontal length.
/// Returns 0.0 if the inputs are invalid (e.g., slope < |dz|).
pub(crate) fn horizontal_length_from_slope(length: f64, dz: f64) -> f64 {
    let len_sq = length * length - dz * dz;
    if len_sq <= 0.0 {
        0.0
    } else {
        len_sq.sqrt()
    }
}

/// Circular mean of a shot's two **forward** azimuth readings, normalised to
/// `[0, 360)`.
///
/// `azimuth_in`/`azimuth_out` are not a foresight/backsight pair - they are two
/// readings of the same forward line - so geometry uses their mean, not either
/// one alone.
///
/// The mean of two readings exactly 180 degrees apart is undefined (both
/// perpendiculars are equidistant); the `+540, wrap, -180` normalisation
/// resolves that tie to `azimuth_in - 90`. Reversing a line has to cope with
/// that arbitrary choice - see [`crate::models::reverse_shots`].
///
/// Wraps with `rem_euclid`, not `%`. Rust's `%` is *truncated* - it keeps the
/// sign of the dividend - so for a reading far outside `[0, 360)` it leaves
/// `diff` outside `(-180, 180]` and returns the ANTIPODE of the true circular
/// mean. That is not academic: `validation.rs` only warns on an out-of-range
/// azimuth, and the DMP parser stores headings verbatim where the Compass and
/// TML importers normalise theirs. It also broke the reverse, whose helpers all
/// use `rem_euclid`: the two disagreed, so the degenerate-pair guard missed and
/// the bearing survived un-turned while the rest of the line flipped, moving
/// the cave by its own span. In-range readings are NOT bit-for-bit what `%`
/// returned: the old wrap was `(x + 360.0) % 360.0` and its addend rounds, where
/// `x.rem_euclid(360.0)` returns an in-range `x` untouched. About half of the
/// in-range pairs therefore differ, by at most half an ulp of 720 (~5.7e-14
/// degrees, ~1e-15 radians - sub-picometre over a kilometre of passage).
///
/// Both wraps go through [`normalize_azimuth`], not a bare `rem_euclid`, because
/// `rem_euclid` alone can return exactly `360.0` - and this function is the
/// ordinary way to get there. Every fore/back pair whose circular mean is
/// exactly north does it, 360 of them on a plain 0.1-degree grid starting at
/// `(0.1, 359.9)`. The `horizontal <= 1e-9` fallbacks in `closure.rs` and
/// `command_move.rs` then write that value verbatim onto both legs, where
/// `>= 360.0` is a non-ignorable `INVALID_AZIMUTH`.
pub fn average_azimuth_of(azimuth_in: f64, azimuth_out: f64) -> f64 {
    // Handle wraparound at 360 degrees
    let diff = normalize_azimuth(azimuth_out - azimuth_in + 540.0) - 180.0;
    normalize_azimuth(azimuth_in + diff / 2.0)
}

/// Compute pitch (degrees) from horizontal + vertical components.
/// Positive pitch corresponds to increasing depth (downwards).
pub fn pitch_from_components(horizontal: f64, dz: f64) -> f64 {
    dz.atan2(horizontal).to_degrees()
}

/// What a shot's own slope length and depth change imply the inclination
/// should have been - `dz = depth_out - depth_in`, `horizontal =
/// sqrt(length^2 - dz^2)`, then pitch from the two components.
///
/// The single source of truth for this computation. Shared by
/// `core::validation`'s `INCLINATION_DEPTH_MISMATCH` cross-check and the TML
/// importer's zero-means-absent pitch inference (#482): both need "what
/// would the geometry have said the inclination was", and if they computed
/// it separately they could silently drift apart.
pub fn implied_pitch_deg(length: f64, depth_in: f64, depth_out: f64) -> f64 {
    let dz = depth_out - depth_in;
    let horizontal = horizontal_length_from_slope(length, dz);
    pitch_from_components(horizontal, dz)
}

/// What a shot represents on the line (#27).
///
/// Splits the old single `is_reference` boolean into the two things surveyors
/// actually mean by a "virtual" shot:
///
/// * [`Jump`](ShotKind::Jump) - a **swum** shortcut between two mainlines. It
///   counts toward gas and penetration, at a raised RMV (`jump_rmv_multiplier`),
///   because tying in a jump is extra task-loading.
/// * [`Reference`](ShotKind::Reference) - a **non-swimmable** survey tie (a tape
///   measurement or backsight used to build the tree). It must not count toward
///   gas, penetration or route swim distance: treating one as swimmable is the
///   false-shortcut risk #77 closed with.
///
/// Both still export to Ariane TML as `VIRTUAL` (there is only one virtual
/// category there), so the Jump/Reference axis is local-only: it does not
/// survive an export and re-import through `.tml`.
///
/// The initial kind is chosen once at import by the chain classifier
/// (`models::project::classify`); it is never recomputed afterwards.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum ShotKind {
    /// A permanent, surveyed leg of the cave line.
    #[default]
    Normal,
    /// A swum shortcut between two mainlines. Counts as swum.
    Jump,
    /// A non-swimmable survey tie. Does not count as swum.
    Reference,
}

/// A shot whose stored `kind` discriminant this build does not recognise (#27).
///
/// Surfaced from the load so the user can decide what each one should be, rather
/// than living with the fail-closed `Reference` substitution silently. Carries
/// the raw value so a genuinely newer file is distinguishable from corruption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnrecognisedShotKind {
    pub line_id: LineId,
    /// In-memory shot id, i.e. what `set_shot_kind` expects.
    pub shot_id: ShotId,
    /// 1-based position in the line, for display.
    pub shot_number: u32,
    pub line_name: Option<String>,
    /// The value actually stored, rendered as text. A string rather than an
    /// integer because the column is not STRICT: a corrupt or hand-edited row
    /// can hold TEXT, REAL or a BLOB, and the case most worth showing the user
    /// is exactly the one an integer read would fail on.
    pub raw_value: String,
}

impl ShotKind {
    /// Compact discriminant for SQLite storage and the binary IPC flags byte.
    /// Stable on the wire: do not renumber.
    pub fn as_u8(self) -> u8 {
        match self {
            ShotKind::Normal => 0,
            ShotKind::Jump => 1,
            ShotKind::Reference => 2,
        }
    }

    /// Inverse of [`as_u8`](Self::as_u8), for a value already known to be valid.
    /// Unknown discriminants fall back to [`ShotKind::Reference`] - a corrupt or
    /// future byte should not make a project unopenable, but it must not invent a
    /// swimmable leg either.
    ///
    /// `Reference` is the fail-closed choice, and the direction matters: `Normal`
    /// is swimmable, so an unrecognised byte would silently enter gas,
    /// penetration and the ruler at normal cost - the false shortcut #27 exists
    /// to remove, and the overstating-reach direction §6 forbids. A `Reference`
    /// can only ever shrink the route. It is a fallback, not a verdict: the load
    /// path uses [`try_from_u8`](Self::try_from_u8) to collect the affected shots
    /// and asks the user which kind each should be.
    pub fn from_u8(value: u8) -> Self {
        Self::try_from_u8(value).unwrap_or(ShotKind::Reference)
    }

    /// Strict inverse of [`as_u8`](Self::as_u8): `None` for a discriminant this
    /// build does not know. Callers that can report a problem should use this and
    /// surface the raw value rather than letting [`from_u8`](Self::from_u8)
    /// quietly substitute a kind.
    pub fn try_from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ShotKind::Normal),
            1 => Some(ShotKind::Jump),
            2 => Some(ShotKind::Reference),
            _ => None,
        }
    }

    /// True for the two virtual kinds. Most call sites only care about this,
    /// not which virtual kind it is.
    pub fn is_virtual(self) -> bool {
        self != ShotKind::Normal
    }

    /// Whether a diver swims this leg, and so whether it counts toward gas,
    /// penetration and route distance. A [`Jump`](ShotKind::Jump) does (at a
    /// raised RMV); a [`Reference`](ShotKind::Reference) does not (#27 q7).
    pub fn is_swum(self) -> bool {
        self != ShotKind::Reference
    }

    /// User-facing label for undo history and command descriptions.
    pub fn label(self) -> &'static str {
        match self {
            ShotKind::Normal => "Permanent",
            ShotKind::Jump => "Jump",
            ShotKind::Reference => "Reference",
        }
    }
}

/// A single survey measurement between two stations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    /// Internal immutable ID, regenerated on load from sequence order.
    /// Use [`Shot::uuid`] instead when you need an identity that survives
    /// save/load: station cross-references anchor to it
    /// (`StationKey::AfterShot`), and the Ariane export emits it as `<UUID>`.
    pub id: ShotId,
    /// Persistent v4 UUID minted at shot creation, stable across saves
    /// and reloads.  Emitted into the TML on export so Ariane 26+ can
    /// round-trip shot identity without depending on the lossy
    /// `(line_id, shot_index)` key (which drifts as soon as the user
    /// inserts or deletes a shot in the same line).  `None` only for
    /// shots loaded from .stalx files written before schema v6; those
    /// get a fresh UUID minted lazily on the next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// First forward azimuth reading for the shot (degrees, 0-360)
    pub azimuth_in: f64,
    /// Second forward azimuth reading for the shot (degrees, 0-360).
    /// This is not a backsight/reciprocal field.
    pub azimuth_out: f64,
    /// Length of the shot in meters
    pub length: f64,
    /// Depth at the start of the shot (meters, positive = below surface)
    pub depth_in: f64,
    /// Depth at the end of the shot (meters, positive = below surface)
    pub depth_out: f64,
    /// Pitch reading when entering (degrees, -90 to 90)
    pub pitch_in: Option<f64>,
    /// Pitch reading when exiting (degrees, -90 to 90)
    pub pitch_out: Option<f64>,
    /// Left passage dimension (meters)
    pub left: Option<f64>,
    /// Right passage dimension (meters)
    pub right: Option<f64>,
    /// Up passage dimension (meters)
    pub up: Option<f64>,
    /// Down passage dimension (meters)
    pub down: Option<f64>,
    /// Optional comment
    pub comment: Option<String>,
    /// Whether this is a permanent leg, a swum jump, or a survey reference (#27).
    /// Reads legacy `is_reference` / `is_virtual` booleans (see [`de_shot_kind`]).
    #[serde(
        default,
        alias = "is_reference",
        alias = "is_virtual",
        deserialize_with = "de_shot_kind"
    )]
    pub kind: ShotKind,
    /// Physical marker index on cave line
    pub marker_index: Option<u32>,
    /// Water temperature in celsius
    pub temperature: Option<f64>,
    /// Who surveyed and explored this shot, and when. Read it through
    /// [`Shot::credit`] and write it through [`Shot::set_credit`].
    ///
    /// `None` and an empty credit mean the same thing, and
    /// [`credit_absent_or_empty`] keeps BOTH off the wire: `get_project_state`
    /// serializes every shot, so an uncredited project pays nothing for the
    /// field. `#[ts(optional)]` is explicit because ts-rs only infers it from
    /// the literal `Option::is_none` predicate.
    ///
    /// Behind an [`Arc`] so shots that share one credit share one allocation:
    /// a line's legacy surveyor/date becomes the credit of every shot that
    /// has no row of its own (`persistence::sqlite::shot_credits`), and a
    /// copy per shot let a file with a long legacy list and many shots blow
    /// up the memory of every open. The wire shape is the credit itself.
    #[serde(
        default,
        skip_serializing_if = "shared_credit_absent_or_empty",
        deserialize_with = "de_storable_credit"
    )]
    pub credit: Option<Arc<SurveyCredit>>,
}

/// A credit in the form a shot stores it ([`SurveyCredit::into_storable`])
/// behind one allocation, for many shots to share ([`Shot::set_shared_credit`]):
/// a line's legacy template, the fallback of every shot without a row. It
/// is normalized once when made, so a long list over many shots is checked
/// once, not once per shot.
#[derive(Debug, Clone)]
pub struct SharedCredit(Arc<SurveyCredit>);

impl SharedCredit {
    /// `credit` made storable, behind one allocation.
    pub fn new(credit: SurveyCredit) -> Self {
        Self(Arc::new(credit.into_storable()))
    }

    /// The shared allocation itself.
    pub fn arc(&self) -> &Arc<SurveyCredit> {
        &self.0
    }
}

impl std::ops::Deref for SharedCredit {
    type Target = SurveyCredit;

    fn deref(&self) -> &SurveyCredit {
        &self.0
    }
}

/// How a shot stores a credit that may be shared: the same allocation when it
/// already is storable (shots given one credit keep sharing it), else a
/// normalized copy of its own ([`SurveyCredit::into_storable`]); `None` for
/// an empty one.
fn storable_shared(credit: Arc<SurveyCredit>) -> Option<Arc<SurveyCredit>> {
    if credit.is_storable() {
        return (!credit.is_empty()).then_some(credit);
    }
    stored_credit(SurveyCredit::clone(&credit).into_storable()).map(Arc::new)
}

/// `deserialize_with` for [`Shot::credit`]: a shot read back from a snapshot
/// or an undo payload stores its credit as [`Shot::set_credit`] would.
fn de_storable_credit<'de, D>(de: D) -> Result<Option<Arc<SurveyCredit>>, D::Error>
where
    D: Deserializer<'de>,
{
    let credit = Option::<SurveyCredit>::deserialize(de)?;
    Ok(credit
        .map(SurveyCredit::into_storable)
        .and_then(stored_credit)
        .map(Arc::new))
}

/// `skip_serializing_if` for [`ShotData::credit`]: absent and empty are the
/// same fact, so both leave the key off the wire.
#[expect(
    clippy::ref_option,
    reason = "serde calls skip_serializing_if with &FieldType; Option<&SurveyCredit> does not typecheck there"
)]
fn credit_absent_or_empty(credit: &Option<Arc<SurveyCredit>>) -> bool {
    credit.as_deref().is_none_or(SurveyCredit::is_empty)
}

/// [`credit_absent_or_empty`] for [`Shot::credit`].
#[expect(
    clippy::ref_option,
    reason = "serde calls skip_serializing_if with &FieldType; Option<&Arc<_>> does not typecheck there"
)]
fn shared_credit_absent_or_empty(credit: &Option<Arc<SurveyCredit>>) -> bool {
    credit.as_deref().is_none_or(SurveyCredit::is_empty)
}

/// `Some` for a credit worth storing, `None` for an empty one - the one
/// spelling of "no credit" every writer stores.
fn stored_credit(credit: SurveyCredit) -> Option<SurveyCredit> {
    (!credit.is_empty()).then_some(credit)
}

/// Data for creating or updating a shot
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShotData {
    /// Persistent identity for the shot this data will become, or `None` to
    /// mint a fresh one. Set by the Ariane importer from the file's `<UUID>`
    /// (#423) and by [`Shot::to_shot_data`], so identity survives every
    /// transform that rebuilds shots from their data - a line split, a
    /// reverse, a project merge, an undo/redo round trip.
    ///
    /// **Any future clone/duplicate-shot path MUST clear this field.** The
    /// uniqueness repair ([`crate::models::Project::ensure_unique_shot_uuids`],
    /// and the same rule in [`crate::models::Line::new`]) keeps the FIRST
    /// occurrence, so a copy that kept its source's identity would get the
    /// original re-minted instead of the copy - silently moving every station
    /// cross-reference anchored on it.
    #[serde(default)]
    pub uuid: Option<String>,
    pub azimuth_in: f64,
    pub azimuth_out: f64,
    pub length: f64,
    pub depth_in: f64,
    pub depth_out: f64,
    #[serde(default)]
    pub pitch_in: Option<f64>,
    #[serde(default)]
    pub pitch_out: Option<f64>,
    #[serde(default)]
    pub left: Option<f64>,
    #[serde(default)]
    pub right: Option<f64>,
    #[serde(default)]
    pub up: Option<f64>,
    #[serde(default)]
    pub down: Option<f64>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(
        default,
        alias = "is_reference",
        alias = "is_virtual",
        deserialize_with = "de_shot_kind"
    )]
    pub kind: ShotKind,
    #[serde(default)]
    pub marker_index: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    /// The shot's survey credit (see [`Shot::credit`]). `None` and empty are the
    /// same; a create with no credit inherits its neighbour's
    /// (`AppState::create_shot`).
    ///
    /// Behind an [`Arc`] like [`Shot::credit`], so an import that gives one
    /// credit to a whole survey (a Compass or Walls team line) hands every
    /// shot the same allocation instead of a copy each, and the shots keep
    /// sharing it ([`Shot::with_uuid`]).
    #[serde(default, skip_serializing_if = "credit_absent_or_empty")]
    pub credit: Option<Arc<SurveyCredit>>,
}

/// Accepts either the modern [`ShotKind`] string or a legacy `is_reference` /
/// `is_virtual` boolean, so projects and undo entries written before #27 still
/// load. A legacy `true` becomes [`ShotKind::Jump`] (issue #27 q2: most
/// user-toggled reference shots are swum shortcuts in practice); the chain
/// classifier then refines that once, at the post-migration pass.
pub fn de_shot_kind<'de, D>(de: D) -> Result<ShotKind, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Kind(ShotKind),
        Legacy(bool),
    }

    Ok(match Wire::deserialize(de)? {
        Wire::Kind(kind) => kind,
        Wire::Legacy(true) => ShotKind::Jump,
        Wire::Legacy(false) => ShotKind::Normal,
    })
}

/// [`de_shot_kind`] for the optional patch field on [`ShotUpdates`]. Absent stays
/// `None` (no change); a present legacy boolean maps the same way.
fn de_shot_kind_opt<'de, D>(de: D) -> Result<Option<ShotKind>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Kind(ShotKind),
        Legacy(bool),
    }

    Ok(Option::<Wire>::deserialize(de)?.map(|w| match w {
        Wire::Kind(kind) => kind,
        Wire::Legacy(true) => ShotKind::Jump,
        Wire::Legacy(false) => ShotKind::Normal,
    }))
}

/// Deserialize an `Option<Option<T>>` field so JSON `null` means "clear":
/// absent → `None` (no update), `null` → `Some(None)`, value → `Some(Some(v))`.
/// With plain `#[serde(default)]`, `null` folds into the outer `None` and a
/// clear sent by the frontend becomes a silent no-op.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// Partial updates to a shot
///
/// Nullable fields use `double_option` + `skip_serializing_if` so the JSON
/// round-trip is lossless: absent ↔ outer `None`, `null` ↔ `Some(None)`.
/// Undo relies on this - previous values are stored as serialized `ShotUpdates`
/// (`values_json`), so an un-set field must serialize as absent, not `null`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShotUpdates {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_in: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_out: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_in: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_out: Option<f64>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub pitch_in: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub pitch_out: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub left: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub right: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub up: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub down: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub comment: Option<Option<String>>,
    #[serde(
        default,
        alias = "is_reference",
        alias = "is_virtual",
        deserialize_with = "de_shot_kind_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind: Option<ShotKind>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub marker_index: Option<Option<u32>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub temperature: Option<Option<f64>>,
}

impl ShotUpdates {
    /// False if this update would write a non-finite (NaN/±inf) number into any
    /// numeric field. Used to reject corrupt geometry at the command boundary
    /// before it poisons coordinate computation.
    pub fn all_finite(&self) -> bool {
        let direct = [
            self.azimuth_in,
            self.azimuth_out,
            self.length,
            self.depth_in,
            self.depth_out,
        ];
        if direct.into_iter().flatten().any(|v| !v.is_finite()) {
            return false;
        }
        // Option<Option<f64>>: outer Some = "set this field", inner Some = value.
        let nested = [
            self.pitch_in,
            self.pitch_out,
            self.left,
            self.right,
            self.up,
            self.down,
            self.temperature,
        ];
        !nested
            .into_iter()
            .any(|o| matches!(o, Some(Some(v)) if !v.is_finite()))
    }

    /// Returns true if this update touches any geometry fields
    /// (azimuth, length, depth, pitch).
    pub fn touches_geometry(&self) -> bool {
        self.azimuth_in.is_some()
            || self.azimuth_out.is_some()
            || self.length.is_some()
            || self.depth_in.is_some()
            || self.depth_out.is_some()
            || self.pitch_in.is_some()
            || self.pitch_out.is_some()
    }
}

impl Shot {
    /// False if any numeric field is non-finite (NaN/±inf).
    pub fn all_finite(&self) -> bool {
        let direct = [
            self.azimuth_in,
            self.azimuth_out,
            self.length,
            self.depth_in,
            self.depth_out,
        ];
        if direct.iter().any(|v| !v.is_finite()) {
            return false;
        }
        let opts = [
            self.pitch_in,
            self.pitch_out,
            self.left,
            self.right,
            self.up,
            self.down,
            self.temperature,
        ];
        !opts.into_iter().flatten().any(|v| !v.is_finite())
    }

    /// Replace any non-finite (NaN/±inf) numeric field with a safe finite value:
    /// the required core fields become `0.0`, optional fields become `None`.
    /// Returns true if anything was changed. Used at the load boundary to
    /// neutralize geometry corrupt-on-disk before it can poison coordinate
    /// computation (a single NaN propagates through the whole cascade); the
    /// degenerate-but-finite result is localized and visible.
    pub fn sanitize_non_finite(&mut self) -> bool {
        if self.all_finite() {
            return false;
        }
        for f in [
            &mut self.azimuth_in,
            &mut self.azimuth_out,
            &mut self.length,
            &mut self.depth_in,
            &mut self.depth_out,
        ] {
            if !f.is_finite() {
                *f = 0.0;
            }
        }
        for o in [
            &mut self.pitch_in,
            &mut self.pitch_out,
            &mut self.left,
            &mut self.right,
            &mut self.up,
            &mut self.down,
            &mut self.temperature,
        ] {
            if matches!(o, Some(v) if !v.is_finite()) {
                *o = None;
            }
        }
        true
    }

    /// Create a new shot from shot data.  Identity comes from
    /// [`ShotData::uuid`] when the data carries one (an Ariane import, a split,
    /// a reverse); otherwise a fresh v4 is minted.  See [`Shot::with_uuid`] for
    /// the full precedence rule.
    pub fn new(id: ShotId, data: ShotData) -> Self {
        Self::with_uuid(id, data, Some(Self::uuid_or_mint(None)))
    }

    /// A recorded identity, or a fresh v4 - the fallback half of the precedence
    /// rule documented on [`Shot::with_uuid`].
    ///
    /// Every path that INSERTS a shot goes through this rather than handing
    /// `with_uuid` a bare `None`, so "the caller had nothing recorded" can never
    /// become "this shot has no identity". The distinction is not cosmetic: a
    /// `None` uuid is invisible to `shot_uuid_index` and unnameable by any
    /// `StationKey::AfterShot`, so a station cross-reference cannot be made to
    /// the shot at all until some later save mints one.
    pub fn uuid_or_mint(recorded: Option<String>) -> String {
        recorded.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }

    /// Construct a shot, resolving its persistent identity.
    ///
    /// **This is the ONE place shot identity is decided**, and the rule is
    /// `data.uuid.or(explicit)`: the data's own id wins, because it travels
    /// with the record through every transform that rebuilds a shot from its
    /// data. `explicit` is the fallback, and exists only for callers holding an
    /// identity the payload cannot carry:
    ///
    /// * undo/redo entries persisted before #423, whose `ShotData` has no
    ///   `uuid` but which stored one alongside (`core/undo.rs` `ShotInfo`,
    ///   replayed at `core/state/undo_restore.rs`);
    /// * `Shot::new`'s fresh mint, used when the data carries no identity.
    ///
    /// `None` from both leaves the shot with NO identity, which is a real state
    /// (a pre-v6 `.stalx` row, minted on the next save) but a poor one to create
    /// on purpose: until something mints it, the shot is absent from
    /// `shot_uuid_index` and no `AfterShot` anchor can name it. Any INSERT path
    /// must therefore pass [`Shot::uuid_or_mint`] rather than a bare `None`.
    ///
    /// Because every `Shot`-from-`ShotData` construction funnels through here,
    /// it is not possible to build a shot that silently drops the identity its
    /// data was carrying.
    pub fn with_uuid(id: ShotId, data: ShotData, uuid: Option<String>) -> Self {
        Self {
            id,
            uuid: data.uuid.or(uuid),
            azimuth_in: data.azimuth_in,
            azimuth_out: data.azimuth_out,
            length: data.length,
            depth_in: data.depth_in,
            depth_out: data.depth_out,
            pitch_in: data.pitch_in,
            pitch_out: data.pitch_out,
            left: data.left,
            right: data.right,
            up: data.up,
            down: data.down,
            comment: data.comment,
            kind: data.kind,
            marker_index: data.marker_index,
            temperature: data.temperature,
            credit: data.credit.and_then(storable_shared),
        }
    }

    /// This shot's survey credit; the empty credit when it has none.
    pub fn credit(&self) -> &SurveyCredit {
        self.credit.as_deref().unwrap_or(&EMPTY_CREDIT)
    }

    /// Replace this shot's survey credit (an empty one is stored as `None`).
    ///
    /// Every way a credit reaches a shot goes through here or
    /// [`Self::set_shared_credit`] (an import's `ShotData`, an edit, an undo
    /// or redo replayed from the file, a deserialized snapshot), and each
    /// stores it [`SurveyCredit::into_storable`]: no control character, no
    /// name past the length cap, no role past [`crate::models::MAX_PEOPLE_PER_ROLE`]
    /// people.
    pub fn set_credit(&mut self, credit: SurveyCredit) {
        self.credit = stored_credit(credit.into_storable()).map(Arc::new);
    }

    /// Give this shot a credit SHARED with other shots: one allocation for all
    /// of them (an empty one is stored as `None`). The credit was made
    /// storable once, when the [`SharedCredit`] was built, not once per shot.
    pub fn set_shared_credit(&mut self, credit: &SharedCredit) {
        self.credit = (!credit.is_empty()).then(|| Arc::clone(&credit.0));
    }

    /// Apply partial updates to this shot
    pub fn apply_updates(&mut self, updates: &ShotUpdates) {
        if let Some(v) = updates.azimuth_in {
            self.azimuth_in = v;
        }
        if let Some(v) = updates.azimuth_out {
            self.azimuth_out = v;
        }
        if let Some(v) = updates.length {
            self.length = v;
        }
        if let Some(v) = updates.depth_in {
            self.depth_in = v;
        }
        if let Some(v) = updates.depth_out {
            self.depth_out = v;
        }
        if let Some(v) = &updates.pitch_in {
            self.pitch_in = *v;
        }
        if let Some(v) = &updates.pitch_out {
            self.pitch_out = *v;
        }
        if let Some(v) = &updates.left {
            self.left = *v;
        }
        if let Some(v) = &updates.right {
            self.right = *v;
        }
        if let Some(v) = &updates.up {
            self.up = *v;
        }
        if let Some(v) = &updates.down {
            self.down = *v;
        }
        if let Some(v) = &updates.comment {
            // Normalize empty string → None (clear comment). JSON `null` now
            // arrives as Some(None) (see `double_option`), but older callers
            // (EditShotDialog) send "" to signal "clear" - both must work.
            self.comment = match v {
                Some(s) if s.is_empty() => None,
                _ => v.clone(),
            };
        }
        if let Some(v) = updates.kind {
            self.kind = v;
        }
        if let Some(v) = &updates.marker_index {
            self.marker_index = *v;
        }
        if let Some(v) = &updates.temperature {
            self.temperature = *v;
        }
    }

    /// The circular mean of the two readings **as measured** - the number the
    /// source file carried, with no declination applied (#438).
    ///
    /// Named the long way on purpose. This is not the bearing geometry walks:
    /// that is [`Self::average_azimuth_true`], which adds the line's
    /// correction. Use this one only where the MEASUREMENT is what is wanted -
    /// display of the raw reading, validation of the reading itself, an export
    /// that has declared the file magnetic. A bare call in a geometry path is a
    /// bug, which is what the name is for.
    pub fn average_azimuth_measured(&self) -> f64 {
        average_azimuth_of(self.azimuth_in, self.azimuth_out)
    }

    /// The true-north bearing this shot walks, in degrees: the measured mean
    /// plus the line's declination correction (#438).
    ///
    /// `reference` is the line's resolved [`AzimuthReference`] - get it from
    /// [`Project::azimuth_reference_for`](crate::models::models::Project::azimuth_reference_for)
    /// once per line, not once per shot.
    pub fn average_azimuth_true(&self, reference: AzimuthReference) -> f64 {
        reference.to_true(self.average_azimuth_measured())
    }

    /// [`Self::average_azimuth_true`] in radians - the form every geometry walk
    /// wants, so that no call site has to remember both the correction and the
    /// conversion.
    pub fn average_azimuth_true_rad(&self, reference: AzimuthReference) -> f64 {
        self.average_azimuth_true(reference).to_radians()
    }

    /// Horizontal projection of the shot length (meters).
    /// Shot.length is slope distance, so horizontal is derived from depth change.
    pub fn horizontal_length(&self) -> f64 {
        horizontal_length_from_slope(self.length, self.depth_out - self.depth_in)
    }

    /// Get depth change for this shot
    pub fn depth_change(&self) -> f64 {
        self.depth_out - self.depth_in
    }

    /// Convert to ShotData (for undo snapshots)
    pub fn to_shot_data(&self) -> ShotData {
        ShotData {
            // Identity travels with the record. This is what lets a line
            // split, a reverse or a merge rebuild shots from their data without
            // re-minting - see [`ShotData::uuid`] for the one path (clone /
            // duplicate) that must clear it instead.
            uuid: self.uuid.clone(),
            azimuth_in: self.azimuth_in,
            azimuth_out: self.azimuth_out,
            length: self.length,
            depth_in: self.depth_in,
            depth_out: self.depth_out,
            pitch_in: self.pitch_in,
            pitch_out: self.pitch_out,
            left: self.left,
            right: self.right,
            up: self.up,
            down: self.down,
            comment: self.comment.clone(),
            kind: self.kind,
            marker_index: self.marker_index,
            temperature: self.temperature,
            credit: self.credit.clone().filter(|c| !c.is_empty()),
        }
    }
}

impl ShotData {
    /// The credit this data carries; the empty credit when it has none.
    pub fn credit(&self) -> &SurveyCredit {
        self.credit.as_deref().unwrap_or(&EMPTY_CREDIT)
    }

    /// False if any numeric field is non-finite (NaN/±inf). Used to reject
    /// corrupt geometry at ingest before it poisons coordinate computation
    /// (NaN passes the existing zero/negative-length checks).
    pub fn all_finite(&self) -> bool {
        let direct = [
            self.azimuth_in,
            self.azimuth_out,
            self.length,
            self.depth_in,
            self.depth_out,
        ];
        if direct.iter().any(|v| !v.is_finite()) {
            return false;
        }
        let opts = [
            self.pitch_in,
            self.pitch_out,
            self.left,
            self.right,
            self.up,
            self.down,
            self.temperature,
        ];
        !opts.into_iter().flatten().any(|v| !v.is_finite())
    }

    /// Horizontal projection of the shot length (meters).
    /// ShotData.length is slope distance, so horizontal is derived from depth change.
    pub fn horizontal_length(&self) -> f64 {
        horizontal_length_from_slope(self.length, self.depth_out - self.depth_in)
    }

    /// Circular mean of the two forward azimuth readings, AS MEASURED. Mirrors
    /// [`Shot::average_azimuth_measured`] for the pre-creation form used by the
    /// importers; see that method for why the raw form is named the long way.
    pub fn average_azimuth_measured(&self) -> f64 {
        average_azimuth_of(self.azimuth_in, self.azimuth_out)
    }

    /// True-north bearing in degrees. Mirrors [`Shot::average_azimuth_true`].
    pub fn average_azimuth_true(&self, reference: AzimuthReference) -> f64 {
        reference.to_true(self.average_azimuth_measured())
    }

    /// True-north bearing in radians. Mirrors [`Shot::average_azimuth_true_rad`].
    pub fn average_azimuth_true_rad(&self, reference: AzimuthReference) -> f64 {
        self.average_azimuth_true(reference).to_radians()
    }
}

impl From<Shot> for ShotData {
    fn from(shot: Shot) -> Self {
        ShotData {
            uuid: shot.uuid,
            azimuth_in: shot.azimuth_in,
            azimuth_out: shot.azimuth_out,
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            pitch_in: shot.pitch_in,
            pitch_out: shot.pitch_out,
            left: shot.left,
            right: shot.right,
            up: shot.up,
            down: shot.down,
            comment: shot.comment,
            kind: shot.kind,
            marker_index: shot.marker_index,
            temperature: shot.temperature,
            credit: shot.credit.filter(|c| !c.is_empty()),
        }
    }
}
