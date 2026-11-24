//! Legacy (SQLite) `.stalx` files for tests: built with the frozen 0.6.0
//! model, written by its save path (`stalagmight_legacy::write_v18`) and read
//! back by its open, so a test holds exactly "what an old build wrote".
//! Fully synthetic data only.

use std::path::Path;

pub(super) use stalagmight_legacy::models as old;
pub(super) use stalagmight_legacy::{LegacyExtras, LegacyProject as Legacy};

/// A live value as the frozen 0.6.0 type of the same name (a shot, a GPS
/// position, a strategy, options): both serialize alike, the frozen copy only
/// dropped the TS derives.
pub(super) fn frozen<T: serde::Serialize, U: serde::de::DeserializeOwned>(live: &T) -> U {
    serde_json::to_value(live)
        .and_then(serde_json::from_value)
        .expect("the frozen type reads the live value")
}

/// A frozen-model slot.
pub(super) fn slot(line_id: crate::models::LineId, index: u32) -> old::StationRef {
    old::StationRef::new(line_id, index)
}

/// `project` saved at `path` as the 0.6.0 save writes it (no extras).
pub(super) fn write_legacy_file(path: &Path, project: &Legacy) {
    write_legacy_file_with(path, project, &LegacyExtras::default());
}

/// `project` and `extras` saved at `path` as the 0.6.0 save writes them.
pub(super) fn write_legacy_file_with(path: &Path, project: &Legacy, extras: &LegacyExtras) {
    let bytes = stalagmight_legacy::write_v18(project, extras).expect("the legacy save");
    std::fs::write(path, bytes).expect("write the legacy file");
}

/// The project the 0.6.0 open reads from the legacy file at `path` (the
/// save's heals applied: minted uuids, shot ids by position).
pub(super) fn read_legacy_file(path: &Path) -> Legacy {
    let options = stalagmight_legacy::OpenOptions {
        with_adjusted: false,
        ..Default::default()
    };
    stalagmight_legacy::open_path(path, &options)
        .expect("the legacy file opens")
        .project
}
