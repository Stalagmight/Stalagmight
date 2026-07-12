use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use super::{SectionId, ShotId};

/// Deserializes `Option<Option<T>>` so that an absent key → `None` (leave
/// unchanged) and an explicit JSON `null` → `Some(None)` (clear the field).
/// Without this, serde folds both absent and `null` into `None`.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// The visual type of a survey marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MarkerType.ts")]
pub enum MarkerType {
    /// Non-Directional Marker — a fixed physical reference point.
    Ndm,
    /// Directional arrow marker — indicates direction of travel along the line.
    Arrow,
}

/// Direction for an Arrow marker (relative to the shot it sits on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MarkerDirection.ts")]
pub enum MarkerDirection {
    /// Arrow points toward the start station of the shot.
    TowardStart,
    /// Arrow points toward the end station of the shot.
    TowardEnd,
}

/// Partial update applied to an existing marker.
///
/// Each field is `Some` only when the caller wants to change that field.
/// `direction` uses a nested `Option`: `Some(Some(d))` sets a direction,
/// `Some(None)` clears it, `None` leaves it unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MarkerUpdates.ts")]
pub struct MarkerUpdates {
    pub position: Option<f32>,
    pub marker_type: Option<MarkerType>,
    pub color: Option<String>,
    pub text: Option<String>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub direction: Option<Option<MarkerDirection>>,
}

/// A survey marker placed at a position along a shot.
///
/// Markers are project-level entities (stored in `Project.markers`) that
/// reference their host shot by `(section_id, shot_id)`. Position is a
/// percentage (0.0 = start station, 100.0 = end station).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/Marker.ts")]
pub struct Marker {
    /// Unique identifier — monotonically assigned, never reused.
    pub id: u64,
    /// Section that owns the host shot.
    pub section_id: SectionId,
    /// ID of the shot this marker sits on.
    pub shot_id: ShotId,
    /// Position along the shot as a percentage (0.0 = start, 100.0 = end).
    pub position: f32,
    /// Visual type of this marker.
    pub marker_type: MarkerType,
    /// Display color. One of the preset names ("white", "orange", "yellow",
    /// "blue") or a hex string like "#a3b4c5".
    pub color: String,
    /// Optional text label shown alongside the marker on the map.
    #[serde(default)]
    pub text: String,
    /// Direction for Arrow markers. Ignored for other types.
    #[serde(default)]
    pub direction: Option<MarkerDirection>,
}
