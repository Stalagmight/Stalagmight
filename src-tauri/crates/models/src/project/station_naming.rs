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
//!   after `2` -> `2a`, again -> `2b`, giving `2 - 2b - 2a - 3`. A name with
//!   no digit takes the suffix after a hyphen, so it still reads as its own
//!   word: after `Junction` -> `Junction-a`, then `Junction-b`.
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
/// taken (`2` -> `2a`, `2a` -> `2aa`, `A3` -> `A3a`). A `base` with no ASCII
/// digit takes the suffix after a hyphen (`Junction` -> `Junction-a`), so a
/// word does not run into its letter. Also the rule a repeated name and a
/// taken seed are made unique by.
pub fn name_with_free_letter(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    name_with_free_letter_from(base, &mut 0, taken)
}

/// [`name_with_free_letter`], searching from suffix number `*from` and
/// leaving `*from` past the one it returns. A pass that names many stations
/// after one base, over a taken set that only grows, keeps one cursor per base
/// so each search resumes where the last stopped: N repeats of one name cost
/// O(N) probes, not O(N^2).
pub fn name_with_free_letter_from(
    base: &str,
    from: &mut u64,
    taken: &dyn Fn(&str) -> bool,
) -> String {
    let separator = if base.bytes().any(|b| b.is_ascii_digit()) {
        ""
    } else {
        "-"
    };
    first_free(
        from,
        |index| format!("{base}{separator}{}", letter_suffix(index)),
        taken,
    )
}

/// The first `candidate(n)`, `n >= *from`, that is not taken, leaving `*from`
/// at the `n` after it. Over a taken set that only grows, a name once seen
/// taken stays taken, so resuming from the cursor finds the same name a search
/// from the start would.
fn first_free(
    from: &mut u64,
    candidate: impl Fn(u64) -> String,
    taken: &dyn Fn(&str) -> bool,
) -> String {
    loop {
        let name = candidate(*from);
        *from = from.saturating_add(1);
        if !taken(&name) || *from == u64::MAX {
            return name;
        }
    }
}

/// The seed of an unnamed line: `Line n` with the smallest `n >= 1` no line
/// holds.
pub fn seed_line_name(taken: &dyn Fn(&str) -> bool) -> String {
    seed_line_name_from(&mut 1, taken)
}

/// [`seed_line_name`] searching from `n = *from`, with a cursor as
/// [`name_with_free_letter_from`] keeps one.
pub fn seed_line_name_from(from: &mut u64, taken: &dyn Fn(&str) -> bool) -> String {
    first_free(from, |n| format!("Line {n}"), taken)
}

/// A repeated line name made unique: `name (k)` with the smallest `k >= 2`
/// no line holds.
pub fn dedupe_line_name(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    dedupe_line_name_from(name, &mut 2, taken)
}

/// [`dedupe_line_name`] searching from `k = *from`, with a cursor as
/// [`name_with_free_letter_from`] keeps one.
pub fn dedupe_line_name_from(name: &str, from: &mut u64, taken: &dyn Fn(&str) -> bool) -> String {
    first_free(from, |k| format!("{name} ({k})"), taken)
}

/// The family letter of a lettered name: an ASCII capital directly followed
/// by a digit (`"A3"` -> `A`, `"B12a"` -> `B`).
pub fn family_letter(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let first = chars.next().filter(char::is_ascii_uppercase)?;
    chars.next().filter(char::is_ascii_digit).map(|_| first)
}

/// Why a Combine Lines renames stations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombinePrefixReason {
    /// A name of the other line (its join station left out) is also a name
    /// of the kept line.
    Collision,
    /// The kept line is the product of an earlier lettered combine (two or
    /// more letter families) and the other line has no letter family, so it
    /// continues the scheme.
    Lettered,
}

/// The renames a Combine Lines applies to the two lines' station names, old
/// name -> new name, per line. Empty maps (and no reason) when nothing is
/// prefixed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CombinePrefixes {
    pub kept: HashMap<String, String>,
    pub other: HashMap<String, String>,
    /// The keys of `other` renamed by the Mid rule (a name the kept line also
    /// holds, `Sump` -> `Sump-a`) rather than lettered.
    pub other_suffixed: HashSet<String>,
    pub reason: Option<CombinePrefixReason>,
}

impl CombinePrefixes {
    /// Whether the plan renames nothing.
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty() && self.other.is_empty()
    }
}

/// The first capital letter not in `used`.
fn free_letter(used: &BTreeSet<char>) -> Option<char> {
    ('A'..='Z').find(|c| !used.contains(c))
}

/// The letter families among `names` (see [`family_letter`]).
pub fn letter_families<'a>(names: impl IntoIterator<Item = &'a str>) -> BTreeSet<char> {
    names.into_iter().filter_map(family_letter).collect()
}

