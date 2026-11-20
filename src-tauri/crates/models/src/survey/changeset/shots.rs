//! A line's shot list on one side of a changeset: how that state's shots are
//! built from the OTHER state.
//!
//! A side is a list of segments. A copy run takes `n` consecutive shots of a
//! line of the other state (any touched line: a split's tail, a combine's
//! appended line), forwards or backwards, through one of four transforms
//! ([`RunMode`]: as they are, reversed, reversed and flipped as Reverse Line
//! flips a shot, the same with left and right swapped), then writes the
//! field-level exceptions where the result is not yet the recorded shot.
//! Literal rows carry shots the other state does not hold (a created shot on
//! the redo side, a deleted one on the undo side), column by column.
//!
//! So an edit of one cell is a run plus one value per side, a reverse is one
//! run per side plus the few ulps the flip does not give back (#808's
//! precedent: the flip is not an exact involution, so its exceptions are
//! computed by re-applying it and diffing the bits), a split or a combine a
//! few runs. Every side also records the line's length and content hash: the
//! live line must hash to the recorded source side before a step, and the
//! line built must hash to the recorded target side after it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{reversed_azimuths, LineId, ShotId, SurveyCredit};

use super::super::{Line, Shot};
use super::bits::line_hash;
use super::lines::LineImage;
use super::values::{blank_shot, Cols, ColsBuilder, Credits, ShotField};
use super::Refusal;

/// One side of a line's shot list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ShotSide {
    /// The number of shots on this side.
    pub(crate) n: u32,
    /// The line's content hash on this side ([`line_hash`]).
    pub(crate) h: u64,
    /// The line's metadata, when the line exists on this side only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<Box<LineImage>>,
    /// How the shots are built, in order.
    pub(crate) s: Vec<Seg>,
    /// Session only: the shot ids and allocator this side had, so a step in
    /// the same session gives every shot its id back. Never persisted (plan
    /// 1.1): a decoded entry mints ids instead.
    #[serde(skip)]
    pub(crate) hint: Option<Box<SessionIds>>,
}

/// The session shot ids of one side.
#[derive(Debug, Clone)]
pub(crate) struct SessionIds {
    pub(crate) ids: Vec<ShotId>,
    pub(crate) next: ShotId,
}

/// One stretch of a side.
#[derive(Debug, Clone)]
pub(crate) enum Seg {
    Copy(CopyRun),
    Rows(Rows),
}

/// `n` shots of line `line` of the other state from index `at` (forwards, or
/// backwards for the reversed modes), transformed by `mode`, then `edits`.
#[derive(Debug, Clone)]
pub(crate) struct CopyRun {
    pub(crate) line: LineId,
    pub(crate) at: u32,
    pub(crate) n: u32,
    pub(crate) mode: RunMode,
    /// The uuid of the shot at `at`, checked on apply. Recorded for a run out
    /// of another line; a run within its own line is covered by the line's
    /// hash alone.
    pub(crate) anchor: Option<String>,
    /// Field values the transform does not give, by offset in the run.
    pub(crate) edits: Cols,
}

/// How a copy run turns a source shot into a target shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMode {
    /// The shot as it is.
    Forward,
    /// Read backwards: each shot ends where its source started, values as
    /// they are.
    Reversed,
    /// Read backwards and flipped as Reverse Line flips a shot.
    Flipped,
    /// Flipped, with left and right swapped.
    FlippedSwapLr,
}

impl RunMode {
    fn code(self) -> u8 {
        match self {
            Self::Forward => 0,
            Self::Reversed => 1,
            Self::Flipped => 2,
            Self::FlippedSwapLr => 3,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Forward),
            1 => Some(Self::Reversed),
            2 => Some(Self::Flipped),
            3 => Some(Self::FlippedSwapLr),
            _ => None,
        }
    }

    pub(crate) fn reversed(self) -> bool {
        self != Self::Forward
    }

    /// The source index of offset `t` of a run starting at `at`.
    pub(crate) fn source_index(self, at: usize, t: usize) -> usize {
        if self.reversed() {
            at - t
        } else {
            at + t
        }
    }
}

/// Shots the other state does not hold: uuids and columns.
#[derive(Debug, Clone)]
pub(crate) struct Rows {
    pub(crate) uuids: Vec<String>,
    pub(crate) cols: Cols,
}

