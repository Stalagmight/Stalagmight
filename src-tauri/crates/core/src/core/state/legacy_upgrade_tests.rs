//! The first version 2 write over a legacy (SQLite) `.stalx`
//! (`legacy_upgrade`), interrupted after every step, on each original an old
//! build or another tool leaves:
//!
//! | kind | beside the file | the survey 0.6.0 showed |
//! |---|---|---|
//! | clean | nothing | the file |
//! | debris | an unarmed journal, an empty WAL, a WAL index | the file |
//! | hot journal | the armed journal of a write a kill interrupted after it spilled pages into the file | the file with the journal rolled back |
//! | WAL | the WAL (and index) of a commit never checkpointed | the file with the WAL read through |
//!
//! Two engines build the files a kill leaves:
//!
//! - STAGED: every state between two steps, copied byte by byte (the module
//!   docs of `legacy_upgrade` list the steps): before the upgrade; the settled
//!   copy staged beside the original (torn, then whole); the settled copy
//!   renamed over the original with the old sidecars still beside it; the
//!   sidecars gone; the backup staged (torn); the backup kept; the version 2
//!   temp written (torn, then whole); renamed.
//! - REAL: the save itself, killed after each `legacy_upgrade::Step` (the
//!   injected kill unwinds past every cleanup, as a kill would), plus the
//!   expected path of each kind (which steps it must pass through).
//!
//! The oracle, on every state:
//!
//! - never a version 2 file beside an armed journal or a WAL;
//! - the project's path opens as EXACTLY the survey the open showed (OLD), or
//!   once renamed the upgraded one (NEW);
//! - the kept original (at most one) opens as OLD; Recovery offers it, never a
//!   hidden staging file, and every backup it offers restores: the kept
//!   original as OLD, a complete version 2 temp as NEW;
//! - the next open and save succeeds, leaves no sidecar, no temp and nothing
//!   hidden behind, and the file reads back as the survey that was open.
//!
//! Everything is generated in code: the projects, the journals, the WALs and
//! every injected artifact. No fixture is read, copied or derived from.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use tempfile::TempDir;

use super::legacy_upgrade::{settled_bytes, Step, KILL_AT};
use super::project_io::upgrade_backup_dir;
use super::robustness_kit::{
    canonical, canonical_snapshot, entries, first_difference, open_fresh, panic_text, write_legacy,
    Rng,
};
use super::speleodb_sidecars::{has_hot_journal, sidecar_path};
use super::AppState;
use crate::persistence::stalx_v2::{self, DecodeOptions, Format};
use crate::test_helpers::isolated_config;

// ===========================================================================
// The originals
// ===========================================================================

/// What lies beside the legacy original the upgrade meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Clean,
    Debris,
    HotJournal,
    Wal,
}

impl Kind {
    pub(super) const ALL: [Self; 4] = [Self::Clean, Self::Debris, Self::HotJournal, Self::Wal];

    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Debris => "debris",
            Self::HotJournal => "hot-journal",
            Self::Wal => "wal",
        }
    }

    /// Whether the original's survey is partly in a sidecar (the upgrade
    /// settles it).
    const fn unfinished(self) -> bool {
        matches!(self, Self::HotJournal | Self::Wal)
    }

    /// The steps the real upgrade passes through for this kind.
    fn path(self) -> Vec<Step> {
        Step::ALL
            .into_iter()
            .filter(|step| match step {
                Step::Settled | Step::Staged | Step::Replaced => self.unfinished(),
                Step::SidecarRemoved => self != Self::Clean,
                Step::BackupStaged | Step::BackupKept => true,
            })
            .collect()
    }
}

/// A legacy file caught mid-transaction, with the hot rollback journal a kill
/// at that instant leaves beside it. The one-page cache makes the transaction
/// SPILL its pages into the database file, the state a kill must be
/// recoverable from. Returns `(database bytes, journal bytes)`.
pub(super) fn mid_transaction(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let scratch = TempDir::new().ok()?;
    let path = scratch.path().join("interrupted.stalx");
    std::fs::write(&path, bytes).ok()?;
    let connection = Connection::open(&path).ok()?;
    connection.execute_batch("PRAGMA cache_size = 1;").ok()?;
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE project SET name = 'interrupted-write';
             UPDATE shots SET length = length + 1000.0;
             UPDATE sections SET name = 'interrupted-write';",
        )
        .ok()?;
    let interrupted = std::fs::read(&path).ok()?;
    let journal = std::fs::read(sidecar_path(&path, "-journal")).ok()?;
    let _ = connection.execute_batch("ROLLBACK;");
    drop(connection);
    Some((interrupted, journal))
}

/// A legacy file switched to WAL mode by another tool, whose last commit is
/// still in the WAL (never checkpointed). Returns `(database, WAL, index)`.
fn wal_commit(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let scratch = TempDir::new().ok()?;
    let path = scratch.path().join("wal.stalx");
    std::fs::write(&path, bytes).ok()?;
    let connection = Connection::open(&path).ok()?;
    let mode: String = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .ok()?;
    if mode != "wal" {
        return None;
    }
    connection
        .execute_batch(
            "PRAGMA wal_autocheckpoint = 0;
             UPDATE project SET name = 'committed-in-the-wal';
             UPDATE shots SET length = length + 2.5;",
        )
        .ok()?;
    let main = std::fs::read(&path).ok()?;
    let wal = std::fs::read(sidecar_path(&path, "-wal")).ok()?;
    let shm = std::fs::read(sidecar_path(&path, "-shm")).ok()?;
    drop(connection);
    Some((main, wal, shm))
}

