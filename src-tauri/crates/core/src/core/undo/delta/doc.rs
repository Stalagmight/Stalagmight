//! Deltas over serialized TEXT: a project split at its top level, every part
//! kept as the raw JSON it was written as, and parsed only where two states
//! differ.
//!
//! A step through an undo entry serializes the whole live project once, as
//! every snapshot restore always has. What it must not do is build a JSON tree
//! of the whole project, clone it, and walk it twice more to find the handful
//! of lines the entry touches: on a 20,000-shot survey that tree work was
//! four times the cost of the restore itself. Here an untouched line is a
//! borrowed slice of the text, compared by `memcmp` and copied by pointer.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::json::{apply_edit, apply_members, diff_member, diff_members, myers, same};
use super::shots::{build_shots, ShotPool};
use super::stations::{self, built_line_id, raw_line_id, LineText, Stations, STATION_KEYED};
use super::{
    line_id_of, line_id_of_map, shots_array, DeltaError, Edit, LineRun, LiteralShot, ProjectDelta,
    GAUGE_NEIGHBOUR, GAUGE_SHOT, ID, LINES, NEXT_LINE_ID, NEXT_SHOT_ID, SHOTS,
};
use crate::models::{LineId, FEATURE_ID_STRIDE};

/// A serialized project split at its top level, each part still raw JSON.
#[derive(Debug, Clone)]
pub struct Doc<'a> {
    /// The top-level members other than `lines`.
    members: BTreeMap<String, Cow<'a, str>>,
    /// The lines, in order.
    lines: Vec<Cow<'a, str>>,
}

fn parse_value(raw: &str) -> Result<Value, DeltaError> {
    serde_json::from_str(raw).map_err(|e| e.to_string())
}

fn to_text(value: &Value) -> Result<String, DeltaError> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

impl<'a> Doc<'a> {
    /// Split `text`, a serialized project, without parsing its parts.
    pub fn parse(text: &'a str) -> Result<Self, DeltaError> {
        let mut members: BTreeMap<String, &'a RawValue> = serde_json::from_str(text)
            .map_err(|e| format!("a project snapshot is a JSON object: {e}"))?;
        let lines = match members.remove(LINES) {
            None => Vec::new(),
            Some(raw) => serde_json::from_str::<Vec<&'a RawValue>>(raw.get())
                .map_err(|e| format!("a project snapshot's lines are an array: {e}"))?
                .into_iter()
                .map(|line| Cow::Borrowed(line.get()))
                .collect(),
        };
        Ok(Self {
            members: members
                .into_iter()
                .map(|(key, raw)| (key, Cow::Borrowed(raw.get())))
                .collect(),
            lines,
        })
    }

    /// The project as text again.
    pub fn text(&self) -> String {
        let size = self
            .members
            .values()
            .map(|raw| raw.len() + 24)
            .sum::<usize>()
            + self.lines.iter().map(|raw| raw.len() + 1).sum::<usize>()
            + 16;
        let mut out = String::with_capacity(size);
        out.push('{');
        for (key, raw) in &self.members {
            // A key is a plain identifier, but write it the way JSON must.
            out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| format!("\"{key}\"")));
            out.push(':');
            out.push_str(raw);
            out.push(',');
        }
        out.push_str("\"lines\":[");
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(line);
        }
        out.push_str("]}");
        out
    }

    /// How many lines the project holds.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// The shot ids one line of a SOURCE holds, by position: what an apply
/// reads to keep ids unique without parsing the lines it copies verbatim.
/// Built from the live project itself in the undo path.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LineIds {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub next_shot_id: Option<u64>,
    #[serde(default, deserialize_with = "shot_ids")]
    pub shots: Vec<u64>,
}

fn shot_ids<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<u64>, D::Error> {
    #[derive(Deserialize)]
    struct ShotId {
        #[serde(default)]
        id: Option<u64>,
    }
    Ok(Vec::<ShotId>::deserialize(deserializer)?
        .into_iter()
        .filter_map(|shot| shot.id)
        .collect())
}

