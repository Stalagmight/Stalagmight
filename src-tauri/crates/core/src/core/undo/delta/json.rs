//! The generic half of the delta engine: bitwise JSON equality, a structural
//! diff of two values into an [`Edit`], and its inverse.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use serde_json::{Map, Number, Value};

use super::{DeltaError, Edit, Run, MAX_ARRAY_EDITS, SMALL_ARRAY, SMALL_LIST, UNORDERED_MEMBERS};

/// Which of serde_json's three number representations a number has: equal
/// values in different representations are different JSON.
fn number_kind(n: &Number) -> u8 {
    if n.is_f64() {
        0
    } else if n.is_u64() {
        1
    } else {
        2
    }
}

fn same_number(a: &Number, b: &Number) -> bool {
    if number_kind(a) != number_kind(b) {
        return false;
    }
    if a.is_f64() {
        return a.as_f64().map(f64::to_bits) == b.as_f64().map(f64::to_bits);
    }
    if a.is_u64() {
        return a.as_u64() == b.as_u64();
    }
    a.as_i64() == b.as_i64()
}

/// JSON equality with floats compared by bits.
pub(crate) fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => same_number(x, y),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((ka, va), (kb, vb))| ka == kb && same(va, vb))
        }
        _ => false,
    }
}

fn hash_into(value: &Value, h: &mut DefaultHasher) {
    match value {
        Value::Null => 0u8.hash(h),
        Value::Bool(b) => {
            1u8.hash(h);
            b.hash(h);
        }
        Value::Number(n) => {
            2u8.hash(h);
            number_kind(n).hash(h);
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                f.to_bits().hash(h);
            } else if let Some(u) = n.as_u64() {
                u.hash(h);
            } else if let Some(i) = n.as_i64() {
                i.hash(h);
            }
        }
        Value::String(s) => {
            3u8.hash(h);
            s.hash(h);
        }
        Value::Array(items) => {
            4u8.hash(h);
            items.len().hash(h);
            for item in items {
                hash_into(item, h);
            }
        }
        Value::Object(members) => {
            5u8.hash(h);
            members.len().hash(h);
            for (key, member) in members {
                key.hash(h);
                hash_into(member, h);
            }
        }
    }
}

pub(super) fn hash_of(value: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    hash_into(value, &mut h);
    h.finish()
}

// === Generic diff ===

/// The edit that turns `from` into `to`, or `None` when they are equal.
pub(crate) fn diff(from: &Value, to: &Value) -> Option<Edit> {
    if same(from, to) {
        return None;
    }
    Some(match (from, to) {
        (Value::Object(a), Value::Object(b)) => Edit::Obj(diff_members(a, b)),
        (Value::Array(a), Value::Array(b)) if b.len() > SMALL_ARRAY && !a.is_empty() => {
            Edit::Arr(a.len(), diff_array(a, b))
        }
        _ => Edit::Set(to.clone()),
    })
}

/// [`diff`] for a top-level member of the project, which knows which lists are
/// unordered ([`UNORDERED_MEMBERS`]) and which are short ([`SMALL_LIST`]).
pub(super) fn diff_member(key: &str, from: &Value, to: &Value) -> Option<Edit> {
    if same(from, to) {
        return None;
    }
    match (from, to) {
        (Value::Array(a), Value::Array(b)) if UNORDERED_MEMBERS.contains(&key) => {
            Some(diff_bag(a, b))
        }
        (Value::Array(_), Value::Array(b)) if b.len() <= SMALL_LIST => Some(Edit::Set(to.clone())),
        _ => diff(from, to),
    }
}

/// The elements of `from` that `to` lacks and the reverse, as multisets.
pub(super) fn diff_bag(from: &[Value], to: &[Value]) -> Edit {
    let mut unmatched: HashMap<u64, Vec<usize>> = HashMap::new();
    for (j, value) in to.iter().enumerate() {
        unmatched.entry(hash_of(value)).or_default().push(j);
    }
    let mut matched = vec![false; to.len()];
    let mut del = Vec::new();
    for value in from {
        let slot = unmatched.get_mut(&hash_of(value)).and_then(|candidates| {
            let k = candidates.iter().position(|&j| same(&to[j], value))?;
            Some(candidates.swap_remove(k))
        });
        match slot {
            Some(j) => matched[j] = true,
            None => del.push(value.clone()),
        }
    }
    let add = to
        .iter()
        .enumerate()
        .zip(&matched)
        .filter(|(_, hit)| !**hit)
        .map(|((j, value), _)| (j, value.clone()))
        .collect();
    Edit::Bag {
        del,
        add,
        station_del: Vec::new(),
        station_add: Vec::new(),
    }
}

pub(super) fn diff_members(
    from: &Map<String, Value>,
    to: &Map<String, Value>,
) -> BTreeMap<String, Edit> {
    let mut out = BTreeMap::new();
    for (key, a) in from {
        match to.get(key) {
            None => {
                out.insert(key.clone(), Edit::Del);
            }
            Some(b) => {
                if let Some(edit) = diff(a, b) {
                    out.insert(key.clone(), edit);
                }
            }
        }
    }
    for (key, b) in to {
        if !from.contains_key(key) {
            out.insert(key.clone(), Edit::Set(b.clone()));
        }
    }
    out
}

