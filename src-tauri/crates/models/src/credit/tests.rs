use super::*;

fn names(people: &[Person]) -> Vec<&str> {
    people.iter().map(|p| p.name.as_str()).collect()
}

fn person(name: &str, email: Option<&str>) -> Person {
    Person::new(name, email).unwrap()
}

#[test]
fn legacy_splitter_table() {
    // (input, explorers, surveyors)
    let cases: &[(&str, &[&str], &[&str])] = &[
        // The Ariane importer's "explorers / surveyors" join.
        ("nh / ds", &["nh"], &["ds"]),
        ("A, B, C / D; E", &["A", "B", "C"], &["D", "E"]),
        // Its halves are Ariane name lists: no `Last, First` rule (#427).
        ("Doe, John / C", &["Doe", "John"], &["C"]),
        ("ds, kb / ab", &["ds", "kb"], &["ab"]),
        // One half empty, as the importer wrote it for a missing role.
        (" / ds", &[], &["ds"]),
        ("nh / ", &["nh"], &[]),
        // Explorers only survives trimming (an older build's Edit Section
        // trims on every save): a trimmed text ending in " /", with no other
        // " / ", is explorers only too.
        ("nh /", &["nh"], &[]),
        ("nh; kb /\t", &["nh", "kb"], &[]),
        ("nh / ds /", &["nh"], &["ds /"]),
        ("a / b / c /", &[], &["a / b / c /"]),
        ("nh /ds", &[], &["nh /ds"]),
        // A placeholder half names nobody.
        ("unknown / ds", &[], &["ds"]),
        ("nh / ?", &["nh"], &[]),
        // Anything else is surveyors only.
        ("John Doe", &[], &["John Doe"]),
        ("a / b / c", &[], &["a / b / c"]),
        ("a/b", &[], &["a/b"]),
        // In free text, `Last, First` is one person...
        ("Doe, John", &[], &["Doe, John"]),
        ("Doe,John", &[], &["Doe,John"]),
        ("ds, kb", &[], &["ds, kb"]),
        // ...but not when either part is more than one word, or there is a `;`.
        ("John Doe, Jane Roe", &[], &["John Doe", "Jane Roe"]),
        ("A, B; C", &[], &["A", "B", "C"]),
        ("A, B, C", &[], &["A", "B", "C"]),
        ("A; B", &[], &["A", "B"]),
        // Pieces are trimmed; empty pieces and placeholders are dropped.
        ("  A ;; , B  ", &[], &["A", "B"]),
        ("unknown", &[], &[]),
        ("None", &[], &[]),
        ("?", &[], &[]),
        ("UNKNOWN; A", &[], &["A"]),
        ("", &[], &[]),
        ("   ", &[], &[]),
        // Duplicates go, by identity (lowercase + whitespace collapse).
        ("A, a, B", &[], &["A", "B"]),
        ("John  Doe; john doe", &[], &["John  Doe"]),
    ];
    for (input, explorers, surveyors) in cases {
        let (e, s) = legacy_people(input);
        assert_eq!(names(&e), *explorers, "explorers of {input:?}");
        assert_eq!(names(&s), *surveyors, "surveyors of {input:?}");
        assert_eq!(
            split_roles(input, NameSource::FreeText),
            (e, s),
            "legacy_people is the free-text splitter"
        );
    }
}

/// The same splitter for text from an Ariane file (#719 review): a name list
/// everywhere, so only the `Last, First` cases differ from the legacy column's
/// free text, and a single name is still a surveyor. Mirrored by the vitest
/// table in `src/utils/__tests__/sectionCredit.test.ts`.
#[test]
fn ariane_name_list_splitter_table() {
    // (input, explorers, surveyors)
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("Tester", &[], &["Tester"]),
        ("ds, kb", &[], &["ds", "kb"]),
        ("Doe, John", &[], &["Doe", "John"]),
        ("Doe,John", &[], &["Doe", "John"]),
        ("ds, kb / ab", &["ds", "kb"], &["ab"]),
        ("nh /", &["nh"], &[]),
        ("A, B; C", &[], &["A", "B", "C"]),
        ("unknown / ds", &[], &["ds"]),
        ("a / b / c", &[], &["a / b / c"]),
    ];
    for (input, explorers, surveyors) in cases {
        let (e, s) = split_roles(input, NameSource::NameList);
        assert_eq!(names(&e), *explorers, "explorers of {input:?}");
        assert_eq!(names(&s), *surveyors, "surveyors of {input:?}");
    }
}