impl LineIds {
    /// The table for `project`, line by line in its order.
    pub fn of_project(project: &crate::models::Project) -> Vec<Self> {
        project
            .lines
            .iter()
            .map(|line| Self {
                id: Some(u64::from(line.id)),
                next_shot_id: Some(u64::from(line.next_shot_id)),
                shots: line.shots.iter().map(|shot| u64::from(shot.id)).collect(),
            })
            .collect()
    }

    /// The table for `doc`, read out of its text (a test, or a caller with no
    /// typed project at hand).
    pub fn of_doc(doc: &Doc<'_>) -> Result<Vec<Self>, DeltaError> {
        doc.lines
            .iter()
            .map(|raw| serde_json::from_str(raw).map_err(|e| e.to_string()))
            .collect()
    }

    fn of_value(line: &Value) -> Self {
        Self {
            id: line_id_of(line),
            next_shot_id: line.get(NEXT_SHOT_ID).and_then(Value::as_u64),
            shots: shots_array(line)
                .into_iter()
                .flatten()
                .filter_map(|shot| shot.get(ID).and_then(Value::as_u64))
                .collect(),
        }
    }
}

// === Encode ===

fn text_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The delta from `source` to `target`.
pub(super) fn encode(source: &Doc<'_>, target: &Doc<'_>) -> Result<ProjectDelta, DeltaError> {
    let mut fields = BTreeMap::new();
    for (key, raw) in &source.members {
        match target.members.get(key) {
            None => {
                fields.insert(key.clone(), Edit::Del);
            }
            Some(other) if other == raw => {}
            Some(other) => {
                if let Some(edit) = diff_member(key, &parse_value(raw)?, &parse_value(other)?) {
                    fields.insert(key.clone(), edit);
                }
            }
        }
    }
    for (key, raw) in &target.members {
        if !source.members.contains_key(key) {
            fields.insert(key.clone(), Edit::Set(parse_value(raw)?));
        }
    }
    positional_station_keys(&mut fields, source, target);
    let lines = encode_lines(&source.lines, &target.lines)?.map(|runs| (source.lines.len(), runs));
    let delta = ProjectDelta { fields, lines };
    // The encoder keeps every rule an apply checks; one it broke would only
    // be refused later, at an undo, so refuse it here instead.
    delta.check_shape()?;
    Ok(delta)
}

/// The lines of `doc` by id, for the station-keyed members' positions.
fn doc_stations<'s>(doc: &'s Doc<'_>) -> Stations<'s> {
    Stations::new(
        doc.lines
            .iter()
            .map(|raw| (raw_line_id(raw), LineText::Raw(raw))),
    )
}

/// Write the station-keyed members' pairs as positions where they name a
/// station of their state (`delta/stations.rs`). The lines are only read
/// when such a member changed.
fn positional_station_keys(
    fields: &mut BTreeMap<String, Edit>,
    source: &Doc<'_>,
    target: &Doc<'_>,
) {
    let keyed: Vec<String> = fields
        .iter()
        .filter(|(key, edit)| {
            STATION_KEYED.contains(&key.as_str()) && matches!(edit, Edit::Bag { .. })
        })
        .map(|(key, _)| key.clone())
        .collect();
    if keyed.is_empty() {
        return;
    }
    let mut from = doc_stations(source);
    let mut to = doc_stations(target);
    for key in keyed {
        if let Some(edit) = fields.remove(&key) {
            fields.insert(key, stations::positional(edit, &mut from, &mut to));
        }
    }
}

