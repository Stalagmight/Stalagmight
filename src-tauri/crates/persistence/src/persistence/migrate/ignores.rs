//! Ignored warnings, re-keyed by shot uuid (plan 7.2.9).
//!
//! A legacy `resolved_warnings` row names its shot by `(line id, shot uuid)`,
//! or, for a pre-v8 row the load could not back-fill, by `(line id, session
//! shot id)`. The 0.6.0 warning pass matched a row to a shot by the first rule
//! that applies; the migration follows the same rules once, then keys the
//! ignore by the shot's final uuid (shot uuids are project-unique in the new
//! model, so the line is no longer needed):
//!
//! 1. the row's uuid is held by a shot of its line;
//! 2. the uuid the load re-minted for the row's line holds it (the row was
//!    stored against an id two lines shared; `reminted_shot_uuid`);
//! 3. a shot of another line holds the uuid (a split or combine moved it);
//! 4. a row without uuid: the shot of its line whose session id it names.
//!
//! A row matching no shot, or repeating an earlier row's (shot, code), is
//! dropped.

use std::collections::{HashMap, HashSet};

use stalagmight_legacy::models::{reminted_shot_uuid, LineId};
use stalagmight_legacy::LegacySurvey;

use super::super::stalx_v2::IgnoredWarning;
use super::MigrationReport;

/// Re-key the legacy ignores onto the migrated shot uuids. `final_uuid`
/// maps a legacy shot (line, uuid after the load) to its migrated uuid.
pub(super) fn rekey(
    legacy: &LegacySurvey,
    final_uuid: &HashMap<(LineId, String), String>,
    report: &mut MigrationReport,
) -> Vec<IgnoredWarning> {
    let project = &legacy.project;
    let mut by_line: HashMap<LineId, (HashSet<&str>, HashMap<u32, &str>)> = HashMap::new();
    let mut anywhere: HashMap<&str, LineId> = HashMap::new();
    for line in &project.lines {
        let entry = by_line.entry(line.id).or_default();
        for shot in &line.shots {
            if let Some(uuid) = shot.uuid.as_deref() {
                entry.0.insert(uuid);
                entry.1.entry(shot.id).or_insert(uuid);
                anywhere.entry(uuid).or_insert(line.id);
            }
        }
    }
    let holds = |line: LineId, uuid: &str| by_line.get(&line).is_some_and(|l| l.0.contains(uuid));

    let mut out = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for row in &legacy.extras.resolved_warnings {
        let located: Option<(LineId, String)> = match row.shot_uuid.as_deref() {
            Some(stored) if holds(row.line_id, stored) => Some((row.line_id, stored.to_string())),
            Some(stored) => {
                let renamed = reminted_shot_uuid(row.line_id, stored);
                if holds(row.line_id, &renamed) {
                    Some((row.line_id, renamed))
                } else {
                    anywhere.get(stored).map(|&line| (line, stored.to_string()))
                }
            }
            None => by_line
                .get(&row.line_id)
                .and_then(|l| l.1.get(&row.shot_id))
                .map(|uuid| (row.line_id, (*uuid).to_string())),
        };
        let Some(shot_uuid) = located.and_then(|key| final_uuid.get(&key).cloned()) else {
            report.ignores_dropped += 1;
            continue;
        };
        if !seen.insert((shot_uuid.clone(), row.warning_code.clone())) {
            report.ignores_dropped += 1;
            continue;
        }
        if row.shot_uuid.as_deref() == Some(shot_uuid.as_str()) {
            report.ignores_kept += 1;
        } else {
            report.ignores_rekeyed += 1;
        }
        out.push(IgnoredWarning {
            shot: shot_uuid,
            code: row.warning_code.clone(),
            at: row.resolved_at,
            comment: row.resolved_comment.clone(),
        });
    }
    out
}
