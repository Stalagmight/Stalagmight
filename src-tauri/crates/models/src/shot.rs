use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

/// Unique identifier for a shot within a section
pub type ShotId = u32;

use super::{normalize_azimuth, AzimuthReference, SectionId, FEATURE_ID_STRIDE};

/// Convert a raw ShotId to a 1-indexed display position.
///
/// After database reload, shot IDs have the form `section_id * FEATURE_ID_STRIDE + sequence_order`
/// where `sequence_order` is 0-indexed. Before first save, they come from the
/// section's counter, which `Section::new` starts at `section_id * FEATURE_ID_STRIDE`
/// and which counts up as shots are created; only section 0's are plain 0, 1, 2.
///
/// Returns a 1-indexed position suitable for user display (e.g., "13:5").
///
/// Total for every value the type admits: `Command::description()` calls this
/// on every command path BEFORE the id is validated against the section, so an
/// out-of-range id (e.g. `ShotId::MAX`) must saturate rather than overflow
/// (#413 finding 24).
pub fn shot_display_position(shot_id: ShotId, section_id: SectionId) -> u32 {
    let base = section_id.saturating_mul(FEATURE_ID_STRIDE);
    if shot_id >= base {
        (shot_id - base).saturating_add(1)
    } else {
        shot_id.saturating_add(1)
    }
}

/// Convert slope length + vertical delta into horizontal length.
/// Returns 0.0 if the inputs are invalid (e.g., slope < |dz|).
///
/// Finite inputs give a finite length (#576 NF-TML-1): above ~1.34e154 m a
/// square overflows, so an absurd tape used to come out as `inf` (or NaN once
/// both squares did) and poison every station after it. Only that case takes
/// the scaled form; every ordinary input keeps its bit-exact result.
pub(crate) fn horizontal_length_from_slope(length: f64, dz: f64) -> f64 {
    let len_sq = length * length - dz * dz;
    if len_sq.is_finite() || !(length.is_finite() && dz.is_finite()) {
        return if len_sq <= 0.0 { 0.0 } else { len_sq.sqrt() };
    }
    // A square overflowed: the same length with both scaled into [0, 1].
    let scale = length.abs().max(dz.abs());
    let (l, d) = (length.abs() / scale, dz.abs() / scale);
    let scaled_sq = (l - d) * (l + d);
    if scaled_sq <= 0.0 {
        0.0
    } else {
        scale * scaled_sq.sqrt()
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
/// resolves that tie to `azimuth_in - 90`. Reversing a section has to cope with
/// that arbitrary choice - see [`crate::reverse_shots`].
///
/// Wraps with `rem_euclid`, not `%`. Rust's `%` is *truncated* - it keeps the
/// sign of the dividend - so for a reading far outside `[0, 360)` it leaves
/// `diff` outside `(-180, 180]` and returns the ANTIPODE of the true circular
/// mean. That is not academic: `validation.rs` only warns on an out-of-range
/// azimuth, and the DMP parser stores headings verbatim where the Compass and
/// TML importers normalise theirs. It also broke the reverse, whose helpers all
/// use `rem_euclid`: the two disagreed, so the degenerate-pair guard missed and
/// the bearing survived un-turned while the rest of the section flipped, moving
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

/// Convert horizontal + vertical components into slope length.
#[cfg_attr(not(test), expect(dead_code))] // used only by tests; dead in the lib build
pub(crate) fn slope_length_from_components(horizontal: f64, dz: f64) -> f64 {
    (horizontal * horizontal + dz * dz).sqrt()
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, TS)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UnrecognisedShotKind {
    pub section_id: SectionId,
    /// In-memory shot id, i.e. what `set_shot_kind` expects.
    pub shot_id: ShotId,
    /// 1-based position in the section, for display.
    pub shot_number: u32,
    pub section_name: Option<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Shot {
    /// Internal immutable ID, regenerated on load from sequence order.
    /// Use [`Shot::uuid`] instead when you need an identity that survives
    /// save/load: station cross-references anchor to it
    /// (`StationKey::AfterShot`), and the Ariane export emits it as `<UUID>`.
    pub id: ShotId,
    /// Persistent v4 UUID minted at shot creation, stable across saves
    /// and reloads.  Emitted into the TML on export so Ariane 26+ can
    /// round-trip shot identity without depending on the lossy
    /// `(section_id, shot_index)` key (which drifts as soon as the user
    /// inserts or deletes a shot in the same section).  `None` only for
    /// shots loaded from .stalx files written before schema v6; those
    /// get a fresh UUID minted lazily on the next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
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
}

/// Data for creating or updating a shot
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct ShotData {
    /// Persistent identity for the shot this data will become, or `None` to
    /// mint a fresh one. Set by the Ariane importer from the file's `<UUID>`
    /// (#423) and by [`Shot::to_shot_data`], so identity survives every
    /// transform that rebuilds shots from their data - a section split, a
    /// reverse, a project merge, an undo/redo round trip.
    ///
    /// **Any future clone/duplicate-shot path MUST clear this field.** The
    /// uniqueness repair ([`crate::Project::ensure_unique_shot_uuids`],
    /// and the same rule in [`crate::Section::new`]) keeps the FIRST
    /// occurrence, so a copy that kept its source's identity would get the
    /// original re-minted instead of the copy - silently moving every station
    /// cross-reference anchored on it.
    #[serde(default)]
    #[ts(optional)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct ShotUpdates {
    #[serde(default)]
    #[ts(optional)]
    pub azimuth_in: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub azimuth_out: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub length: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub depth_in: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub depth_out: Option<f64>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub pitch_in: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub pitch_out: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub left: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub right: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub up: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub down: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub comment: Option<Option<String>>,
    #[serde(
        default,
        alias = "is_reference",
        alias = "is_virtual",
        deserialize_with = "de_shot_kind_opt",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub kind: Option<ShotKind>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub marker_index: Option<Option<u32>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
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
        recorded.unwrap_or_else(crate::uuid_mint::mint)
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
        }
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
    /// that is [`Self::average_azimuth_true`], which adds the section's
    /// correction. Use this one only where the MEASUREMENT is what is wanted -
    /// display of the raw reading, validation of the reading itself, an export
    /// that has declared the file magnetic. A bare call in a geometry path is a
    /// bug, which is what the name is for.
    pub fn average_azimuth_measured(&self) -> f64 {
        average_azimuth_of(self.azimuth_in, self.azimuth_out)
    }

    /// The true-north bearing this shot walks, in degrees: the measured mean
    /// plus the section's declination correction (#438).
    ///
    /// `reference` is the section's resolved [`AzimuthReference`] - get it from
    /// [`Project::azimuth_reference_for`](crate::models::Project::azimuth_reference_for)
    /// once per section, not once per shot.
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
            // Identity travels with the record. This is what lets a section
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
        }
    }
}