/// For every target line, the source line whose TEXT it repeats, when
/// one does: the common prefix and suffix, and a shortest edit script over
/// the middle. Equal text is equal state, so nothing matched here is parsed.
fn match_identical(source: &[Cow<'_, str>], target: &[Cow<'_, str>]) -> Vec<Option<usize>> {
    let mut matched = vec![None; target.len()];
    let mut prefix = 0;
    while prefix < source.len() && prefix < target.len() && source[prefix] == target[prefix] {
        matched[prefix] = Some(prefix);
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < source.len() - prefix
        && suffix < target.len() - prefix
        && source[source.len() - 1 - suffix] == target[target.len() - 1 - suffix]
    {
        matched[target.len() - 1 - suffix] = Some(source.len() - 1 - suffix);
        suffix += 1;
    }
    let a = &source[prefix..source.len() - suffix];
    let b = &target[prefix..target.len() - suffix];
    let ha: Vec<u64> = a.iter().map(|raw| text_hash(raw)).collect();
    let hb: Vec<u64> = b.iter().map(|raw| text_hash(raw)).collect();
    if let Some(pairs) = myers(a.len(), b.len(), |x, y| ha[x] == hb[y] && a[x] == b[y]) {
        for (x, y) in pairs {
            matched[prefix + y] = Some(prefix + x);
        }
    }
    matched
}

fn push_keep(runs: &mut Vec<LineRun>, at: usize) {
    if let Some(LineRun::Keep(start, n)) = runs.last_mut() {
        if *start + *n == at {
            *n += 1;
            return;
        }
    }
    runs.push(LineRun::Keep(at, 1));
}

fn encode_lines(
    source: &[Cow<'_, str>],
    target: &[Cow<'_, str>],
) -> Result<Option<Vec<LineRun>>, DeltaError> {
    if source.len() == target.len() && source.iter().zip(target).all(|(a, b)| a == b) {
        return Ok(None);
    }
    let matched = match_identical(source, target);
    let hit: HashSet<usize> = matched.iter().flatten().copied().collect();
    // Only the lines the text could not match are parsed: the changed ones.
    let mut parsed: BTreeMap<usize, Value> = BTreeMap::new();
    for (p, raw) in source.iter().enumerate() {
        if !hit.contains(&p) {
            parsed.insert(p, parse_value(raw)?);
        }
    }
    let mut by_id: HashMap<u64, usize> = HashMap::new();
    for (p, line) in &parsed {
        if let Some(id) = line_id_of(line) {
            by_id.entry(id).or_insert(*p);
        }
    }
    // Every target line's plan first, so the pool knows which source
    // lines keep their shots in place before any shot is copied out of one.
    let mut plans: Vec<Plan> = Vec::with_capacity(target.len());
    for (j, raw) in target.iter().enumerate() {
        if let Some(at) = matched[j] {
            plans.push(Plan::Keep(at));
            continue;
        }
        let line = parse_value(raw)?;
        // A source line is read at most once (`check_shape`): a second
        // target line under the same id is written as a new one.
        let at = line_id_of(&line).and_then(|id| by_id.remove(&id));
        plans.push(match at.and_then(|at| Some((at, parsed.get(&at)?))) {
            Some((at, from)) if same(from, &line) => Plan::Keep(at),
            Some((at, _)) => Plan::Edit(at, line),
            None => Plan::New(line),
        });
    }
    let mut pool = ShotPool::new(&parsed);
    for plan in &plans {
        match plan {
            Plan::Keep(at) => pool.reserve(*at),
            Plan::Edit(at, line) => {
                if parsed
                    .get(at)
                    .is_some_and(|from| !rebuilds_shots(from, line))
                {
                    pool.reserve(*at);
                }
            }
            Plan::New(_) => {}
        }
    }
    let mut runs: Vec<LineRun> = Vec::new();
    for plan in plans {
        match plan {
            Plan::Keep(at) => push_keep(&mut runs, at),
            Plan::Edit(at, line) => {
                let Some(from) = parsed.get(&at) else {
                    return Err(format!("line {at} was planned as an edit but not parsed"));
                };
                runs.push(edit_line(from, &line, at, &mut pool));
            }
            Plan::New(line) => runs.push(new_line(&line, &mut pool)),
        }
    }
    Ok(Some(runs))
}

/// What a target line is, over the source.
enum Plan {
    /// The source line at this position, unchanged.
    Keep(usize),
    /// The source line at this position, edited into this value.
    Edit(usize, Value),
    /// A line the source does not hold.
    New(Value),
}

/// Whether [`edit_line`] rebuilds the shots of `from` to make `to`: when
/// both hold a shots array and the arrays differ. Otherwise the line keeps
/// its shots where they are, and no other line may copy one.
fn rebuilds_shots(from: &Value, to: &Value) -> bool {
    match (shots_array(from), shots_array(to)) {
        (Some(a), Some(b)) => a.len() != b.len() || a.iter().zip(b).any(|(x, y)| !same(x, y)),
        _ => false,
    }
}

fn gauge_of(line: &Value) -> (Option<&str>, Option<&str>) {
    (
        line.get(GAUGE_SHOT).and_then(Value::as_str),
        line.get(GAUGE_NEIGHBOUR).and_then(Value::as_str),
    )
}

fn edit_line(from: &Value, to: &Value, at: usize, pool: &mut ShotPool<'_>) -> LineRun {
    let id = line_id_of(from)
        .and_then(|id| LineId::try_from(id).ok())
        .unwrap_or_default();
    let (Value::Object(a), Value::Object(b)) = (from, to) else {
        // Unreachable for a serialized project; keep the result exact anyway.
        return LineRun::New {
            fields: to.as_object().cloned().unwrap_or_default(),
            shots: None,
        };
    };
    let (Some(_), Some(to_shots)) = (shots_array(from), shots_array(to)) else {
        return LineRun::Edit {
            at,
            id,
            fields: diff_members(a, b),
            shots: None,
            ungauged: false,
        };
    };
    let mut fields = diff_members(a, b);
    fields.remove(SHOTS);
    let shots = rebuilds_shots(from, to).then(|| pool.rebuild(to_shots, Some(at)));
    let gauge_untouched = !fields.contains_key(GAUGE_SHOT) && !fields.contains_key(GAUGE_NEIGHBOUR);
    let ungauged = shots.is_some() && gauge_untouched && gauge_of(from).0.is_none();
    LineRun::Edit {
        at,
        id,
        fields,
        shots,
        ungauged,
    }
}

fn new_line(to: &Value, pool: &mut ShotPool<'_>) -> LineRun {
    let mut fields = to.as_object().cloned().unwrap_or_default();
    let shots = match fields.get(SHOTS) {
        Some(Value::Array(shots)) => {
            let runs = pool.rebuild(shots, None);
            fields.remove(SHOTS);
            Some(runs)
        }
        _ => None,
    };
    LineRun::New { fields, shots }
}

// === Apply ===

/// One line of an applied delta: the source's text where the delta kept
/// it, the rebuilt value where it did not.
#[derive(Debug)]
pub enum LineOut<'a> {
    Kept { raw: Cow<'a, str>, from: usize },
    Built(Value),
}

/// A delta applied to a [`Doc`]: what [`Applied::into_doc`] turns into the
/// target's text, with the rebuilt lines still open for the caller.
#[derive(Debug)]
pub struct Applied<'a> {
    members: BTreeMap<String, Cow<'a, str>>,
    lines: Vec<LineOut<'a>>,
}

impl<'a> Applied<'a> {
    /// The lines the delta rebuilt, to adjust before they are written.
    pub fn built_lines_mut(&mut self) -> Vec<&mut Value> {
        self.lines
            .iter_mut()
            .filter_map(|line| match line {
                LineOut::Built(value) => Some(value),
                LineOut::Kept { .. } => None,
            })
            .collect()
    }

    /// The target, split as a [`Doc`] (the rebuilt lines written out).
    pub fn into_doc(self) -> Result<Doc<'a>, DeltaError> {
        let lines = self
            .lines
            .into_iter()
            .map(|line| match line {
                LineOut::Kept { raw, .. } => Ok(raw),
                LineOut::Built(value) => to_text(&value).map(Cow::Owned),
            })
            .collect::<Result<_, _>>()?;
        Ok(Doc {
            members: self.members,
            lines,
        })
    }
}

/// `delta` applied to `source`, whose lines' ids `ids` lists in order.
pub(super) fn apply<'a>(
    delta: &ProjectDelta,
    source: &Doc<'a>,
    ids: &[LineIds],
) -> Result<Applied<'a>, DeltaError> {
    delta.check_shape()?;
    if ids.len() != source.lines.len() {
        return Err(format!(
            "the id table lists {} lines, the source holds {}",
            ids.len(),
            source.lines.len()
        ));
    }
    let mut lines = match &delta.lines {
        None => source
            .lines
            .iter()
            .enumerate()
            .map(|(from, raw)| LineOut::Kept {
                raw: raw.clone(),
                from,
            })
            .collect(),
        Some((count, runs)) => {
            if *count != source.lines.len() {
                return Err(format!(
                    "the delta was recorded over {count} lines, the source holds {}",
                    source.lines.len()
                ));
            }
            build_lines(source, runs)?
        }
    };
    // The members after the lines: a station-keyed pair written as a
    // position is rebuilt from the target's shots.
    let members = apply_fields(delta, source, ids, &lines)?;
    let mut members = members;
    let literal = literal_shots(delta);
    settle_ids(&mut lines, ids, &literal)?;
    settle_line_counter(&mut members, &lines, ids);
    Ok(Applied { members, lines })
}