/// A legacy original, and what the app makes of it.
pub(super) struct Original {
    pub(super) kind: Kind,
    main: Vec<u8>,
    sidecars: Vec<(&'static str, Vec<u8>)>,
    /// The file with its unfinished changes folded in (`main` when none).
    settled: Vec<u8>,
    /// The survey the app opens it as (canonical text).
    pub(super) survey: String,
    /// The version 2 file the real upgrading save writes for it.
    upgraded: Vec<u8>,
    /// The survey that file holds (canonical text).
    upgraded_survey: String,
}

/// A stem no other test in any process uses (the backups folder is shared).
pub(super) fn unique_stem(what: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{what}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Remove everything the backups folder holds for `stem` (kept originals and
/// partial ones).
pub(super) fn forget_backups(stem: &str) {
    let Some(folder) = upgrade_backup_dir() else {
        return;
    };
    let (kept, hidden) = (format!("{stem}."), format!(".{stem}."));
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&kept) || name.starts_with(&hidden) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The originals the backups folder keeps for `stem`, as Recovery names them.
fn kept_originals(stem: &str) -> Vec<PathBuf> {
    let Some(folder) = upgrade_backup_dir() else {
        return Vec::new();
    };
    let prefix = format!("{stem}.");
    let mut kept: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".pre-v2.stalx"))
                .and_then(|rest| rest.split_once('.'))
                .is_some_and(|(key, stamp)| {
                    key.len() == 12 && super::project_restore::is_upgrade_stamp(stamp)
                })
        })
        .map(|entry| entry.path())
        .collect();
    kept.sort();
    kept
}

impl Original {
    pub(super) fn build(seed: u32, kind: Kind) -> Option<Self> {
        crate::app_dirs::init_for_test();
        let scratch = TempDir::new().ok()?;
        let clean_path = scratch.path().join("clean.stalx");
        write_legacy(&clean_path, &mut Rng::new(seed), "Legacy");
        let clean = std::fs::read(&clean_path).ok()?;
        let (main, sidecars) = match kind {
            Kind::Clean => (clean, Vec::new()),
            Kind::Debris => (
                clean,
                vec![
                    ("-journal", vec![0; 512]),
                    ("-wal", Vec::new()),
                    ("-shm", vec![0; 64]),
                ],
            ),
            Kind::HotJournal => {
                let (main, journal) = mid_transaction(&clean)?;
                (main, vec![("-journal", journal)])
            }
            Kind::Wal => {
                let (main, wal, shm) = wal_commit(&clean)?;
                (main, vec![("-wal", wal), ("-shm", shm)])
            }
        };
        let mut original = Self {
            kind,
            main,
            sidecars,
            settled: Vec::new(),
            survey: String::new(),
            upgraded: Vec::new(),
            upgraded_survey: String::new(),
        };
        let stem = unique_stem(&format!("upgrade-ref-{}-{seed}", kind.id()));
        let path = original.place(scratch.path(), &format!("{stem}.stalx"))?;
        original.settled = if kind.unfinished() {
            settled_bytes(&original.main, &path).ok()?
        } else {
            original.main.clone()
        };
        let state = open_fresh(&path)?;
        original.survey = canonical_snapshot(&state);
        let saved = state.save_project(None).success;
        drop(state);
        forget_backups(&stem);
        if !saved {
            return None;
        }
        original.upgraded = std::fs::read(&path).ok()?;
        original.upgraded_survey = canonical(
            &stalx_v2::decode(&original.upgraded, &DecodeOptions::default())
                .ok()?
                .project,
        );
        Some(original)
    }

    /// The original (the file and its sidecars) at `dir/name`.
    pub(super) fn place(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        let path = dir.join(name);
        std::fs::write(&path, &self.main).ok()?;
        for (suffix, bytes) in &self.sidecars {
            std::fs::write(sidecar_path(&path, suffix), bytes).ok()?;
        }
        Some(path)
    }
}

// ===========================================================================
// The oracle
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// The survey the open showed: nothing was renamed over the project yet.
    Old,
    /// The upgraded file.
    New,
}

/// The canonical survey the file at `path` opens as, or why it does not.
fn opened_as(path: &Path) -> Result<String, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_fresh(path))) {
        Err(payload) => Err(format!(
            "opening it PANICKED: {}",
            panic_text(payload.as_ref())
        )),
        Ok(None) => Err("it would not open".to_string()),
        Ok(Some(state)) => Ok(canonical_snapshot(&state)),
    }
}

/// `destination` and its sidecars copied into `dir` under the same names.
fn copy_project(destination: &Path, dir: &Path) -> Option<PathBuf> {
    let name = destination.file_name()?;
    let copy = dir.join(name);
    std::fs::copy(destination, &copy).ok()?;
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = sidecar_path(destination, suffix);
        if sidecar.is_file() {
            std::fs::copy(&sidecar, sidecar_path(&copy, suffix)).ok()?;
        }
    }
    Some(copy)
}

