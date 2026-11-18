//! Shot runs: how a delta rebuilds a section's shots out of the source's,
//! copying by uuid from any section, in either direction, turned around when
//! the section was reversed.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Number, Value};

use super::json::{apply_edit, diff};
use super::{shots_array, DeltaError, Edit, ShotRun, SHOTS, UUID};

/// `ShotRun::Put` as the history stores it: consecutive objects with the same
/// keys become one table - the keys once, then a row of values per shot - and
/// anything else stays as it is. Read back to the same values in order.
pub(super) mod shot_rows {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::{Map, Value};

    #[derive(Serialize, Deserialize)]
    enum Chunk {
        #[serde(rename = "o")]
        One(Value),
        #[serde(rename = "t")]
        Table { k: Vec<String>, r: Vec<Vec<Value>> },
    }

    fn keys_of(value: &Value) -> Option<Vec<&String>> {
        value.as_object().map(|object| object.keys().collect())
    }

    pub(crate) fn serialize<S: Serializer>(
        values: &[Value],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut i = 0;
        while i < values.len() {
            let keys = keys_of(&values[i]);
            let mut j = i + 1;
            while keys.is_some() && j < values.len() && keys_of(&values[j]) == keys {
                j += 1;
            }
            match keys {
                Some(keys) if j - i > 1 => chunks.push(Chunk::Table {
                    k: keys.into_iter().cloned().collect(),
                    r: values[i..j]
                        .iter()
                        .filter_map(Value::as_object)
                        .map(|object| object.values().cloned().collect())
                        .collect(),
                }),
                _ => chunks.extend(values[i..j].iter().cloned().map(Chunk::One)),
            }
            i = j;
        }
        chunks.serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Value>, D::Error> {
        let chunks = Vec::<Chunk>::deserialize(deserializer)?;
        let mut out = Vec::new();
        for chunk in chunks {
            match chunk {
                Chunk::One(value) => out.push(value),
                Chunk::Table { k, r } => {
                    for row in r {
                        if row.len() != k.len() {
                            return Err(serde::de::Error::custom(
                                "a shot row does not match its keys",
                            ));
                        }
                        let object: Map<String, Value> = k.iter().cloned().zip(row).collect();
                        out.push(Value::Object(object));
                    }
                }
            }
        }
        Ok(out)
    }
}

/// One shot turned around the way Reverse Section turns it
/// (`crate::models::reverse_shots`): the bearing reversed, the depths and the
/// pitches swapped end for end (the pitches negated), and left and right
/// swapped when `swap_lr`. A member that is missing, or not a number where a
/// number is expected, is left as it is: whatever this does not reproduce,
/// the copy's edits (`m`) carry, so the result is exact either way.
pub(super) fn flip_shot(shot: &Value, swap_lr: bool) -> Value {
    let Value::Object(members) = shot else {
        return shot.clone();
    };
    let mut out = members.clone();
    let number = |key: &str| members.get(key).and_then(Value::as_f64);
    let set = |out: &mut Map<String, Value>, key: &str, value: f64| {
        if let Some(n) = Number::from_f64(value) {
            out.insert(key.to_string(), Value::Number(n));
        }
    };
    if let (Some(a_in), Some(a_out)) = (number("azimuth_in"), number("azimuth_out")) {
        let (r_in, r_out) = crate::models::reversed_azimuths(a_in, a_out);
        set(&mut out, "azimuth_in", r_in);
        set(&mut out, "azimuth_out", r_out);
    }
    let swap = |out: &mut Map<String, Value>, a: &str, b: &str| {
        let (va, vb) = (members.get(a).cloned(), members.get(b).cloned());
        match vb {
            Some(v) => out.insert(a.to_string(), v),
            None => out.remove(a),
        };
        match va {
            Some(v) => out.insert(b.to_string(), v),
            None => out.remove(b),
        };
    };
    swap(&mut out, "depth_in", "depth_out");
    let negated = |key: &str| -> Option<Value> {
        match members.get(key)? {
            Value::Number(n) => n
                .as_f64()
                .and_then(|f| Number::from_f64(-f))
                .map(Value::Number),
            other => Some(other.clone()),
        }
    };
    match negated("pitch_out") {
        Some(v) => out.insert("pitch_in".to_string(), v),
        None => out.remove("pitch_in"),
    };
    match negated("pitch_in") {
        Some(v) => out.insert("pitch_out".to_string(), v),
        None => out.remove("pitch_out"),
    };
    if swap_lr {
        swap(&mut out, "left", "right");
    }
    Value::Object(out)
}

/// The source's shots, indexed by uuid, built on first use: only an edited or
/// new section needs it.
pub(super) struct ShotPool<'a> {
    sections: &'a BTreeMap<usize, Value>,
    by_uuid: Option<HashMap<&'a str, Vec<(usize, usize)>>>,
}

