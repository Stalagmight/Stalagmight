//! The unordered members (`UNORDERED_MEMBERS`) diffed and edited as text,
//! element by element.
//!
//! Those members are the largest of a project's top-level members: every
//! station has a name and a Station ID, so on a 20,000-shot survey
//! `station_names` and `station_ids` are about 20,000 pairs each. A delta that
//! re-keys a line's stations (a delete, a reverse, a split) edits both, and
//! parsing each whole into a `serde_json::Value` tree on every encode and
//! every apply - then writing it back - was most of such a step. Here a member
//! is split into its elements' raw text (one scan, no tree), elements are
//! matched by their text, and only the few that differ are parsed.
//!
//! Text equality implies value equality, so a text match is always right. The
//! converse holds for the text this build serializes a project with - the
//! same element always serializes the same way - but not for every text (a key
//! order a `Value` round trip changed): whatever the text cannot match is
//! matched by value, exactly as the value-level bag diff and apply do
//! (`json::diff_bag`, `json::apply_bag`), so the result never depends on it;
//! only the speed does.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::json::{apply_bag, diff_bag, same};
use super::{DeltaError, Edit};
use crate::models::StationKey;

/// The raw text of each element of `text`, a JSON array.
fn elements(text: &str) -> Result<Vec<&str>, DeltaError> {
    let raw: Vec<&RawValue> = serde_json::from_str(text)
        .map_err(|e| format!("an unordered member is not an array: {e}"))?;
    Ok(raw.into_iter().map(RawValue::get).collect())
}

fn parse(raw: &str) -> Result<Value, DeltaError> {
    serde_json::from_str(raw).map_err(|e| e.to_string())
}

/// An element of an unordered member as this build serializes it: a
/// `[station key, value]` pair written through [`StationKey`]'s own
/// serialization (its fields in declaration order, which a `Value` does not
/// keep), anything else as the `Value` writes it.
fn element_text(value: &Value) -> Option<String> {
    if let Some([key, inner]) = value.as_array().map(Vec::as_slice) {
        if key.is_object() {
            let key = StationKey::deserialize(key).ok()?;
            return serde_json::to_string(&(key, inner)).ok();
        }
    }
    serde_json::to_string(value).ok()
}

/// How this build writes every `[station key, value]` pair of a
/// station-keyed member: the key first, its line id first. No string value
/// can hold the sequence (its quotes would be escaped) and no other value
/// writes it, so where it occurs a pair starts.
const PAIR_START: &str = "[{\"line_id\":";

/// How many leading bytes `a` and `b` share, compared a block at a time.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const BLOCK: usize = 256;
    let n = a.len().min(b.len());
    let mut at = 0;
    while at + BLOCK <= n && a[at..at + BLOCK] == b[at..at + BLOCK] {
        at += BLOCK;
    }
    at + a[at..n]
        .iter()
        .zip(&b[at..n])
        .take_while(|(x, y)| x == y)
        .count()
}

/// How many trailing bytes `a` and `b` share, at most `max`.
fn common_suffix(a: &[u8], b: &[u8], max: usize) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take(max)
        .take_while(|(x, y)| x == y)
        .count()
}