/// Everything the oracle checks on the files a kill left at `destination`
/// (named `<stem>.stalx`). Removes the backups it finds for `stem` when done.
fn inspect(
    label: &dyn Fn(&str) -> String,
    destination: &Path,
    stem: &str,
    original: &Original,
    expect: Expect,
) -> Vec<String> {
    let mut problems = Vec::new();
    let (expected, expected_name) = match expect {
        Expect::Old => (&original.survey, "the survey the open showed (OLD)"),
        Expect::New => (&original.upgraded_survey, "the upgraded file (NEW)"),
    };

    // Never a JSON file beside an armed journal or a WAL.
    let format = stalx_v2::sniff(&std::fs::read(destination).unwrap_or_default());
    if format == Format::V2
        && (has_hot_journal(destination) || sidecar_path(destination, "-wal").exists())
    {
        problems.push(label(
            "a version 2 file sits beside an armed journal or a WAL",
        ));
    }

    // The project's path is exactly what it must be.
    match opened_as(destination) {
        Ok(found) if &found == expected => {}
        Ok(found) => {
            let verdict = if found == original.survey || found == original.upgraded_survey {
                "the other side of the rename"
            } else {
                "ANOTHER survey"
            };
            problems.push(label(&format!(
                "the project opens as {verdict}, not {expected_name}: {}",
                first_difference(expected, &found)
            )));
        }
        Err(why) => problems.push(label(&format!("the project: {why}"))),
    }

    // The kept original, and what Recovery offers.
    let kept = kept_originals(stem);
    if kept.len() > 1 {
        problems.push(label(&format!(
            "{} originals are kept: {kept:?}",
            kept.len()
        )));
    }
    for path in &kept {
        match opened_as(path) {
            Ok(found) if found == original.survey => {}
            Ok(found) => problems.push(label(&format!(
                "the kept original is not the survey the open showed: {}",
                first_difference(&original.survey, &found)
            ))),
            Err(why) => problems.push(label(&format!("the kept original: {why}"))),
        }
    }
    let offered: Vec<PathBuf> = AppState::list_project_backups(destination)
        .into_iter()
        .map(|b| PathBuf::from(b.path))
        .collect();
    for path in &kept {
        if !offered.contains(path) {
            problems.push(label("Recovery does not offer the kept original"));
        }
    }
    let Some(scratch) = TempDir::new().ok() else {
        return problems;
    };
    for (i, backup) in offered.iter().enumerate() {
        if backup
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            problems.push(label(&format!(
                "Recovery offers a hidden staging file: {}",
                backup.display()
            )));
            continue;
        }
        let restored_as = match stalx_v2::sniff(&std::fs::read(backup).unwrap_or_default()) {
            Format::V2 => &original.upgraded_survey,
            _ => &original.survey,
        };
        let dir = scratch.path().join(format!("restore-{i}"));
        let Some(copy) = std::fs::create_dir_all(&dir)
            .ok()
            .and_then(|()| copy_project(destination, &dir))
        else {
            continue;
        };
        let Ok(fresh) = AppState::new() else {
            continue;
        };
        match fresh.restore_project_from_backup(&copy, backup) {
            Ok(_) => match opened_as(&copy) {
                Ok(found) if &found == restored_as => {}
                Ok(found) => problems.push(label(&format!(
                    "restoring {} installed another survey: {}",
                    backup.display(),
                    first_difference(restored_as, &found)
                ))),
                Err(why) => problems.push(label(&format!(
                    "the project restored from {}: {why}",
                    backup.display()
                ))),
            },
            Err(e) => problems.push(label(&format!(
                "Recovery refuses to restore {}: {e}",
                backup.display()
            ))),
        }
    }

    // Recovery forward: the next open and save leave everything clean.
    match open_fresh(destination) {
        Some(state) => {
            let saved = state.save_project(None);
            if saved.success {
                let left: Vec<String> = destination
                    .parent()
                    .map(entries)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|name| name.contains(stem))
                    .filter(|name| *name != format!("{stem}.stalx"))
                    .collect();
                if !left.is_empty() {
                    problems.push(label(&format!(
                        "the next save left files beside the project: {left:?}"
                    )));
                }
                let partial = upgrade_backup_dir()
                    .map(|dir| entries(&dir))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|name| name.starts_with(&format!(".{stem}.")))
                    .count();
                if partial > 0 {
                    problems.push(label("the next save left a partial backup behind"));
                }
                let reread = std::fs::read(destination).unwrap_or_default();
                if stalx_v2::sniff(&reread) != Format::V2 {
                    problems.push(label("the next save did not write version 2"));
                }
                match opened_as(destination) {
                    Ok(found) if &found == expected => {}
                    Ok(found) => problems.push(label(&format!(
                        "the next save wrote another survey: {}",
                        first_difference(expected, &found)
                    ))),
                    Err(why) => problems.push(label(&format!("after the next save: {why}"))),
                }
            } else {
                problems.push(label(&format!(
                    "the project opened but the next save failed: {:?}",
                    saved.error
                )));
            }
        }
        None => problems.push(label("the project would not open for the next save")),
    }
    forget_backups(stem);
    problems
}

// ===========================================================================
// The staged states
// ===========================================================================

/// The files a kill between two steps leaves, built by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Staged {
    Before,
    CopyTorn,
    CopyWhole,
    Replaced,
    SidecarsGone,
    BackupTorn,
    BackupKept,
    TempTorn,
    TempWhole,
    Renamed,
}

impl Staged {
    const ALL: [Self; 10] = [
        Self::Before,
        Self::CopyTorn,
        Self::CopyWhole,
        Self::Replaced,
        Self::SidecarsGone,
        Self::BackupTorn,
        Self::BackupKept,
        Self::TempTorn,
        Self::TempWhole,
        Self::Renamed,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Before => "before-the-upgrade",
            Self::CopyTorn => "settled-copy-torn",
            Self::CopyWhole => "settled-copy-staged",
            Self::Replaced => "settled-copy-renamed",
            Self::SidecarsGone => "sidecars-removed",
            Self::BackupTorn => "backup-torn",
            Self::BackupKept => "backup-kept",
            Self::TempTorn => "temp-torn",
            Self::TempWhole => "temp-written",
            Self::Renamed => "renamed",
        }
    }

    const fn after(self) -> &'static str {
        match self {
            Self::Before => "before anything, or while the private copy is settled",
            Self::CopyTorn => "while the settled copy is written beside the original",
            Self::CopyWhole => "after the settled copy is synced, before its rename",
            Self::Replaced => {
                "after the settled copy replaced the original, before its sidecars go"
            }
            Self::SidecarsGone => "after the sidecars are removed, before the backup",
            Self::BackupTorn => "while the backup is written under its hidden name",
            Self::BackupKept => "after the backup is under its name, before the temp",
            Self::TempTorn => "while the version 2 temp is written",
            Self::TempWhole => "after the temp is synced, before the rename",
            Self::Renamed => "after the rename",
        }
    }

    /// Whether the upgrade of `kind` passes through this state.
    fn reached(self, kind: Kind) -> bool {
        match self {
            Self::CopyTorn | Self::CopyWhole | Self::Replaced => kind.unfinished(),
            Self::SidecarsGone => kind != Kind::Clean,
            _ => true,
        }
    }
}

