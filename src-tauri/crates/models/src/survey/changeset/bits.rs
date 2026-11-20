//! Bitwise equality ([`Same`]) and the stable content hash of a line
//! ([`line_hash`]).
//!
//! Undo restores bits, not values: `-0.0` is not `0.0`, and a precondition
//! that compared floats with `==` would let a step through on a project that
//! differs from the record. Every comparison the engine makes goes through
//! [`Same`].
//!
//! The line hash is persisted (it is a side's precondition and
//! postcondition), so its byte layout is part of the history format: a change
//! to it, or to the fields it covers, needs a new
//! [`super::HISTORY_FORMAT_VERSION`].

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::{
    AzimuthReference, CorrectionSource, GpsCoordinates, NorthReference, Person, Section, ShotKind,
    SurveyCredit, SurveyDirection,
};

use super::super::{
    ClosureStrategy, Line, LoopClosure, Placement, ProjectSettings, Shot, Station, StationAlias,
    ZeroDatum,
};

/// Equality to the bit: floats by their bit patterns, everything else by
/// value.
pub(crate) trait Same {
    fn same(&self, other: &Self) -> bool;
}

impl Same for f64 {
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

macro_rules! same_by_eq {
    ($($t:ty),* $(,)?) => {
        $(impl Same for $t {
            fn same(&self, other: &Self) -> bool {
                self == other
            }
        })*
    };
}

same_by_eq!(
    u32,
    u64,
    bool,
    String,
    str,
    ShotKind,
    SurveyDirection,
    NaiveDate,
    DateTime<Utc>,
    Station,
    StationAlias,
    LoopClosure,
    ClosureStrategy,
    Section,
    SurveyCredit,
    Person,
    NorthReference,
    CorrectionSource,
);

impl<T: Same + ?Sized> Same for Arc<T> {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(self, other) || (**self).same(&**other)
    }
}

impl<T: Same> Same for Option<T> {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        }
    }
}

impl<T: Same> Same for [T] {
    fn same(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.same(b))
    }
}

impl<T: Same> Same for Vec<T> {
    fn same(&self, other: &Self) -> bool {
        self.as_slice().same(other.as_slice())
    }
}

impl<T: Same, const N: usize> Same for [T; N] {
    fn same(&self, other: &Self) -> bool {
        self.as_slice().same(other.as_slice())
    }
}

impl<K: Same, V: Same> Same for BTreeMap<K, V> {
    fn same(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other)
                .all(|((ka, va), (kb, vb))| ka.same(kb) && va.same(vb))
    }
}

impl Same for GpsCoordinates {
    fn same(&self, other: &Self) -> bool {
        self.latitude.same(&other.latitude)
            && self.longitude.same(&other.longitude)
            && self.depth_reference.same(&other.depth_reference)
    }
}

impl Same for AzimuthReference {
    fn same(&self, other: &Self) -> bool {
        self.north.same(&other.north)
            && self.correction.same(&other.correction)
            && self.source.same(&other.source)
    }
}

impl Same for ZeroDatum {
    fn same(&self, other: &Self) -> bool {
        self.station == other.station
            && self.via == other.via
            && self.gps.same(&other.gps)
            && self.accuracy_m.same(&other.accuracy_m)
            && self.entrance.same(&other.entrance)
            && self.correction.same(&other.correction)
    }
}

impl Same for Placement {
    fn same(&self, other: &Self) -> bool {
        self.station == other.station && self.x.same(&other.x) && self.y.same(&other.y)
    }
}