/// For two station-keyed members written by this build, the stretch where
/// they differ, widened to whole pairs: `(start, from_end, to_end, before)`,
/// the byte range `start..from_end` of `from` and `start..to_end` of `to`
/// (the pairs, comma-separated), and how many pairs come before `start`.
/// The rest of the two texts is byte for byte the same, the same pairs at the
/// same places. `None` when the texts are not arrays of such pairs.
fn differing_pairs(from: &str, to: &str) -> Option<(usize, usize, usize, usize)> {
    let pairs = |text: &str| {
        text.starts_with('[')
            && text.ends_with(']')
            && (text == "[]" || text[1..].starts_with(PAIR_START))
    };
    if !pairs(from) || !pairs(to) {
        return None;
    }
    let (a, b) = (from.as_bytes(), to.as_bytes());
    let prefix = common_prefix(a, b);
    // The pair the first difference falls in starts at the last pair start
    // at or before it (or the array does). The shared prefix is counted in
    // bytes, so it can end inside a character the two texts start alike (an
    // accented letter in a station name): the cut is taken back to the
    // character it is in. A pair start is ASCII, so wherever one is found,
    // both texts have a character boundary there.
    let mut cut = (prefix + PAIR_START.len()).min(from.len());
    while !from.is_char_boundary(cut) {
        cut -= 1;
    }
    let start = from[..cut].rfind(PAIR_START).unwrap_or(1).max(1);
    let room = from.len().min(to.len()) - start;
    let suffix = common_suffix(a, b, room);
    // The pairs the shared tail holds whole start at its first pair start.
    // Counted in bytes like the prefix, so moved up to the character
    // boundary it may fall short of: the tail only gets shorter.
    let mut tail = from.len() - suffix;
    while !from.is_char_boundary(tail) {
        tail += 1;
    }
    let (from_end, to_end) = match from[tail..].find(PAIR_START) {
        Some(at) => {
            let end = tail + at - 1;
            (end, end + to.len() - from.len())
        }
        None => (from.len() - 1, to.len() - 1),
    };
    let before = to[..start].matches(PAIR_START).count();
    Some((start, from_end.max(start), to_end.max(start), before))
}

/// The elements of `text[start..end]`, a comma-separated run of whole
/// elements of an array.
fn elements_between(text: &str, start: usize, end: usize) -> Result<Vec<&str>, DeltaError> {
    let run = &text[start..end];
    if run.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Wrapped as an array to be split, then each element pointed back into
    // `text`: the wrapper only lives for the split.
    let wrapped = format!("[{run}]");
    let base = wrapped.as_ptr() as usize + 1;
    Ok(elements(&wrapped)?
        .into_iter()
        .map(|raw| {
            let at = start + (raw.as_ptr() as usize - base);
            &text[at..at + raw.len()]
        })
        .collect())
}

/// The bag edit that turns the member `from` into `to`, both raw arrays, or
/// `None` when the two hold the same elements in the same order: what
/// `json::diff_member` makes of them parsed, without parsing what they share.
/// For a station-keyed member (`station_keyed`) only the stretch where the two
/// texts differ is split at all ([`differing_pairs`]).
pub(super) fn diff_bag_text(
    from: &str,
    to: &str,
    station_keyed: bool,
) -> Result<Option<Edit>, DeltaError> {
    if station_keyed {
        if let Some((start, from_end, to_end, before)) = differing_pairs(from, to) {
            let a = elements_between(from, start, from_end)?;
            let b = elements_between(to, start, to_end)?;
            return Ok(diff_elements(&a, &b)?.map(|edit| shift_additions(edit, before)));
        }
    }
    diff_elements(&elements(from)?, &elements(to)?)
}

/// `edit` with its additions' target indices moved `by` places on.
fn shift_additions(edit: Edit, by: usize) -> Edit {
    match edit {
        Edit::Bag {
            del,
            add,
            station_del,
            station_add,
        } => Edit::Bag {
            del,
            add: add
                .into_iter()
                .map(|(at, value)| (at + by, value))
                .collect(),
            station_del,
            station_add,
        },
        edit => edit,
    }
}

