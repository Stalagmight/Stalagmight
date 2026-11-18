//! Deltas over serialized TEXT: a project split at its top level, every part
//! kept as the raw JSON it was written as, and parsed only where two states
//! differ.
//!
//! A step through an undo entry serializes the whole live project once, as
//! every snapshot restore always has. What it must not do is build a JSON tree
//! of the whole project, clone it, and walk it twice more to find the handful
//! of sections the entry touches: on a 20,000-shot survey that tree work was
//! four times the cost of the restore itself. Here an untouched section is a
//! borrowed slice of the text, compared by `memcmp` and copied by pointer.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::json::{apply_edit, apply_members, diff_member, diff_members, myers, same};
use super::shots::{build_shots, ShotPool};
use super::{
    section_id_of, section_id_of_map, shots_array, DeltaError, Edit, LiteralShot, ProjectDelta,
    SectionRun, GAUGE_NEIGHBOUR, GAUGE_SHOT, ID, NEXT_SECTION_ID, NEXT_SHOT_ID, SECTIONS, SHOTS,
};
use crate::models::{SectionId, FEATURE_ID_STRIDE};

/// A serialized project split at its top level, each part still raw JSON.
#[derive(Debug, Clone)]
pub struct Doc<'a> {
    /// The top-level members other than `sections`.
    members: BTreeMap<String, Cow<'a, str>>,
    /// The sections, in order.
    sections: Vec<Cow<'a, str>>,
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
        let sections = match members.remove(SECTIONS) {
            None => Vec::new(),
            Some(raw) => serde_json::from_str::<Vec<&'a RawValue>>(raw.get())
                .map_err(|e| format!("a project snapshot's sections are an array: {e}"))?
                .into_iter()
                .map(|section| Cow::Borrowed(section.get()))
                .collect(),
        };
        Ok(Self {
            members: members
                .into_iter()
                .map(|(key, raw)| (key, Cow::Borrowed(raw.get())))
                .collect(),
            sections,
        })
    }

    /// The project as text again.
    pub fn text(&self) -> String {
        let size = self
            .members
            .values()
            .map(|raw| raw.len() + 24)
            .sum::<usize>()
            + self.sections.iter().map(|raw| raw.len() + 1).sum::<usize>()
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
        out.push_str("\"sections\":[");
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(section);
        }
        out.push_str("]}");
        out
    }

    /// How many sections the project holds.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
}

/// The shot ids one section of a SOURCE holds, by position: what an apply
/// reads to keep ids unique without parsing the sections it copies verbatim.
/// Built from the live project itself in the undo path.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SectionIds {
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

impl SectionIds {
    /// The table for `project`, section by section in its order.
    pub fn of_project(project: &crate::models::Project) -> Vec<Self> {
        project
            .sections
            .iter()
            .map(|section| Self {
                id: Some(u64::from(section.id)),
                next_shot_id: Some(u64::from(section.next_shot_id)),
                shots: section
                    .shots
                    .iter()
                    .map(|shot| u64::from(shot.id))
                    .collect(),
            })
            .collect()
    }

    /// The table for `doc`, read out of its text (a test, or a caller with no
    /// typed project at hand).
    pub fn of_doc(doc: &Doc<'_>) -> Result<Vec<Self>, DeltaError> {
        doc.sections
            .iter()
            .map(|raw| serde_json::from_str(raw).map_err(|e| e.to_string()))
            .collect()
    }