impl ShotData {
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
        }
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn base_shot_data() -> ShotData {
        ShotData {
            uuid: None,
            azimuth_in: 10.0,
            azimuth_out: 20.0,
            length: 30.0,
            depth_in: 5.0,
            depth_out: 7.0,
            pitch_in: Some(1.0),
            pitch_out: Some(2.0),
            left: Some(0.5),
            right: Some(0.6),
            up: Some(0.7),
            down: Some(0.8),
            comment: Some("note".to_string()),
            kind: ShotKind::Normal,
            marker_index: Some(3),
            temperature: Some(21.5),
        }
    }

    #[test]
    fn with_uuid_prefers_the_datas_own_identity_over_the_explicit_fallback() {
        // The precedence rule `data.uuid.or(explicit)` is load-bearing (every
        // transform that rebuilds a shot from its data relies on the data's id
        // winning), yet on most live paths the two inputs are equal - so a
        // flipped `.or()` would survive those tests. Pin it where they differ.
        let data = ShotData {
            uuid: Some("data-owned".to_string()),
            ..base_shot_data()
        };
        let shot = Shot::with_uuid(7, data, Some("explicit-fallback".to_string()));
        assert_eq!(shot.uuid.as_deref(), Some("data-owned"));

        let shot = Shot::with_uuid(7, base_shot_data(), Some("explicit-fallback".to_string()));
        assert_eq!(shot.uuid.as_deref(), Some("explicit-fallback"));
    }

    #[test]
    fn apply_updates_changes_only_provided_fields() {
        let mut shot = Shot::new(1, base_shot_data());
        let updates = ShotUpdates {
            azimuth_in: Some(11.0),
            depth_out: Some(8.0),
            pitch_in: Some(None),
            right: Some(Some(1.2)),
            comment: Some(Some("updated".to_string())),
            kind: Some(ShotKind::Jump),
            marker_index: Some(None),
            ..Default::default()
        };

        shot.apply_updates(&updates);

        assert_eq!(shot.azimuth_in, 11.0);
        assert_eq!(shot.depth_out, 8.0);
        assert!(shot.pitch_in.is_none());
        assert_eq!(shot.right, Some(1.2));
        assert_eq!(shot.comment.as_deref(), Some("updated"));
        assert_eq!(shot.kind, ShotKind::Jump);
        assert!(shot.marker_index.is_none());

        assert_eq!(shot.azimuth_out, 20.0);
        assert_eq!(shot.length, 30.0);
        assert_eq!(shot.depth_in, 5.0);
        assert_eq!(shot.pitch_out, Some(2.0));
        assert_eq!(shot.left, Some(0.5));
        assert_eq!(shot.up, Some(0.7));
        assert_eq!(shot.down, Some(0.8));
        assert_eq!(shot.temperature, Some(21.5));
    }

    #[test]
    fn shot_kind_discriminants_round_trip() {
        for kind in [ShotKind::Normal, ShotKind::Jump, ShotKind::Reference] {
            assert_eq!(ShotKind::from_u8(kind.as_u8()), kind);
        }
        // Stable on the wire: these numbers are persisted in SQLite and in the
        // binary IPC flags byte, so a renumber would silently corrupt projects.
        assert_eq!(ShotKind::Normal.as_u8(), 0);
        assert_eq!(ShotKind::Jump.as_u8(), 1);
        assert_eq!(ShotKind::Reference.as_u8(), 2);
        // An unrecognised discriminant degrades safely, never panics.
        // An unrecognised discriminant fails CLOSED, to Reference: Normal would
        // be swimmable and would silently enter the route graph.
        assert_eq!(ShotKind::from_u8(9), ShotKind::Reference);
        assert_eq!(ShotKind::try_from_u8(9), None);
        assert_eq!(ShotKind::try_from_u8(0), Some(ShotKind::Normal));
        assert_eq!(ShotKind::try_from_u8(1), Some(ShotKind::Jump));
        assert_eq!(ShotKind::try_from_u8(2), Some(ShotKind::Reference));
    }

    #[test]
    fn shot_kind_is_virtual_covers_both_virtual_kinds() {
        assert!(!ShotKind::Normal.is_virtual());
        assert!(ShotKind::Jump.is_virtual());
        assert!(ShotKind::Reference.is_virtual());
    }

    #[test]
    fn legacy_is_reference_boolean_deserializes_to_kind() {
        // Projects and undo entries written before #27 carry a boolean. Per q2 a
        // legacy `true` lands as Jump; the chain classifier refines it later.
        let legacy_true: ShotData =
            serde_json::from_str(r#"{"azimuth_in":1.0,"azimuth_out":2.0,"length":3.0,"depth_in":0.0,"depth_out":0.0,"is_reference":true}"#)
                .unwrap();
        assert_eq!(legacy_true.kind, ShotKind::Jump);

        let legacy_false: ShotData =
            serde_json::from_str(r#"{"azimuth_in":1.0,"azimuth_out":2.0,"length":3.0,"depth_in":0.0,"depth_out":0.0,"is_reference":false}"#)
                .unwrap();
        assert_eq!(legacy_false.kind, ShotKind::Normal);

        // The even older `is_virtual` spelling still works too.
        let ancient: ShotData =
            serde_json::from_str(r#"{"azimuth_in":1.0,"azimuth_out":2.0,"length":3.0,"depth_in":0.0,"depth_out":0.0,"is_virtual":true}"#)
                .unwrap();
        assert_eq!(ancient.kind, ShotKind::Jump);

        // Absent entirely → Normal.
        let absent: ShotData = serde_json::from_str(
            r#"{"azimuth_in":1.0,"azimuth_out":2.0,"length":3.0,"depth_in":0.0,"depth_out":0.0}"#,
        )
        .unwrap();
        assert_eq!(absent.kind, ShotKind::Normal);
    }

    #[test]
    fn modern_kind_string_deserializes() {
        let modern: ShotData =
            serde_json::from_str(r#"{"azimuth_in":1.0,"azimuth_out":2.0,"length":3.0,"depth_in":0.0,"depth_out":0.0,"kind":"Reference"}"#)
                .unwrap();
        assert_eq!(modern.kind, ShotKind::Reference);
    }

    #[test]
    fn shot_updates_legacy_boolean_and_absent_kind() {
        // A legacy undo entry patching the flag.
        let legacy: ShotUpdates = serde_json::from_str(r#"{"is_reference":true}"#).unwrap();
        assert_eq!(legacy.kind, Some(ShotKind::Jump));

        // Absent means "no change", which must stay distinct from "set to Normal".
        let absent: ShotUpdates = serde_json::from_str(r#"{"azimuth_in":1.0}"#).unwrap();
        assert_eq!(absent.kind, None);

        // Round-trips: an unset kind serializes as absent, not null, so undo's
        // stored `values_json` never reads back as "clear to Normal".
        let json = serde_json::to_string(&absent).unwrap();
        assert!(
            !json.contains("kind"),
            "unset kind must not serialize: {json}"
        );
    }

    #[test]
    fn apply_updates_empty_string_clears_comment() {
        let mut shot = Shot::new(1, base_shot_data());
        assert_eq!(shot.comment.as_deref(), Some("note"));

        // Empty string should clear comment (workaround for JSON null → outer None serde issue)
        let updates = ShotUpdates {
            comment: Some(Some(String::new())),
            ..Default::default()
        };
        shot.apply_updates(&updates);
        assert!(shot.comment.is_none(), "empty string should clear comment");
    }

    #[test]
    fn apply_updates_none_comment_leaves_unchanged() {
        let mut shot = Shot::new(1, base_shot_data());
        assert_eq!(shot.comment.as_deref(), Some("note"));

        // None (missing key) should leave comment unchanged
        let updates = ShotUpdates {
            comment: None,
            ..Default::default()
        };
        shot.apply_updates(&updates);
        assert_eq!(shot.comment.as_deref(), Some("note"));
    }

    #[test]
    fn shot_updates_json_null_deserializes_as_clear() {
        let updates: ShotUpdates =
            serde_json::from_value(serde_json::json!({ "left": null })).unwrap();
        assert_eq!(updates.left, Some(None), "null should mean clear");
        assert_eq!(updates.right, None, "absent field should mean no update");

        let updates: ShotUpdates = serde_json::from_value(serde_json::json!({
            "comment": null,
            "marker_index": null,
            "temperature": 4.5,
        }))
        .unwrap();
        assert_eq!(updates.comment, Some(None));
        assert_eq!(updates.marker_index, Some(None));
        assert_eq!(updates.temperature, Some(Some(4.5)));
        assert_eq!(updates.pitch_in, None);
    }

    #[test]
    fn shot_updates_serde_round_trip_preserves_clear_vs_absent() {
        // Undo stores previous values as serialized ShotUpdates (values_json);
        // a clear must survive the round-trip and an un-set field must not
        // come back as a clear.
        let updates = ShotUpdates {
            left: Some(None),
            comment: Some(Some("note".to_string())),
            ..Default::default()
        };
        let json = serde_json::to_string(&updates).unwrap();
        let back: ShotUpdates = serde_json::from_str(&json).unwrap();
        assert_eq!(back.left, Some(None));
        assert_eq!(back.comment, Some(Some("note".to_string())));
        assert_eq!(back.right, None);
        assert_eq!(back.temperature, None);
    }

    #[test]
    fn apply_updates_json_null_clears_nullable_fields() {
        let mut shot = Shot::new(1, base_shot_data());
        let updates: ShotUpdates =
            serde_json::from_value(serde_json::json!({ "left": null, "comment": null })).unwrap();
        shot.apply_updates(&updates);
        assert!(shot.left.is_none(), "null should clear left");
        assert!(shot.comment.is_none(), "null should clear comment");
        assert_eq!(shot.right, Some(0.6), "absent fields stay untouched");
    }

    #[test]
    fn average_azimuth_handles_wraparound() {
        let shot = Shot::new(
            1,
            ShotData {
                azimuth_in: 350.0,
                azimuth_out: 10.0,
                length: 1.0,
                depth_in: 0.0,
                depth_out: 0.0,
                ..Default::default()
            },
        );
        let average = shot.average_azimuth_measured();
        assert!((average - 0.0).abs() < 1e-6);
    }

    #[test]
    fn depth_change_and_conversion_roundtrip() {
        let shot = Shot::new(4, base_shot_data());
        assert!((shot.depth_change() - 2.0).abs() < 1e-6);

        let data = shot.to_shot_data();
        assert_eq!(data.azimuth_in, 10.0);
        assert_eq!(data.comment.as_deref(), Some("note"));

        let from_shot: ShotData = Shot::new(5, base_shot_data()).into();
        assert_eq!(from_shot.length, 30.0);
        assert_eq!(from_shot.marker_index, Some(3));
    }

    #[test]
    fn shot_display_position_pre_reload_ids() {
        // An id below its section's stride base reads as id + 1: section 0's own
        // pre-save ids (0, 1, 2, ...) and an unstrided id from an older file. Other
        // sections' pre-save ids start at section_id * FEATURE_ID_STRIDE (Section::new).
        assert_eq!(shot_display_position(0, 1), 1); // first shot → position 1
        assert_eq!(shot_display_position(4, 1), 5); // fifth shot → position 5
        assert_eq!(shot_display_position(0, 0), 1); // section 0 edge case
    }

    #[test]
    fn shot_display_position_post_reload_ids() {
        // After reload: shot_id = section_id * FEATURE_ID_STRIDE + sequence_order
        assert_eq!(shot_display_position(FEATURE_ID_STRIDE * 13 + 4, 13), 5); // section 13, seq 4 → pos 5
        assert_eq!(shot_display_position(FEATURE_ID_STRIDE, 1), 1); // section 1, seq 0 → pos 1
        assert_eq!(shot_display_position(FEATURE_ID_STRIDE + 9, 1), 10); // section 1, seq 9 → pos 10
        assert_eq!(shot_display_position(FEATURE_ID_STRIDE * 5 + 2, 5), 3); // section 5, seq 2 → pos 3
    }

    // ========================================================================
    // Geometry edge cases
    // ========================================================================

    #[test]
    fn vertical_shot_has_zero_horizontal_length() {
        // Pure vertical shot: dz == length → horizontal = 0
        let shot = Shot::new(
            1,
            ShotData {
                length: 10.0,
                depth_in: 0.0,
                depth_out: 10.0, // dz == length
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                ..Default::default()
            },
        );
        assert_eq!(shot.horizontal_length(), 0.0);
    }

    #[test]
    fn nearly_vertical_shot_small_horizontal_length() {
        // dz almost equals length - should produce small positive horizontal
        let shot = Shot::new(
            1,
            ShotData {
                length: 10.0,
                depth_in: 0.0,
                depth_out: 9.99,
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                ..Default::default()
            },
        );
        let h = shot.horizontal_length();
        assert!(h > 0.0 && h < 0.5, "nearly vertical shot h={h}");
    }

    #[test]
    fn horizontal_length_with_dz_greater_than_length() {
        // Invalid: |dz| > length - should return 0.0 (not NaN/panic)
        let h = horizontal_length_from_slope(5.0, 10.0);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn horizontal_length_zero_length_zero_dz() {
        assert_eq!(horizontal_length_from_slope(0.0, 0.0), 0.0);
    }

    /// #576 NF-TML-1: an absurd but finite tape squared past f64's range and
    /// came out `inf` (NaN once both squares did). It stays finite and right;
    /// an ordinary input keeps its bit-exact result.
    #[test]
    #[expect(
        clippy::print_stdout,
        reason = "the printed lengths are the evidence a run captures"
    )]
    fn horizontal_length_of_an_absurd_tape_stays_finite() {
        for (length, dz, expected) in [
            (1e200, 0.0, 1e200),
            (1e200, 6e199, 8e199),
            (1e200, 1e200, 0.0),
            (1e200, 2e200, 0.0),
            (f64::MAX, 0.0, f64::MAX),
        ] {
            let h = horizontal_length_from_slope(length, dz);
            println!("horizontal_length_from_slope({length:e}, {dz:e}) = {h:e}");
            assert!(
                h.is_finite() && (h - expected).abs() <= expected * 1e-12,
                "({length:e}, {dz:e}) -> {h:e}, expected {expected:e}"
            );
        }
        assert_eq!(
            horizontal_length_from_slope(5.0, 3.0).to_bits(),
            4.0f64.to_bits()
        );
        let ordinary = 12.34f64.mul_add(12.34, -(5.6f64 * 5.6)).sqrt();
        assert_eq!(
            horizontal_length_from_slope(12.34, 5.6).to_bits(),
            (12.34f64 * 12.34 - 5.6 * 5.6).sqrt().to_bits(),
            "unchanged for ordinary input (fused would be {ordinary})"
        );
    }

    #[test]
    fn average_azimuth_same_in_out() {
        let shot = Shot::new(
            1,
            ShotData {
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 1.0,
                ..Default::default()
            },
        );
        assert!((shot.average_azimuth_measured() - 90.0).abs() < 1e-6);
    }

    #[test]
    fn average_azimuth_wraps_around_north() {
        // 350° and 10° should average to 0° (or 360°)
        let shot = Shot::new(
            1,
            ShotData {
                azimuth_in: 350.0,
                azimuth_out: 10.0,
                length: 1.0,
                ..Default::default()
            },
        );
        let avg = shot.average_azimuth_measured();
        assert!(
            avg < 1.0 || avg > 359.0,
            "350-10 wrap should give ~0/360, got {avg}"
        );
    }

    #[test]
    fn average_azimuth_wraps_around_south() {
        // 170° and 190° should average to 180°
        let shot = Shot::new(
            1,
            ShotData {
                azimuth_in: 170.0,
                azimuth_out: 190.0,
                length: 1.0,
                ..Default::default()
            },
        );
        assert!((shot.average_azimuth_measured() - 180.0).abs() < 1e-6);
    }

    #[test]
    fn average_azimuth_opposite_direction() {
        // in=0, out=180 → diff = (180+540)%360-180 = -180 → avg = (0 + -90 + 360)%360 = 270
        let shot = Shot::new(
            1,
            ShotData {
                azimuth_in: 0.0,
                azimuth_out: 180.0,
                length: 1.0,
                ..Default::default()
            },
        );
        assert!((shot.average_azimuth_measured() - 270.0).abs() < 1e-6);
    }

    /// The `rem_euclid` wrap is a CORRECTION, not a reinterpretation of stored data.
    ///
    /// Rust's `%` is truncated, so for a reading pair more than 540 degrees apart in the
    /// negative direction the old `(az_out - az_in + 540.0) % 360.0` left `diff` outside
    /// `(-180, 180]` and the result came back 180 degrees from the true circular mean.
    /// An azimuth still means what it always meant (degrees clockwise from north), so
    /// unlike the v11 -> v12 inclination flip there is no stored VALUE whose meaning
    /// changed and therefore nothing a migration could rewrite without baking the
    /// 180-degree error in permanently.
    ///
    /// The affected data is also a strict subset of data the app already flags: the pair
    /// has to differ by more than 540 degrees, which is impossible while both readings sit
    /// in `[0, 360)`, so at least one of them is out of range and the shot already raises
    /// `INVALID_AZIMUTH` in the warnings panel. (Reachable because the DMP parser stores
    /// headings verbatim and validation only warns; Compass and TML normalise on import.)
    #[test]
    fn average_azimuth_of_far_out_of_range_readings_is_the_true_circular_mean() {
        // 600 degrees is 240; paired with 0 the true mean is 300 (the short way round).
        let truncated = |azimuth_in: f64, azimuth_out: f64| {
            let diff = (azimuth_out - azimuth_in + 540.0) % 360.0 - 180.0;
            (azimuth_in + diff / 2.0 + 360.0) % 360.0
        };
        assert!((average_azimuth_of(600.0, 0.0) - 300.0).abs() < 1e-9);
        assert!(
            (truncated(600.0, 0.0) - 120.0).abs() < 1e-9,
            "precondition: the old truncated wrap returned the antipode"
        );
        // The mean of the SAME bearings written in range is identical, which is what makes
        // normalising stored readings a no-op for geometry rather than a second fix.
        assert!((average_azimuth_of(240.0, 0.0) - average_azimuth_of(600.0, 0.0)).abs() < 1e-9);
        // In-range pairs are untouched by the change beyond float noise.
        assert!((average_azimuth_of(350.0, 10.0) - 0.0).abs() < 1e-9);
    }

    /// The reachable half of the `rem_euclid` escape, and the reason
    /// `average_azimuth_of` wraps through `normalize_azimuth`.
    ///
    /// `(0.1, 359.9)` is an ordinary fore/back pair. Its circular mean is exactly
    /// north, and a bare `rem_euclid` returns `360.0` for it - out of range, and a
    /// non-ignorable `INVALID_AZIMUTH` once the `horizontal <= 1e-9` fallbacks in
    /// `closure.rs` / `command_move.rs` copy it onto both legs. There are 360 such
    /// pairs on a plain 0.1-degree grid, not one crafted ulp.
    #[test]
    fn average_of_ordinary_fore_back_readings_is_never_360() {
        let naive = |azimuth_in: f64, azimuth_out: f64| {
            let diff = (azimuth_out - azimuth_in + 540.0).rem_euclid(360.0) - 180.0;
            (azimuth_in + diff / 2.0).rem_euclid(360.0)
        };
        assert_eq!(
            naive(0.1, 359.9),
            360.0,
            "precondition: the bare rem_euclid wrap returns exactly 360.0 here"
        );

        let mean = average_azimuth_of(0.1, 359.9);
        assert!(
            (0.0..360.0).contains(&mean),
            "average_azimuth_of(0.1, 359.9) = {mean} escaped [0, 360)"
        );
        assert_eq!(mean, 0.0);

        // Sweep the whole 0.1-degree grid: no in-range pair may escape the range.
        // The grid is built by PARSING each decimal, which is what a typed reading
        // or a stored one actually is - `i as f64 * 0.1` accumulates a different
        // rounding and hits only 203 of the 360 cases.
        let grid: Vec<f64> = (0..3600)
            .map(|i| format!("{}.{}", i / 10, i % 10).parse().unwrap())
            .collect();
        let mut naive_escapes = 0_u32;
        for &azimuth_in in &grid {
            for &azimuth_out in &grid {
                let mean = average_azimuth_of(azimuth_in, azimuth_out);
                assert!(
                    (0.0..360.0).contains(&mean),
                    "average_azimuth_of({azimuth_in}, {azimuth_out}) = {mean}"
                );
                if naive(azimuth_in, azimuth_out) >= 360.0 {
                    naive_escapes += 1;
                }
            }
        }
        assert_eq!(
            naive_escapes, 360,
            "precondition: the bare rem_euclid wrap escapes [0, 360) for 360 grid pairs"
        );
    }

    #[test]
    fn slope_length_from_components_pythagorean() {
        let s = slope_length_from_components(3.0, 4.0);
        assert!((s - 5.0).abs() < 1e-6);
    }

    #[test]
    fn pitch_from_components_45_degrees() {
        let p = pitch_from_components(1.0, 1.0);
        assert!((p - 45.0).abs() < 1e-6);
    }

    #[test]
    fn pitch_from_components_pure_vertical() {
        let p = pitch_from_components(0.0, 1.0);
        assert!((p - 90.0).abs() < 1e-6);
    }

    #[test]
    fn pitch_from_components_pure_horizontal() {
        let p = pitch_from_components(1.0, 0.0);
        assert!(p.abs() < 1e-6);
    }
}
