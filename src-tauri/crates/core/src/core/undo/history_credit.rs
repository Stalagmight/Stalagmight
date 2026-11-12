//! History labels and previews for survey-credit edits
//! (`UndoData::ShotCredits`).

use super::history_view::{FieldChange, PreviewData};
use super::{ShotCreditChange, UndoData, UndoEntry};
use crate::core::Command;
use crate::models::{
    dedupe_people, join_names, CreditEdit, CreditEditKind, Person, SectionId, SurveyCredit,
};

/// Longest name list a preview row shows, in chars.
const MAX_PREVIEW_CHARS: usize = 80;

/// "Edit surveyors (12 shots)": the edit's label and how many shots it changed.
/// `None` for every other entry, which keeps its command's own description.
pub(super) fn description(entry: &UndoEntry) -> Option<String> {
    let (Command::EditShotCredit { edit, .. }, UndoData::ShotCredits { changes, .. }) =
        (&entry.command, &entry.undo_data)
    else {
        return None;
    };
    let n = changes.len();
    Some(format!(
        "{} ({n} shot{})",
        edit.label(),
        if n == 1 { "" } else { "s" }
    ))
}

fn union(changes: &[ShotCreditChange], pick: impl Fn(&ShotCreditChange) -> &[Person]) -> String {
    let people = dedupe_people(
        changes
            .iter()
            .flat_map(|c| pick(c).iter().cloned())
            .collect(),
    );
    let text = join_names(&people).unwrap_or_default();
    match text.char_indices().nth(MAX_PREVIEW_CHARS) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text,
    }
}

fn date_range(
    changes: &[ShotCreditChange],
    pick: impl Fn(&ShotCreditChange) -> &SurveyCredit,
) -> String {
    let dates: Vec<_> = changes.iter().filter_map(|c| pick(c).date).collect();
    match (dates.iter().min(), dates.iter().max()) {
        (Some(lo), Some(hi)) if lo == hi => lo.to_string(),
        (Some(lo), Some(hi)) => format!("{lo} - {hi}"),
        _ => String::new(),
    }
}

/// The hover preview of an `EditShotCredit`: the touched roles' names before
/// and after (the union over the changed shots), the date range, the count.
pub(super) fn credit_preview(edit: &CreditEdit, changes: &[ShotCreditChange]) -> PreviewData {
    let kind = edit.kind();
    let all = matches!(kind, CreditEditKind::Mixed | CreditEditKind::Nothing);
    let mut rows = Vec::new();
    if all || kind == CreditEditKind::Surveyors {
        rows.push(FieldChange {
            field: "Surveyors".to_string(),
            old_value: union(changes, |c| &c.before.surveyors),
            new_value: union(changes, |c| &c.after.surveyors),
        });
    }
    if all || kind == CreditEditKind::Explorers {
        rows.push(FieldChange {
            field: "Explorers".to_string(),
            old_value: union(changes, |c| &c.before.explorers),
            new_value: union(changes, |c| &c.after.explorers),
        });
    }
    if all || kind == CreditEditKind::Date {
        rows.push(FieldChange {
            field: "Date".to_string(),
            old_value: date_range(changes, |c| &c.before),
            new_value: date_range(changes, |c| &c.after),
        });
    }
    rows.retain(|r| r.old_value != r.new_value);
    rows.push(FieldChange {
        field: "Shots".to_string(),
        old_value: String::new(),
        new_value: changes.len().to_string(),
    });
    PreviewData {
        changes: rows,
        additional_changes: 0,
    }
}

/// The `values_json` a legacy section-metadata edit's preview reads: the
/// recorded other fields, plus the surveyor names and earliest date the shots
/// carried before.
pub(super) fn legacy_section_values(
    changes: &[ShotCreditChange],
    section_values: Option<&(SectionId, String)>,
) -> String {
    let mut prev = section_values
        .and_then(|(_, json)| serde_json::from_str::<serde_json::Value>(json).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = prev.as_object_mut() {
        map.insert(
            "surveyor".to_string(),
            serde_json::Value::String(union(changes, |c| &c.before.surveyors)),
        );
        let earliest = changes.iter().filter_map(|c| c.before.date).min();
        map.insert(
            "date".to_string(),
            earliest.map_or(serde_json::Value::Null, |d| {
                serde_json::Value::String(d.to_string())
            }),
        );
    }
    prev.to_string()
}