impl Same for ProjectSettings {
    /// Compared through serde (rare, and it covers every field a later build
    /// adds to the validation thresholds): floats are written by their
    /// shortest round-trip spelling, so equal text is equal bits.
    fn same(&self, other: &Self) -> bool {
        match (serde_json::to_string(self), serde_json::to_string(other)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

/// Whether two shots are the same record to the bit (the session id aside).
pub(crate) fn same_shot(a: &Shot, b: &Shot) -> bool {
    a.to == b.to
        && a.length.same(&b.length)
        && a.azimuth_in.same(&b.azimuth_in)
        && a.azimuth_out.same(&b.azimuth_out)
        && a.depth_in.same(&b.depth_in)
        && a.depth_out.same(&b.depth_out)
        && a.pitch_in.same(&b.pitch_in)
        && a.pitch_out.same(&b.pitch_out)
        && a.left.same(&b.left)
        && a.right.same(&b.right)
        && a.up.same(&b.up)
        && a.down.same(&b.down)
        && a.temperature.same(&b.temperature)
        && a.kind == b.kind
        && a.marker_index == b.marker_index
        && a.uuid == b.uuid
        && a.comment == b.comment
        && a.credit.same(&b.credit)
}

/// Whether two lines carry the same metadata (everything but the shots and
/// the session allocator).
pub(crate) fn same_line_meta(a: &Line, b: &Line) -> bool {
    a.id == b.id
        && a.start == b.start
        && a.direction == b.direction
        && a.quality.same(&b.quality)
        && a.section_id == b.section_id
        && a.azimuth_reference.same(&b.azimuth_reference)
        && a.legacy_date == b.legacy_date
        && a.name == b.name
        && a.color == b.color
        && a.legacy_surveyor == b.legacy_surveyor
        && a.hand_moves.same(&b.hand_moves)
}

/// Whether two lines' shot lists are the same to the bit.
pub(crate) fn same_shots(a: &[Shot], b: &[Shot]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_shot(x, y))
}

/// Whether two projects hold the same persisted model to the bit, counters
/// aside (session ids, the file path and the caches are not part of it).
/// Every field is named, so a new field cannot be missed silently.
pub(crate) fn same_model_but_counters(
    a: &super::super::Project,
    b: &super::super::Project,
) -> bool {
    let super::super::Project {
        name,
        location,
        created_at,
        modified_at,
        stations,
        lines,
        datums,
        placements,
        closures,
        sections,
        settings,
        imported_file_hashes,
        imported_line_hashes,
        counters: _,
        file_path: _,
        caches: _,
    } = a;
    *name == b.name
        && *location == b.location
        && *created_at == b.created_at
        && *modified_at == b.modified_at
        && *stations == b.stations
        && lines.len() == b.lines().len()
        && lines
            .iter()
            .zip(b.lines())
            .all(|(x, y)| same_line_meta(x, y) && same_shots(&x.shots, &y.shots))
        && datums.same(&b.datums)
        && placements.same(&b.placements)
        && closures.same(&b.closures)
        && sections.same(&b.sections)
        && settings.same(&b.settings)
        && imported_file_hashes.same(&b.imported_file_hashes)
        && imported_line_hashes.same(&b.imported_line_hashes)
}

/// The largest integer a JSON reader keeps exactly (2^53 - 1): hashes are
/// cut to it so they persist as plain integers.
pub(crate) const HASH_MASK: u64 = (1 << 53) - 1;

/// FNV-1a, 64 bits: small, fixed, the same on every platform and build.
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn byte(&mut self, b: u8) {
        self.0 ^= u64::from(b);
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    fn tag(&mut self, present: bool) {
        self.byte(u8::from(present));
    }

    pub(crate) fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }

    fn opt_str(&mut self, s: Option<&str>) {
        self.tag(s.is_some());
        if let Some(s) = s {
            self.str(s);
        }
    }

    fn opt_f64(&mut self, v: Option<f64>) {
        self.tag(v.is_some());
        if let Some(v) = v {
            self.f64(v);
        }
    }

    fn opt_u32(&mut self, v: Option<u32>) {
        self.tag(v.is_some());
        if let Some(v) = v {
            self.u32(v);
        }
    }

    fn date(&mut self, d: Option<NaiveDate>) {
        self.tag(d.is_some());
        if let Some(d) = d {
            self.u32(d.num_days_from_ce() as u32);
        }
    }

    fn people(&mut self, people: &[Person]) {
        self.u64(people.len() as u64);
        for person in people {
            self.str(&person.name);
            self.opt_str(person.email.as_deref());
        }
    }

    fn credit(&mut self, credit: Option<&SurveyCredit>) {
        self.tag(credit.is_some());
        if let Some(credit) = credit {
            self.people(&credit.surveyors);
            self.people(&credit.explorers);
            self.date(credit.date);
        }
    }

    fn azimuth_reference(&mut self, reference: Option<&AzimuthReference>) {
        self.tag(reference.is_some());
        if let Some(r) = reference {
            self.byte(match r.north {
                NorthReference::True => 0,
                NorthReference::Magnetic => 1,
                NorthReference::Grid => 2,
            });
            self.f64(r.correction);
            match r.source {
                CorrectionSource::Unknown => self.byte(0),
                CorrectionSource::FileDeclared => self.byte(1),
                CorrectionSource::IgrfAuto { epoch_year } => {
                    self.byte(2);
                    self.u32(epoch_year as u32);
                }
                CorrectionSource::Manual => self.byte(3),
            }
        }
    }

    fn shot(&mut self, shot: &Shot) {
        self.str(&shot.uuid);
        self.u32(shot.to);
        for v in [
            shot.length,
            shot.azimuth_in,
            shot.azimuth_out,
            shot.depth_in,
            shot.depth_out,
        ] {
            self.f64(v);
        }
        for v in [
            shot.pitch_in,
            shot.pitch_out,
            shot.left,
            shot.right,
            shot.up,
            shot.down,
            shot.temperature,
        ] {
            self.opt_f64(v);
        }
        self.opt_str(shot.comment.as_deref());
        self.byte(match shot.kind {
            ShotKind::Normal => 0,
            ShotKind::Jump => 1,
            ShotKind::Reference => 2,
        });
        self.opt_u32(shot.marker_index);
        self.credit(shot.credit.as_deref());
    }

    pub(crate) fn finish(&self) -> u64 {
        self.0 & HASH_MASK
    }
}

/// The content hash of a whole line: its metadata, start, hand moves and
/// every shot, session ids aside. A side's precondition (the live line is the
/// one recorded) and its postcondition (the line built is the one recorded).
pub(crate) fn line_hash(line: &Line) -> u64 {
    let mut h = Fnv::new();
    h.u32(line.id);
    h.opt_str(line.name.as_deref());
    h.str(&line.color);
    h.byte(match line.direction {
        SurveyDirection::In => 0,
        SurveyDirection::Out => 1,
    });
    h.f64(line.quality);
    h.azimuth_reference(line.azimuth_reference.as_ref());
    h.opt_u32(line.section_id);
    h.opt_str(line.legacy_surveyor.as_deref());
    h.date(line.legacy_date);
    h.u32(line.start);
    h.u64(line.hand_moves.len() as u64);
    for (station, [x, y, z]) in &line.hand_moves {
        h.u32(*station);
        h.f64(*x);
        h.f64(*y);
        h.f64(*z);
    }
    h.u64(line.shots.len() as u64);
    for shot in &line.shots {
        h.shot(shot);
    }
    h.finish()
}
