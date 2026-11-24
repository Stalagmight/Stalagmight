//! The JSON document's records. They are filled from the station graph by
//! `survey::json_export` (`survey/tabular.rs`).

use serde::Serialize;

use crate::models::{Person, ShotKind};

/// JSON export format
#[derive(Debug, Serialize)]
pub struct JsonExport {
    pub project: JsonProjectMeta,
    pub lines: Vec<JsonLine>,
}

#[derive(Debug, Serialize)]
pub struct JsonProjectMeta {
    pub name: String,
    pub location: Option<String>,
    pub exported_at: String,
}

/// A line. Its survey credit is the aggregate of its shots' (each shot
/// carries its own): people by NAME only - emails never leave in an export.
#[derive(Debug, Serialize)]
pub struct JsonLine {
    /// The line's name - its identity in the export (line names are unique);
    /// the internal line id is not written.
    pub name: Option<String>,
    /// The distinct surveyor names joined with `", "` - the legacy field.
    pub surveyor: Option<String>,
    /// The earliest shot date.
    pub date: Option<String>,
    /// Every surveyor of any shot, first-appearance order.
    pub surveyors: Vec<String>,
    /// Every explorer of any shot, first-appearance order.
    pub explorers: Vec<String>,
    /// The latest shot date (`date` is the earliest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_last: Option<String>,
    pub color: String,
    /// The name of the Section (named group of lines) the line belongs to;
    /// absent for a line in no section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub shots: Vec<JsonShot>,
}

/// A shot, identified the way the user reads it: `from` -> `to` by station
/// name, with the two stations' Station IDs; the internal shot id is not
/// written.
#[derive(Debug, Serialize)]
pub struct JsonShot {
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_id: Option<u32>,
    pub length: f64,
    pub azimuth_in: f64,
    pub azimuth_out: f64,
    pub depth_in: f64,
    pub depth_out: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lrud: Option<[f64; 4]>,
    pub kind: ShotKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The shot's surveyors, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub surveyors: Vec<String>,
    /// The shot's explorers, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub explorers: Vec<String>,
    /// The shot's survey date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

pub(crate) fn names(people: &[Person]) -> Vec<String> {
    people.iter().map(|p| p.name.clone()).collect()
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use crate::export::survey::test_helpers::Survey;
    use crate::export::survey::{export_json, json_export};
    use crate::models::{Person, ShotData, ShotKind, SurveyCredit};
    use tempfile::NamedTempFile;

    /// A shot credited to `surveyor` (the per-shot form of a 0.6.0 line's
    /// surveyor text).
    fn credited(shot: ShotData, surveyor: &str) -> ShotData {
        ShotData {
            credit: SurveyCredit::from_legacy(Some(surveyor), None)
                .stored()
                .map(Into::into),
            ..shot
        }
    }

    #[test]
    fn test_export_json() {
        let mut survey = Survey::new("Test");
        survey.named(
            "Main",
            "#4ecdc4",
            vec![credited(
                ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 225.0,
                    length: 10.0,
                    depth_in: 5.0,
                    depth_out: 6.0,
                    left: Some(1.0),
                    right: Some(2.0),
                    up: Some(3.0),
                    down: Some(4.0),
                    ..Default::default()
                },
                "Tester",
            )],
        );
        let project = survey.build();

        let file = NamedTempFile::new().unwrap();
        let result = export_json(&project, file.path());
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(file.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed["project"]["name"], "Test");
        assert_eq!(parsed["lines"][0]["name"], "Main");
        assert_eq!(parsed["lines"][0]["surveyor"], "Tester");
        assert_eq!(parsed["lines"][0]["surveyors"][0], "Tester");
        assert_eq!(parsed["lines"][0]["shots"][0]["surveyors"][0], "Tester");
    }

    #[test]
    fn a_line_in_a_section_carries_the_section_name() {
        let mut survey = Survey::new("Test");
        let shots = || {
            vec![ShotData {
                length: 10.0,
                ..Default::default()
            }]
        };
        let grouped = survey.named("Main", "#4ecdc4", shots());
        survey.named("Side", "#4ecdc4", shots());
        let mut project = survey.build();
        let upper = project.mint_section_id().unwrap();
        project.sections.push(crate::models::Section {
            id: upper,
            name: "Upper series".to_string(),
            color: "#ff6b6b".to_string(),
        });
        project.line_mut(grouped).unwrap().section_id = Some(upper);

        let file = NamedTempFile::new().unwrap();
        export_json(&project, file.path()).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
        assert_eq!(parsed["lines"][0]["section"], "Upper series");
        assert!(parsed["lines"][1].get("section").is_none());
    }