/// The stamp the staged kept original carries.
const STAMP: &str = "20261120T000000.000000000Z";

/// Build `state` for `original` in `dir` as `<stem>.stalx`.
fn stage(dir: &Path, stem: &str, original: &Original, state: Staged) -> Option<PathBuf> {
    let destination = dir.join(format!("{stem}.stalx"));
    let backups = upgrade_backup_dir()?;
    std::fs::create_dir_all(&backups).ok()?;
    let prefix = super::project_io::upgrade_backup_prefix(&destination);
    let kept = backups.join(format!("{prefix}{STAMP}.pre-v2.stalx"));
    let partial = backups.join(format!(".{prefix}{STAMP}.pre-v2.stalx.partial"));
    let staged_copy = dir.join(format!(".{stem}.stalx.settled-1-0"));
    let temp = stalx_v2::temp_path(&destination);
    let half = |bytes: &[u8]| bytes[..bytes.len() / 2].to_vec();
    let settled = &original.settled;
    match state {
        Staged::Before => {
            original.place(dir, &format!("{stem}.stalx"))?;
        }
        Staged::CopyTorn | Staged::CopyWhole => {
            original.place(dir, &format!("{stem}.stalx"))?;
            let copy = if state == Staged::CopyTorn {
                half(settled)
            } else {
                settled.clone()
            };
            std::fs::write(&staged_copy, copy).ok()?;
        }
        Staged::Replaced => {
            original.place(dir, &format!("{stem}.stalx"))?;
            std::fs::write(&destination, settled).ok()?;
        }
        Staged::SidecarsGone => std::fs::write(&destination, settled).ok()?,
        Staged::BackupTorn => {
            std::fs::write(&destination, settled).ok()?;
            std::fs::write(&partial, half(settled)).ok()?;
        }
        Staged::BackupKept | Staged::TempTorn | Staged::TempWhole => {
            std::fs::write(&destination, settled).ok()?;
            std::fs::write(&kept, settled).ok()?;
            match state {
                Staged::TempTorn => std::fs::write(&temp, half(&original.upgraded)).ok()?,
                Staged::TempWhole => std::fs::write(&temp, &original.upgraded).ok()?,
                _ => {}
            }
        }
        Staged::Renamed => {
            std::fs::write(&destination, &original.upgraded).ok()?;
            std::fs::write(&kept, settled).ok()?;
        }
    }
    Some(destination)
}

/// Every staged state of the upgrade of `kind`, for one seed. Counts the
/// states checked in `notes`.
pub(super) fn check_upgrade_states(
    seed: u32,
    kind: Kind,
    notes: &mut std::collections::BTreeMap<String, usize>,
) -> Vec<String> {
    let Some(original) = Original::build(seed, kind) else {
        return vec![format!(
            "seed {seed} {}: the legacy original or its upgrade could not be built",
            kind.id()
        )];
    };
    let mut problems = Vec::new();
    for state in Staged::ALL.into_iter().filter(|state| state.reached(kind)) {
        let label = |what: &str| {
            format!(
                "seed {seed} {} [upgrade {}]: {what}\n  the kill lands: {}",
                kind.id(),
                state.id(),
                state.after()
            )
        };
        let Ok(dir) = TempDir::new() else {
            continue;
        };
        let stem = unique_stem(&format!("staged-{}-{seed}", kind.id()));
        let Some(destination) = stage(dir.path(), &stem, &original, state) else {
            problems.push(label("the state could not be built"));
            forget_backups(&stem);
            continue;
        };
        let expect = if state == Staged::Renamed {
            Expect::New
        } else {
            Expect::Old
        };
        problems.extend(inspect(&label, &destination, &stem, &original, expect));
        *notes
            .entry(format!("upgrade states ({})", kind.id()))
            .or_default() += 1;
    }
    problems
}

// ===========================================================================
// The real pipeline, killed
// ===========================================================================

/// The real upgrading save of `original` killed after `step`. Returns whether
/// the kill landed (the step is on this kind's path) and the problems found.
fn kill_real_upgrade(seed: u32, original: &Original, step: Step) -> (bool, Vec<String>) {
    let label = |what: &str| {
        format!(
            "seed {seed} {} [real upgrade killed after {step:?}]: {what}",
            original.kind.id()
        )
    };
    let Ok(dir) = TempDir::new() else {
        return (false, Vec::new());
    };
    let stem = unique_stem(&format!("killed-{}-{seed}", original.kind.id()));
    let Some(destination) = original.place(dir.path(), &format!("{stem}.stalx")) else {
        return (false, vec![label("the original could not be placed")]);
    };
    let Some(state) = open_fresh(&destination) else {
        return (false, vec![label("the original would not open")]);
    };
    KILL_AT.with(|kill| kill.set(Some(step)));
    let saved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.save_project(None)));
    KILL_AT.with(|kill| kill.set(None));
    drop(state);
    let mut problems = Vec::new();
    let killed = match saved {
        Err(payload) => {
            let text = panic_text(payload.as_ref());
            if !text.contains("injected kill") {
                problems.push(label(&format!("the save PANICKED: {text}")));
            }
            true
        }
        Ok(result) => {
            if !result.success {
                problems.push(label(&format!("the save failed: {:?}", result.error)));
            }
            false
        }
    };
    let expect = if killed { Expect::Old } else { Expect::New };
    problems.extend(inspect(&label, &destination, &stem, original, expect));
    (killed, problems)
}

