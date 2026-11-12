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
        // Two one-word comma parts read as `Last, First` in either half.
        ("Doe, John / C", &["Doe, John"], &["C"]),
        // One half empty, as the importer wrote it for a missing role.
        (" / ds", &[], &["ds"]),
        ("nh / ", &["nh"], &[]),
        // A placeholder half names nobody.
        ("unknown / ds", &[], &["ds"]),
        ("nh / ?", &["nh"], &[]),
        // Anything else is surveyors only.
        ("John Doe", &[], &["John Doe"]),
        ("a / b / c", &[], &["a / b / c"]),
        ("a/b", &[], &["a/b"]),
        // `Last, First` is one person...
        ("Doe, John", &[], &["Doe, John"]),
        ("Doe,John", &[], &["Doe,John"]),
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
        // Duplicates go, by identity (casefold + whitespace collapse).
        ("A, a, B", &[], &["A", "B"]),
        ("John  Doe; john doe", &[], &["John  Doe"]),
    ];
    for (input, explorers, surveyors) in cases {
        let (e, s) = legacy_people(input);
        assert_eq!(names(&e), *explorers, "explorers of {input:?}");
        assert_eq!(names(&s), *surveyors, "surveyors of {input:?}");
    }
}

#[test]
fn split_names_splits_on_both_separators_without_the_last_first_rule() {
    assert_eq!(names(&split_names("Doe, John")), ["Doe", "John"]);
    assert_eq!(names(&split_names("A;B, c ; a")), ["A", "B", "c"]);
    assert!(split_names(" ; , ").is_empty());
}

#[test]
fn identity_is_casefolded_name_and_lowercased_email() {
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
    assert_eq!(after.surveyors.len(), MAX_PEOPLE_PER_ROLE);
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
