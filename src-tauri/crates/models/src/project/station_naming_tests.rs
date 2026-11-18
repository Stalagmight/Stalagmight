//! The naming rules (`station_naming`) as pure functions.

use std::collections::{BTreeSet, HashSet};

use super::station_naming::{
    dedupe_line_name, family_letter, fill_line_station_names, is_number_led, letter_families,
    name_after_end, name_before_start, name_with_free_letter, next_free_integer,
    plan_combine_prefixes, seed_line_name, settle_line_station_names, trailing_number,
    CombinePrefixReason, CombinePrefixes,
};

fn set<'a>(names: &'a [&'a str]) -> HashSet<&'a str> {
    names.iter().copied().collect()
}

fn slots(names: &[Option<&str>]) -> Vec<Option<String>> {
    names.iter().map(|n| n.map(str::to_string)).collect()
}

fn filled(names: &[Option<String>]) -> Vec<&str> {
    names.iter().map(|n| n.as_deref().unwrap_or("?")).collect()
}

#[test]
fn a_trailing_number_splits_off_with_its_width() {
    assert_eq!(trailing_number("A3"), Some(("A", 3, 1)));
    assert_eq!(trailing_number("12"), Some(("", 12, 2)));
    assert_eq!(trailing_number("007"), Some(("", 7, 3)));
    assert_eq!(trailing_number("X"), None);
    assert_eq!(trailing_number("2a"), None);
    assert_eq!(trailing_number(""), None);
}

#[test]
fn the_end_rule_adds_one_to_the_previous_name() {
    assert_eq!(name_after_end("5", ["0", "5"]), "6");
    assert_eq!(name_after_end("A3", ["A3"]), "A4");
    assert_eq!(name_after_end("007", ["007"]), "008");
    // Taken: keep incrementing.
    assert_eq!(name_after_end("5", ["5", "6", "7"]), "8");
    // No trailing number: the next free integer of the line.
    assert_eq!(name_after_end("X", ["X", "0", "1", "4"]), "5");
    assert_eq!(name_after_end("X", ["X"]), "0");
}

#[test]
fn the_mid_rule_takes_the_first_free_letter() {
    let taken = set(&["2", "3"]);
    assert_eq!(name_with_free_letter("2", &|n| taken.contains(n)), "2a");
    let taken = set(&["2", "2a", "3"]);
    assert_eq!(name_with_free_letter("2", &|n| taken.contains(n)), "2b");
    // After z come the two-letter suffixes.
    let mut names: Vec<String> = ('a'..='z').map(|c| format!("4{c}")).collect();
    names.push("4".to_string());
    let held: HashSet<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(name_with_free_letter("4", &|n| held.contains(n)), "4aa");
}

#[test]
fn the_mid_rule_hyphenates_a_name_with_no_digit() {
    // A custom name keeps reading as its own word: `Junction-a`, not
    // `Junctiona`.
    let taken = set(&["Junction"]);
    assert_eq!(
        name_with_free_letter("Junction", &|n| taken.contains(n)),
        "Junction-a"
    );
    let taken = set(&["Junction", "Junction-a"]);
    assert_eq!(
        name_with_free_letter("Junction", &|n| taken.contains(n)),
        "Junction-b"
    );
    // Any digit in the base keeps the plain concatenation.
    let none = set(&[]);
    assert_eq!(name_with_free_letter("2a", &|n| none.contains(n)), "2aa");
    assert_eq!(name_with_free_letter("A3", &|n| none.contains(n)), "A3a");
    assert_eq!(
        name_with_free_letter("Sump2", &|n| none.contains(n)),
        "Sump2a"
    );
}

#[test]
fn the_prepend_rule_takes_the_next_number_of_the_first_station_family() {
    assert_eq!(
        name_before_start("0", ["0", "1", "2", "3", "4", "5", "6"]),
        "7"
    );
    assert_eq!(name_before_start("A0", ["A0", "A1", "A2", "A3"]), "A4");
    // The family is the old first station's, not the line's other names.
    assert_eq!(name_before_start("A0", ["A0", "A1", "9"]), "A2");
    assert_eq!(name_before_start("X", ["X", "Y"]), "0");
}

#[test]
fn next_free_integer_follows_the_all_digit_names() {
    assert_eq!(next_free_integer(["3", "1", "A9"]), "4");
    assert_eq!(next_free_integer(["A", "B"]), "0");
    assert_eq!(next_free_integer(Vec::<&str>::new()), "0");
}

#[test]
fn settling_seeds_by_index_and_letters_the_later_repeat() {
    let mut names = slots(&[Some("5"), Some("5"), None, Some("1")]);
    let (seeded, renamed) = settle_line_station_names(&mut names);
    assert_eq!(filled(&names), ["5", "5a", "2", "1"]);
    assert_eq!((seeded, renamed), (1, 1));

    // A seed that is taken is letter-suffixed.
    let mut names = slots(&[Some("1"), None]);
    settle_line_station_names(&mut names);
    assert_eq!(filled(&names), ["1", "1a"]);

    // A blank name counts as missing.
    let mut names = slots(&[Some("  "), Some("x")]);
    settle_line_station_names(&mut names);
    assert_eq!(filled(&names), ["0", "x"]);

    // A repeated custom name takes the hyphenated suffix.
    let mut names = slots(&[Some("Sump"), Some("Sump"), Some("Sump")]);
    settle_line_station_names(&mut names);
    assert_eq!(filled(&names), ["Sump", "Sump-a", "Sump-b"]);
}

#[test]
fn filling_a_partly_named_line_follows_the_creation_rules() {
    // A run inside the line takes letters, the tail continues the numbers,
    // the head takes prepend names backwards.
    let mut names = slots(&[None, Some("5"), None, None, Some("6"), None, None]);
    assert_eq!(fill_line_station_names(&mut names), 5);
    assert_eq!(filled(&names), ["9", "5", "5a", "5b", "6", "7", "8"]);

    // Nothing named: left for the seeding rule.
    let mut names = slots(&[None, None]);
    assert_eq!(fill_line_station_names(&mut names), 0);

    // A run after a custom name takes hyphenated letters.
    let mut names = slots(&[Some("Junction"), None, None, Some("6")]);
    fill_line_station_names(&mut names);
    assert_eq!(
        filled(&names),
        ["Junction", "Junction-a", "Junction-b", "6"]
    );
}

#[test]
fn line_names_seed_and_dedupe() {
    let taken = set(&["Line 1", "Line 3"]);
    assert_eq!(seed_line_name(&|n| taken.contains(n)), "Line 2");
    let taken = set(&["Main", "Main (2)"]);
    assert_eq!(dedupe_line_name("Main", &|n| taken.contains(n)), "Main (3)");
}

#[test]
fn a_lettered_name_is_a_capital_then_a_digit() {
    assert_eq!(family_letter("A3"), Some('A'));
    assert_eq!(family_letter("B12a"), Some('B'));
    assert_eq!(family_letter("a3"), None);
    assert_eq!(family_letter("AB"), None);
    assert_eq!(
        letter_families(["A0", "B1", "A2", "7"]),
        BTreeSet::from(['A', 'B'])
    );
    assert!(letter_families(["0", "1"]).is_empty());
}

#[test]
fn colliding_lines_are_prefixed_a_and_b() {
    // The contract's first example: kept 0..3 joined at 3 to other 0..2.
    let plan = plan_combine_prefixes(&["0", "1", "2", "3"], &["0", "1", "2"], Some("0"));
    assert_eq!(plan.kept["0"], "A0");
    assert_eq!(plan.kept["3"], "A3");
    assert_eq!(plan.other["0"], "B0");
    assert_eq!(plan.other["2"], "B2");
}

#[test]
fn a_lettered_kept_line_gives_the_other_the_next_free_letter() {
    // The contract's second example: kept `A0 A1 A2 A3=B0 B1 B2` (the join
    // kept A3) + other `0 1` -> the other becomes C.
    let kept = ["A0", "A1", "A2", "A3", "B1", "B2"];
    let plan = plan_combine_prefixes(&kept, &["0", "1"], Some("0"));
    assert!(
        plan.kept.is_empty(),
        "a lettered kept line keeps its letters"
    );
    assert_eq!(plan.other["0"], "C0");
    assert_eq!(plan.other["1"], "C1");
}

#[test]
fn lines_that_do_not_collide_are_left_alone() {
    // The join station's own name never counts: it is retired.
    let plan = plan_combine_prefixes(&["0", "1", "2", "3"], &["3", "4", "5"], Some("3"));
    assert!(plan.kept.is_empty() && plan.other.is_empty());
}

#[test]
fn a_lettered_other_line_reletters_only_the_families_it_shares() {
    // `A1` collides; the kept line keeps its letters, the other line's
    // clashing family A moves to the next free letter, B is left alone.
    let plan = plan_combine_prefixes(&["A0", "A1"], &["A9", "A1", "B6", "B7"], Some("A9"));
    assert_eq!(plan.reason, Some(CombinePrefixReason::Collision));
    assert!(plan.kept.is_empty());
    assert_eq!(plan.other["A9"], "C9");
    assert_eq!(plan.other["A1"], "C1");
    assert!(!plan.other.contains_key("B6"));
}

#[test]
fn an_unlettered_kept_line_and_a_lettered_other_without_a_collision_keep_their_names() {
    // Review counter-example 1: kept `0..200` + other `A1..A9`.
    let kept: Vec<String> = (0..=200).map(|i| i.to_string()).collect();
    let kept: Vec<&str> = kept.iter().map(String::as_str).collect();
    let other: Vec<String> = (1..=9).map(|i| format!("A{i}")).collect();
    let other: Vec<&str> = other.iter().map(String::as_str).collect();
    let plan = plan_combine_prefixes(&kept, &other, Some("A1"));
    assert_eq!(plan, CombinePrefixes::default());
}

#[test]
fn a_line_split_from_one_family_and_combined_back_keeps_its_names() {
    // Review counter-example 2: an imported `E0..E10` split at E5 and
    // combined back: the join's own name is retired, nothing collides.
    let kept = ["E0", "E1", "E2", "E3", "E4", "E5"];
    let other = ["E5", "E6", "E7", "E8", "E9", "E10"];
    let plan = plan_combine_prefixes(&kept, &other, Some("E5"));
    assert_eq!(plan, CombinePrefixes::default());
    // One letter family is not the product of an earlier lettered combine:
    // an unlettered other line without a collision keeps its names too.
    let plan = plan_combine_prefixes(&["A0", "A1", "A2"], &["0", "1"], Some("0"));
    assert_eq!(plan, CombinePrefixes::default());
}

#[test]
fn a_lettered_kept_line_with_a_plain_name_keeps_it_and_the_other_takes_a_free_letter() {
    // A collision on a plain name of a lettered kept line: the kept line
    // keeps every name (the caller's uniquify settles the rest), the
    // unlettered other line takes the first free letter.
    let plan = plan_combine_prefixes(&["A0", "A1", "7"], &["7", "8"], Some("8"));
    assert_eq!(plan.reason, Some(CombinePrefixReason::Collision));
    assert!(plan.kept.is_empty());
    assert_eq!(plan.other["7"], "B7");
    assert_eq!(plan.other["8"], "B8");
}

#[test]
fn a_number_led_name_starts_with_a_digit() {
    assert!(is_number_led("0"));
    assert!(is_number_led("12"));
    assert!(is_number_led("1a"));
    assert!(!is_number_led("A3"));
    assert!(!is_number_led("Junction"));
    assert!(!is_number_led(""));
}

#[test]
fn a_combine_letters_only_number_led_names_and_keeps_custom_ones() {
    // The real-app case: kept `0 1 Junction Junctiona 5 .. 10` + other
    // `0 1 2 3` joined at the other's `0`. The digits collide; the custom
    // names are the kept line's own and keep their names.
    let kept = [
        "0",
        "1",
        "Junction",
        "Junctiona",
        "5",
        "6",
        "7",
        "8",
        "9",
        "10",
    ];
    let plan = plan_combine_prefixes(&kept, &["0", "1", "2", "3"], Some("0"));
    assert_eq!(plan.reason, Some(CombinePrefixReason::Collision));
    assert_eq!(plan.kept["0"], "A0");
    assert_eq!(plan.kept["1"], "A1");
    assert_eq!(plan.kept["5"], "A5");
    assert_eq!(plan.kept["10"], "A10");
    assert!(!plan.kept.contains_key("Junction"));
    assert!(!plan.kept.contains_key("Junctiona"));
    assert_eq!(plan.kept.len(), 8);
    assert_eq!(plan.other["1"], "B1");
    assert_eq!(plan.other["3"], "B3");
}

#[test]
fn a_custom_name_both_lines_hold_is_suffixed_on_the_other_line() {
    // `Sump` is in both lines: the kept line's copy keeps it, the other
    // line's takes the hyphenated Mid suffix; the digits are lettered.
    let plan = plan_combine_prefixes(&["0", "1", "Sump", "3"], &["0", "Sump", "2"], Some("0"));
    assert_eq!(plan.reason, Some(CombinePrefixReason::Collision));
    assert!(!plan.kept.contains_key("Sump"));
    assert_eq!(plan.kept["3"], "A3");
    assert_eq!(plan.other["Sump"], "Sump-a");
    assert_eq!(plan.other["2"], "B2");
    assert_eq!(plan.other_suffixed, HashSet::from(["Sump".to_string()]));

    // The suffix avoids every name either line ends up with.
    let plan = plan_combine_prefixes(&["0", "Sump", "Sump-a"], &["0", "Sump", "2"], Some("0"));
    assert_eq!(plan.other["Sump"], "Sump-b");

    // A custom name that is the other line's join station is retired, so it
    // collides with nothing.
    let plan = plan_combine_prefixes(&["0", "1", "Sump"], &["Sump", "5", "6"], Some("Sump"));
    assert_eq!(plan, CombinePrefixes::default());
}

#[test]
fn a_line_of_custom_names_spends_no_letter() {
    // The kept line has no number-led name, so it keeps every name and the
    // other line takes the first letter, `A`.
    let plan = plan_combine_prefixes(&["Entrance", "Sump", "Pit"], &["0", "Sump", "2"], Some("0"));
    assert!(plan.kept.is_empty());
    assert_eq!(plan.other["2"], "A2");
    assert_eq!(plan.other["Sump"], "Sump-a");

    // A lettered kept line continued by a line of custom names renames
    // nothing.
    let kept = ["A0", "A1", "B1", "B2"];
    let plan = plan_combine_prefixes(&kept, &["Pit", "Dome"], Some("Pit"));
    assert_eq!(plan, CombinePrefixes::default());
}

#[test]
fn a_plain_name_a_lettered_other_line_shares_is_suffixed_on_the_other_line() {
    // Both lines are lettered with letters of their own; their plain `7`
    // collides, and only the other line's copy changes.
    let plan = plan_combine_prefixes(&["A0", "B1", "7"], &["C0", "7"], Some("C0"));
    assert_eq!(plan.reason, Some(CombinePrefixReason::Collision));
    assert!(plan.kept.is_empty());
    assert_eq!(plan.other.len(), 1);
    assert_eq!(plan.other["7"], "7a");
}