fn check_real_kills(seed: u32, kind: Kind) -> Vec<String> {
    let Some(original) = Original::build(seed, kind) else {
        return vec![format!(
            "seed {seed} {}: the legacy original or its upgrade could not be built",
            kind.id()
        )];
    };
    let mut problems = Vec::new();
    let mut reached = Vec::new();
    for step in Step::ALL {
        let (killed, found) = kill_real_upgrade(seed, &original, step);
        if killed {
            reached.push(step);
        }
        problems.extend(found);
    }
    if reached != kind.path() {
        problems.push(format!(
            "seed {seed} {}: the upgrade passed through {reached:?}, expected {:?}",
            kind.id(),
            kind.path()
        ));
    }
    problems
}

/// The seeds both engines run in the default test run.
const GATE_SEEDS: &[u32] = &[1, 2];

/// THE UPGRADE GATE, staged engine. Not `#[ignore]`d.
#[test]
fn every_staged_upgrade_state_leaves_the_survey_whole() {
    let _config = isolated_config();
    let mut notes = std::collections::BTreeMap::new();
    let mut problems = Vec::new();
    for &seed in GATE_SEEDS {
        for kind in Kind::ALL {
            problems.extend(check_upgrade_states(seed, kind, &mut notes));
        }
    }
    let checked: usize = notes.values().sum();
    let per_seed: usize = Kind::ALL
        .iter()
        .map(|kind| Staged::ALL.iter().filter(|s| s.reached(*kind)).count())
        .sum();
    assert_eq!(
        checked,
        GATE_SEEDS.len() * per_seed,
        "the staged states were not all built: {notes:?}"
    );
    assert!(
        problems.is_empty(),
        "{} upgrade crash-state failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// THE UPGRADE GATE, real engine: the save itself, killed after every step.
#[test]
fn a_kill_after_any_step_of_the_real_upgrade_leaves_the_survey_whole() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for &seed in GATE_SEEDS {
        for kind in Kind::ALL {
            problems.extend(check_real_kills(seed, kind));
        }
    }
    assert!(
        problems.is_empty(),
        "{} real-upgrade kill failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The SWEEP over both engines (the staged states also run in
/// `crash_property_tests::crash_property_sweep`).
///
/// ```text
/// STALAGMIGHT_RUST_CRASH_SEEDS=40 TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib legacy_upgrade_sweep -- --ignored --nocapture
/// ```
#[test]
#[ignore = "sweep: both upgrade engines over STALAGMIGHT_RUST_CRASH_SEEDS seeds"]
fn legacy_upgrade_sweep() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_CRASH_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    let mut notes = std::collections::BTreeMap::new();
    let mut dirty = Vec::new();
    for seed in 1..=seeds {
        let mut problems = Vec::new();
        for kind in Kind::ALL {
            problems.extend(check_upgrade_states(seed, kind, &mut notes));
            problems.extend(check_real_kills(seed, kind));
        }
        if !problems.is_empty() {
            eprintln!("{}", problems.join("\n\n"));
            dirty.push(seed);
        }
    }
    eprintln!(
        "legacy upgrade sweep: {seeds} seed(s), staged states {notes:?}, real kills {} per \
         seed; seeds with a failure ({}): {dirty:?}",
        Kind::ALL.len() * Step::ALL.len(),
        dirty.len()
    );
    assert!(dirty.is_empty(), "seeds with a failure: {dirty:?}");
}

/// The originals are what they claim: a hot journal or a WAL holds part of the
/// survey (the file alone opens as another one), the debris holds nothing.
#[test]
fn the_unfinished_originals_need_their_sidecars() {
    let _config = isolated_config();
    for kind in Kind::ALL {
        let original = Original::build(3, kind).unwrap();
        let dir = TempDir::new().unwrap();
        let alone = dir.path().join("alone.stalx");
        std::fs::write(&alone, &original.main).unwrap();
        let found = opened_as(&alone);
        if kind.unfinished() {
            assert_ne!(
                found.as_deref().ok(),
                Some(original.survey.as_str()),
                "{}: the file alone must not be the survey",
                kind.id()
            );
            assert_ne!(original.settled, original.main, "{}", kind.id());
        } else {
            assert_eq!(found.as_deref().ok(), Some(original.survey.as_str()));
        }
        let marker = match kind {
            Kind::HotJournal => Some("interrupted-write"),
            Kind::Wal => Some("committed-in-the-wal"),
            _ => None,
        };
        if let Some(marker) = marker {
            assert_eq!(
                original.survey.contains(marker),
                kind == Kind::Wal,
                "{}: the open shows the committed survey",
                kind.id()
            );
        }
    }
}

/// The real upgrade of each kind, run through: the JSON file stands alone, the
/// kept original is whole (no sidecar beside it, the survey the open showed),
/// and Recovery restores it.
#[test]
fn a_completed_upgrade_keeps_the_whole_original_and_leaves_no_sidecar() {
    let _config = isolated_config();
    for kind in Kind::ALL {
        let original = Original::build(4, kind).unwrap();
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("completed");
        let path = original
            .place(dir.path(), &format!("{stem}.stalx"))
            .unwrap();
        let state = open_fresh(&path).unwrap();
        assert!(state.save_project(None).success, "{}", kind.id());
        drop(state);

        assert_eq!(stalx_v2::sniff(&std::fs::read(&path).unwrap()), Format::V2);
        assert_eq!(
            entries(dir.path()),
            [format!("{stem}.stalx")],
            "{}",
            kind.id()
        );
        let kept = kept_originals(&stem);
        assert_eq!(kept.len(), 1, "{}: {kept:?}", kind.id());
        assert_eq!(
            std::fs::read(&kept[0]).unwrap(),
            original.settled,
            "{}",
            kind.id()
        );
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(!sidecar_path(&kept[0], suffix).exists(), "{}", kind.id());
        }
        assert_eq!(opened_as(&kept[0]).as_deref(), Ok(original.survey.as_str()));

        let offered = AppState::list_project_backups(&path);
        assert!(
            offered.iter().any(|b| Path::new(&b.path) == kept[0]),
            "{}: Recovery offers the kept original",
            kind.id()
        );
        let upgraded = std::fs::read(&path).unwrap();
        AppState::new()
            .unwrap()
            .restore_project_from_backup(&path, &kept[0])
            .unwrap();
        assert_eq!(opened_as(&path).as_deref(), Ok(original.survey.as_str()));
        assert_eq!(
            std::fs::read(super::project_restore::pre_restore_path(&path)).unwrap(),
            upgraded,
            "{}: the restore keeps the version 2 file as its safety copy",
            kind.id()
        );
        forget_backups(&stem);
    }
}

/// A write that fails after the original was settled: the original is whole
/// (the settled file, no sidecar, the same survey), the redundant backup is
/// gone, and the session's guard follows the settled bytes, so the next save
/// works.
#[test]
fn a_failed_upgrade_leaves_the_original_whole_and_the_next_save_works() {
    let _config = isolated_config();
    for kind in Kind::ALL {
        let original = Original::build(5, kind).unwrap();
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("failed");
        let path = original
            .place(dir.path(), &format!("{stem}.stalx"))
            .unwrap();
        let state = open_fresh(&path).unwrap();
        let survey = canonical_snapshot(&state);
        // The write cannot stage its temp.
        let temp = stalx_v2::temp_path(&path);
        std::fs::create_dir_all(&temp).unwrap();

        assert!(!state.save_project(None).success, "{}", kind.id());

        assert_eq!(
            std::fs::read(&path).unwrap(),
            original.settled,
            "{}",
            kind.id()
        );
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(
                !sidecar_path(&path, suffix).exists(),
                "{}: {suffix}",
                kind.id()
            );
        }
        assert_eq!(opened_as(&path).as_deref(), Ok(survey.as_str()));
        assert!(
            kept_originals(&stem).is_empty(),
            "{}: a failed upgrade keeps no backup while the original is whole",
            kind.id()
        );

        std::fs::remove_dir(&temp).unwrap();
        let again = state.save_project(None);
        assert!(again.success, "{}: {:?}", kind.id(), again.error);
        assert_eq!(stalx_v2::sniff(&std::fs::read(&path).unwrap()), Format::V2);
        assert_eq!(kept_originals(&stem).len(), 1, "{}", kind.id());
        forget_backups(&stem);
    }
}