/// The target array as runs over the source: a common prefix and suffix, and a
/// shortest edit script (Myers) over the middle, bounded by
/// [`MAX_ARRAY_EDITS`].
pub(super) fn diff_array(from: &[Value], to: &[Value]) -> Vec<Run> {
    let mut prefix = 0;
    while prefix < from.len() && prefix < to.len() && same(&from[prefix], &to[prefix]) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < from.len() - prefix
        && suffix < to.len() - prefix
        && same(&from[from.len() - 1 - suffix], &to[to.len() - 1 - suffix])
    {
        suffix += 1;
    }
    let a = &from[prefix..from.len() - suffix];
    let b = &to[prefix..to.len() - suffix];
    let ha: Vec<u64> = a.iter().map(hash_of).collect();
    let hb: Vec<u64> = b.iter().map(hash_of).collect();
    let matches = myers(a.len(), b.len(), |x, y| {
        ha[x] == hb[y] && same(&a[x], &b[y])
    })
    .unwrap_or_default();

    let mut runs = RunBuilder::default();
    runs.keep(0, prefix);
    let (mut i, mut j) = (0, 0);
    for (mi, mj) in matches
        .into_iter()
        .chain(std::iter::once((a.len(), b.len())))
    {
        runs.hunk(&a[i..mi], prefix + i, &b[j..mj]);
        if mi < a.len() {
            runs.keep(prefix + mi, 1);
        }
        i = mi + 1;
        j = mj + 1;
    }
    runs.keep(from.len() - suffix, suffix);
    runs.finish()
}

/// Builds a run list, merging adjacent `Keep`s and `Put`s.
#[derive(Default)]
struct RunBuilder {
    runs: Vec<Run>,
}

impl RunBuilder {
    fn keep(&mut self, at: usize, len: usize) {
        if len == 0 {
            return;
        }
        if let Some(Run::Keep(start, n)) = self.runs.last_mut() {
            if *start + *n == at {
                *n += len;
                return;
            }
        }
        self.runs.push(Run::Keep(at, len));
    }

    fn put(&mut self, value: &Value) {
        if let Some(Run::Put(values)) = self.runs.last_mut() {
            values.push(value.clone());
            return;
        }
        self.runs.push(Run::Put(vec![value.clone()]));
    }

    /// A stretch where the source's `removed` (starting at `at`) became
    /// `added`. Equal lengths pair element by element - an edited element is a
    /// `Mod` when it is an object or array that changed in part - otherwise
    /// the added elements are literal.
    fn hunk(&mut self, removed: &[Value], at: usize, added: &[Value]) {
        if removed.len() == added.len() {
            for (k, (old, new)) in removed.iter().zip(added).enumerate() {
                match diff(old, new) {
                    Some(Edit::Set(_)) | None => self.put(new),
                    Some(edit) => self.runs.push(Run::Mod(at + k, Box::new(edit))),
                }
            }
            return;
        }
        for value in added {
            self.put(value);
        }
    }

    fn finish(self) -> Vec<Run> {
        self.runs
    }
}

/// Matched index pairs of a shortest edit script between two sequences of
/// lengths `n` and `m` whose elements `equal` compares, in ascending order,
/// or `None` when it needs more than [`MAX_ARRAY_EDITS`] edits (the caller
/// then treats the whole middle as changed). The classic greedy forward pass
/// with a saved trace, walked back from the end.
pub(super) fn myers(
    n: usize,
    m: usize,
    equal: impl Fn(usize, usize) -> bool,
) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (n as isize, m as isize);
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    let max = (n + m).min(MAX_ARRAY_EDITS as isize);
    let at = |k: isize| (k + max) as usize;
    let mut v = vec![0isize; 2 * max as usize + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut done = false;
    for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && equal(x as usize, y as usize) {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                done = true;
                break;
            }
            k += 2;
        }
        if done {
            break;
        }
    }
    if !done {
        return None;
    }
    let mut matches = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[at(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            matches.push((x as usize, y as usize));
        }
        if d > 0 {
            x = prev_x;
            y = prev_y;
        }
    }
    matches.reverse();
    Some(matches)
}

// === Generic apply ===

pub(super) fn apply_edit(from: &Value, edit: &Edit) -> Result<Value, DeltaError> {
    match edit {
        Edit::Set(value) => Ok(value.clone()),
        Edit::Del => Err("a delete edits an object member, not a value".to_string()),
        Edit::Obj(members) => match from {
            Value::Object(object) => Ok(Value::Object(apply_members(object, members, None)?)),
            _ => Err("an object edit met a value that is not an object".to_string()),
        },
        Edit::Arr(count, runs) => match from {
            Value::Array(items) if items.len() == *count => {
                Ok(Value::Array(apply_runs(items, runs)?))
            }
            Value::Array(items) => Err(format!(
                "an array edit was recorded over {count} elements, the source holds {}",
                items.len()
            )),
            _ => Err("an array edit met a value that is not an array".to_string()),
        },
        Edit::Bag {
            station_del,
            station_add,
            ..
        } if !station_del.is_empty() || !station_add.is_empty() => Err(
            "a station-keyed bag edit is applied with the states its positions name".to_string(),
        ),
        Edit::Bag { del, add, .. } => match from {
            Value::Array(items) => Ok(Value::Array(apply_bag(items, del, add))),
            _ => Err("a bag edit met a value that is not an array".to_string()),
        },
    }
}