/// The source's top-level members with `delta`'s edits applied. `lines` is the
/// target's, for the station-keyed members' positions (read only when one is
/// written as a position).
fn apply_fields<'a>(
    delta: &ProjectDelta,
    source: &Doc<'a>,
    ids: &[LineIds],
    lines: &[LineOut<'a>],
) -> Result<BTreeMap<String, Cow<'a, str>>, DeltaError> {
    let line_id = |ids: &LineIds| ids.id.and_then(|id| LineId::try_from(id).ok());
    let mut source_stations: Option<Stations<'_>> = None;
    let mut target_stations: Option<Stations<'_>> = None;
    let mut members = source.members.clone();
    for (key, edit) in &delta.fields {
        if key == LINES {
            return Err("an edit of the whole lines member".to_string());
        }
        let positional = matches!(
            edit,
            Edit::Bag { station_del, station_add, .. }
                if !station_del.is_empty() || !station_add.is_empty()
        );
        let expanded;
        let edit = if positional {
            let from = source_stations.get_or_insert_with(|| {
                Stations::new(
                    source
                        .lines
                        .iter()
                        .zip(ids)
                        .map(|(raw, ids)| (line_id(ids), LineText::Raw(raw))),
                )
            });
            let to = target_stations.get_or_insert_with(|| {
                Stations::new(lines.iter().map(|line| match line {
                    LineOut::Kept { raw, from } => {
                        (ids.get(*from).and_then(line_id), LineText::Raw(raw))
                    }
                    LineOut::Built(value) => (built_line_id(value), LineText::Built(value)),
                }))
            });
            expanded = stations::literal(edit, from, to)?;
            &expanded
        } else {
            edit
        };
        match (members.get(key), edit) {
            (Some(_), Edit::Del) => {
                members.remove(key);
            }
            (Some(raw), edit) => {
                let value = apply_edit(&parse_value(raw)?, edit)?;
                members.insert(key.clone(), Cow::Owned(to_text(&value)?));
            }
            (None, Edit::Set(value)) => {
                members.insert(key.clone(), Cow::Owned(to_text(value)?));
            }
            (None, Edit::Del) => {}
            // A member serialized only when non-empty (`counter_parity_shots`,
            // `held_gauges`) is absent once a save or a load emptied it: edit the
            // empty value it stands for.
            (None, edit @ (Edit::Bag { .. } | Edit::Obj(_))) => {
                let empty = if matches!(edit, Edit::Obj(_)) {
                    Value::Object(serde_json::Map::new())
                } else {
                    Value::Array(Vec::new())
                };
                let value = apply_edit(&empty, edit)?;
                members.insert(key.clone(), Cow::Owned(to_text(&value)?));
            }
            (None, _) => {
                return Err(format!(
                    "an edit names member {key:?}, which the source lacks"
                ))
            }
        }
    }
    Ok(members)
}

