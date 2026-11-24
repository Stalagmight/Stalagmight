//! A line's metadata in a changeset: a whole image for a line that exists on
//! one side only (created or deleted), field-level `[before, after]` pairs
//! for a line on both sides.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::{AzimuthReference, LineId, SectionId, StationId, SurveyDirection};

use super::super::Line;
use super::bits::Same;
use super::shots::ShotSide;
use super::values::finite_f64;
use super::Direction;

/// What one line's change records. Lines are listed by ascending id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LineChange {
    pub(crate) id: LineId,
    /// Field-level metadata edits of a line on both sides.
    #[serde(default, skip_serializing_if = "LineMetaEdit::is_empty")]
    pub(crate) meta: LineMetaEdit,
    /// How the BEFORE state's shot list is built from the after state
    /// (absent: the line does not exist before, or its shots did not change).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) before: Option<ShotSide>,
    /// How the AFTER state's shot list is built from the before state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) after: Option<ShotSide>,
}

impl LineChange {
    /// The side a step in `direction` builds (its target).
    pub(crate) fn target(&self, direction: Direction) -> Option<&ShotSide> {
        match direction {
            Direction::Undo => self.before.as_ref(),
            Direction::Redo => self.after.as_ref(),
        }
    }

    /// The side a step in `direction` starts from.
    pub(crate) fn source(&self, direction: Direction) -> Option<&ShotSide> {
        match direction {
            Direction::Undo => self.after.as_ref(),
            Direction::Redo => self.before.as_ref(),
        }
    }

    /// Whether the line exists on the BEFORE (`after == false`) or AFTER
    /// side: a side with an image is the only state it exists in.
    pub(crate) fn exists(&self, after: bool) -> bool {
        let (this, other) = if after {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        this.is_some() || other.as_ref().is_none_or(|side| side.line.is_none())
    }
}

/// A whole line's metadata (a created or deleted line).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LineImage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    pub(crate) color: String,
    pub(crate) dir: SurveyDirection,
    #[serde(serialize_with = "ser_f64")]
    pub(crate) q: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) az: Option<AzimuthReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sec: Option<SectionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) surveyor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) date: Option<NaiveDate>,
    pub(crate) start: StationId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) hand: Vec<(StationId, [f64; 3])>,
}

fn ser_f64<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    finite_f64(*v, s)
}

impl LineImage {
    pub(crate) fn of(line: &Line) -> Self {
        Self {
            name: line.name.clone(),
            color: line.color.clone(),
            dir: line.direction,
            q: line.quality,
            az: line.azimuth_reference,
            sec: line.section_id,
            surveyor: line.legacy_surveyor.clone(),
            date: line.legacy_date,
            start: line.start,
            hand: line.hand_moves.iter().map(|(s, h)| (*s, *h)).collect(),
        }
    }

    /// A line with this metadata and no shots.
    pub(crate) fn to_line(&self, id: LineId) -> Line {
        Line {
            id,
            name: self.name.clone(),
            color: self.color.clone(),
            direction: self.dir,
            quality: self.q,
            azimuth_reference: self.az,
            section_id: self.sec,
            legacy_surveyor: self.surveyor.clone(),
            legacy_date: self.date,
            start: self.start,
            shots: Vec::new(),
            hand_moves: self.hand.iter().copied().collect::<BTreeMap<_, _>>(),
            next_shot_id: id.saturating_mul(crate::FEATURE_ID_STRIDE),
        }
    }

    pub(crate) fn finite(&self) -> bool {
        self.q.is_finite()
            && self.az.is_none_or(|a| a.correction.is_finite())
            && self
                .hand
                .iter()
                .all(|(_, h)| h.iter().all(|v| v.is_finite()))
    }
}

/// The metadata fields of a line on both sides that changed, each as
/// `[before, after]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LineMetaEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<(Option<String>, Option<String>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dir: Option<(SurveyDirection, SurveyDirection)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) q: Option<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) az: Option<(Option<AzimuthReference>, Option<AzimuthReference>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sec: Option<(Option<SectionId>, Option<SectionId>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) surveyor: Option<(Option<String>, Option<String>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) date: Option<(Option<NaiveDate>, Option<NaiveDate>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<(StationId, StationId)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hand: Option<(Vec<(StationId, [f64; 3])>, Vec<(StationId, [f64; 3])>)>,
}