impl<'a> ShotPool<'a> {
    pub(super) fn new(sections: &'a BTreeMap<usize, Value>) -> Self {
        Self {
            sections,
            by_uuid: None,
        }
    }

    fn shots(&self, section: usize) -> &'a [Value] {
        self.sections
            .get(&section)
            .and_then(|s| s.get(SHOTS))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn index(&mut self) -> &HashMap<&'a str, Vec<(usize, usize)>> {
        let sections = self.sections;
        self.by_uuid.get_or_insert_with(|| {
            let mut index: HashMap<&'a str, Vec<(usize, usize)>> = HashMap::new();
            for (&p, section) in sections {
                let shots = section
                    .get(SHOTS)
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                for (i, shot) in shots.iter().enumerate() {
                    if let Some(uuid) = shot.get(UUID).and_then(Value::as_str) {
                        index.entry(uuid).or_default().push((p, i));
                    }
                }
            }
            index
        })
    }

    /// The target shots `target` as runs over the source's shots. `home` is
    /// the source position of the section being rebuilt, preferred when a
    /// uuid is held in several places.
    pub(super) fn rebuild(&mut self, target: &[Value], home: Option<usize>) -> Vec<ShotRun> {
        let mut out = Vec::new();
        let mut open: Option<OpenCopy> = None;
        for (t, shot) in target.iter().enumerate() {
            let candidates: Vec<(usize, usize)> = shot
                .get(UUID)
                .and_then(Value::as_str)
                .and_then(|uuid| self.index().get(uuid))
                .cloned()
                .unwrap_or_default();
            let extended = open.as_ref().and_then(|run| run.continuation(&candidates));
            let chosen = if extended.is_some() {
                extended
            } else {
                if let Some(run) = open.take() {
                    out.push({
                        let shots = self.shots(run.sec);
                        run.finish(shots, target)
                    });
                }
                candidates
                    .iter()
                    .find(|(p, _)| Some(*p) == home)
                    .or_else(|| candidates.first())
                    .copied()
                    .map(|(p, i)| (p, i, None))
            };
            let Some((p, i, rev)) = chosen else {
                push_put(&mut out, shot);
                continue;
            };
            let edit = self.shots(p).get(i).and_then(|source| diff(source, shot));
            if matches!(edit, Some(Edit::Set(_))) {
                // Not an object any more: nothing to share with the source.
                if let Some(run) = open.take() {
                    out.push({
                        let shots = self.shots(run.sec);
                        run.finish(shots, target)
                    });
                }
                push_put(&mut out, shot);
                continue;
            }
            match open.as_mut() {
                Some(run) if extended.is_some() => run.extend(rev),
                _ => {
                    open = Some(OpenCopy {
                        sec: p,
                        at: i,
                        n: 1,
                        rev: None,
                        t0: t,
                    });
                }
            }
        }
        if let Some(run) = open {
            out.push({
                let shots = self.shots(run.sec);
                run.finish(shots, target)
            });
        }
        out
    }
}

fn push_put(out: &mut Vec<ShotRun>, shot: &Value) {
    if let Some(ShotRun::Put(values)) = out.last_mut() {
        values.push(shot.clone());
        return;
    }
    out.push(ShotRun::Put(vec![shot.clone()]));
}