fn build_lines<'a>(source: &Doc<'a>, runs: &[LineRun]) -> Result<Vec<LineOut<'a>>, DeltaError> {
    // Parse exactly the source lines the runs read: the ones they edit and
    // the ones they copy shots out of.
    let mut needed: HashSet<usize> = HashSet::new();
    for run in runs {
        let shots = match run {
            LineRun::Keep(..) => continue,
            LineRun::Edit { at, shots, .. } => {
                needed.insert(*at);
                shots
            }
            LineRun::New { shots, .. } => shots,
        };
        for shot_run in shots.iter().flatten() {
            if let super::ShotRun::Copy { sec, .. } = shot_run {
                needed.insert(*sec);
            }
        }
    }
    let mut parsed: BTreeMap<usize, Value> = BTreeMap::new();
    for p in needed {
        let Some(raw) = source.lines.get(p) else {
            return Err(format!("a line run reads line {p}, past the source"));
        };
        parsed.insert(p, parse_value(raw)?);
    }
    let mut out: Vec<LineOut<'a>> = Vec::with_capacity(source.lines.len());
    for run in runs {
        match run {
            LineRun::Keep(at, len) => {
                let end = at
                    .checked_add(*len)
                    .filter(|end| *end <= source.lines.len());
                let Some(end) = end else {
                    return Err(format!(
                        "a line run keeps {len} lines from {at}, past the source's {}",
                        source.lines.len()
                    ));
                };
                for from in *at..end {
                    out.push(LineOut::Kept {
                        raw: source.lines[from].clone(),
                        from,
                    });
                }
            }
            LineRun::Edit {
                at,
                id,
                fields,
                shots,
                ..
            } => {
                let Some(Value::Object(base)) = parsed.get(at) else {
                    return Err(format!("a line run edits line {at}, past the source"));
                };
                if line_id_of_map(base) != Some(*id) {
                    return Err(format!(
                        "a line run edits line {id} at {at}, which holds another line"
                    ));
                }
                let skip = shots.as_ref().map(|_| SHOTS);
                let mut line = apply_members(base, fields, skip)?;
                if let Some(shot_runs) = shots {
                    let built = build_shots(&parsed, shot_runs)?;
                    line.insert(SHOTS.to_string(), Value::Array(built));
                }
                out.push(LineOut::Built(Value::Object(line)));
            }
            LineRun::New { fields, shots } => {
                let mut line = fields.clone();
                if let Some(shot_runs) = shots {
                    let built = build_shots(&parsed, shot_runs)?;
                    line.insert(SHOTS.to_string(), Value::Array(built));
                }
                out.push(LineOut::Built(Value::Object(line)));
            }
        }
    }
    Ok(out)
}

