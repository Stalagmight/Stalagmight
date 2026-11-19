//! Shot runs: how a delta rebuilds a line's shots out of the source's,
//! copying by uuid from any line, in either direction, turned around when
//! the line was reversed.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Number, Value};

use super::json::{apply_edit, diff};
use super::{shots_array, DeltaError, Edit, ShotRun, ID, SHOTS, UUID};

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

    /// The longest key a stored shot table may name: past every member name
    /// a shot has, and short enough that copying the keys into each row costs
    /// no more than parsing the same bytes as plain JSON would.
    const MAX_KEY_BYTES: usize = 64;

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Value>, D::Error> {
        let chunks = Vec::<Chunk>::deserialize(deserializer)?;
        let mut out = Vec::new();
        for chunk in chunks {
            match chunk {
                Chunk::One(value) => out.push(value),
                Chunk::Table { k, r } => {
                    // Every row copies the keys, so a long key would let a
                    // few bytes per row expand without bound. Shot members
                    // are short names; rows already match their key count.
                    if k.iter().any(|key| key.len() > MAX_KEY_BYTES) {
                        return Err(serde::de::Error::custom(
                            "a shot table names a key longer than any shot member",
                        ));
                    }
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

/// One shot turned around the way Reverse Line turns it
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
/// new line needs it.
///
/// Each source shot is copied at most once, and never out of a line the
/// target keeps in place ([`Self::reserve`]): a shot matched a second time (two
/// shots sharing a uuid) is written literally instead. That is what lets an
/// apply refuse any delta that copies a source shot twice
/// ([`super::ProjectDelta::check_shape`]), so an applied delta is never larger
/// than its source plus the literals it carries.
pub(super) struct ShotPool<'a> {
    lines: &'a BTreeMap<usize, Value>,
    by_uuid: Option<HashMap<&'a str, Vec<(usize, usize)>>>,
    used: HashSet<(usize, usize)>,
}

impl<'a> ShotPool<'a> {
    pub(super) fn new(lines: &'a BTreeMap<usize, Value>) -> Self {
        Self {
            lines,
            by_uuid: None,
            used: HashSet::new(),
        }
    }

    /// Take every shot of the source line at `line` out of the pool: the
    /// target keeps them where they are.
    pub(super) fn reserve(&mut self, line: usize) {
        let count = self.shots(line).len();
        self.used.extend((0..count).map(|i| (line, i)));
    }

    fn shots(&self, line: usize) -> &'a [Value] {
        self.lines
            .get(&line)
            .and_then(|s| s.get(SHOTS))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn index(&mut self) -> &HashMap<&'a str, Vec<(usize, usize)>> {
        let lines = self.lines;
        self.by_uuid.get_or_insert_with(|| {
            let mut index: HashMap<&'a str, Vec<(usize, usize)>> = HashMap::new();
            for (&p, line) in lines {
                let shots = line
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
    /// the source position of the line being rebuilt, preferred when a
    /// uuid is held in several places.
    pub(super) fn rebuild(&mut self, target: &[Value], home: Option<usize>) -> Vec<ShotRun> {
        let mut out = Vec::new();
        let mut open: Option<OpenCopy> = None;
        for (t, shot) in target.iter().enumerate() {
            let mut candidates: Vec<(usize, usize)> = shot
                .get(UUID)
                .and_then(Value::as_str)
                .and_then(|uuid| self.index().get(uuid))
                .cloned()
                .unwrap_or_default();
            candidates.retain(|position| !self.used.contains(position));
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
            if !matches!(edit, Some(Edit::Set(_))) {
                self.used.insert((p, i));
            }
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

    /// The finished run over `source` (the copied line's shots) towards
    /// `target`. A run copied in reverse is a reversed line, so its copies
    /// are also tried turned around ([`flip_shot`], with and without the
    /// left-right swap), and a run whose target ids step by one is also tried
    /// renumbered ([`id_run`]): whichever leaves the fewest bytes is kept.
    fn finish(self, source: &[Value], target: &[Value]) -> ShotRun {
        let rev = self.rev == Some(true);
        let mods_for = |flip: Option<bool>, ids: Option<(u64, i8)>| -> Vec<(usize, Edit)> {
            (0..self.n)
                .filter_map(|k| {
                    let index = if rev {
                        self.at.checked_sub(k)?
                    } else {
                        self.at + k
                    };
                    let from = source.get(index)?;
                    let to = target.get(self.t0 + k)?;
                    let base = copy_base(from, flip, ids.and_then(|ids| run_id(ids, k)));
                    Some((k, diff(&base, to)?))
                })
                .collect()
        };
        let flips: &[Option<bool>] = if rev {
            &[None, Some(false), Some(true)]
        } else {
            &[None]
        };
        let renumbered = id_run(target.get(self.t0..self.t0 + self.n).unwrap_or(&[]));
        let ids_options: Vec<Option<(u64, i8)>> =
            std::iter::once(None).chain(renumbered.map(Some)).collect();
        let cost = |ids: Option<(u64, i8)>, mods: &Vec<(usize, Edit)>| {
            let ids = ids.map_or(0, |ids| {
                serde_json::to_string(&ids).map_or(usize::MAX, |json| json.len() + 5)
            });
            serde_json::to_string(mods).map_or(usize::MAX, |json| json.len().saturating_add(ids))
        };
        let (flip, ids, m) = flips
            .iter()
            .flat_map(|flip| ids_options.iter().map(move |ids| (*flip, *ids)))
            .map(|(flip, ids)| (flip, ids, mods_for(flip, ids)))
            .min_by_key(|(_, ids, mods)| cost(*ids, mods))
            .unwrap_or((None, None, Vec::new()));
        ShotRun::Copy {
            sec: self.sec,
            at: self.at,
            n: self.n,
            rev,
            flip,
            ids,
            m,
        }
    }
}

/// The ids of `shots` as a run, `(start, step)`, when each is a number one
/// on from the one before (`step` 1) or one back (`step` -1); a single shot's
/// id is a run of one.
fn id_run(shots: &[Value]) -> Option<(u64, i8)> {
    let ids: Vec<u64> = shots
        .iter()
        .map(|shot| shot.get(ID).and_then(Value::as_u64))
        .collect::<Option<_>>()?;
    let (&first, rest) = ids.split_first()?;
    let step: i8 = match rest.first() {
        None => return Some((first, 1)),
        Some(&next) if Some(next) == first.checked_add(1) => 1,
        Some(&next) if Some(next) == first.checked_sub(1) => -1,
        Some(_) => return None,
    };
    let fits = ids
        .iter()
        .enumerate()
        .all(|(k, id)| run_id((first, step), k) == Some(*id));
    fits.then_some((first, step))
}

/// The id copy `k` of a run renumbered by `ids` takes, unless it would
/// leave the `u64` range.
pub(super) fn run_id((start, step): (u64, i8), k: usize) -> Option<u64> {
    let k = u64::try_from(k).ok()?;
    match step {
        1 => start.checked_add(k),
        -1 => start.checked_sub(k),
        _ => None,
    }
}

/// What a copy starts from before its edits: the source shot, turned around
/// when `flip` is set, with id `id` when the run is renumbered.
fn copy_base(shot: &Value, flip: Option<bool>, id: Option<u64>) -> Value {
    let mut base = match flip {
        Some(swap_lr) => flip_shot(shot, swap_lr),
        None => shot.clone(),
    };
    if let (Some(id), Value::Object(members)) = (id, &mut base) {
        members.insert(ID.to_string(), Value::from(id));
    }
    base
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
                ids,
                m,
            } => {
                let shots = source
                    .get(sec)
                    .and_then(shots_array)
                    .ok_or_else(|| format!("a shot run copies from line {sec}, past the source"))?;
                let mut mods = m.iter().peekable();
                for k in 0..*n {
                    let index = if *rev {
                        at.checked_sub(k)
                    } else {
                        at.checked_add(k)
                    };
                    let Some(shot) = index.and_then(|i| shots.get(i)) else {
                        return Err(format!(
                            "a shot run copies {n} shots from {at} of line {sec}, past its {}",
                            shots.len()
                        ));
                    };
                    let id =
                        match ids {
                            Some(ids) => Some(run_id(*ids, k).ok_or_else(|| {
                                "a shot run numbers a copy past any id".to_string()
                            })?),
                            None => None,
                        };
                    let base = copy_base(shot, *flip, id);
                    match mods.next_if(|(offset, _)| *offset == k) {
                        Some((_, edit)) => out.push(apply_edit(&base, edit)?),
                        None => out.push(base),
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