    #[test]
    fn credit_is_per_shot_with_line_aggregates_and_never_emails() {
        let mut survey = Survey::new("Credit");
        let shot = |credit: SurveyCredit| ShotData {
            length: 3.0,
            credit: credit.stored().map(Into::into),
            ..Default::default()
        };
        let ann = Person::new("Ann", Some("ann@example.org")).unwrap();
        let bo = Person::named("Bo").unwrap();
        survey.line(
            crate::models::survey::LineMeta::default(),
            vec![
                shot(SurveyCredit {
                    surveyors: vec![ann],
                    explorers: vec![bo.clone()],
                    date: chrono::NaiveDate::from_ymd_opt(2024, 5, 2),
                }),
                shot(SurveyCredit {
                    surveyors: vec![bo],
                    explorers: Vec::new(),
                    date: chrono::NaiveDate::from_ymd_opt(2024, 5, 9),
                }),
                shot(SurveyCredit::default()),
            ],
        );
        let project = survey.build();
        let file = NamedTempFile::new().unwrap();
        export_json(&project, file.path()).unwrap();
        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert!(
            !contents.contains("example.org"),
            "no email leaves: {contents}"
        );
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let line = &parsed["lines"][0];
        assert_eq!(line["surveyors"], serde_json::json!(["Ann", "Bo"]));
        assert_eq!(line["explorers"], serde_json::json!(["Bo"]));
        assert_eq!(line["surveyor"], "Ann, Bo");
        assert_eq!(line["date"], "2024-05-02");
        assert_eq!(line["date_last"], "2024-05-09");
        assert_eq!(line["shots"][0]["surveyors"], serde_json::json!(["Ann"]));
        assert_eq!(line["shots"][1]["date"], "2024-05-09");
        let bare = line["shots"][2].as_object().unwrap();
        assert!(!bare.contains_key("surveyors") && !bare.contains_key("date"));
    }

    #[test]
    fn test_export_json_empty_project() {
        let project = Survey::new("Empty").build();
        let file = NamedTempFile::new().unwrap();
        let result = export_json(&project, file.path());
        assert!(result.is_ok(), "empty project JSON export should succeed");

        let contents = std::fs::read_to_string(file.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed["project"]["name"], "Empty");
        assert!(parsed["lines"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_export_json_unicode_names() {
        let mut survey = Survey::new("日本語テスト");
        survey.named(
            "Grotte de l'Ours",
            "#ff0000",
            vec![credited(
                ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 225.0,
                    length: 5.0,
                    depth_in: 0.0,
                    depth_out: 1.0,
                    comment: Some("特殊なコメント".to_string()),
                    ..Default::default()
                },
                "Ñoño",
            )],
        );
        let project = survey.build();

        let file = NamedTempFile::new().unwrap();
        let result = export_json(&project, file.path());
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(file.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed["project"]["name"], "日本語テスト");
        assert_eq!(parsed["lines"][0]["name"], "Grotte de l'Ours");
    }

    #[test]
    fn test_export_json_reference_shots_and_optional_fields() {
        let mut survey = Survey::new("Reference");
        survey.line(
            crate::models::survey::LineMeta {
                color: "#000000".to_string(),
                ..Default::default()
            },
            vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 1.0,
                depth_in: 0.0,
                depth_out: 0.0,
                kind: ShotKind::Reference,
                ..Default::default()
            }],
        );
        let project = survey.build();

        let file = NamedTempFile::new().unwrap();
        let result = export_json(&project, file.path());
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(file.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let shot = &parsed["lines"][0]["shots"][0];
        assert_eq!(shot["kind"], "Reference");
    }

    /// A shot is written as `from` -> `to` by station name, with the two
    /// Station IDs; no internal line or shot id leaves.
    #[test]
    fn shots_carry_station_names_and_ids_not_internal_ids() {
        let mut survey = Survey::new("Ids");
        let line = survey.named("Main", "#ff0000", vec![ShotData::default(); 2]);
        survey.rename((line, 1), "sump");
        let project = survey.build();

        let value = serde_json::to_value(json_export(&project, chrono::Utc::now())).unwrap();
        let line = &value["lines"][0];
        assert!(line.get("id").is_none(), "{line}");
        let shots = line["shots"].as_array().unwrap();
        assert_eq!(shots[0]["from"], "0");
        assert_eq!(shots[0]["to"], "sump");
        assert_eq!(shots[0]["from_id"], 0);
        assert_eq!(shots[0]["to_id"], 1);
        assert_eq!(shots[1]["from"], "sump");
        assert_eq!(shots[1]["to"], "2");
        assert_eq!(shots[1]["to_id"], 2);
        assert!(shots[0].get("id").is_none(), "{}", shots[0]);
    }
}
