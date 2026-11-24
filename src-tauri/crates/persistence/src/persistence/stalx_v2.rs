//! The `.stalx` version 2 file format: canonical, line-oriented JSON for the
//! station-graph survey model ([`stalagmight_models::survey::Project`]).
//!
//! The public specification for third-party readers is `docs/file-format.md`;
//! the design is plan `docs/plans/2026-11-20_station-graph-model.md` section 6.
//!
//! Entry points:
//! - [`encode`] / [`encode_with_report`]: a project plus its [`Extras`] to
//!   canonical bytes, in one of the serializer [`Mode`]s.
//! - [`decode`]: bytes to a [`Decoded`] project, with size and count limits
//!   ([`DecodeOptions`]), the format / version / conventions / `requires`
//!   gates, and the model invariants (`Project::validate`).
//! - [`decode_salvage`]: best-effort repair of a file that parses but breaks
//!   the invariants.
//! - [`probe_bytes`] / [`probe_path`] / [`sniff`]: the cheap header read
//!   (the foreign-copy probe, link readers), and [`probe_path_listing`] for
//!   Recents.
//! - [`rewrite_local`] / [`rename_project`]: rewrite the per-machine members
//!   (or the project name) while copying every survey member verbatim.
//! - [`write_atomic`] and [`Fingerprint`]: the durable save and the
//!   external-modification guard.
//! - [`survey_digest`]: the digest of the survey a file holds, which tells a
//!   SpeleoDB copy that holds exactly its BASE revision from one with
//!   un-uploaded work.
//!
//! This is the format the app opens, saves and uploads. A 0.6.0 (SQLite)
//! `.stalx` is only read: converted on open by [`super::migrate`] (after
//! [`super::legacy_settle`] on a copy), and its SpeleoDB link read by
//! [`super::legacy_link`]; nothing writes the SQLite format any more.

mod atomic;
mod canon;
mod decode;
mod decode_meta;
mod decode_records;
mod digest;
mod encode;
mod error;
mod ignores;
mod local;
mod probe;
mod salvage;
mod scan;
mod summary;
mod tokens;
mod types;
mod upgrade;
mod write;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers: a refused build is a broken test setup"
)]
mod test_helpers;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point; one ignored test prints a sample"
)]
mod format_v2_roundtrip_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_canonical_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_gating_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_hostile_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_owner_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_probe_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_salvage_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_modes_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_digest_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point; it prints its measurements"
)]
mod format_v2_diff_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_crash_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "bench code: prints its measurements"
)]
mod format_v2_bench_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_doc_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_upgrade_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_limits_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod format_v2_forward_tests;

#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::print_stderr,
    reason = "fuzz test: a failing seed prints its replay line, a sweep its verdicts"
)]
mod decode_fuzz_tests;

pub use atomic::{check_unchanged, temp_path, write_atomic, Fingerprint, Guard, Stage, WriteError};
pub use decode::{decode, decode_salvage};
pub use digest::survey_digest;
pub use encode::{encode, encode_with_report, EncodeReport, HistoryCut};
pub use error::{DecodeError, EncodeError};
pub use local::{rename_project, rewrite_local, rewrite_local_keeping_history};
pub use probe::{probe_bytes, probe_path, probe_path_listing, sniff, Format, Header, ProjectInfo};
pub use salvage::{Repair, SalvageReport};
pub use summary::Summary;
pub use types::{
    CountLimits, DecodeNote, DecodeOptions, Decoded, Earlier, Extras, History, IgnoredWarning,
    Limit, LocalState, LocalSync, Mode, Preserved, PushOptions, RawJson, ReadOnly, SpeleoDbLink,
    MAX_PROJECT_FILE_BYTES, MAX_V2_FILE_BYTES,
};
pub use upgrade::UpgradeReport;
/// The format's date rule (0001-01-01 ..= 9999-12-31, F10), which the
/// migration applies so a migrated model is what its file will hold.
pub(crate) use write::normalized_date;

/// The fixed first bytes of every canonical file. A reader sniffs them; a
/// server can check them the way it checks the SQLite header today.
pub const PREFIX: &str = "{\"format\":\"stalx\",\"version\":";

/// The first bytes of a legacy (SQLite) `.stalx`.
pub const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// The major format version this build reads and writes.
pub const VERSION: u64 = 2;

/// The solver version this build derives positions with (plan section 3).
pub const SOLVER: u64 = 1;

/// The `requires` feature of kept and archived stations (plan 2026-11-23
/// RS 8.1): station records' `archived` objects, a closure's `archived`
/// end, and the shown-identity rule (a closure-archived station shows its
/// kept end's name). Every file this build writes lists it, whatever it
/// holds: a version 2 file without it was written by an earlier build of
/// version 2 (U1 per-line aliases) and takes the load-time upgrade
/// ([`Decoded::upgrade`], RS 8.2).
pub const ARCHIVED_STATIONS: &str = "archived-stations";

/// The `requires` feature names this build understands. A file that names
/// any other opens read-only.
pub const KNOWN_FEATURES: &[&str] = &[ARCHIVED_STATIONS];

/// The generator written when [`Extras::generator`] is empty.
pub const DEFAULT_GENERATOR: &str = "Stalagmight";

/// The fixed `conventions` of version 2, in file order. Any other value, a
/// missing key or an extra key refuses the file.
pub const CONVENTIONS: [(&str, &str); 8] = [
    ("length", "m"),
    ("angles", "deg"),
    ("azimuth", "cw-from-north"),
    ("inclination", "+down"),
    ("depth", "+down"),
    ("local", "enu-m"),
    ("gps", "wgs84-deg"),
    ("temperature", "degC"),
];

/// The largest integer the format holds (2^53 - 1): every JSON reader
/// represents it exactly.
pub const MAX_INTEGER: u64 = (1 << 53) - 1;

// The closure id space is the format's integers (validate bounds both).
const _: () = assert!(crate::models::survey::MAX_CLOSURE_ID == MAX_INTEGER);

/// The deepest nesting accepted in preserved (unknown) content and history
/// entries. The survey records themselves are at most five levels deep.
pub const MAX_DEPTH: usize = 128;

/// The most shots one line holds: the model's bound, checked by `validate`
/// on both sides (re-exported so format readers find it here).
pub use crate::models::survey::MAX_SHOTS_PER_LINE;

/// The SpeleoDB upload limit a push is cut to by default (5 MiB, inclusive).
pub const PUSH_BYTE_CAP: usize = 5 * 1024 * 1024;