/// The guard comes first: an original another program changed since the open
/// refuses the upgrade before anything is written (no settle, no backup). The
/// Overwrite answer then upgrades what is there with the same steps.
#[test]
fn an_upgrade_over_a_changed_original_writes_nothing_until_overwrite() {
    let _config = isolated_config();
    let original = Original::build(6, Kind::HotJournal).unwrap();
    let dir = TempDir::new().unwrap();
    let stem = unique_stem("changed");
    let path = original
        .place(dir.path(), &format!("{stem}.stalx"))
        .unwrap();
    let state = open_fresh(&path).unwrap();
    // Another program rolled the journal back and stopped before deleting it.
    std::fs::write(&path, &original.settled).unwrap();
    let journal = sidecar_path(&path, "-journal");
    let journal_bytes = std::fs::read(&journal).unwrap();

    let saved = state.save_project(None);

    assert!(!saved.success);
    assert!(
        saved
            .error
            .as_deref()
            .unwrap_or_default()
            .starts_with(super::open_file::EXTERNALLY_MODIFIED_CODE),
        "{:?}",
        saved.error
    );
    assert_eq!(std::fs::read(&path).unwrap(), original.settled);
    assert_eq!(std::fs::read(&journal).unwrap(), journal_bytes);
    assert_eq!(entries(dir.path()).len(), 2, "{:?}", entries(dir.path()));
    assert!(kept_originals(&stem).is_empty());

    let overwritten = state.save_project_overwriting(None);

    assert!(overwritten.success, "{:?}", overwritten.error);
    assert_eq!(entries(dir.path()), [format!("{stem}.stalx")]);
    assert_eq!(opened_as(&path).as_deref(), Ok(original.survey.as_str()));
    let kept = kept_originals(&stem);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read(&kept[0]).unwrap(), original.settled);
    forget_backups(&stem);
}