impl Seg {
    /// How many shots the segment contributes.
    pub(crate) fn len(&self) -> u32 {
        match self {
            Self::Copy(run) => run.n,
            Self::Rows(rows) => rows.uuids.len() as u32,
        }
    }
}

// ----- the transform ---------------------------------------------------------

/// Re-associate one shot's readings with the opposite direction of travel:
/// exactly Reverse Line's per-shot step (`project::reverse_shots`).
fn flip(shot: &mut Shot, swap_lr: bool) {
    (shot.azimuth_in, shot.azimuth_out) = reversed_azimuths(shot.azimuth_in, shot.azimuth_out);
    std::mem::swap(&mut shot.depth_in, &mut shot.depth_out);
    let pitch_in = shot.pitch_in.map(|p| -p);
    shot.pitch_in = shot.pitch_out.map(|p| -p);
    shot.pitch_out = pitch_in;
    if swap_lr {
        std::mem::swap(&mut shot.left, &mut shot.right);
    }
}

/// What a run in `mode` makes of shot `k` of `source`.
fn transformed(source: &Line, k: usize, mode: RunMode) -> Cow<'_, Shot> {
    let shot = &source.shots[k];
    if mode == RunMode::Forward {
        return Cow::Borrowed(shot);
    }
    let mut out = shot.clone();
    // Backwards, shot k runs from its old end to its old start.
    out.to = source.station(k).unwrap_or(source.start);
    match mode {
        RunMode::Flipped => flip(&mut out, false),
        RunMode::FlippedSwapLr => flip(&mut out, true),
        RunMode::Forward | RunMode::Reversed => {}
    }
    Cow::Owned(out)
}

// ----- capture ---------------------------------------------------------------

/// The shots of one state's touched lines, for the runs of the other state's
/// sides: where each uuid is, and which shots a run already took.
pub(crate) struct SourceState<'a> {
    pub(crate) lines: HashMap<LineId, &'a Line>,
    index: HashMap<&'a str, (LineId, u32)>,
    used: HashMap<LineId, Vec<bool>>,
}

impl<'a> SourceState<'a> {
    pub(crate) fn new(lines: impl IntoIterator<Item = &'a Line>) -> Self {
        let mut state = Self {
            lines: HashMap::new(),
            index: HashMap::new(),
            used: HashMap::new(),
        };
        for line in lines {
            state.lines.insert(line.id, line);
            state.used.insert(line.id, vec![false; line.shots.len()]);
            for (i, shot) in line.shots.iter().enumerate() {
                // A repeated uuid (an invalid model) copies from its first
                // holder; the others are written literally.
                state
                    .index
                    .entry(shot.uuid.as_str())
                    .or_insert((line.id, i as u32));
            }
        }
        state
    }

    /// Where shot `uuid` is, if no run took it yet.
    fn find(&self, uuid: &str) -> Option<(LineId, u32)> {
        let (line, i) = *self.index.get(uuid)?;
        let taken = self
            .used
            .get(&line)
            .and_then(|u| u.get(i as usize))
            .copied()
            .unwrap_or(true);
        (!taken).then_some((line, i))
    }

    fn take(&mut self, line: LineId, i: u32) {
        if let Some(slot) = self.used.get_mut(&line).and_then(|u| u.get_mut(i as usize)) {
            *slot = true;
        }
    }
}

