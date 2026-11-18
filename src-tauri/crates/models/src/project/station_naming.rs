//! The naming rules for stations and lines, as pure functions over the names
//! already taken.
//!
//! A station's name is unique within its line; a line's name is unique within
//! the project. The rules below decide the name of a station an operation
//! creates, the name a missing or repeated one is given when a project is
//! normalized, and the letters a Combine Lines prefixes when the two lines'
//! names collide. They are documented for users in `docs/SPEC.md` (4.1, 4.2).
//!
//! - **End** (a shot appended at the end of a line): the previous station's
//!   name plus one - `5` -> `6`, `A3` -> `A4`, `007` -> `008` - incremented
//!   until free. A previous name with no trailing number gives the next free
//!   integer of the line ([`next_free_integer`]).
//! - **Mid** (a station inserted inside a line: Insert Shot after/before,
//!   Insert Station, split shot at distance): the previous station's name plus
//!   the first free lowercase suffix, `a`..`z`, then `aa`..`zz`, `aaa`, ... -
//!   after `2` -> `2a`, again -> `2b`, giving `2 - 2b - 2a - 3`.
//! - **Prepend** (a shot added before the first station): the next free number
//!   of the old first station's prefix family - `0 1 .. 6` -> `7`, `A0..A3` ->
//!   `A4`; a first name with no trailing number gives the next free integer.
//! - **Seed** (a station with no name when a project is normalized): its
//!   positional index, `i` -> `"i"`, letter-suffixed when taken.
//! - **Repeat** (two stations of a line with one name): the later one gets the
//!   Mid suffix of the shared name - `5`, `5` -> `5`, `5a`.

use std::collections::{BTreeSet, HashMap, HashSet};

/// A name split into its prefix and its trailing decimal number: `"A3"` ->
/// `("A", 3, 1)`, `"007"` -> `("", 7, 3)`. The width is the digit count, so a
/// zero-padded number keeps its padding when incremented. `None` when the name
/// does not end in a digit, or the number does not fit a `u64`.
pub fn trailing_number(name: &str) -> Option<(&str, u64, usize)> {
    let digits = name.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let split = name.len() - digits;
    let number = name.get(split..)?.parse::<u64>().ok()?;
    Some((name.get(..split)?, number, digits))
}

/// `prefix` followed by `number`, zero-padded to `width` digits.
fn numbered(prefix: &str, number: u64, width: usize) -> String {
    format!("{prefix}{number:0width$}")
}

/// The next name of `prefix`'s number family after `from`: `prefix` + the
/// first number above `from` whose name is not taken.
fn next_numbered(prefix: &str, from: u64, width: usize, taken: &dyn Fn(&str) -> bool) -> String {
    let mut number = from;
    loop {
        number = number.saturating_add(1);
        let candidate = numbered(prefix, number, width);
        if !taken(&candidate) || number == u64::MAX {
            return candidate;
        }
    }
}

/// The highest number of `prefix`'s family among `names` (`None` when the
/// family is empty). A name belongs to the family when it is `prefix`
/// followed by digits only.
fn family_max<'a>(prefix: &str, names: impl IntoIterator<Item = &'a str>) -> Option<u64> {
    names
        .into_iter()
        .filter_map(trailing_number)
        .filter(|(p, _, _)| *p == prefix)
        .map(|(_, n, _)| n)
        .max()
}

/// The next free integer of a line: one above the highest all-digit name of
/// `names` (`0` when there is none), incremented until free.
pub fn next_free_integer<'a>(names: impl IntoIterator<Item = &'a str> + Clone) -> String {
    let taken: HashSet<&str> = names.clone().into_iter().collect();
    // With no all-digit name, `0` is free by definition.
    family_max("", names).map_or_else(
        || "0".to_string(),
        |max| next_numbered("", max, 1, &|n| taken.contains(n)),
    )
}

/// The End rule: the name of a station appended after `previous`.
pub fn name_after_end<'a>(
    previous: &str,
    names: impl IntoIterator<Item = &'a str> + Clone,
) -> String {
    let taken: HashSet<&str> = names.clone().into_iter().collect();
    match trailing_number(previous) {
        Some((prefix, number, width)) => {
            next_numbered(prefix, number, width, &|n| taken.contains(n))
        }
        None => next_free_integer(names),
    }
}

