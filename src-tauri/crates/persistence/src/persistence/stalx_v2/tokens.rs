//! The file's spelling of every enumerated value, both ways. Unknown tokens
//! are decoded to a fail-closed fallback by the caller, which also marks the
//! project read-only (tier 1).

use crate::models::survey::ClosureStrategy;
use crate::models::{CorrectionSource, NorthReference, ShotKind, SurveyDirection, UndoMode};

pub(super) fn kind(kind: ShotKind) -> Option<&'static str> {
    match kind {
        ShotKind::Normal => None,
        ShotKind::Jump => Some("jump"),
        ShotKind::Reference => Some("reference"),
    }
}

pub(super) fn parse_kind(token: &str) -> Option<ShotKind> {
    match token {
        "normal" => Some(ShotKind::Normal),
        "jump" => Some(ShotKind::Jump),
        "reference" => Some(ShotKind::Reference),
        _ => None,
    }
}

pub(super) fn direction(direction: SurveyDirection) -> &'static str {
    match direction {
        SurveyDirection::In => "in",
        SurveyDirection::Out => "out",
    }
}

pub(super) fn parse_direction(token: &str) -> Option<SurveyDirection> {
    match token {
        "in" => Some(SurveyDirection::In),
        "out" => Some(SurveyDirection::Out),
        _ => None,
    }
}

pub(super) fn strategy(strategy: ClosureStrategy) -> &'static str {
    match strategy {
        ClosureStrategy::ProportionalByLength => "proportional_by_length",
        ClosureStrategy::EqualDistribution => "equal_distribution",
        ClosureStrategy::Batch => "batch",
        ClosureStrategy::LeastSquaresWeighted => "least_squares_weighted",
        ClosureStrategy::Move => "move",
        ClosureStrategy::Tie => "tie",
    }
}

pub(super) fn parse_strategy(token: &str) -> Option<ClosureStrategy> {
    match token {
        "proportional_by_length" => Some(ClosureStrategy::ProportionalByLength),
        "equal_distribution" => Some(ClosureStrategy::EqualDistribution),
        "batch" => Some(ClosureStrategy::Batch),
        "least_squares_weighted" => Some(ClosureStrategy::LeastSquaresWeighted),
        "move" => Some(ClosureStrategy::Move),
        "tie" => Some(ClosureStrategy::Tie),
        _ => None,
    }
}

pub(super) fn north(north: NorthReference) -> &'static str {
    north.as_str()
}

pub(super) fn parse_north(token: &str) -> Option<NorthReference> {
    match token {
        "magnetic" => Some(NorthReference::Magnetic),
        "true" => Some(NorthReference::True),
        "grid" => Some(NorthReference::Grid),
        _ => None,
    }
}

/// `(token, epoch)` of a correction source.
pub(super) fn source(source: CorrectionSource) -> (&'static str, Option<i32>) {
    (source.as_str(), source.epoch_year())
}

pub(super) fn parse_source(token: &str, epoch: Option<i32>) -> Option<CorrectionSource> {
    match (token, epoch) {
        ("unknown", None) => Some(CorrectionSource::Unknown),
        ("file", None) => Some(CorrectionSource::FileDeclared),
        ("manual", None) => Some(CorrectionSource::Manual),
        ("igrf", Some(epoch_year)) => Some(CorrectionSource::IgrfAuto { epoch_year }),
        _ => None,
    }
}

pub(super) fn undo_mode(mode: UndoMode) -> &'static str {
    mode.as_db_str()
}

/// `full` or `disabled`; any other token reads as `disabled` (fail closed:
/// history never rides a push by mistake).
pub(super) fn parse_undo_mode(token: &str) -> UndoMode {
    UndoMode::from_db_str(token)
}