/// Where the shots the delta carried literally landed: (line, shot) in
/// the output. A run's literal shots are its `Put` runs, in order.
fn literal_shots(delta: &ProjectDelta) -> Vec<LiteralShot> {
    let Some((_, runs)) = &delta.lines else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut s = 0;
    for run in runs {
        let shots = match run {
            LineRun::Keep(_, len) => {
                s += len;
                continue;
            }
            LineRun::Edit { shots, .. } | LineRun::New { shots, .. } => shots,
        };
        if let Some(shot_runs) = shots {
            let mut i = 0;
            for shot_run in shot_runs {
                match shot_run {
                    super::ShotRun::Copy { n, .. } => i += n,
                    super::ShotRun::Put(values) => {
                        for _ in values {
                            out.push((s, i));
                            i += 1;
                        }
                    }
                }
            }
        }
        s += 1;
    }
    out
}

/// Make every shot id unique and every line's allocator safe, after a
/// rebuild that put `literal` shots back with their recorded ids.
///
/// The loader renumbers every shot (`line_id * FEATURE_ID_STRIDE +
/// sequence_order`), so a shot recorded before a reopen can carry an id that a
/// renumbered live shot now holds. Copied shots keep their live ids; a literal
/// whose id is taken (or a second copy of one) takes a fresh one from its
/// line's allocator. Each allocator is then raised past every id in its
/// window that any line holds, so the next shot it hands out cannot collide
/// either. A state recorded in this session is already consistent, and then
/// nothing here changes a byte - and a kept line is never even parsed.
fn settle_ids(
    lines: &mut [LineOut<'_>],
    ids: &[LineIds],
    literal: &[LiteralShot],
) -> Result<(), DeltaError> {
    let literal: HashSet<LiteralShot> = literal.iter().copied().collect();
    let mut taken: HashSet<u64> = HashSet::new();
    let mut clash: Vec<LiteralShot> = Vec::new();
    // Copies (and kept lines) first, so a literal is what yields.
    for pass_literal in [false, true] {
        for (s, line) in lines.iter().enumerate() {
            match line {
                LineOut::Kept { from, .. } => {
                    if !pass_literal {
                        taken.extend(ids[*from].shots.iter().copied());
                    }
                }
                LineOut::Built(value) => {
                    for (i, shot) in shots_array(value).into_iter().flatten().enumerate() {
                        if literal.contains(&(s, i)) != pass_literal {
                            continue;
                        }
                        if let Some(id) = shot.get(ID).and_then(Value::as_u64) {
                            if !taken.insert(id) {
                                clash.push((s, i));
                            }
                        }
                    }
                }
            }
        }
    }
    let stride = u64::from(FEATURE_ID_STRIDE);
    let mut window_max: HashMap<u64, u64> = HashMap::new();
    for id in &taken {
        let slot = window_max.entry(id / stride).or_insert(*id);
        *slot = (*slot).max(*id);
    }
    for line in lines.iter_mut() {
        let current = match line {
            LineOut::Kept { from, .. } => ids[*from].clone(),
            LineOut::Built(value) => LineIds::of_value(value),
        };
        let (Some(id), Some(next)) = (current.id, current.next_shot_id) else {
            continue;
        };
        let Some(max) = window_max.get(&id).copied() else {
            continue;
        };
        if next > max {
            continue;
        }
        // Rare: only a state recorded before a reopen gets here. A kept
        // line has to be opened to have its allocator raised.
        if let LineOut::Kept { raw, .. } = line {
            *line = LineOut::Built(parse_value(raw)?);
        }
        if let LineOut::Built(Value::Object(members)) = line {
            members.insert(NEXT_SHOT_ID.to_string(), Value::from(max.saturating_add(1)));
        }
    }
    for (s, i) in clash {
        let Some(LineOut::Built(Value::Object(line))) = lines.get_mut(s) else {
            continue;
        };
        let Some(line_id) = line.get(ID).and_then(Value::as_u64) else {
            return Err("a line without an id holds a shot whose id is taken".to_string());
        };
        // The ids come from the file: checked, so a hostile one is refused
        // rather than overflowing.
        let Some(end) = line_id.checked_add(1).and_then(|s| s.checked_mul(stride)) else {
            return Err(format!("line {line_id} is past the shot-id encode bound"));
        };
        let mut next = line
            .get(NEXT_SHOT_ID)
            .and_then(Value::as_u64)
            .unwrap_or(end - stride);
        while next < end && taken.contains(&next) {
            next += 1;
        }
        if next >= end {
            return Err(format!("line {line_id} has no shot id left to give back"));
        }
        taken.insert(next);
        line.insert(NEXT_SHOT_ID.to_string(), Value::from(next + 1));
        if let Some(Value::Object(shot)) = line
            .get_mut(SHOTS)
            .and_then(Value::as_array_mut)
            .and_then(|shots| shots.get_mut(i))
        {
            shot.insert(ID.to_string(), Value::from(next));
        }
    }
    Ok(())
}

/// Raise the project's line allocator past every line the target
/// holds. The loader recomputes it as `max + 1`, so after a reopen the
/// recorded value of a state that held a since-deleted line can be at or
/// below that line's id, and the next new line would take it again.
fn settle_line_counter(
    members: &mut BTreeMap<String, Cow<'_, str>>,
    lines: &[LineOut<'_>],
    ids: &[LineIds],
) {
    let max = lines
        .iter()
        .filter_map(|line| match line {
            LineOut::Kept { from, .. } => ids[*from].id,
            LineOut::Built(value) => line_id_of(value),
        })
        .max();
    let Some(max) = max else {
        return;
    };
    let Some(next) = members
        .get(NEXT_LINE_ID)
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        return;
    };
    if next <= max {
        members.insert(
            NEXT_LINE_ID.to_string(),
            Cow::Owned(max.saturating_add(1).to_string()),
        );
    }
}