fn pair<T: Same + Clone>(before: &T, after: &T) -> Option<(T, T)> {
    (!before.same(after)).then(|| (before.clone(), after.clone()))
}

impl Same for (StationId, [f64; 3]) {
    fn same(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1.same(&other.1)
    }
}

impl LineMetaEdit {
    pub(crate) fn between(before: &Line, after: &Line) -> Self {
        let hand = |l: &Line| -> Vec<(StationId, [f64; 3])> {
            l.hand_moves.iter().map(|(s, h)| (*s, *h)).collect()
        };
        Self {
            name: pair(&before.name, &after.name),
            color: pair(&before.color, &after.color),
            dir: pair(&before.direction, &after.direction),
            q: pair(&before.quality, &after.quality),
            az: pair(&before.azimuth_reference, &after.azimuth_reference),
            sec: pair(&before.section_id, &after.section_id),
            surveyor: pair(&before.legacy_surveyor, &after.legacy_surveyor),
            date: pair(&before.legacy_date, &after.legacy_date),
            start: pair(&before.start, &after.start),
            hand: (!before.hand_moves.same(&after.hand_moves)).then(|| (hand(before), hand(after))),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.color.is_none()
            && self.dir.is_none()
            && self.q.is_none()
            && self.az.is_none()
            && self.sec.is_none()
            && self.surveyor.is_none()
            && self.date.is_none()
            && self.start.is_none()
            && self.hand.is_none()
    }

    /// Whether `line` holds every recorded field's value on the side a step
    /// in `direction` starts from.
    pub(crate) fn matches(&self, line: &Line, direction: Direction) -> bool {
        fn from<T>(pair: &(T, T), direction: Direction) -> &T {
            match direction {
                Direction::Undo => &pair.1,
                Direction::Redo => &pair.0,
            }
        }
        let hand: Vec<(StationId, [f64; 3])> =
            line.hand_moves.iter().map(|(s, h)| (*s, *h)).collect();
        self.name
            .as_ref()
            .is_none_or(|p| from(p, direction).same(&line.name))
            && self
                .color
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.color))
            && self
                .dir
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.direction))
            && self
                .q
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.quality))
            && self
                .az
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.azimuth_reference))
            && self
                .sec
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.section_id))
            && self
                .surveyor
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.legacy_surveyor))
            && self
                .date
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.legacy_date))
            && self
                .start
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&line.start))
            && self
                .hand
                .as_ref()
                .is_none_or(|p| from(p, direction).same(&hand))
    }

    /// Write every recorded field's value on the side a step in `direction`
    /// lands on.
    pub(crate) fn apply(&self, line: &mut Line, direction: Direction) {
        fn to<T: Clone>(pair: &(T, T), direction: Direction) -> T {
            match direction {
                Direction::Undo => pair.0.clone(),
                Direction::Redo => pair.1.clone(),
            }
        }
        if let Some(p) = &self.name {
            line.name = to(p, direction);
        }
        if let Some(p) = &self.color {
            line.color = to(p, direction);
        }
        if let Some(p) = &self.dir {
            line.direction = to(p, direction);
        }
        if let Some(p) = &self.q {
            line.quality = to(p, direction);
        }
        if let Some(p) = &self.az {
            line.azimuth_reference = to(p, direction);
        }
        if let Some(p) = &self.sec {
            line.section_id = to(p, direction);
        }
        if let Some(p) = &self.surveyor {
            line.legacy_surveyor = to(p, direction);
        }
        if let Some(p) = &self.date {
            line.legacy_date = to(p, direction);
        }
        if let Some(p) = &self.start {
            line.start = to(p, direction);
        }
        if let Some(p) = &self.hand {
            line.hand_moves = to(p, direction).into_iter().collect();
        }
    }

    pub(crate) fn finite(&self) -> bool {
        let hand_ok =
            |h: &Vec<(StationId, [f64; 3])>| h.iter().all(|(_, v)| v.iter().all(|x| x.is_finite()));
        self.q.is_none_or(|(a, b)| a.is_finite() && b.is_finite())
            && self.az.is_none_or(|(a, b)| {
                a.is_none_or(|r| r.correction.is_finite())
                    && b.is_none_or(|r| r.correction.is_finite())
            })
            && self
                .hand
                .as_ref()
                .is_none_or(|(a, b)| hand_ok(a) && hand_ok(b))
    }
}