/// The side recording `target` (one state's line) against `source` (the
/// other state's touched lines). `image`: the line exists in this state only.
pub(crate) fn capture_side(
    target: &Line,
    source: &mut SourceState<'_>,
    credits: &mut Credits,
    image: bool,
) -> ShotSide {
    let shots = &target.shots;
    let mut segs = Vec::new();
    let mut literal: Vec<usize> = Vec::new();
    let mut j = 0;
    while j < shots.len() {
        let Some((k, i)) = source.find(&shots[j].uuid) else {
            literal.push(j);
            j += 1;
            continue;
        };
        if !literal.is_empty() {
            segs.push(Seg::Rows(rows_of(shots, &literal, credits)));
            literal.clear();
        }
        source.take(k, i);
        let mut n = 1usize;
        let mut reversed: Option<bool> = None;
        while j + n < shots.len() {
            let next = source.find(&shots[j + n].uuid);
            let forward = next == Some((k, i + n as u32));
            let backward = (i as usize) >= n && next == Some((k, i - n as u32));
            let go = match reversed {
                None if forward => Some(false),
                None if backward => Some(true),
                Some(false) if forward => Some(false),
                Some(true) if backward => Some(true),
                _ => None,
            };
            let Some(dir) = go else { break };
            reversed = Some(dir);
            let src = if dir { i - n as u32 } else { i + n as u32 };
            source.take(k, src);
            n += 1;
        }
        let Some(src) = source.lines.get(&k).copied() else {
            // Unreachable: every indexed uuid belongs to an indexed line.
            literal.extend(j..j + n);
            j += n;
            continue;
        };
        let modes: &[RunMode] = if reversed == Some(true) {
            &[RunMode::Flipped, RunMode::FlippedSwapLr, RunMode::Reversed]
        } else {
            &[RunMode::Forward]
        };
        let target_run = &shots[j..j + n];
        let mode = modes
            .iter()
            .copied()
            .min_by_key(|&mode| run_cost(target_run, src, i as usize, mode))
            .unwrap_or(RunMode::Forward);
        let mut edits = ColsBuilder::default();
        for (t, shot) in target_run.iter().enumerate() {
            let base = transformed(src, mode.source_index(i as usize, t), mode);
            for field in ShotField::ALL {
                if !field.same(shot, &base) {
                    edits.push(field, t as u32, field.get(shot, credits));
                }
            }
        }
        segs.push(Seg::Copy(CopyRun {
            line: k,
            at: i,
            n: n as u32,
            mode,
            anchor: (k != target.id).then(|| src.shots[i as usize].uuid.clone()),
            edits: edits.finish(),
        }));
        j += n;
    }
    if !literal.is_empty() {
        segs.push(Seg::Rows(rows_of(shots, &literal, credits)));
    }
    ShotSide {
        n: shots.len() as u32,
        h: line_hash(target),
        line: image.then(|| Box::new(LineImage::of(target))),
        s: segs,
        hint: Some(Box::new(SessionIds {
            ids: shots.iter().map(|s| s.id).collect(),
            next: target.next_shot_id,
        })),
    }
}

/// How many fields a run in `mode` leaves to write as exceptions.
fn run_cost(target: &[Shot], source: &Line, at: usize, mode: RunMode) -> usize {
    target
        .iter()
        .enumerate()
        .map(|(t, shot)| {
            let base = transformed(source, mode.source_index(at, t), mode);
            ShotField::ALL
                .iter()
                .filter(|field| !field.same(shot, &base))
                .count()
        })
        .sum()
}

/// The literal rows of `shots[offsets]`.
fn rows_of(shots: &[Shot], offsets: &[usize], credits: &mut Credits) -> Rows {
    let blank = blank_shot(String::new());
    let mut cols = ColsBuilder::default();
    let mut uuids = Vec::with_capacity(offsets.len());
    for (r, &j) in offsets.iter().enumerate() {
        let shot = &shots[j];
        uuids.push(shot.uuid.clone());
        for field in ShotField::ALL {
            if !field.same(shot, &blank) {
                cols.push(field, r as u32, field.get(shot, credits));
            }
        }
    }
    Rows {
        uuids,
        cols: cols.finish(),
    }
}

// ----- apply -----------------------------------------------------------------

/// The shots of one target side, each with the live session id it keeps
/// (`Some` for a shot copied within its own line) or `None` (minted).
pub(crate) struct Built {
    pub(crate) shots: Vec<Shot>,
    pub(crate) kept: Vec<Option<ShotId>>,
}