#[test]
fn a_name_list_splits_on_both_separators_without_the_last_first_rule() {
    assert_eq!(
        names(&split_role("Doe, John", NameSource::NameList)),
        ["Doe", "John"]
    );
    assert_eq!(
        names(&split_role("A;B, c ; a", NameSource::NameList)),
        ["A", "B", "c"]
    );
    assert!(split_role(" ; , ", NameSource::NameList).is_empty());
}

#[test]
fn identity_is_lowercased_name_and_email() {
    let a = person("John  Doe", None);
    assert!(a.same_as(&person(" john doe ", None)));
    assert!(!a.same_as(&person("John Doe", Some("j@x.org"))));
    let b = person("John Doe", Some("J@X.org"));
    assert!(b.same_as(&person("john doe", Some("j@x.ORG"))));
    assert!(!b.same_as(&person("John Doe", Some("k@x.org"))));
    // An empty email is no email.
    assert_eq!(person("A", Some("  ")).email, None);
    assert!(Person::named("   ").is_none());
}

/// The vectors `src/utils/__tests__/people.test.ts` pins for the frontend's
/// `personKey`: the two implementations must agree on every one of them.
#[test]
fn identity_vectors_shared_with_the_frontend() {
    let raw = |name: &str, email: Option<&str>| Person {
        name: name.to_string(),
        email: email.map(str::to_string),
    };
    // No Unicode normalization and no sharp-s folding.
    assert!(!raw("Jose\u{301}", None).same_as(&raw("Jos\u{e9}", None)));
    assert!(!raw("Strau\u{df}", None).same_as(&raw("STRAUSS", None)));
    assert!(raw("STRAU\u{df}", None).same_as(&raw("strau\u{df}", None)));
    // NEL and NBSP are whitespace; U+FEFF is not.
    assert!(raw("Ann\u{85}Lee", None).same_as(&raw("ann lee", None)));
    assert!(raw("\u{a0}Ann \u{3000} Lee\u{2003}", None).same_as(&raw("ann lee", None)));
    assert!(!raw("Ann\u{feff}Lee", None).same_as(&raw("ann lee", None)));
    // The email is trimmed and lower-cased, its inner text kept.
    assert!(raw("Ann", Some("\u{a0}A@X.org\n")).same_as(&raw("ann", Some("a@x.org"))));
    assert!(!raw("Ann", Some("a @x.org")).same_as(&raw("ann", Some("a@x.org"))));
}

#[test]
fn join_distinct_names_writes_each_display_name_once() {
    let people = vec![
        person("Jo Diver", Some("jo@caves.example")),
        person("Ann", None),
        person("JO   diver", Some("jo.diver@divers.example")),
        person("ann", Some("ann@x.org")),
    ];
    assert_eq!(
        join_names(&people).as_deref(),
        Some("Jo Diver, Ann, JO   diver, ann")
    );
    assert_eq!(
        join_distinct_names(&people).as_deref(),
        Some("Jo Diver, Ann")
    );
    assert_eq!(join_distinct_names(&[]), None);
}

