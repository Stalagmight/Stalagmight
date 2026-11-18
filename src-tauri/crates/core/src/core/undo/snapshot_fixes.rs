//! GPS fixes in a whole-project undo snapshot (#741).
//!
//! A `ProjectSnapshot` / `ProjectSnapshotPair` is a serialized [`Project`], and
//! it is persisted in the `.stalx` undo history, where an older build reads it
//! back with `serde_json::from_str::<Project>`. That build assumes at most one
//! zero datum per merge component, so a GPS fix in the plain `zero_datums`
//! list would reach it as a second placement: an Undo there could restore two
//! datums in one merged group, and its next save would write both to the v18
//! `zero_datums` table an outside reader reads.
//!
//! So a snapshot mirrors the file's table split (`zero_datum_extras`):
//!
//! - `zero_datums` holds the placement datums and the dangling ones, in list
//!   order: what an older build may see;
//! - [`FIXES_KEY`] holds each GPS fix with its place in the whole list. An
//!   older build ignores the key (`Project` does not deny unknown fields), so
//!   it restores the placements alone, as it reads the file.
//!
//! A placement datum's accuracy stays on it: an older build ignores
//! `accuracy_m` the same way. Restoring merges the fixes back in list order.
//! A snapshot without the key (every one written before it, and any project
//! without a fix) restores its `zero_datums` as they are.

use serde::{Deserialize, Serialize};

use crate::models::{DatumRole, Project, ZeroDatum};

/// The snapshot key holding the GPS fixes.
pub(crate) const FIXES_KEY: &str = "zero_datum_fixes";

/// One GPS fix and its index in the whole datum list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SnapshotFix {
    pub position: usize,
    pub datum: ZeroDatum,
}

/// A project as a snapshot writes it: its fields, plus [`FIXES_KEY`] when it
/// holds a fix.
#[derive(Serialize)]
struct SnapshotOut<'a> {
    #[serde(flatten)]
    project: &'a Project,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    zero_datum_fixes: Vec<SnapshotFix>,
}

/// Serialize `project` for an undo snapshot with its GPS fixes split out of
/// `zero_datums` (see the module doc). `project` is the copy the caller is
/// about to serialize, so its fixes are taken out of it in place.
pub(crate) fn serialize_split(project: &mut Project) -> serde_json::Result<String> {
    let roles = project.datum_roles();
    let mut fixes = Vec::new();
    let mut kept = Vec::with_capacity(project.zero_datums.len());
    for (position, (datum, role)) in std::mem::take(&mut project.zero_datums)
        .into_iter()
        .zip(roles)
        .enumerate()
    {
        if role == Some(DatumRole::GpsFix) {
            fixes.push(SnapshotFix { position, datum });
        } else {
            kept.push(datum);
        }
    }
    project.zero_datums = kept;
    serde_json::to_string(&SnapshotOut {
        project,
        zero_datum_fixes: fixes,
    })
}

/// `AppState::serialize_project_snapshot`, as a JSON value rather than text:
/// the same machine-local `file_path` cleared, the same fixes split out, so
/// `serde_json::from_str` of that text and this agree exactly (floats
/// included - `serde_json::to_value` keeps the bits the text round-trips).
/// What a delta is encoded over and applied to, built without the clone the
/// text form needs.
pub(crate) fn snapshot_value(project: &Project) -> serde_json::Result<serde_json::Value> {
    use serde_json::Value;
    let mut value = serde_json::to_value(project)?;
    let Value::Object(members) = &mut value else {
        return Ok(value);
    };
    if project.file_path.is_some() {
        members.insert("file_path".to_string(), Value::Null);
    }
    let roles = project.datum_roles();
    if roles.contains(&Some(DatumRole::GpsFix)) {
        if let Some(Value::Array(datums)) = members.get_mut("zero_datums") {
            let mut fixes = Vec::new();
            let mut kept = Vec::with_capacity(datums.len());
            for (position, (datum, role)) in std::mem::take(datums).into_iter().zip(roles).enumerate()
            {
                if role == Some(DatumRole::GpsFix) {
                    fixes.push(serde_json::json!({ "position": position, "datum": datum }));
                } else {
                    kept.push(datum);
                }
            }
            *datums = kept;
            members.insert(FIXES_KEY.to_string(), Value::Array(fixes));
        }
    }
    Ok(value)
}

/// Whether `project` holds a GPS fix, so a snapshot of it must split.
pub(crate) fn holds_fix(project: &Project) -> bool {
    project.datum_roles().contains(&Some(DatumRole::GpsFix))
}

/// The fixes `snapshot` recorded under [`FIXES_KEY`]: empty for a snapshot
/// without the key, `Err` for one whose fixes cannot be read.
pub(crate) fn read_fixes(snapshot: &str) -> serde_json::Result<Vec<SnapshotFix>> {
    // The key can only appear as an object key: inside a JSON string its
    // quotes are escaped. So a snapshot without it skips the second parse.
    if !snapshot.contains(&format!("\"{FIXES_KEY}\":")) {
        return Ok(Vec::new());
    }
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        zero_datum_fixes: Vec<SnapshotFix>,
    }
    serde_json::from_str::<Probe>(snapshot).map(|probe| probe.zero_datum_fixes)
}

/// Put `fixes` back into `datums` (the snapshot's `zero_datums`) at their
/// recorded places, so the whole list comes back in its order. A place past
/// the end appends, so a hand-edited snapshot cannot panic the restore.
pub(crate) fn merge_fixes(datums: &mut Vec<ZeroDatum>, mut fixes: Vec<SnapshotFix>) {
    fixes.sort_by_key(|fix| fix.position);
    for fix in fixes {
        let at = fix.position.min(datums.len());
        datums.insert(at, fix.datum);
    }
}