/// An original whose unfinished changes SQLite cannot fold in (the pair is
/// damaged) is never replaced, not even by the Overwrite answer: replacing it
/// would leave no whole copy of it. The save says why and changes nothing;
/// Save As still works.
#[test]
fn a_damaged_pair_is_never_replaced() {
    let _config = isolated_config();
    let original = Original::build(6, Kind::HotJournal).unwrap();
    let dir = TempDir::new().unwrap();
    let stem = unique_stem("damaged");
    let path = original
        .place(dir.path(), &format!("{stem}.stalx"))
        .unwrap();
    let state = open_fresh(&path).unwrap();
    let mut damaged = original.main.clone();
    damaged[100] ^= 1;
    std::fs::write(&path, &damaged).unwrap();
    let journal_bytes = std::fs::read(sidecar_path(&path, "-journal")).unwrap();

    let overwritten = state.save_project_overwriting(None);

    assert!(!overwritten.success);
    let error = overwritten.error.unwrap_or_default();
    assert!(
        error.starts_with("Can't save: the original's unfinished changes could not be finished"),
        "{error}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
    assert_eq!(
        std::fs::read(sidecar_path(&path, "-journal")).unwrap(),
        journal_bytes
    );
    assert_eq!(entries(dir.path()).len(), 2, "{:?}", entries(dir.path()));
    assert!(kept_originals(&stem).is_empty());
    let elsewhere = dir.path().join(format!("{stem}-copy.stalx"));
    assert!(state.save_project(Some(elsewhere.clone())).success);
    assert_eq!(
        opened_as(&elsewhere).as_deref(),
        Ok(original.survey.as_str())
    );
    forget_backups(&stem);
}

/// Make `path` read-only, or say why it cannot be (root ignores mode bits):
/// `false` means skip, not pass.
#[cfg(unix)]
fn make_read_only(path: &Path, mode: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    let probe = if path.is_dir() {
        std::fs::File::create(path.join(".probe")).is_ok()
    } else {
        std::fs::OpenOptions::new().append(true).open(path).is_ok()
    };
    if probe {
        let _ = std::fs::remove_file(path.join(".probe"));
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if path.is_dir() { 0o755 } else { 0o644 }),
        )
        .unwrap();
        return false;
    }
    true
}

/// On write-protected media the upgrade is refused with the permission marker
/// and changes nothing: the original and its sidecars as they were, nothing
/// staged, no backup kept.
#[test]
#[cfg(unix)]
fn on_write_protected_media_an_upgrade_changes_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let _config = isolated_config();
    for kind in Kind::ALL {
        let original = Original::build(7, kind).unwrap();
        let temp = TempDir::new().unwrap();
        let media = temp.path().join("media");
        std::fs::create_dir(&media).unwrap();
        let stem = unique_stem("media");
        let path = original.place(&media, &format!("{stem}.stalx")).unwrap();
        let before: Vec<(String, Vec<u8>)> = entries(&media)
            .into_iter()
            .map(|name| {
                let bytes = std::fs::read(media.join(&name)).unwrap();
                (name, bytes)
            })
            .collect();
        let state = open_fresh(&path).unwrap();
        if !make_read_only(&media, 0o555) {
            eprintln!("skipped: this user can write to a 0o555 folder");
            return;
        }

        let saved = state.save_project(None);
        std::fs::set_permissions(&media, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!saved.success, "{}", kind.id());
        assert!(
            saved
                .error
                .as_deref()
                .unwrap_or_default()
                .starts_with(crate::core::loading::PERMISSION_DENIED_CODE),
            "{}: {:?}",
            kind.id(),
            saved.error
        );
        let after: Vec<(String, Vec<u8>)> = entries(&media)
            .into_iter()
            .map(|name| {
                let bytes = std::fs::read(media.join(&name)).unwrap();
                (name, bytes)
            })
            .collect();
        assert!(after == before, "{}: the media changed", kind.id());
        assert!(kept_originals(&stem).is_empty(), "{}", kind.id());
        forget_backups(&stem);
    }
}

/// Settling replaces the original by a rename, as the version 2 write does, so
/// a write-protected legacy file in a writable folder upgrades like any other,
/// and the version 2 file keeps its permissions.
#[test]
#[cfg(unix)]
fn a_write_protected_original_upgrades_whole() {
    use std::os::unix::fs::PermissionsExt;

    let _config = isolated_config();
    for kind in [Kind::HotJournal, Kind::Wal] {
        let original = Original::build(8, kind).unwrap();
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("protected");
        let path = original
            .place(dir.path(), &format!("{stem}.stalx"))
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let state = open_fresh(&path).unwrap();

        let saved = state.save_project(None);

        assert!(saved.success, "{}: {:?}", kind.id(), saved.error);
        assert_eq!(
            entries(dir.path()),
            [format!("{stem}.stalx")],
            "{}",
            kind.id()
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444, "{}", kind.id());
        let kept = kept_originals(&stem);
        assert_eq!(kept.len(), 1, "{}", kind.id());
        assert_eq!(opened_as(&kept[0]).as_deref(), Ok(original.survey.as_str()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        forget_backups(&stem);
    }
}

/// A Recents rename of a closed legacy file goes through the same upgrade:
/// the survey the open shows, renamed, and the whole original kept.
#[test]
fn a_recents_rename_of_a_hot_original_keeps_its_survey() {
    let _config = isolated_config();
    for kind in [Kind::HotJournal, Kind::Wal] {
        let original = Original::build(9, kind).unwrap();
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("rename");
        let path = original
            .place(dir.path(), &format!("{stem}.stalx"))
            .unwrap();
        let state = AppState::new().unwrap();

        let renamed = state.set_project_name_for_path(path.clone(), "Renamed".to_string());

        assert!(renamed.success, "{}: {:?}", kind.id(), renamed.error);
        assert_eq!(
            entries(dir.path()),
            [format!("{stem}.stalx")],
            "{}",
            kind.id()
        );
        let decode = |bytes: &[u8]| {
            stalx_v2::decode(bytes, &DecodeOptions::default())
                .unwrap()
                .project
        };
        let mut project = decode(&std::fs::read(&path).unwrap());
        assert_eq!(project.name, "Renamed");
        project.name = decode(&original.upgraded).name;
        let found = canonical(&project);
        assert!(
            found == original.upgraded_survey,
            "{}: the renamed file is another survey: {}",
            kind.id(),
            first_difference(&original.upgraded_survey, &found)
        );
        let kept = kept_originals(&stem);
        assert_eq!(kept.len(), 1, "{}", kind.id());
        assert_eq!(opened_as(&kept[0]).as_deref(), Ok(original.survey.as_str()));
        forget_backups(&stem);
    }
}

/// A 0.6.0 file over the version 2 decoder's 256 MiB cap opens and upgrades,
/// as it opened in 0.6.0 (review finding D-migration-upgrade-2): the read
/// before the format sniff is capped at the frozen reader's 1 GiB, and only a
/// version 2 file meets the decoder's own cap. The file's size is free pages
/// a dropped table left behind, which a 0.6.0 file that was never vacuumed
/// carries.
#[test]
fn a_legacy_file_over_the_version_2_cap_still_opens_and_upgrades() {
    let _config = isolated_config();
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = unique_stem("oversized");
    let path = dir.path().join(format!("{stem}.stalx"));
    write_legacy(&path, &mut Rng::new(7), "Legacy");
    let pad = stalx_v2::MAX_V2_FILE_BYTES + 4 * 1024 * 1024;
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE pad(b); INSERT INTO pad VALUES (zeroblob({pad})); DROP TABLE pad;"
        ))
        .unwrap();
    }
    assert!(std::fs::metadata(&path).unwrap().len() > stalx_v2::MAX_V2_FILE_BYTES);
    let state = AppState::new().unwrap();
    let opened = state.execute(crate::core::Command::OpenProject { path: path.clone() });
    assert!(opened.success, "the open: {:?}", opened.error);
    let saved = state.save_project(None);
    assert!(saved.success, "the upgrading save: {:?}", saved.error);
    drop(state);
    assert_eq!(stalx_v2::sniff(&std::fs::read(&path).unwrap()), Format::V2);
    assert!(open_fresh(&path).is_some(), "the upgraded file reopens");
    forget_backups(&stem);
}