/// #718: the legacy column's text per case, and what [`legacy_people`] reads
/// back from it.
#[test]
fn legacy_credit_text_table() {
    let people =
        |names: &[&str]| -> Vec<Person> { names.iter().map(|n| person(n, None)).collect() };
    // (explorers, surveyors, text, explorers read back, surveyors read back)
    type Case<'a> = (
        &'a [&'a str],
        &'a [&'a str],
        Option<&'a str>,
        &'a [&'a str],
        &'a [&'a str],
    );
    let cases: &[Case] = &[
        (&[], &[], None, &[], &[]),
        // Surveyors only: the text every build before #718 wrote.
        (
            &[],
            &["Ann Lee", "Bo Ray"],
            Some("Ann Lee, Bo Ray"),
            &[],
            &["Ann Lee", "Bo Ray"],
        ),
        // Explorers only: an empty second half.
        (&["Ida Ice"], &[], Some("Ida Ice /"), &["Ida Ice"], &[]),
        // The " / " form's halves are name lists: two one-word names stay two.
        (
            &["E1", "E2"],
            &["S1", "S2"],
            Some("E1, E2 / S1, S2"),
            &["E1", "E2"],
            &["S1", "S2"],
        ),
        // Surveyors only, two one-word names: "; ", since free text would
        // read "Ann, Bo" as one `Last, First` person.
        (&[], &["Ann", "Bo"], Some("Ann; Bo"), &[], &["Ann", "Bo"]),
        // One person named `Last, First` stays one in free text...
        (&[], &["Doe, John"], Some("Doe, John"), &[], &["Doe, John"]),
        // ...but not in the " / " form (a `,` inside one name cannot come
        // back from a name list; this build's per-shot rows keep it).
        (
            &["Doe, John"],
            &[],
            Some("Doe, John /"),
            &["Doe", "John"],
            &[],
        ),
        (
            &["E1", "E2", "E3"],
            &["S1 S", "S2"],
            Some("E1, E2, E3 / S1 S, S2"),
            &["E1", "E2", "E3"],
            &["S1 S", "S2"],
        ),
        // A person in both roles is in both halves.
        (
            &["Ann Lee"],
            &["Ann Lee"],
            Some("Ann Lee / Ann Lee"),
            &["Ann Lee"],
            &["Ann Lee"],
        ),
        // A `/` that could join the separator is written `-`, in either role;
        // one with a letter on either side stays.
        (&["A / B"], &[], Some("A - B /"), &["A - B"], &[]),
        (&[], &["A / B"], Some("A - B"), &[], &["A - B"]),
        (&["X /"], &["/ Y"], Some("X - / - Y"), &["X -"], &["- Y"]),
        (&["/"], &["S"], Some("- / S"), &["-"], &["S"]),
        (
            &["E", "/ F"],
            &["a//b"],
            Some("E, - F / a//b"),
            &["E", "- F"],
            &["a//b"],
        ),
        (
            &["a/b"],
            &["c /d", "e/ f"],
            Some("a/b / c /d, e/ f"),
            &["a/b"],
            &["c /d", "e/ f"],
        ),
        // Each written name once per half, compared like `Person::name_key`.
        (
            &["A / B", "a - b"],
            &["Cy", "CY"],
            Some("A - B / Cy"),
            &["A - B"],
            &["Cy"],
        ),
    ];
    for (explorers, surveyors, text, read_explorers, read_surveyors) in cases {
        let written = legacy_credit_text(&people(explorers), &people(surveyors));
        assert_eq!(written.as_deref(), *text, "{explorers:?} / {surveyors:?}");
        // The text as written, and as an older build's Edit Section saves it.
        let written = written.unwrap_or_default();
        for read in [written.as_str(), written.trim()] {
            let (e, s) = legacy_people(read);
            assert_eq!(names(&e), *read_explorers, "explorers of {read:?}");
            assert_eq!(names(&s), *read_surveyors, "surveyors of {read:?}");
        }
    }
    // A raw name (a hand-built payload) is trimmed before its edges are
    // judged: "A /\t" keeps no `/` that could join the separator.
    let raw = |name: &str| Person {
        name: name.to_string(),
        email: None,
    };
    for (explorers, surveyors, text) in [
        (vec![raw("A /\t")], vec![raw("S")], "A - / S"),
        (vec![raw("A /\t")], vec![], "A - /"),
        (vec![raw("E")], vec![raw("\t/ B")], "E / - B"),
    ] {
        let written = legacy_credit_text(&explorers, &surveyors).unwrap();
        assert_eq!(written, text);
        let (e, s) = legacy_people(&written);
        assert_eq!(names(&e).len(), explorers.len(), "{written:?}");
        assert_eq!(names(&s).len(), surveyors.len(), "{written:?}");
    }
    // Same-named people (different emails) are one name in the text.
    let jo = [
        person("Jo Diver", Some("jo@caves.example")),
        person("jo  diver", Some("jo.diver@divers.example")),
    ];
    assert_eq!(
        legacy_credit_text(&jo, &jo).as_deref(),
        Some("Jo Diver / Jo Diver")
    );
}

