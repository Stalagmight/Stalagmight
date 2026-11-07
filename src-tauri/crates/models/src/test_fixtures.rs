//! Survey-data fixtures shared by the workspace's Rust tests (#576 E1).
//!
//! Compiled for this crate's own tests and for every dependent that enables
//! `test-support` in its `[dev-dependencies]`; production builds never see this
//! module. `stalagmight_core::test_fixtures` re-exports everything here next to
//! its `AppState` fixtures, so a core or app-crate test imports from one place.
//!
//! # Shots
//!
//! Four named shapes cover what the corpus used to build in a private helper
//! per file:
//!
//! | fixture | azimuth in / out | depth in / out |
//! |---|---|---|
//! | [`shot`]`(length)` | 45 / 225 | 10 / 11 |
//! | [`straight_shot`]`(azimuth, length)` | azimuth / azimuth | 0 / 0 |
//! | [`heading_shot`]`(azimuth, length)` | azimuth / (azimuth + 180) mod 360 | 10 / 10 |
//! | [`bare_shot`]`(length)` | 0 / 0 | 0 / 0 |
//!
//! Every field the table does not name is `ShotData::default()`'s. Anything else
//! starts from one of them and chains the `with_*` setters below
//! (`straight_shot(90.0, 5.0).with_depths(1.0, 2.0).with_kind(ShotKind::Jump)`):
//! each replaces exactly the fields it names and nothing else.
//!
//! A file that needs its own shape many times may name it in a one-line local
//! wrapper over these; a shape a third file needs belongs here instead.

use crate::{ShotData, ShotKind};

/// The corpus's default shot: a forward/back-sighted pair (45 / 225) descending
/// from 10 m to 11 m.
pub fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

/// A level shot at the surface whose two azimuth readings agree, so the
/// heading is unambiguous: the shape to reason about geometry with.
pub fn straight_shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

/// A level shot at 10 m on a fixed heading, recorded as a forward reading and
/// its back sight (`(azimuth + 180) % 360`).
pub fn heading_shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        length,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    }
}

/// A shot that is only a length: every reading is `ShotData::default()`'s.
pub fn bare_shot(length: f64) -> ShotData {
    ShotData {
        length,
        ..Default::default()
    }
}

/// The fixture setters: each consumes the shot and returns it with exactly the
/// named fields replaced.
impl ShotData {
    /// Replace the length.
    #[must_use]
    pub fn with_length(mut self, length: f64) -> Self {
        self.length = length;
        self
    }

    /// Replace both azimuth readings.
    #[must_use]
    pub fn with_azimuths(mut self, azimuth_in: f64, azimuth_out: f64) -> Self {
        self.azimuth_in = azimuth_in;
        self.azimuth_out = azimuth_out;
        self
    }

    /// Replace both depth readings.
    #[must_use]
    pub fn with_depths(mut self, depth_in: f64, depth_out: f64) -> Self {
        self.depth_in = depth_in;
        self.depth_out = depth_out;
        self
    }

    /// Replace both pitch readings.
    #[must_use]
    pub fn with_pitches(mut self, pitch_in: Option<f64>, pitch_out: Option<f64>) -> Self {
        self.pitch_in = pitch_in;
        self.pitch_out = pitch_out;
        self
    }

    /// Replace all four passage dimensions.
    #[must_use]
    pub fn with_lrud(
        mut self,
        left: Option<f64>,
        right: Option<f64>,
        up: Option<f64>,
        down: Option<f64>,
    ) -> Self {
        self.left = left;
        self.right = right;
        self.up = up;
        self.down = down;
        self
    }

    /// Replace the kind.
    #[must_use]
    pub fn with_kind(mut self, kind: ShotKind) -> Self {
        self.kind = kind;
        self
    }

    /// Give the shot a stable identity (a `None` uuid is minted on insert).
    #[must_use]
    pub fn with_uuid(mut self, uuid: &str) -> Self {
        self.uuid = Some(uuid.to_string());
        self
    }

    /// Replace the comment.
    #[must_use]
    pub fn with_comment(mut self, comment: &str) -> Self {
        self.comment = Some(comment.to_string());
        self
    }

    /// Replace the marker index.
    #[must_use]
    pub fn with_marker_index(mut self, marker_index: u32) -> Self {
        self.marker_index = Some(marker_index);
        self
    }

    /// Replace the temperature.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }
}