/// Build `side`'s shots for line `target` out of the live lines.
pub(crate) fn build_shots(
    side: &ShotSide,
    target: LineId,
    live: &HashMap<LineId, &Line>,
    credits: &[Arc<SurveyCredit>],
) -> Result<Built, Refusal> {
    let total = side.n as usize;
    let mut shots: Vec<Shot> = Vec::with_capacity(total);
    let mut kept: Vec<Option<ShotId>> = Vec::with_capacity(total);
    for seg in &side.s {
        let start = shots.len();
        if start + seg.len() as usize > total {
            return Err(Refusal::Shape(format!(
                "line {target}: its runs hold more shots than recorded"
            )));
        }
        match seg {
            Seg::Copy(run) => {
                let Some(src) = live.get(&run.line).copied() else {
                    return Err(Refusal::Line(run.line));
                };
                let (at, n) = (run.at as usize, run.n as usize);
                let fits = if run.mode.reversed() {
                    at < src.shots.len() && n <= at + 1
                } else {
                    at.checked_add(n).is_some_and(|end| end <= src.shots.len())
                };
                if n == 0 || !fits {
                    return Err(Refusal::Line(run.line));
                }
                if let Some(anchor) = &run.anchor {
                    if src.shots[at].uuid != *anchor {
                        return Err(Refusal::Anchor(run.line));
                    }
                }
                for t in 0..n {
                    let k = run.mode.source_index(at, t);
                    shots.push(transformed(src, k, run.mode).into_owned());
                    kept.push((src.id == target).then(|| src.shots[k].id));
                }
                if !run.edits.apply(&mut shots[start..], credits) {
                    return Err(Refusal::Shape(format!(
                        "line {target}: an edit does not fit its run"
                    )));
                }
            }
            Seg::Rows(rows) => {
                shots.extend(rows.uuids.iter().map(|uuid| blank_shot(uuid.clone())));
                kept.resize(shots.len(), None);
                if !rows.cols.apply(&mut shots[start..], credits) {
                    return Err(Refusal::Shape(format!(
                        "line {target}: a row value does not fit"
                    )));
                }
            }
        }
    }
    if shots.len() != total {
        return Err(Refusal::Shape(format!(
            "line {target}: its runs hold {} shots, {total} recorded",
            shots.len()
        )));
    }
    Ok(Built { shots, kept })
}

// ----- persisted form --------------------------------------------------------

impl Serialize for Seg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Copy(run) => {
                let len = if run.edits.is_empty() { 5 } else { 6 };
                let mut seq = s.serialize_seq(Some(len))?;
                seq.serialize_element(&run.line)?;
                seq.serialize_element(&run.at)?;
                seq.serialize_element(&run.n)?;
                seq.serialize_element(&run.mode.code())?;
                seq.serialize_element(&run.anchor)?;
                if !run.edits.is_empty() {
                    seq.serialize_element(&run.edits)?;
                }
                seq.end()
            }
            Self::Rows(rows) => {
                let len = if rows.cols.is_empty() { 1 } else { 2 };
                let mut map = s.serialize_map(Some(len))?;
                map.serialize_entry("u", &rows.uuids)?;
                if !rows.cols.is_empty() {
                    map.serialize_entry("f", &rows.cols)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Seg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Seg;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a copy run (array) or literal rows (object)")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Seg, A::Error> {
                let missing = || de::Error::custom("a copy run is missing an element");
                let line: LineId = seq.next_element()?.ok_or_else(missing)?;
                let at: u32 = seq.next_element()?.ok_or_else(missing)?;
                let n: u32 = seq.next_element()?.ok_or_else(missing)?;
                let code: u8 = seq.next_element()?.ok_or_else(missing)?;
                let mode = RunMode::from_code(code)
                    .ok_or_else(|| de::Error::custom(format!("unknown run mode {code}")))?;
                let anchor: Option<String> = seq.next_element()?.ok_or_else(missing)?;
                let edits: Cols = seq.next_element()?.unwrap_or_default();
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("a copy run with extra elements"));
                }
                Ok(Seg::Copy(CopyRun {
                    line,
                    at,
                    n,
                    mode,
                    anchor,
                    edits,
                }))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Seg, A::Error> {
                let mut uuids: Option<Vec<String>> = None;
                let mut cols: Option<Cols> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "u" if uuids.is_none() => uuids = Some(map.next_value()?),
                        "f" if cols.is_none() => cols = Some(map.next_value()?),
                        other => {
                            return Err(de::Error::custom(format!(
                                "unexpected key {other:?} in literal rows"
                            )))
                        }
                    }
                }
                Ok(Seg::Rows(Rows {
                    uuids: uuids.ok_or_else(|| de::Error::custom("literal rows without uuids"))?,
                    cols: cols.unwrap_or_default(),
                }))
            }
        }
        d.deserialize_any(V)
    }
}