/// #718: whatever the names, the text splits back into the roles it was
/// written from. Exhaustive over every name of up to three characters from
/// `a`, space and `/` - every way a name can touch the `" / "` separator - in
/// lists of one, two and three names per role (two one-word surveyors alone
/// are written with "; ", see `legacy_credit_text_table`).
#[test]
fn legacy_credit_text_always_splits_back_into_its_roles() {
    let mut raw: Vec<String> = vec![String::new()];
    let mut names_up_to_3: Vec<String> = Vec::new();
    for _ in 0..3 {
        raw = raw
            .iter()
            .flat_map(|prefix| ['a', ' ', '/'].map(|c| format!("{prefix}{c}")))
            .collect();
        names_up_to_3.extend(raw.iter().cloned());
    }
    let pool: Vec<Person> = names_up_to_3
        .iter()
        .filter_map(|n| Person::named(n))
        .collect();
    let filler = person("z z", None);
    // What each list reads back as: its written names, each once.
    let expected = |list: &[Person]| -> Vec<String> {
        let mut seen = HashSet::new();
        list.iter()
            .map(|p| legacy_name(&p.name))
            .filter(|n| seen.insert(Person::named(n).unwrap().name_key()))
            .collect()
    };
    let check = |explorers: &[Person], surveyors: &[Person]| {
        let text = legacy_credit_text(explorers, surveyors).unwrap_or_default();
        // Trimmed too: an older build's Edit Section trims it on every save.
        for read in [text.as_str(), text.trim()] {
            let (e, s) = legacy_people(read);
            let got = |list: Vec<Person>| list.into_iter().map(|p| p.name).collect::<Vec<_>>();
            assert_eq!(got(e), expected(explorers), "explorers of {read:?}");
            assert_eq!(got(s), expected(surveyors), "surveyors of {read:?}");
        }
    };
    for a in &pool {
        for b in &pool {
            let three = [a.clone(), b.clone(), filler.clone()];
            let two = [a.clone(), b.clone()];
            check(std::slice::from_ref(a), std::slice::from_ref(b));
            check(&two, &[]);
            check(&[], &two);
            check(&two, &two);
            check(&three, &[]);
            check(&[], &three);
            check(&three, std::slice::from_ref(a));
            check(std::slice::from_ref(b), &three);
        }
    }
}

/// Review of #719, item 2: stored text never keeps a control character (one
/// would make an exported XML document invalid): whitespace controls become a
/// space, others and XML's forbidden noncharacters go.
#[test]
fn people_are_cleaned_of_control_characters() {
    assert_eq!(clean_text("plain"), std::borrow::Cow::Borrowed("plain"));
    assert_eq!(
        clean_text("a\tb\nc\u{1}d\u{7f}e\u{9c}f\u{fffe}g"),
        "a b cdefg"
    );
    let person = Person::new(" Bad\u{1}Name\u{b}X ", Some("a\u{0}@x.org\r\n")).unwrap();
    assert_eq!(person.name, "BadName X");
    assert_eq!(person.email.as_deref(), Some("a@x.org"));
    assert!(Person::named("\u{1}\u{2}").is_none());
    let raw = Person {
        name: "Tab\tName".to_string(),
        email: None,
    };
    assert!(!raw.is_sanitized());
    let clean = raw.sanitized().unwrap();
    assert_eq!(clean.name, "Tab Name");
    assert!(clean.is_sanitized());
}