/// Whether a name is number-led: its first character is an ASCII digit
/// (`0`, `12`, `1a`). Only such a name takes a Combine Lines family letter;
/// a custom name (`Junction`, `Sump`) or a lettered one (`A3`) keeps its
/// form.
pub fn is_number_led(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

/// Decide the Combine Lines prefixing (decision 5 of the line-identity
/// contract, as refined after review; documented in docs/SPEC.md 4.2).
///
/// The lines are prefixed only when
/// - a name of the other line, its join station left out (the join keeps the
///   kept line's name), is also a name of the kept line - a real collision;
///   or
/// - the kept line has two or more letter families (it is the product of an
///   earlier lettered combine) and the other line has none, so the other
///   line continues the scheme (`A0..A3 B1 B2` + `0 1` -> `.. C0 C1`).
///
/// A line with no collision is never renamed: kept `0..200` + other
/// `A1..A9`, or a line split from `E0..E10` and combined back, keep their
/// names. When prefixing:
/// - a line that already has letter families keeps its names, except that
///   the other line's families whose letter the kept line uses are
///   re-lettered to the next free capital (the kept line goes first);
/// - a line with no letter family gets the first free capital no name of
///   either line uses on its NUMBER-LED names ([`is_number_led`]) - the kept
///   line first, then the other (`3` -> `A3`, `0` -> `B0`); a name that does
///   not start with a digit (`Junction`) keeps it;
/// - a name of the other line (its join station aside) that still equals a
///   name of the kept line after that - a custom name both lines hold, or a
///   plain name of a lettered line - is made unique by the Mid rule on the
///   other line's copy (`Sump` -> `Sump-a`, `7` -> `7a`); the kept line's
///   copy keeps it.
///
/// With all 26 letters in use nothing is prefixed; the caller's uniquify
/// still makes every name unique.
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
    let kept_families = letter_families(kept.iter().copied());
    let other_families = letter_families(other.iter().copied());
    let reason = if collides {
        CombinePrefixReason::Collision
    } else if kept_families.len() >= 2 && other_families.is_empty() {
        CombinePrefixReason::Lettered
    } else {
        return CombinePrefixes::default();
    };
    let mut used: BTreeSet<char> = kept_families.union(&other_families).copied().collect();

    let mut plan = CombinePrefixes {
        reason: Some(reason),
        ..CombinePrefixes::default()
    };
    let kept_letters: BTreeSet<char> = if kept_families.is_empty() {
        let Some(letters) = letter_number_led(kept, &mut plan.kept, &mut used) else {
            return CombinePrefixes::default();
        };
        letters
    } else {
        kept_families
    };

    if other_families.is_empty() {
        if letter_number_led(other, &mut plan.other, &mut used).is_none() {
            return CombinePrefixes::default();
        }
    } else {
        let mut relettered: HashMap<char, char> = HashMap::new();
        for family in other_families.iter().filter(|f| kept_letters.contains(f)) {
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
    }
    suffix_other_line_clashes(kept, other, other_join, &mut plan);
    if plan.is_empty() {
        return CombinePrefixes::default();
    }
    plan
}

/// Letter the number-led names of a line with no letter family with the first
/// capital not in `used`, recording the renames: the letters it applied
/// (none for a line of custom names only, which leaves the letter free), or
/// `None` when every letter is in use.
fn letter_number_led(
    names: &[&str],
    renames: &mut HashMap<String, String>,
    used: &mut BTreeSet<char>,
) -> Option<BTreeSet<char>> {
    if !names.iter().any(|name| is_number_led(name)) {
        return Some(BTreeSet::new());
    }
    let letter = free_letter(used)?;
    used.insert(letter);
    for name in names.iter().filter(|name| is_number_led(name)) {
        renames.insert((*name).to_string(), format!("{letter}{name}"));
    }
    Some(BTreeSet::from([letter]))
}

/// The last step of [`plan_combine_prefixes`]: a name of the other line (its
/// join station aside, whose name is retired) that still equals a name of the
/// kept line once both are lettered gets the Mid rule on the other line's
/// copy, avoiding every name either line ends up with.
fn suffix_other_line_clashes(
    kept: &[&str],
    other: &[&str],
    other_join: Option<&str>,
    plan: &mut CombinePrefixes,
) {
    let after = |renames: &HashMap<String, String>, name: &str| -> String {
        renames
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    };
    let kept_after: HashSet<String> = kept.iter().map(|name| after(&plan.kept, name)).collect();
    let mut taken: HashSet<String> = kept_after.clone();
    taken.extend(other.iter().map(|name| after(&plan.other, name)));
    for name in other.iter().filter(|name| Some(**name) != other_join) {
        let current = after(&plan.other, name);
        if !kept_after.contains(&current) {
            continue;
        }
        let fresh = name_with_free_letter(&current, &|n| taken.contains(n));
        taken.insert(fresh.clone());
        plan.other.insert((*name).to_string(), fresh);
        plan.other_suffixed.insert((*name).to_string());
    }
}

/// Whether `c` may not appear in a name the user types for a station, a line
/// or a Section: a control character (Unicode `Cc`: tab, newline, NUL, ...)
/// or an invisible formatting character that can make one name look like
/// another - the zero-width space and (non-)joiners (U+200B..U+200D), the
/// left-to-right and right-to-left marks (U+200E, U+200F), the bidi
/// embeddings and overrides (U+202A..U+202E), the bidi isolates
/// (U+2066..U+2069) and the byte-order mark (U+FEFF). Visible separators
/// such as ` / ` stay allowed.
pub fn is_forbidden_name_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// The refusal for a user-typed `what` name (`"station"`, `"line"`,
/// `"section"`) that holds a character [`is_forbidden_name_char`] rejects,
/// naming the first one by its code point; `None` when the name is clean.
pub fn name_characters_refusal(what: &str, name: &str) -> Option<String> {
    name.chars().find(|c| is_forbidden_name_char(*c)).map(|c| {
        format!(
            "A {what} name cannot contain control or invisible formatting characters \
                 (U+{:04X})",
            u32::from(c)
        )
    })
}

/// Make the names of one line unique, in station order: the first holder of a
/// name keeps it, a later one gets the Mid suffix of it (`5`, `5` -> `5`,
/// `5a`), and a missing name is seeded from the station's index (letter
/// suffixed when that is taken). `names[i]` is station `i`'s name. Returns
/// how many names were seeded and how many renamed.
pub fn settle_line_station_names(names: &mut [Option<String>]) -> (usize, usize) {
    // Names are stored and compared trimmed: `5 ` is `5`, and a blank one is
    // missing. A trimmed name counts as renamed.
    let mut trimmed = 0;
    for slot in names.iter_mut() {
        if let Some(name) = slot {
            if name.trim().len() != name.len() {
                let bare = name.trim();
                *slot = (!bare.is_empty()).then(|| bare.to_string());
                trimmed += 1;
            }
        }
    }
    // Every name present, so a seed or a rename never takes a name a later
    // station already holds.
    let mut taken: HashSet<String> = names.iter().flatten().cloned().collect();
    let mut kept: HashSet<String> = HashSet::with_capacity(names.len());
    // One suffix cursor per base: `taken` only grows (see `first_free`).
    let mut letter_from: HashMap<String, u64> = HashMap::new();
    let (mut seeded, mut renamed) = (0, 0);
    for (index, slot) in names.iter_mut().enumerate() {
        match slot {
            Some(name) if !name.trim().is_empty() && kept.insert(name.clone()) => {}
            Some(name) if !name.trim().is_empty() => {
                let from = letter_from.entry(name.clone()).or_insert(0);
                let fresh = name_with_free_letter_from(name, from, &|n| taken.contains(n));
                taken.insert(fresh.clone());
                kept.insert(fresh.clone());
                *slot = Some(fresh);
                renamed += 1;
            }
            _ => {
                let seed = index.to_string();
                let fresh = if taken.contains(&seed) {
                    let from = letter_from.entry(seed.clone()).or_insert(0);
                    name_with_free_letter_from(&seed, from, &|n| taken.contains(n))
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
    (seeded, renamed + trimmed)
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
    // Each step below names one station against `taken`, which only grows: the
    // End and Prepend rules are applied here with the set (and the Prepend
    // family's highest number) kept across steps, and the Mid rule with a
    // cursor per base, so a long unnamed run costs O(run) rather than a
    // rebuild of the set (or a rescan of the suffixes) per station.
    let mut letter_from: HashMap<String, u64> = HashMap::new();
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
            // `name_after_end`, without rebuilding the set per station.
            match trailing_number(&previous) {
                Some((prefix, number, width)) => {
                    next_numbered(prefix, number, width, &|n| taken.contains(n))
                }
                None => next_free_integer(taken.iter().map(String::as_str)),
            }
        } else {
            let from = letter_from.entry(base.clone()).or_insert(0);
            name_with_free_letter_from(&base, from, &|n| taken.contains(n))
        };
        taken.insert(fresh.clone());
        names[index] = Some(fresh);
        filled += 1;
    }
    // Backward: the head, by `name_before_start` with each family's highest
    // number computed once and raised as the run names its stations.
    let mut family_tops: HashMap<String, u64> = HashMap::new();
    for index in (0..first_named).rev() {
        let next = names.get(index + 1).cloned().flatten().unwrap_or_default();
        let fresh = match trailing_number(&next) {
            Some((prefix, number, width)) => {
                let top = *family_tops.entry(prefix.to_string()).or_insert_with(|| {
                    family_max(prefix, taken.iter().map(String::as_str)).unwrap_or(number)
                });
                next_numbered(prefix, top.max(number), width, &|n| taken.contains(n))
            }
            None => next_free_integer(taken.iter().map(String::as_str)),
        };
        if let Some((prefix, number, _)) = trailing_number(&fresh) {
            if let Some(top) = family_tops.get_mut(prefix) {
                *top = (*top).max(number);
            }
        }
        taken.insert(fresh.clone());
        names[index] = Some(fresh);
        filled += 1;
    }
    filled
}
