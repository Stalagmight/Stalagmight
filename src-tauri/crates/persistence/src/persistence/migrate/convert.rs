//! Value conversions from the frozen 0.6.0 types (`stalagmight_legacy::models`)
//! to the live shared types the station-graph model uses.
//!
//! The two crates hold the same type definitions (the legacy crate is a frozen
//! copy), so every conversion is a field-for-field copy. They are written out
//! by hand rather than through serde so that a later change to a live type
//! breaks the build here instead of silently dropping a field, and so floats
//! are copied bit for bit.

use std::collections::HashMap;
use std::sync::Arc;

use stalagmight_legacy::models as old;

use crate::models as live;
use crate::models::survey;

pub(super) fn gps(g: &old::GpsCoordinates) -> live::GpsCoordinates {
    live::GpsCoordinates {
        latitude: g.latitude,
        longitude: g.longitude,
        depth_reference: g.depth_reference,
    }
}

pub(super) fn shot_kind(kind: old::ShotKind) -> live::ShotKind {
    match kind {
        old::ShotKind::Normal => live::ShotKind::Normal,
        old::ShotKind::Jump => live::ShotKind::Jump,
        old::ShotKind::Reference => live::ShotKind::Reference,
    }
}

pub(super) fn direction(direction: old::SurveyDirection) -> live::SurveyDirection {
    match direction {
        old::SurveyDirection::In => live::SurveyDirection::In,
        old::SurveyDirection::Out => live::SurveyDirection::Out,
    }
}

pub(super) fn azimuth_reference(r: &old::AzimuthReference) -> live::AzimuthReference {
    live::AzimuthReference {
        north: match r.north {
            old::NorthReference::Magnetic => live::NorthReference::Magnetic,
            old::NorthReference::True => live::NorthReference::True,
            old::NorthReference::Grid => live::NorthReference::Grid,
        },
        correction: r.correction,
        source: match r.source {
            old::CorrectionSource::Unknown => live::CorrectionSource::Unknown,
            old::CorrectionSource::FileDeclared => live::CorrectionSource::FileDeclared,
            old::CorrectionSource::IgrfAuto { epoch_year } => {
                live::CorrectionSource::IgrfAuto { epoch_year }
            }
            old::CorrectionSource::Manual => live::CorrectionSource::Manual,
        },
    }
}

pub(super) fn validation(v: &old::config::ValidationConfig) -> live::config::ValidationConfig {
    live::config::ValidationConfig {
        warn_shot_length_min: v.warn_shot_length_min,
        warn_shot_length_max: v.warn_shot_length_max,
        warn_depth_change_per_shot_max: v.warn_depth_change_per_shot_max,
        warn_azimuth_deviation_max: v.warn_azimuth_deviation_max,
        azimuth_diff_threshold: v.azimuth_diff_threshold,
        pitch_diff_threshold: v.pitch_diff_threshold,
        inclination_depth_diff_threshold: v.inclination_depth_diff_threshold,
    }
}

pub(super) fn closure_adjustment(
    c: &old::config::ClosureAdjustmentConfig,
) -> live::config::ClosureAdjustmentConfig {
    live::config::ClosureAdjustmentConfig {
        base_sigma_length_m: c.base_sigma_length_m,
        base_sigma_azimuth_deg: c.base_sigma_azimuth_deg,
        base_sigma_depth_m: c.base_sigma_depth_m,
    }
}

/// The app's least-squares config in the frozen reader's type (the legacy
/// open solves its Computed View with the user's config, as 0.6.0 did).
pub(super) fn legacy_closure_adjustment(
    c: &live::config::ClosureAdjustmentConfig,
) -> old::config::ClosureAdjustmentConfig {
    old::config::ClosureAdjustmentConfig {
        base_sigma_length_m: c.base_sigma_length_m,
        base_sigma_azimuth_deg: c.base_sigma_azimuth_deg,
        base_sigma_depth_m: c.base_sigma_depth_m,
    }
}

pub(super) fn section(s: &old::Section) -> live::Section {
    live::Section {
        id: s.id,
        name: s.name.clone(),
        color: s.color.clone(),
    }
}