/// A copy run being extended shot by shot; its edits are worked out when it
/// is finished.
struct OpenCopy {
    sec: usize,
    at: usize,
    n: usize,
    /// `None` until the second shot fixes the direction.
    rev: Option<bool>,
    /// Where the run starts in the target.
    t0: usize,
}

impl OpenCopy {
    /// The source shot that would continue this run, if it is a candidate,
    /// with the direction that continuing it fixes.
    fn continuation(&self, candidates: &[(usize, usize)]) -> Option<(usize, usize, Option<bool>)> {
        let up = self.at + self.n;
        let down = self.at.checked_sub(self.n);
        if self.rev != Some(true) && candidates.contains(&(self.sec, up)) {
            return Some((self.sec, up, Some(false)));
        }
        if self.rev != Some(false) {
            if let Some(down) = down.filter(|d| candidates.contains(&(self.sec, *d))) {
                return Some((self.sec, down, Some(true)));
            }
        }
        None
    }

    fn extend(&mut self, rev: Option<bool>) {
        if self.rev.is_none() {
            self.rev = rev;
        }
        self.n += 1;
    }

    /// The finished run over `source` (the copied section's shots) towards
    /// `target`. A run copied in reverse is a reversed section, so its copies
    /// are also tried turned around ([`flip_shot`], with and without the
    /// left-right swap): whichever leaves the fewest edits is kept.
    fn finish(self, source: &[Value], target: &[Value]) -> ShotRun {
        let rev = self.rev == Some(true);
        let mods_for = |flip: Option<bool>| -> Vec<(usize, Edit)> {
            (0..self.n)
                .filter_map(|k| {
                    let index = if rev {
                        self.at.checked_sub(k)?
                    } else {
                        self.at + k
                    };
                    let from = source.get(index)?;
                    let to = target.get(self.t0 + k)?;
                    let edit = match flip {
                        Some(swap_lr) => diff(&flip_shot(from, swap_lr), to),
                        None => diff(from, to),
                    };
                    Some((k, edit?))
                })
                .collect()
        };
        let candidates: &[Option<bool>] = if rev {
            &[None, Some(false), Some(true)]
        } else {
            &[None]
        };
        let (flip, m) = candidates
            .iter()
            .map(|flip| (*flip, mods_for(*flip)))
            .min_by_key(|(_, mods)| {
                serde_json::to_string(mods).map_or(usize::MAX, |json| json.len())
            })
            .unwrap_or((None, Vec::new()));
        ShotRun::Copy {
            sec: self.sec,
            at: self.at,
            n: self.n,
            rev,
            flip,
            m,
        }
    }
}

pub(super) fn build_shots(
    source: &BTreeMap<usize, Value>,
    runs: &[ShotRun],
) -> Result<Vec<Value>, DeltaError> {
    let mut out = Vec::new();
    for run in runs {
        match run {
            ShotRun::Copy {
                sec,
                at,
                n,
                rev,
                flip,
                m,
            } => {
                let shots = source.get(sec).and_then(shots_array).ok_or_else(|| {
                    format!("a shot run copies from section {sec}, past the source")
                })?;
                let mut mods = m.iter().peekable();
                for k in 0..*n {
                    let index = if *rev {
                        at.checked_sub(k)
                    } else {
                        at.checked_add(k)
                    };
                    let Some(shot) = index.and_then(|i| shots.get(i)) else {
                        return Err(format!(
                            "a shot run copies {n} shots from {at} of section {sec}, past its {}",
                            shots.len()
                        ));
                    };
                    let flipped;
                    let base = match flip {
                        Some(swap_lr) => {
                            flipped = flip_shot(shot, *swap_lr);
                            &flipped
                        }
                        None => shot,
                    };
                    match mods.next_if(|(offset, _)| *offset == k) {
                        Some((_, edit)) => out.push(apply_edit(base, edit)?),
                        None => out.push(base.clone()),
                    }
                }
                if mods.next().is_some() {
                    return Err("a shot run edits a copy past its end".to_string());
                }
            }
            ShotRun::Put(values) => out.extend(values.iter().cloned()),
        }
    }
    Ok(out)
}