/// [`diff_bag_text`] over two element lists.
fn diff_elements(a: &[&str], b: &[&str]) -> Result<Option<Edit>, DeltaError> {
    // The elements both sides hold at the same place at the start and at the
    // end - all but the few an edit touched, in a member serialized sorted -
    // are matched by place; only the middle is matched by text.
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let (a_end, b_end) = (a.len() - suffix, b.len() - suffix);
    let mut unmatched: HashMap<&str, Vec<usize>> = HashMap::new();
    for j in (prefix..b_end).rev() {
        unmatched.entry(b[j]).or_default().push(j);
    }
    let mut matched = vec![false; b.len()];
    let mut rest_a: Vec<usize> = Vec::new();
    for (i, raw) in a.iter().enumerate().take(a_end).skip(prefix) {
        match unmatched.get_mut(raw).and_then(Vec::pop) {
            Some(j) => matched[j] = true,
            None => rest_a.push(i),
        }
    }
    let rest_b: Vec<usize> = (prefix..b_end).filter(|j| !matched[*j]).collect();
    if rest_a.is_empty() && rest_b.is_empty() && a.iter().zip(b).all(|(x, y)| x == y) {
        return Ok(None);
    }
    // What the text could not match, matched by value.
    let from_rest = rest_a
        .iter()
        .map(|i| parse(a[*i]))
        .collect::<Result<Vec<_>, _>>()?;
    let to_rest = rest_b
        .iter()
        .map(|j| parse(b[*j]))
        .collect::<Result<Vec<_>, _>>()?;
    let Edit::Bag { del, add, .. } = diff_bag(&from_rest, &to_rest) else {
        return Err("a bag diff is a bag edit".to_string());
    };
    if del.is_empty() && add.is_empty() && positionally_same(a, b)? {
        return Ok(None);
    }
    Ok(Some(Edit::Bag {
        del,
        add: add
            .into_iter()
            .map(|(k, value)| (rest_b[k], value))
            .collect(),
        station_del: Vec::new(),
        station_add: Vec::new(),
    }))
}

/// Whether two element lists are the same value element for element (the
/// value-level diff's `same` over the whole arrays).
fn positionally_same(a: &[&str], b: &[&str]) -> Result<bool, DeltaError> {
    if a.len() != b.len() {
        return Ok(false);
    }
    for (x, y) in a.iter().zip(b) {
        if x != y && !same(&parse(x)?, &parse(y)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The member `from`, a raw array, without one element equal to each of `del`
/// and with each of `add` inserted at its recorded index (clamped), as text:
/// `json::apply_bag` without parsing the elements it keeps. The kept elements
/// keep their text; an added one is written as this build serializes it.
pub(super) fn apply_bag_text(
    from: &str,
    del: &[Value],
    add: &[(usize, Value)],
) -> Result<String, DeltaError> {
    let items = elements(from)?;
    // The texts to remove, each with how many times: the source's elements
    // are looked up in it, not indexed themselves.
    let mut wanted: HashMap<String, usize> = HashMap::new();
    let mut unresolved = false;
    for value in del {
        match element_text(value) {
            Some(text) => *wanted.entry(text).or_default() += 1,
            None => unresolved = true,
        }
    }
    let mut removed = vec![false; items.len()];
    let mut found = 0;
    if !wanted.is_empty() {
        for (i, raw) in items.iter().enumerate() {
            if let Some(count) = wanted.get_mut(*raw).filter(|count| **count > 0) {
                *count -= 1;
                removed[i] = true;
                found += 1;
            }
        }
    }
    unresolved |= found < del.len();
    if unresolved {
        // An element the text did not find is either not there (a load or a
        // save pruned it) or written another way: settle it by value, as the
        // value-level apply does, over the whole member.
        let values = items
            .iter()
            .map(|raw| parse(raw))
            .collect::<Result<Vec<_>, _>>()?;
        let out = apply_bag(&values, del, add);
        return serde_json::to_string(&out).map_err(|e| e.to_string());
    }
    let out: Vec<Cow<'_, str>> = items
        .iter()
        .zip(&removed)
        .filter(|(_, gone)| !**gone)
        .map(|(raw, _)| Cow::Borrowed(*raw))
        .collect();
    let mut added = Vec::with_capacity(add.len());
    for (index, value) in add {
        let text = match element_text(value) {
            Some(text) => text,
            None => serde_json::to_string(value).map_err(|e| e.to_string())?,
        };
        added.push((*index, Cow::Owned(text)));
    }
    let out = super::json::merge_additions(out, added);
    let mut text = String::with_capacity(out.iter().map(|e| e.len() + 1).sum::<usize>() + 2);
    text.push('[');
    for (k, element) in out.iter().enumerate() {
        if k > 0 {
            text.push(',');
        }
        text.push_str(element);
    }
    text.push(']');
    Ok(text)
}