pub(super) fn closure_strategy(strategy: old::ClosureStrategy) -> survey::ClosureStrategy {
    match strategy {
        old::ClosureStrategy::ProportionalByLength => survey::ClosureStrategy::ProportionalByLength,
        old::ClosureStrategy::EqualDistribution => survey::ClosureStrategy::EqualDistribution,
        old::ClosureStrategy::Batch => survey::ClosureStrategy::Batch,
        old::ClosureStrategy::LeastSquaresWeighted => survey::ClosureStrategy::LeastSquaresWeighted,
        old::ClosureStrategy::Move => survey::ClosureStrategy::Move,
    }
}

pub(super) fn speleo_link(link: &old::SpeleoLink) -> live::SpeleoLink {
    live::SpeleoLink {
        endpoint: link.endpoint.clone(),
        project_id: link.project_id.clone(),
        project_name: link.project_name.clone(),
        base_commit_sha: link.base_commit_sha.clone(),
        has_unpushed_edits: link.has_unpushed_edits,
        last_edit_at: link.last_edit_at.clone(),
        last_verify_at: link.last_verify_at.clone(),
        undo_mode: link.undo_mode.map(|mode| match mode {
            old::UndoMode::Full => live::UndoMode::Full,
            old::UndoMode::Disabled => live::UndoMode::Disabled,
            old::UndoMode::Unknown => live::UndoMode::Unknown,
        }),
    }
}

fn person(p: &old::Person) -> live::Person {
    live::Person {
        name: p.name.clone(),
        email: p.email.clone(),
    }
}

/// Converts shot credits, keeping the sharing the legacy load set up: shots
/// that shared one `Arc` (a line's legacy surveyor template) share one in the
/// result too, so a long list over many shots is not copied per shot.
#[derive(Default)]
pub(super) struct Credits {
    by_source: HashMap<usize, Arc<live::SurveyCredit>>,
}

impl Credits {
    /// The live credit for a legacy shot's, `None` for an absent or empty one
    /// (they mean the same thing; the live model stores neither).
    pub(super) fn convert(
        &mut self,
        credit: Option<&Arc<old::SurveyCredit>>,
    ) -> Option<Arc<live::SurveyCredit>> {
        let source = credit?;
        if source.is_empty() {
            return None;
        }
        let key = Arc::as_ptr(source) as usize;
        let shared = self.by_source.entry(key).or_insert_with(|| {
            Arc::new(live::SurveyCredit {
                surveyors: source.surveyors.iter().map(person).collect(),
                explorers: source.explorers.iter().map(person).collect(),
                date: source.date,
            })
        });
        Some(Arc::clone(shared))
    }
}

/// A legacy shot's measurements as a station-graph [`survey::Shot`] ending at
/// `to`. Non-finite readings are replaced exactly as the 0.6.0 save heal
/// (`Shot::sanitize_non_finite`) would have before writing: a required
/// reading becomes 0, an optional one is dropped. Returns whether anything
/// was replaced.
pub(super) fn shot(
    source: &old::Shot,
    uuid: String,
    to: live::StationId,
    credits: &mut Credits,
) -> (survey::Shot, bool) {
    let mut sanitized = false;
    let mut required = |v: f64| {
        if v.is_finite() {
            v
        } else {
            sanitized = true;
            0.0
        }
    };
    let azimuth_in = required(source.azimuth_in);
    let azimuth_out = required(source.azimuth_out);
    let length = required(source.length);
    let depth_in = required(source.depth_in);
    let depth_out = required(source.depth_out);
    let mut optional = |v: Option<f64>| match v {
        Some(x) if !x.is_finite() => {
            sanitized = true;
            None
        }
        other => other,
    };
    let pitch_in = optional(source.pitch_in);
    let pitch_out = optional(source.pitch_out);
    let left = optional(source.left);
    let right = optional(source.right);
    let up = optional(source.up);
    let down = optional(source.down);
    let temperature = optional(source.temperature);
    let shot = survey::Shot {
        id: 0,
        uuid,
        to,
        azimuth_in,
        azimuth_out,
        length,
        depth_in,
        depth_out,
        pitch_in,
        pitch_out,
        left,
        right,
        up,
        down,
        comment: source.comment.clone(),
        kind: shot_kind(source.kind),
        marker_index: source.marker_index,
        temperature,
        credit: credits.convert(source.credit.as_ref()),
    };
    (shot, sanitized)
}