/// `add_person` stops at the role cap; `merge_people` keeps order, drops
/// repeats by identity and caps.
#[test]
fn adding_and_merging_people_respect_the_role_cap() {
    let mut list: Vec<Person> = (0..MAX_PEOPLE_PER_ROLE)
        .map(|i| person(&format!("P{i}"), None))
        .collect();
    assert!(!add_person(&mut list, person("One More", None)));
    assert_eq!(list.len(), MAX_PEOPLE_PER_ROLE);
    let merged = merge_people([
        vec![person("A", None), person("B", None)],
        vec![person("a", None), person("C", None)],
    ]);
    assert_eq!(names(&merged), ["A", "B", "C"]);
    let many: Vec<Person> = (0..MAX_PEOPLE_PER_ROLE * 2)
        .map(|i| person(&format!("Q{i}"), None))
        .collect();
    assert_eq!(
        merge_people([many.clone(), many]).len(),
        MAX_PEOPLE_PER_ROLE
    );
}

#[test]
fn add_remove_and_dedupe_use_identity() {
    let mut list = vec![person("A", None)];
    assert!(!add_person(&mut list, person("a", None)));
    assert!(add_person(&mut list, person("A", Some("a@x.org"))));
    assert!(add_person(&mut list, person("B", None)));
    assert_eq!(list.len(), 3);
    assert!(remove_person(&mut list, &person(" a ", None)));
    assert_eq!(names(&list), ["A", "B"]);
    assert_eq!(list[0].email.as_deref(), Some("a@x.org"));
    assert!(!remove_person(&mut list, &person("C", None)));

    let deduped = dedupe_people(vec![
        person("B", None),
        person("b", None),
        Person {
            name: "  ".to_string(),
            email: None,
        },
        person("A", None),
    ]);
    assert_eq!(names(&deduped), ["B", "A"]);
}

#[test]
fn from_legacy_and_names() {
    let date = NaiveDate::from_ymd_opt(2024, 3, 9);
    let credit = SurveyCredit::from_legacy(Some("nh / ds; kb"), date);
    assert_eq!(names(&credit.explorers), ["nh"]);
    assert_eq!(names(&credit.surveyors), ["ds", "kb"]);
    assert_eq!(credit.date, date);
    assert_eq!(credit.surveyor_names().as_deref(), Some("ds, kb"));
    assert_eq!(credit.explorer_names().as_deref(), Some("nh"));
    assert!(SurveyCredit::from_legacy(None, None).is_empty());
    assert!(SurveyCredit::from_legacy(Some("unknown"), None).is_empty());
    assert!(EMPTY_CREDIT.is_empty());
}

#[test]
fn summary_counts_people_per_shot_in_first_appearance_order() {
    let d1 = NaiveDate::from_ymd_opt(2024, 1, 5);
    let d2 = NaiveDate::from_ymd_opt(2023, 12, 30);
    let credits = [
        SurveyCredit {
            surveyors: vec![person("B", None), person("A", None), person("b", None)],
            explorers: vec![],
            date: d1,
        },
        SurveyCredit {
            surveyors: vec![person("a", None)],
            explorers: vec![person("E", None)],
            date: d2,
        },
        SurveyCredit::default(),
    ];
    let summary = SectionCreditSummary::from_credits(credits.iter());
    assert_eq!(summary.shot_count, 3);
    let counted: Vec<(&str, u32)> = summary
        .surveyors
        .iter()
        .map(|c| (c.person.name.as_str(), c.shots))
        .collect();
    assert_eq!(counted, [("B", 1), ("A", 2)]);
    assert_eq!(summary.explorers.len(), 1);
    assert_eq!(summary.date_min, d2);
    assert_eq!(summary.date_max, d1);
    assert_eq!(names(&summary.surveyor_people()), ["B", "A"]);

    let empty = SectionCreditSummary::from_credits(std::iter::empty());
    assert_eq!(empty, SectionCreditSummary::default());
}

#[test]
fn person_email_leaves_the_wire_when_absent_and_is_declared_optional() {
    let json = serde_json::to_value(person("A", None)).unwrap();
    assert!(!json.as_object().unwrap().contains_key("email"), "{json}");
    let json = serde_json::to_value(person("A", Some("a@x.org"))).unwrap();
    assert_eq!(json["email"], "a@x.org");
    assert!(Person::decl().contains("email?:"), "{}", Person::decl());
}