/// The Prepend rule: the name of a station added before `first`, the line's
/// old first station.
pub fn name_before_start<'a>(
    first: &str,
    names: impl IntoIterator<Item = &'a str> + Clone,
) -> String {
    let taken: HashSet<&str> = names.clone().into_iter().collect();
    match trailing_number(first) {
        Some((prefix, number, width)) => {
            let max = family_max(prefix, names).unwrap_or(number).max(number);
            next_numbered(prefix, max, width, &|n| taken.contains(n))
        }
        None => next_free_integer(names),
    }
}

/// The lowercase suffix number `index` stands for: `a`..`z`, then `aa`, `ab`,
/// .. `zz`, then `aaa`, ... (bijective base 26).
fn letter_suffix(index: u64) -> String {
    let mut n = index + 1;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        let digit = u8::try_from(n % 26).unwrap_or(0);
        out.push(char::from(b'a' + digit));
        n /= 26;
    }
    out.iter().rev().collect()
}

/// The Mid rule: `base` plus the first lowercase suffix whose name is not
/// taken. Also the rule a repeated name and a taken seed are made unique by.
pub fn name_with_free_letter(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut index: u64 = 0;
    loop {
        let candidate = format!("{base}{}", letter_suffix(index));
        if !taken(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

/// The seed of an unnamed line: `Line n` with the smallest `n >= 1` no line
/// holds.
pub fn seed_line_name(taken: &dyn Fn(&str) -> bool) -> String {
    let mut n: u64 = 1;
    loop {
        let candidate = format!("Line {n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// A repeated line name made unique: `name (k)` with the smallest `k >= 2`
/// no line holds.
pub fn dedupe_line_name(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut k: u64 = 2;
    loop {
        let candidate = format!("{name} ({k})");
        if !taken(&candidate) {
            return candidate;
        }
        k += 1;
    }
}

/// The family letter of a lettered name: an ASCII capital directly followed
/// by a digit (`"A3"` -> `A`, `"B12a"` -> `B`).
pub fn family_letter(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let first = chars.next().filter(char::is_ascii_uppercase)?;
    chars.next().filter(char::is_ascii_digit).map(|_| first)
}

/// Whether every name of a line is lettered (see [`family_letter`]). A line
/// with no names is not.
pub fn is_lettered<'a>(names: impl IntoIterator<Item = &'a str>) -> bool {
    let mut any = false;
    for name in names {
        if family_letter(name).is_none() {
            return false;
        }
        any = true;
    }
    any
}

/// The renames a Combine Lines applies to the two lines' station names, old
/// name -> new name, per line. Empty maps when nothing is prefixed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CombinePrefixes {
    pub kept: HashMap<String, String>,
    pub other: HashMap<String, String>,
}

/// The first capital letter not in `used`.
fn free_letter(used: &BTreeSet<char>) -> Option<char> {
    ('A'..='Z').find(|c| !used.contains(c))
}

/// Decide the Combine Lines prefixing (decision 5 of the line-identity
/// contract).
///
/// The two lines are prefixed when a name of the other line, its join station
/// left out (the join keeps the kept line's name), is also a name of the kept
/// line, or when either line is already lettered (a later combine with a
/// lettered line keeps the scheme). Then:
/// - the kept line keeps its letters when it is lettered; otherwise every name
///   gets the first capital no name of either line uses as a family letter
///   (`3` -> `A3`);
/// - the other line, when lettered, keeps every family whose letter the kept
///   line does not use and re-letters the others to the next free capitals;
///   otherwise every name gets the next free capital (`0` -> `B0`).
///
/// With all 26 letters in use nothing is prefixed; the caller's final
/// uniquify still makes every name unique.
pub fn plan_combine_prefixes(
    kept: &[&str],
    other: &[&str],
    other_join: Option<&str>,
) -> CombinePrefixes {
    let kept_set: HashSet<&str> = kept.iter().copied().collect();
    let collides = other
        .iter()
        .filter(|name| Some(**name) != other_join)
        .any(|name| kept_set.contains(name));
    let kept_lettered = is_lettered(kept.iter().copied());
    let other_lettered = is_lettered(other.iter().copied());
    if !(collides || kept_lettered || other_lettered) {
        return CombinePrefixes::default();
    }
    let mut used: BTreeSet<char> = kept
        .iter()
        .chain(other)
        .filter_map(|name| family_letter(name))
        .collect();

    let mut plan = CombinePrefixes::default();
    let kept_letters: BTreeSet<char> = if kept_lettered {
        kept.iter().filter_map(|name| family_letter(name)).collect()
    } else {
        let Some(letter) = free_letter(&used) else {
            return CombinePrefixes::default();
        };
        used.insert(letter);
        for name in kept {
            plan.kept
                .insert((*name).to_string(), format!("{letter}{name}"));
        }
        BTreeSet::from([letter])
    };

    if other_lettered {
        let families: BTreeSet<char> = other
            .iter()
            .filter_map(|name| family_letter(name))
            .collect();
        let mut relettered: HashMap<char, char> = HashMap::new();
        for family in families.iter().filter(|f| kept_letters.contains(f)) {
            let Some(letter) = free_letter(&used) else {
                return CombinePrefixes::default();
            };
            used.insert(letter);
            relettered.insert(*family, letter);
        }
        for name in other {
            let Some(family) = family_letter(name) else {
                continue;
            };
            if let Some(letter) = relettered.get(&family) {
                let rest = name.get(1..).unwrap_or_default();
                plan.other
                    .insert((*name).to_string(), format!("{letter}{rest}"));
            }
        }
    } else {
        let Some(letter) = free_letter(&used) else {
            return CombinePrefixes::default();
        };
        for name in other {
            plan.other
                .insert((*name).to_string(), format!("{letter}{name}"));
        }
    }
    plan
}

/// Make the names of one line unique, in station order: the first holder of a
/// name keeps it, a later one gets the Mid suffix of it (`5`, `5` -> `5`,
/// `5a`), and a missing name is seeded from the station's index (letter
/// suffixed when that is taken). `names[i]` is station `i`'s name. Returns
/// how many names were seeded and how many renamed.
pub fn settle_line_station_names(names: &mut [Option<String>]) -> (usize, usize) {
    // Every name present, so a seed or a rename never takes a name a later
    // station already holds.
    let mut taken: HashSet<String> = names.iter().flatten().cloned().collect();
    let mut kept: HashSet<String> = HashSet::with_capacity(names.len());
    let (mut seeded, mut renamed) = (0, 0);
    for (index, slot) in names.iter_mut().enumerate() {
        match slot {
            Some(name) if !name.trim().is_empty() && kept.insert(name.clone()) => {}
            Some(name) if !name.trim().is_empty() => {
                let fresh = name_with_free_letter(name, &|n| taken.contains(n));
                taken.insert(fresh.clone());
                kept.insert(fresh.clone());
                *slot = Some(fresh);
                renamed += 1;
            }
            _ => {
                let seed = index.to_string();
                let fresh = if taken.contains(&seed) {
                    name_with_free_letter(&seed, &|n| taken.contains(n))
                } else {
                    seed
                };
                taken.insert(fresh.clone());
                kept.insert(fresh.clone());
                *slot = Some(fresh);
                seeded += 1;
            }
        }
    }
    (seeded, renamed)
}

/// Fill the gaps of one line whose stations are partly named (an import whose
/// file names some stations, or rows the user added in an import wizard) by
/// the creation rules, in station order: a run after the last named station
/// continues it by the End rule (`6`, `7`, ...); a run between two named
/// stations takes Mid suffixes of the station before it (`5a`, `5b`); a run
/// before the first named station takes Prepend names going backwards from
/// it. A line with no name at all is left for the seeding rule. Returns how
/// many names were filled.
pub fn fill_line_station_names(names: &mut [Option<String>]) -> usize {
    let named = |slot: &Option<String>| slot.as_ref().is_some_and(|n| !n.trim().is_empty());
    let Some(first_named) = names.iter().position(named) else {
        return 0;
    };
    let last_named = names.iter().rposition(named).unwrap_or(first_named);
    let mut taken: HashSet<String> = names.iter().flatten().cloned().collect();
    let mut filled = 0;
    // Forward: the middle and the tail.
    let mut anchor: Option<String> = None;
    for index in first_named..names.len() {
        if named(&names[index]) {
            anchor.clone_from(&names[index]);
            continue;
        }
        let Some(base) = anchor.clone() else { continue };
        let fresh = if index > last_named {
            let previous = names[index - 1].clone().unwrap_or(base);
            name_after_end(&previous, taken.iter().map(String::as_str))
        } else {
            name_with_free_letter(&base, &|n| taken.contains(n))
        };
        taken.insert(fresh.clone());
        names[index] = Some(fresh);
        filled += 1;
    }
    // Backward: the head.
    for index in (0..first_named).rev() {
        let next = names.get(index + 1).cloned().flatten().unwrap_or_default();
        let fresh = name_before_start(&next, taken.iter().map(String::as_str));
        taken.insert(fresh.clone());
        names[index] = Some(fresh);
        filled += 1;
    }
    filled
}