    fn of_value(section: &Value) -> Self {
        Self {
            id: section_id_of(section),
            next_shot_id: section.get(NEXT_SHOT_ID).and_then(Value::as_u64),
            shots: shots_array(section)
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
    let sections = encode_sections(&source.sections, &target.sections)?
        .map(|runs| (source.sections.len(), runs));
    let delta = ProjectDelta { fields, sections };
    // The encoder keeps every rule an apply checks; one it broke would only
    // be refused later, at an undo, so refuse it here instead.
    delta.check_shape()?;
    Ok(delta)
}

/// For every target section, the source section whose TEXT it repeats, when
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

fn push_keep(runs: &mut Vec<SectionRun>, at: usize) {
    if let Some(SectionRun::Keep(start, n)) = runs.last_mut() {
        if *start + *n == at {
            *n += 1;
            return;
        }
    }
    runs.push(SectionRun::Keep(at, 1));
}

fn encode_sections(
    source: &[Cow<'_, str>],
    target: &[Cow<'_, str>],
) -> Result<Option<Vec<SectionRun>>, DeltaError> {
    if source.len() == target.len() && source.iter().zip(target).all(|(a, b)| a == b) {
        return Ok(None);
    }
    let matched = match_identical(source, target);
    let hit: HashSet<usize> = matched.iter().flatten().copied().collect();
    // Only the sections the text could not match are parsed: the changed ones.
    let mut parsed: BTreeMap<usize, Value> = BTreeMap::new();
    for (p, raw) in source.iter().enumerate() {
        if !hit.contains(&p) {
            parsed.insert(p, parse_value(raw)?);
        }
    }
    let mut by_id: HashMap<u64, usize> = HashMap::new();
    for (p, section) in &parsed {
        if let Some(id) = section_id_of(section) {
            by_id.entry(id).or_insert(*p);
        }
    }
    // Every target section's plan first, so the pool knows which source
    // sections keep their shots in place before any shot is copied out of one.
    let mut plans: Vec<Plan> = Vec::with_capacity(target.len());
    for (j, raw) in target.iter().enumerate() {
        if let Some(at) = matched[j] {
            plans.push(Plan::Keep(at));
            continue;
        }
        let section = parse_value(raw)?;
        let at = section_id_of(&section).and_then(|id| by_id.get(&id).copied());
        plans.push(match at.and_then(|at| Some((at, parsed.get(&at)?))) {
            Some((at, from)) if same(from, &section) => Plan::Keep(at),
            Some((at, _)) => Plan::Edit(at, section),
            None => Plan::New(section),
        });
    }
    let mut pool = ShotPool::new(&parsed);
    for plan in &plans {
        match plan {
            Plan::Keep(at) => pool.reserve(*at),
            Plan::Edit(at, section) => {
                if parsed
                    .get(at)
                    .is_some_and(|from| !rebuilds_shots(from, section))
                {
                    pool.reserve(*at);
                }
            }
            Plan::New(_) => {}
        }
    }
    let mut runs: Vec<SectionRun> = Vec::new();
    for plan in plans {
        match plan {
            Plan::Keep(at) => push_keep(&mut runs, at),
            Plan::Edit(at, section) => {
                let Some(from) = parsed.get(&at) else {
                    return Err(format!(
                        "section {at} was planned as an edit but not parsed"
                    ));
                };
                runs.push(edit_section(from, &section, at, &mut pool));
            }
            Plan::New(section) => runs.push(new_section(&section, &mut pool)),
        }
    }
    Ok(Some(runs))
}

/// What a target section is, over the source.
enum Plan {
    /// The source section at this position, unchanged.
    Keep(usize),
    /// The source section at this position, edited into this value.
    Edit(usize, Value),
    /// A section the source does not hold.
    New(Value),
}

/// Whether [`edit_section`] rebuilds the shots of `from` to make `to`: when
/// both hold a shots array and the arrays differ. Otherwise the section keeps
/// its shots where they are, and no other section may copy one.
fn rebuilds_shots(from: &Value, to: &Value) -> bool {
    match (shots_array(from), shots_array(to)) {
        (Some(a), Some(b)) => a.len() != b.len() || a.iter().zip(b).any(|(x, y)| !same(x, y)),
        _ => false,
    }
}

fn gauge_of(section: &Value) -> (Option<&str>, Option<&str>) {
    (
        section.get(GAUGE_SHOT).and_then(Value::as_str),
        section.get(GAUGE_NEIGHBOUR).and_then(Value::as_str),
    )
}

fn edit_section(from: &Value, to: &Value, at: usize, pool: &mut ShotPool<'_>) -> SectionRun {
    let id = section_id_of(from)
        .and_then(|id| SectionId::try_from(id).ok())
        .unwrap_or_default();
    let (Value::Object(a), Value::Object(b)) = (from, to) else {
        // Unreachable for a serialized project; keep the result exact anyway.
        return SectionRun::New {
            fields: to.as_object().cloned().unwrap_or_default(),
            shots: None,
        };
    };
    let (Some(_), Some(to_shots)) = (shots_array(from), shots_array(to)) else {
        return SectionRun::Edit {
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
    SectionRun::Edit {
        at,
        id,
        fields,
        shots,
        ungauged,
    }
}

fn new_section(to: &Value, pool: &mut ShotPool<'_>) -> SectionRun {
    let mut fields = to.as_object().cloned().unwrap_or_default();
    let shots = match fields.get(SHOTS) {
        Some(Value::Array(shots)) => {
            let runs = pool.rebuild(shots, None);
            fields.remove(SHOTS);
            Some(runs)
        }
        _ => None,
    };
    SectionRun::New { fields, shots }
}

// === Apply ===

/// One section of an applied delta: the source's text where the delta kept
/// it, the rebuilt value where it did not.
#[derive(Debug)]
pub enum SectionOut<'a> {
    Kept { raw: Cow<'a, str>, from: usize },
    Built(Value),
}

/// A delta applied to a [`Doc`]: what [`Applied::into_doc`] turns into the
/// target's text, with the rebuilt sections still open for the caller.
#[derive(Debug)]
pub struct Applied<'a> {
    members: BTreeMap<String, Cow<'a, str>>,
    sections: Vec<SectionOut<'a>>,
}

impl<'a> Applied<'a> {
    /// The sections the delta rebuilt, to adjust before they are written.
    pub fn built_sections_mut(&mut self) -> Vec<&mut Value> {
        self.sections
            .iter_mut()
            .filter_map(|section| match section {
                SectionOut::Built(value) => Some(value),
                SectionOut::Kept { .. } => None,
            })
            .collect()
    }

    /// The target, split as a [`Doc`] (the rebuilt sections written out).
    pub fn into_doc(self) -> Result<Doc<'a>, DeltaError> {
        let sections = self
            .sections
            .into_iter()
            .map(|section| match section {
                SectionOut::Kept { raw, .. } => Ok(raw),
                SectionOut::Built(value) => to_text(&value).map(Cow::Owned),
            })
            .collect::<Result<_, _>>()?;
        Ok(Doc {
            members: self.members,
            sections,
        })
    }
}

/// `delta` applied to `source`, whose sections' ids `ids` lists in order.
pub(super) fn apply<'a>(
    delta: &ProjectDelta,
    source: &Doc<'a>,
    ids: &[SectionIds],
) -> Result<Applied<'a>, DeltaError> {
    delta.check_shape()?;
    if ids.len() != source.sections.len() {
        return Err(format!(
            "the id table lists {} sections, the source holds {}",
            ids.len(),
            source.sections.len()
        ));
    }
    let mut members = source.members.clone();
    for (key, edit) in &delta.fields {
        if key == SECTIONS {
            return Err("an edit of the whole sections member".to_string());
        }
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
    let mut sections = match &delta.sections {
        None => source
            .sections
            .iter()
            .enumerate()
            .map(|(from, raw)| SectionOut::Kept {
                raw: raw.clone(),
                from,
            })
            .collect(),
        Some((count, runs)) => {
            if *count != source.sections.len() {
                return Err(format!(
                    "the delta was recorded over {count} sections, the source holds {}",
                    source.sections.len()
                ));
            }
            build_sections(source, runs)?
        }
    };
    let literal = literal_shots(delta);
    settle_ids(&mut sections, ids, &literal)?;
    settle_section_counter(&mut members, &sections, ids);
    Ok(Applied { members, sections })
}

fn build_sections<'a>(
    source: &Doc<'a>,
    runs: &[SectionRun],
) -> Result<Vec<SectionOut<'a>>, DeltaError> {
    // Parse exactly the source sections the runs read: the ones they edit and
    // the ones they copy shots out of.
    let mut needed: HashSet<usize> = HashSet::new();
    for run in runs {
        let shots = match run {
            SectionRun::Keep(..) => continue,
            SectionRun::Edit { at, shots, .. } => {
                needed.insert(*at);
                shots
            }
            SectionRun::New { shots, .. } => shots,
        };
        for shot_run in shots.iter().flatten() {
            if let super::ShotRun::Copy { sec, .. } = shot_run {
                needed.insert(*sec);
            }
        }
    }
    let mut parsed: BTreeMap<usize, Value> = BTreeMap::new();
    for p in needed {
        let Some(raw) = source.sections.get(p) else {
            return Err(format!("a section run reads section {p}, past the source"));
        };
        parsed.insert(p, parse_value(raw)?);
    }
    let mut out: Vec<SectionOut<'a>> = Vec::with_capacity(source.sections.len());
    for run in runs {
        match run {
            SectionRun::Keep(at, len) => {
                let end = at
                    .checked_add(*len)
                    .filter(|end| *end <= source.sections.len());
                let Some(end) = end else {
                    return Err(format!(
                        "a section run keeps {len} sections from {at}, past the source's {}",
                        source.sections.len()
                    ));
                };
                for from in *at..end {
                    out.push(SectionOut::Kept {
                        raw: source.sections[from].clone(),
                        from,
                    });
                }
            }
            SectionRun::Edit {
                at,
                id,
                fields,
                shots,
                ..
            } => {
                let Some(Value::Object(base)) = parsed.get(at) else {
                    return Err(format!("a section run edits section {at}, past the source"));
                };
                if section_id_of_map(base) != Some(*id) {
                    return Err(format!(
                        "a section run edits section {id} at {at}, which holds another section"
                    ));
                }
                let skip = shots.as_ref().map(|_| SHOTS);
                let mut section = apply_members(base, fields, skip)?;
                if let Some(shot_runs) = shots {
                    let built = build_shots(&parsed, shot_runs)?;
                    section.insert(SHOTS.to_string(), Value::Array(built));
                }
                out.push(SectionOut::Built(Value::Object(section)));
            }
            SectionRun::New { fields, shots } => {
                let mut section = fields.clone();
                if let Some(shot_runs) = shots {
                    let built = build_shots(&parsed, shot_runs)?;
                    section.insert(SHOTS.to_string(), Value::Array(built));
                }
                out.push(SectionOut::Built(Value::Object(section)));
            }
        }
    }
    Ok(out)
}

/// Where the shots the delta carried literally landed: (section, shot) in
/// the output. A run's literal shots are its `Put` runs, in order.
fn literal_shots(delta: &ProjectDelta) -> Vec<LiteralShot> {
    let Some((_, runs)) = &delta.sections else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut s = 0;
    for run in runs {
        let shots = match run {
            SectionRun::Keep(_, len) => {
                s += len;
                continue;
            }
            SectionRun::Edit { shots, .. } | SectionRun::New { shots, .. } => shots,
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

/// Make every shot id unique and every section's allocator safe, after a
/// rebuild that put `literal` shots back with their recorded ids.
///
/// The loader renumbers every shot (`section_id * FEATURE_ID_STRIDE +
/// sequence_order`), so a shot recorded before a reopen can carry an id that a
/// renumbered live shot now holds. Copied shots keep their live ids; a literal
/// whose id is taken (or a second copy of one) takes a fresh one from its
/// section's allocator. Each allocator is then raised past every id in its
/// window that any section holds, so the next shot it hands out cannot collide
/// either. A state recorded in this session is already consistent, and then
/// nothing here changes a byte - and a kept section is never even parsed.
fn settle_ids(
    sections: &mut [SectionOut<'_>],
    ids: &[SectionIds],
    literal: &[LiteralShot],
) -> Result<(), DeltaError> {
    let literal: HashSet<LiteralShot> = literal.iter().copied().collect();
    let mut taken: HashSet<u64> = HashSet::new();
    let mut clash: Vec<LiteralShot> = Vec::new();
    // Copies (and kept sections) first, so a literal is what yields.
    for pass_literal in [false, true] {
        for (s, section) in sections.iter().enumerate() {
            match section {
                SectionOut::Kept { from, .. } => {
                    if !pass_literal {
                        taken.extend(ids[*from].shots.iter().copied());
                    }
                }
                SectionOut::Built(value) => {
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
    for section in sections.iter_mut() {
        let current = match section {
            SectionOut::Kept { from, .. } => ids[*from].clone(),
            SectionOut::Built(value) => SectionIds::of_value(value),
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
        // section has to be opened to have its allocator raised.
        if let SectionOut::Kept { raw, .. } = section {
            *section = SectionOut::Built(parse_value(raw)?);
        }
        if let SectionOut::Built(Value::Object(members)) = section {
            members.insert(NEXT_SHOT_ID.to_string(), Value::from(max.saturating_add(1)));
        }
    }
    for (s, i) in clash {
        let Some(SectionOut::Built(Value::Object(section))) = sections.get_mut(s) else {
            continue;
        };
        let Some(section_id) = section.get(ID).and_then(Value::as_u64) else {
            return Err("a section without an id holds a shot whose id is taken".to_string());
        };
        // The ids come from the file: checked, so a hostile one is refused
        // rather than overflowing.
        let Some(end) = section_id
            .checked_add(1)
            .and_then(|s| s.checked_mul(stride))
        else {
            return Err(format!(
                "section {section_id} is past the shot-id encode bound"
            ));
        };
        let mut next = section
            .get(NEXT_SHOT_ID)
            .and_then(Value::as_u64)
            .unwrap_or(end - stride);
        while next < end && taken.contains(&next) {
            next += 1;
        }
        if next >= end {
            return Err(format!(
                "section {section_id} has no shot id left to give back"
            ));
        }
        taken.insert(next);
        section.insert(NEXT_SHOT_ID.to_string(), Value::from(next + 1));
        if let Some(Value::Object(shot)) = section
            .get_mut(SHOTS)
            .and_then(Value::as_array_mut)
            .and_then(|shots| shots.get_mut(i))
        {
            shot.insert(ID.to_string(), Value::from(next));
        }
    }
    Ok(())
}

/// Raise the project's section allocator past every section the target
/// holds. The loader recomputes it as `max + 1`, so after a reopen the
/// recorded value of a state that held a since-deleted section can be at or
/// below that section's id, and the next new section would take it again.
fn settle_section_counter(
    members: &mut BTreeMap<String, Cow<'_, str>>,
    sections: &[SectionOut<'_>],
    ids: &[SectionIds],
) {
    let max = sections
        .iter()
        .filter_map(|section| match section {
            SectionOut::Kept { from, .. } => ids[*from].id,
            SectionOut::Built(value) => section_id_of(value),
        })
        .max();
    let Some(max) = max else {
        return;
    };
    let Some(next) = members
        .get(NEXT_SECTION_ID)
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        return;
    };
    if next <= max {
        members.insert(
            NEXT_SECTION_ID.to_string(),
            Cow::Owned(max.saturating_add(1).to_string()),
        );
    }
}