/// `from` without one element equal to each of `del`, and with each of `add`
/// inserted at its recorded index (clamped). An element to remove that is not
/// there is skipped: a load or a save pruned it already.
pub(super) fn apply_bag(from: &[Value], del: &[Value], add: &[(usize, Value)]) -> Vec<Value> {
    let mut by_hash: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, value) in from.iter().enumerate() {
        by_hash.entry(hash_of(value)).or_default().push(i);
    }
    let mut removed = vec![false; from.len()];
    for value in del {
        if let Some(candidates) = by_hash.get_mut(&hash_of(value)) {
            if let Some(k) = candidates.iter().position(|&i| same(&from[i], value)) {
                removed[candidates.swap_remove(k)] = true;
            }
        }
    }
    let mut out: Vec<Value> = from
        .iter()
        .zip(&removed)
        .filter(|(_, gone)| !**gone)
        .map(|(value, _)| value.clone())
        .collect();
    for (index, value) in add {
        out.insert((*index).min(out.len()), value.clone());
    }
    out
}

/// `from` with `edits` applied, leaving out the member named `skip`.
///
/// Tolerant of one thing only: deleting a member that is already absent is a
/// no-op. A save drops state the history never recorded (an orphan offset
/// key), and an entry that removes the same thing must still step through.
pub(super) fn apply_members(
    from: &Map<String, Value>,
    edits: &BTreeMap<String, Edit>,
    skip: Option<&str>,
) -> Result<Map<String, Value>, DeltaError> {
    let mut out = Map::new();
    for (key, value) in from {
        if Some(key.as_str()) == skip {
            continue;
        }
        match edits.get(key) {
            None => {
                out.insert(key.clone(), value.clone());
            }
            Some(Edit::Del) => {}
            Some(edit) => {
                out.insert(key.clone(), apply_edit(value, edit)?);
            }
        }
    }
    for (key, edit) in edits {
        if from.contains_key(key) {
            continue;
        }
        match edit {
            Edit::Set(value) => {
                out.insert(key.clone(), value.clone());
            }
            Edit::Del => {}
            // A member serialized only when non-empty: edit the empty value.
            Edit::Bag { .. } => {
                out.insert(key.clone(), apply_edit(&Value::Array(Vec::new()), edit)?);
            }
            Edit::Obj(_) => {
                out.insert(key.clone(), apply_edit(&Value::Object(Map::new()), edit)?);
            }
            Edit::Arr(..) => {
                return Err(format!(
                    "an edit names member {key:?}, which the source lacks"
                ))
            }
        }
    }
    Ok(out)
}

pub(super) fn apply_runs(from: &[Value], runs: &[Run]) -> Result<Vec<Value>, DeltaError> {
    let mut out = Vec::with_capacity(from.len());
    for run in runs {
        match run {
            Run::Keep(at, len) => {
                let end = at.checked_add(*len).filter(|end| *end <= from.len());
                let Some(end) = end else {
                    return Err(format!(
                        "an array run keeps {len} elements from {at}, past the source's {}",
                        from.len()
                    ));
                };
                out.extend_from_slice(&from[*at..end]);
            }
            Run::Put(values) => out.extend(values.iter().cloned()),
            Run::Mod(at, edit) => {
                let Some(item) = from.get(*at) else {
                    return Err(format!("an array run edits element {at}, past the source"));
                };
                out.push(apply_edit(item, edit)?);
            }
        }
    }
    Ok(out)
}

/// Visit every literal value inside `edit` mutably; `f` returns whether it
/// changed one.
pub(super) fn edit_values_mut(edit: &mut Edit, f: &mut dyn FnMut(&mut Value) -> bool) -> bool {
    match edit {
        Edit::Set(value) => f(value),
        Edit::Del => false,
        Edit::Obj(members) => {
            let mut changed = false;
            for member in members.values_mut() {
                changed |= edit_values_mut(member, f);
            }
            changed
        }
        Edit::Arr(_, runs) => {
            let mut changed = false;
            for run in runs {
                match run {
                    Run::Keep(..) => {}
                    Run::Put(values) => {
                        for value in values {
                            changed |= f(value);
                        }
                    }
                    Run::Mod(_, inner) => changed |= edit_values_mut(inner, f),
                }
            }
            changed
        }
        Edit::Bag {
            del,
            add,
            station_del,
            station_add,
        } => {
            let mut changed = false;
            for value in del
                .iter_mut()
                .chain(add.iter_mut().map(|(_, v)| v))
                .chain(station_del.iter_mut().map(|(_, _, v)| v))
                .chain(station_add.iter_mut().map(|(_, _, _, v)| v))
            {
                changed |= f(value);
            }
            changed
        }
    }
}