#[test]
fn credit_round_trips_and_tolerates_missing_keys() {
    let credit = SurveyCredit {
        surveyors: vec![person("A", Some("a@x.org"))],
        explorers: vec![person("B", None)],
        date: NaiveDate::from_ymd_opt(2020, 2, 29),
    };
    let json = serde_json::to_string(&credit).unwrap();
    assert!(json.contains("\"date\":\"2020-02-29\""), "{json}");
    let back: SurveyCredit = serde_json::from_str(&json).unwrap();
    assert_eq!(back, credit);
    let partial: SurveyCredit = serde_json::from_str(r#"{"surveyors":[{"name":"X"}]}"#).unwrap();
    assert_eq!(names(&partial.surveyors), ["X"]);
    assert!(partial.explorers.is_empty() && partial.date.is_none());
}

#[test]
fn credit_edit_sets_then_removes_then_adds() {
    let before = SurveyCredit {
        surveyors: vec![person("A", None), person("B", None)],
        explorers: vec![person("E", None)],
        date: NaiveDate::from_ymd_opt(2020, 1, 1),
    };
    let edit = CreditEdit {
        add_surveyors: Some(vec![person("b", None), person("C", None)]),
        remove_surveyors: Some(vec![person(" a ", None)]),
        ..Default::default()
    };
    assert_eq!(edit.kind(), CreditEditKind::Surveyors);
    assert_eq!(edit.label(), "Edit surveyors");
    let after = edit.apply(&before);
    assert_eq!(names(&after.surveyors), ["B", "C"]);
    assert_eq!(after.explorers, before.explorers);
    assert_eq!(after.date, before.date);

    let edit = CreditEdit {
        set_explorers: Some(vec![person("X", None), person("x", None)]),
        add_explorers: Some(vec![person("Y", Some("y@x.org"))]),
        remove_explorers: Some(vec![person("X", None)]),
        set_date: Some(None),
        ..Default::default()
    };
    assert_eq!(edit.kind(), CreditEditKind::Mixed);
    let after = edit.apply(&before);
    assert_eq!(names(&after.explorers), ["Y"]);
    assert_eq!(after.date, None);
    assert_eq!(names(&after.surveyors), ["A", "B"]);

    assert_eq!(CreditEdit::default().kind(), CreditEditKind::Nothing);
    assert_eq!(CreditEdit::default().apply(&before), before);
}

#[test]
fn credit_edit_sanitizes_and_bounds_what_it_stores() {
    let long = "n".repeat(MAX_PERSON_TEXT_CHARS + 50);
    let edit = CreditEdit {
        add_surveyors: Some(
            std::iter::once(Person {
                name: format!("  {long}  "),
                email: Some("   ".to_string()),
            })
            .chain(std::iter::once(Person {
                name: "   ".to_string(),
                email: None,
            }))
            .chain((0..MAX_PEOPLE_PER_ROLE + 10).map(|i| person(&format!("P{i}"), None)))
            .collect(),
        ),
        ..Default::default()
    };
    let after = edit.apply(&SurveyCredit::default());
    // The first MAX entries are all that count, the blank one among them.
    assert_eq!(after.surveyors.len(), MAX_PEOPLE_PER_ROLE - 1);
    assert_eq!(
        after.surveyors[0].name.chars().count(),
        MAX_PERSON_TEXT_CHARS
    );
    assert_eq!(after.surveyors[0].email, None);
}

#[test]
fn credit_edit_wire_shape() {
    use ts_rs::TS;
    let edit: CreditEdit = serde_json::from_str(r#"{"set_date":null}"#).unwrap();
    assert_eq!(edit.set_date, Some(None));
    let edit: CreditEdit = serde_json::from_str(r#"{"set_date":"2021-02-03"}"#).unwrap();
    assert_eq!(edit.set_date, Some(NaiveDate::from_ymd_opt(2021, 2, 3)));
    let edit: CreditEdit = serde_json::from_str("{}").unwrap();
    assert_eq!(edit, CreditEdit::default());
    assert_eq!(serde_json::to_string(&CreditEdit::default()).unwrap(), "{}");
    let round: CreditEdit = serde_json::from_str(
        &serde_json::to_string(&CreditEdit {
            set_date: Some(None),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        round.set_date,
        Some(None),
        "a clear survives the round trip"
    );
    let decl = CreditEdit::decl();
    assert!(decl.contains("set_date?: string | null"), "{decl}");
    assert!(decl.contains("add_surveyors?: Array<Person>"), "{decl}");

    let targets: CreditTargets =
        serde_json::from_str(r#"{"type":"shots","shots":[[1,100001]]}"#).unwrap();
    assert_eq!(
        targets,
        CreditTargets::Shots {
            shots: vec![(1, 100_001)]
        }
    );
    let targets: CreditTargets =
        serde_json::from_str(r#"{"type":"section","section_id":4}"#).unwrap();
    assert_eq!(targets, CreditTargets::Section { section_id: 4 });
}

/// A crafted legacy or imported text with a hundred thousand names used to cost
/// a pairwise scan with allocations per compare (it hung): the splitters are
/// linear, stop at [`MAX_PEOPLE_PER_ROLE`] and clamp every name.
#[test]
fn the_splitters_are_bounded_on_huge_text() {
    let many: String = (0..100_000)
        .map(|i| format!("Person {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let started = std::time::Instant::now();
    let surveyors = split_role(&many, NameSource::FreeText);
    let names = split_role(&many, NameSource::NameList);
    let (explorers, legacy_surveyors) = legacy_people(&format!("{many} / {many}"));
    let duplicates = split_role(&"A, ".repeat(1_000_000), NameSource::NameList);
    let separators = legacy_people(&" / ".repeat(1_000_000));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    for list in [&surveyors, &names, &explorers, &legacy_surveyors] {
        assert_eq!(list.len(), MAX_PEOPLE_PER_ROLE);
    }
    assert_eq!(surveyors[0].name, "Person 0");
    assert_eq!(duplicates.len(), 1);
    assert!(separators.0.is_empty());
}

/// Every split name is stored the way it reads back after a save: clamped to
/// [`MAX_PERSON_TEXT_CHARS`] at the split, so a reopen cannot change it.
#[test]
fn split_names_are_clamped_like_every_stored_person_in_either_source() {
    let long = "x".repeat(MAX_PERSON_TEXT_CHARS + 44);
    for people in [
        split_role(&long, NameSource::FreeText),
        split_role(&format!("{long}; B"), NameSource::NameList),
        legacy_people(&format!("{long} / {long}")).0,
    ] {
        assert_eq!(people[0].name.chars().count(), MAX_PERSON_TEXT_CHARS);
        assert_eq!(people[0].sanitized().as_ref(), Some(&people[0]));
    }
}

/// An edit's lists are cut to what a role can hold before they are sanitized
/// or deduplicated: the first `MAX_PEOPLE_PER_ROLE` entries are all that count.
#[test]
fn an_edit_takes_the_first_max_entries_before_deduplicating() {
    let mut set = vec![person("A", None); MAX_PEOPLE_PER_ROLE];
    set.push(person("Past The Cap", None));
    let edit = CreditEdit {
        set_surveyors: Some(set.clone()),
        add_explorers: Some(set),
        ..Default::default()
    };
    let after = edit.apply(&SurveyCredit::default());
    assert_eq!(names(&after.surveyors), ["A"]);
    assert_eq!(names(&after.explorers), ["A"]);

    let huge: Vec<Person> = (0..1_000_000)
        .map(|i| Person {
            name: format!("P{i}"),
            email: None,
        })
        .collect();
    let edit = CreditEdit {
        set_surveyors: Some(huge.clone()),
        remove_surveyors: Some(huge),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let after = edit.apply(&SurveyCredit::default());
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );
    assert!(after.surveyors.is_empty(), "every kept person was removed");
}