/// Two projects with the same file name in different folders each get their
/// own pre-upgrade original, and Recovery offers each only its own (review
/// findings D-migration-upgrade-3, J-docs-tests-truth-1: the backups folder
/// is one for the whole app, and it was keyed by the file name alone).
#[test]
fn same_named_projects_in_different_folders_keep_separate_originals() {
    let _config = isolated_config();
    crate::app_dirs::init_for_test();
    let stem = unique_stem("same-name");
    let (one, two) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let paths = [one.path(), two.path()].map(|dir| dir.join(format!("{stem}.stalx")));
    for (i, path) in paths.iter().enumerate() {
        write_legacy(path, &mut Rng::new(11 + i as u32), "Legacy");
        let state = open_fresh(path).unwrap();
        assert!(state.save_project(None).success, "upgrade {i}");
    }
    let offered: Vec<Vec<PathBuf>> = paths
        .iter()
        .map(|path| {
            AppState::list_project_backups(path)
                .into_iter()
                .map(|b| PathBuf::from(b.path))
                .filter(|b| b.to_string_lossy().ends_with(".pre-v2.stalx"))
                .collect()
        })
        .collect();
    assert_eq!(offered[0].len(), 1, "{offered:?}");
    assert_eq!(offered[1].len(), 1, "{offered:?}");
    assert_ne!(offered[0], offered[1], "each its own original");
    assert_eq!(kept_originals(&stem).len(), 2);
    forget_backups(&stem);
}

/// The app's open (`load_project_full`, the IPC) of a 0.6.0 file an old build
/// was killed writing: its SpeleoDB-link probe cannot read the file read-only
/// past the hot journal, so it opens the original read-write, and SQLite rolls
/// the journal back into it in place, as 0.6.0's own open did. That is one of
/// the two writes an open makes (saving.md, "What an open writes"; review
/// finding D-migration-upgrade-11): the session then reads the survey 0.6.0
/// committed, and the first save upgrades a file with no sidecar left, so the
/// settle step above runs only where this rollback cannot (a write-protected
/// original, an open that skips the probe such as `Command::OpenProject`, the
/// STAGED and REAL engines here).
#[test]
fn the_app_open_rolls_a_hot_journal_back_before_the_first_save() {
    struct NullSink;
    impl crate::core::events::EventSink for NullSink {
        fn emit(&self, _event: &str, _payload: serde_json::Value) {}
        fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
            false
        }
    }
    let _config = isolated_config();
    crate::app_dirs::init_for_test();
    let original = Original::build(11, Kind::HotJournal).unwrap();
    let dir = TempDir::new().unwrap();
    let stem = unique_stem("app-open-hot");
    let path = original
        .place(dir.path(), &format!("{stem}.stalx"))
        .unwrap();
    assert!(has_hot_journal(&path), "the fixture is a hot pair");
    let state = AppState::new().unwrap();
    state.set_event_sink(std::sync::Arc::new(NullSink));

    state.do_load_project_full(path.clone()).unwrap();

    assert!(
        !sidecar_path(&path, "-journal").exists(),
        "the open rolled the journal back and removed it"
    );
    assert_eq!(
        opened_as(&path).as_deref(),
        Ok(original.survey.as_str()),
        "into the survey 0.6.0 committed"
    );
    assert!(state.save_project(None).success);
    assert_eq!(entries(dir.path()), [format!("{stem}.stalx")]);
    let kept = kept_originals(&stem);
    assert_eq!(kept.len(), 1);
    assert_eq!(opened_as(&kept[0]).as_deref(), Ok(original.survey.as_str()));
    forget_backups(&stem);
}
