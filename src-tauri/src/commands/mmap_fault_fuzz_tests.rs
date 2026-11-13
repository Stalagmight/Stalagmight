//! [#706] Fuzz the ways the open project's file can change on disk while SQLite
//! still maps it.
//!
//! A load maps the open `.stalx` (`Database::open_existing` sets a 256 MB
//! `mmap_size`). If something truncates or rewrites that inode in place without
//! going through SQLite, the next read through the mapping faults with SIGBUS.
//! On macOS a fault like that can end in the endless spin #706 reports. An
//! atomic replace (temp file + rename) or an unlink is safe, because the mapping
//! keeps the old inode.
//!
//! Each seed builds synthetic projects in a temp dir and opens one through the
//! real load path. It then runs random steps: app commands aimed at a pool of
//! paths that includes the open file, and moves another process could make on
//! those files. After every step a probe reads every page through the live
//! connection.
//!
//! The oracle is the process surviving. An in-place rewrite of a mapped file
//! kills the test binary with SIGBUS, and the last `FZ706-STEP` line printed
//! names the seed and the step. Ordinary `Err` results from commands are
//! expected and only traced. A probe that fails, or that sees a corrupt live
//! view right after an APP step, is a finding: `FZ706-FINDING kind=torn`.
//!
//! Every restore is checked against the files as they were just before it:
//! - one that reports success must leave the backup's bytes in the target and
//!   the target's old bytes in `<target>.stalx.pre-restore`;
//! - one that is refused must leave the target, the backup and the safety copy
//!   exactly as they were.
//!
//! Knobs:
//! - `FZ706_FIRST`: first seed, default 0.
//! - `FZ706_SEEDS`: number of seeds, default 64.
//! - `FZ706_STEPS`: steps per seed, default 40.
//! - `FZ706_HAZARD=1`: also lets an "external process" truncate the open file
//!   to 0 bytes in place, as a sync client rewriting it would for a moment. That
//!   is the design hazard and is EXPECTED to SIGBUS (SQLite reads the header
//!   through the mapping; a shorter but non-empty file only reads as malformed).
//!   It exists to prove the probe really reads through the mapping.
//! - `FZ706_FOCUS=restore`: make half of all steps restores, aimed at the open
//!   file, backups, safety copies and the project itself.
//! - `FZ706_MUTE=<id>,...`: skip the step shapes of the ids below (counted as
//!   `FZ706-MUTED <id>` lines). Nothing is muted by default: all five are
//!   fixed (#720), and a step that hits one again is a regression.
//!
//! Finding ids, for restores that used to break (#720):
//! - `F1-restore-over-open`: `restore_from_backup` aimed at the OPEN project's
//!   file. It used to copy onto it in place, so the live mapping read a torn
//!   file, or SIGBUS when the copy shrank it. "Open" means the inode the live
//!   connection holds, so it also covers the open file after another process
//!   renamed it, which a guard that compares paths would miss.
//! - `F2-restore-self-empties`: the project restored from itself. It used to
//!   come out as 0 bytes (`std::fs::copy` opens the destination with O_TRUNC
//!   before it reads the source).
//! - `F3-backup-lists-itself`: `list_backups` offering a project whose name
//!   contains `.backup` as a backup of itself.
//! - `F4-pre-restore-over-open`: the safety copy (`<project>.stalx.pre-restore`)
//!   aimed at the open project. It used to be written in place too.
//! - `F5-restore-destroys-its-backup`: restoring from `<project>.stalx.pre-restore`.
//!   It used to overwrite that backup with the project before reading it.
//!
//! Every `FZ706-STEP` line carries a `shape=` token (paths classified as open /
//! self / backup / other / fresh / missing), so a failure can be grouped by
//! shape even when a SIGBUS leaves only the last step line behind.
//!
//! All data is synthetic. No fixture is read or written.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::{export, project, recovery};
use crate::core::events::EventSink;
use crate::core::{AppState, Command};
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

/// The load needs a sink for its progress events; nothing listens here.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// splitmix64: small, seedable, and stable across platforms and releases.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x706F_A11E_D5EE_D706)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n.max(1) as u64)).unwrap_or(0)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// One flushed line on stderr: if the next step kills the process, this line
/// is already out.
fn trace(line: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
}

fn synthetic_project(rng: &mut Rng, name: &str) -> Project {
    let mut project = Project::new(name.to_string(), None);
    let sections = 1 + rng.below(4);
    for s in 0..sections {
        let count = 5 + rng.below(80);
        let mut depth = 3.0;
        let shots = (0..count)
            .map(|_| {
                let step = f64::from(u32::try_from(rng.below(30)).unwrap_or(0)) / 10.0;
                let shot = ShotData {
                    length: 1.0 + f64::from(u32::try_from(rng.below(400)).unwrap_or(0)) / 10.0,
                    azimuth_in: f64::from(u32::try_from(rng.below(360)).unwrap_or(0)),
                    azimuth_out: f64::from(u32::try_from(rng.below(360)).unwrap_or(0)),
                    depth_in: depth,
                    depth_out: depth + step,
                    ..Default::default()
                };
                depth += step;
                shot
            })
            .collect();
        project.add_section(SectionData {
            name: Some(format!("S{s}")),
            color: "#336699".to_string(),
            shots,
            ..Default::default()
        });
    }
    project
}

/// Whether two paths are the same file (canonical paths when both exist).
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// The inode behind `path` (device, inode), if it exists.
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Where `restore_from_backup` writes its safety copy of `project`.
fn safety_copy_of(project: &Path) -> PathBuf {
    project.with_extension("stalx.pre-restore")
}

fn write_project(path: &Path, project: &Project) {
    let mut db = Database::open(path).expect("create a synthetic project file");
    db.save_project(project)
        .expect("save the synthetic project");
}

/// Whether the step was the app's own doing or another process's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Actor {
    App,
    External,
}

struct World {
    dir: TempDir,
    state: AppState,
    /// Files the steps aim at. Some may be gone or not be projects by now.
    pool: Vec<PathBuf>,
    /// A pristine copy no step touches, the source of every external replace.
    spare: PathBuf,
    hazard: bool,
    mutes: Vec<String>,
    muted: BTreeMap<&'static str, u32>,
    fresh: u32,
    /// Probes that ran with a mapped live connection.
    mapped_probes: u32,
    /// The live view was broken by another process, so a bad probe after a
    /// later app step says nothing about that step. Cleared once a probe reads
    /// clean again (a reopen or close replaced the live connection).
    tainted: bool,
    /// The inode the live connection was opened on. Another process may have
    /// renamed it since, so the open project's path no longer names it.
    open_ino: Option<(u64, u64)>,
    /// `FZ706_FOCUS=restore` (set by `run_seed`): half of all steps are restores.
    focus_restore: bool,
}

/// What one step was, and which known finding (if any) its shape belongs to.
struct Step {
    actor: Actor,
    what: String,
    shape: String,
    known: Option<&'static str>,
    /// For a restore: the files it may touch, as they were just before it ran.
    restore_expect: Option<RestoreExpect>,
    /// The traced outcome (`ok`, `err ...`).
    outcome: String,
}

/// The files a restore may touch, with their bytes just before it ran (`None`:
/// the file was not there).
struct RestoreExpect {
    target: PathBuf,
    target_before: Option<Vec<u8>>,
    backup: PathBuf,
    backup_before: Option<Vec<u8>>,
    safety: PathBuf,
    safety_before: Option<Vec<u8>>,
}

impl RestoreExpect {
    fn capture(target: PathBuf, backup: PathBuf) -> Self {
        let safety = safety_copy_of(&target);
        Self {
            target_before: std::fs::read(&target).ok(),
            backup_before: std::fs::read(&backup).ok(),
            safety_before: std::fs::read(&safety).ok(),
            target,
            backup,
            safety,
        }
    }
}

fn size_of(bytes: Option<&Vec<u8>>) -> String {
    bytes.map_or_else(|| "nothing".to_string(), |b| format!("{} bytes", b.len()))
}

impl World {
    fn new(rng: &mut Rng, hazard: bool, mutes: Vec<String>) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut pool = Vec::new();
        for name in ["a.stalx", "b.stalx", "c.backup.stalx"] {
            let path = dir.path().join(name);
            write_project(&path, &synthetic_project(rng, name));
            pool.push(path);
        }
        let spare = dir.path().join("spare.keep");
        write_project(&spare, &synthetic_project(rng, "spare"));
        let state = AppState::new().expect("app state");
        state.set_event_sink(Arc::new(NullSink));
        Self {
            dir,
            state,
            pool,
            spare,
            hazard,
            mutes,
            muted: BTreeMap::new(),
            fresh: 0,
            mapped_probes: 0,
            tainted: false,
            open_ino: None,
            focus_restore: false,
        }
    }

    fn open_path(&self) -> Option<PathBuf> {
        self.state.open_project_path_for_tests()
    }

    /// Whether `path` is the open project: by name, or by being the inode the
    /// live connection was opened on.
    fn is_open(&self, path: &Path) -> bool {
        self.open_path().is_some_and(|open| {
            same_file(&open, path) || (self.open_ino.is_some() && file_id(path) == self.open_ino)
        })
    }

    /// `apply`, then note which inode the live connection is on when an app
    /// step may have opened a new one. Another process's moves never change
    /// it: the connection keeps the inode it has.
    fn apply_tracked(&mut self, what: &str) -> String {
        let before = self.open_path();
        let outcome = self.apply(what);
        let verb = what.split(' ').next().unwrap_or_default();
        let after = self.open_path();
        let reopened = outcome == "ok" && matches!(verb, "reopen" | "create" | "save" | "save-as");
        if after.is_none() {
            self.open_ino = None;
        } else if reopened || after != before {
            self.open_ino = after.as_deref().and_then(file_id);
        }
        outcome
    }

    fn fresh_path(&mut self, ext: &str) -> PathBuf {
        self.fresh += 1;
        let path = self.dir.path().join(format!("n{}.{ext}", self.fresh));
        self.pool.push(path.clone());
        path
    }

    /// A target path: the open file most often, otherwise the pool, a backup
    /// the app made, a temp-file name, or a fresh name.
    fn pick(&mut self, rng: &mut Rng, ext: &str) -> PathBuf {
        let open = self.open_path();
        match rng.below(10) {
            0..=3 => {
                if let Some(open) = open {
                    return open;
                }
            }
            4 => {
                let backups: Vec<PathBuf> =
                    self.pool.iter().flat_map(|p| Self::backups_of(p)).collect();
                if !backups.is_empty() {
                    return backups[rng.below(backups.len())].clone();
                }
            }
            5 => {
                if let Some(open) = open {
                    return open.with_extension("stalx.tmp");
                }
            }
            6 => return self.fresh_path(ext),
            _ => {}
        }
        self.pool[rng.below(self.pool.len())].clone()
    }

    /// An existing file to restore from or merge in.
    fn pick_existing(&self, rng: &mut Rng) -> PathBuf {
        let existing: Vec<PathBuf> = self.pool.iter().filter(|p| p.exists()).cloned().collect();
        if existing.is_empty() || rng.chance(15) {
            return self.spare.clone();
        }
        existing[rng.below(existing.len())].clone()
    }

    /// How `path` relates to the session: the open file, a backup or temp
    /// file of some project, another project, a fresh name, or gone.
    fn shape_of(&self, path: &Path) -> &'static str {
        if self.is_open(path) {
            return "open";
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !path.exists() {
            return if name.starts_with('n') {
                "fresh"
            } else {
                "missing"
            };
        }
        let is_tmp = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("tmp"));
        if name.contains(".backup") || is_tmp {
            "backup"
        } else {
            "other"
        }
    }

    /// Generate one random step (nothing is done yet).
    fn next_step(&mut self, rng: &mut Rng) -> Step {
        let ops = if self.hazard { 18 } else { 17 };
        let op = if self.focus_restore && rng.chance(50) {
            9
        } else {
            rng.below(ops)
        };
        let app = |what: String, shape: String| Step {
            actor: Actor::App,
            what,
            shape,
            known: None,
            restore_expect: None,
            outcome: String::new(),
        };
        match op {
            0 => app("edit".to_string(), "edit".to_string()),
            1 => app("save".to_string(), "save".to_string()),
            2..=4 | 6..=8 | 10 | 11 | 13 | 14 | 16 => {
                let (verb, ext) = match op {
                    2 => ("save-as", "stalx"),
                    3 | 4 => ("reopen", "stalx"),
                    6 => ("export-csv", "csv"),
                    7 => ("export-json", "json"),
                    8 => ("export-tml", "tml"),
                    10 => ("repair", "stalx"),
                    11 => ("salvage", "stalx"),
                    13 => ("create", "stalx"),
                    14 => ("delete", "stalx"),
                    _ => ("list-backups", "stalx"),
                };
                let t = self.pick(rng, ext);
                let shape = format!("{verb}:{}", self.shape_of(&t));
                app(format!("{verb} {}", self.name(&t)), shape)
            }
            5 => app("close".to_string(), "close".to_string()),
            9 => {
                let target = self.pick(rng, "stalx");
                let source = if rng.chance(70) {
                    self.pick_backup_of(rng, &target)
                } else {
                    None
                };
                let source = source.unwrap_or_else(|| self.pick_existing(rng));
                let (ts, ss) = (self.shape_of(&target), self.shape_of(&source));
                let ss = if same_file(&target, &source) {
                    "self"
                } else {
                    ss
                };
                let safety = safety_copy_of(&target);
                let known = if ts == "open" {
                    Some("F1-restore-over-open")
                } else if ss == "self" {
                    Some("F2-restore-self-empties")
                } else if self.is_open(&safety) {
                    Some("F4-pre-restore-over-open")
                } else if source.exists() && same_file(&safety, &source) {
                    Some("F5-restore-destroys-its-backup")
                } else {
                    None
                };
                Step {
                    actor: Actor::App,
                    what: format!("restore {} <- {}", self.name(&target), self.name(&source)),
                    shape: format!("restore:{ts}<-{ss}"),
                    known,
                    restore_expect: None,
                    outcome: String::new(),
                }
            }
            12 => {
                let source = self.pick_existing(rng);
                let out = self.pick(rng, "stalx");
                let shape = format!("merge:{}->{}", self.shape_of(&source), self.shape_of(&out));
                app(
                    format!("merge {} -> {}", self.name(&source), self.name(&out)),
                    shape,
                )
            }
            15 => {
                let target = self.pick(rng, "stalx");
                let kind = match rng.below(3) {
                    0 => "unlink",
                    1 => "rename-away",
                    _ => "replace",
                };
                let shape = format!("ext-{kind}:{}", self.shape_of(&target));
                Step {
                    actor: Actor::External,
                    what: format!("ext-{kind} {}", self.name(&target)),
                    shape,
                    known: None,
                    restore_expect: None,
                    outcome: String::new(),
                }
            }
            _ => Step {
                actor: Actor::External,
                what: "ext-truncate open".to_string(),
                shape: "ext-truncate:open".to_string(),
                known: None,
                restore_expect: None,
                outcome: String::new(),
            },
        }
    }

    /// What `target` can be restored from: the backups the recovery dialog
    /// offers, and the safety copy an earlier restore left (undoing it).
    fn backups_of(target: &Path) -> Vec<PathBuf> {
        let mut backups: Vec<PathBuf> = recovery::list_backups(target.to_path_buf())
            .into_iter()
            .map(|b| PathBuf::from(b.path))
            .collect();
        let safety = safety_copy_of(target);
        if safety.exists() {
            backups.push(safety);
        }
        backups
    }

    /// One of the files `target` can be restored from.
    fn pick_backup_of(&self, rng: &mut Rng, target: &Path) -> Option<PathBuf> {
        let backups = Self::backups_of(target);
        (!backups.is_empty()).then(|| backups[rng.below(backups.len())].clone())
    }

    /// Run one random step; returns it, or None when it was muted.
    fn step(&mut self, rng: &mut Rng, seed: u64, index: u32) -> Option<Step> {
        let mut step = self.next_step(rng);
        if let Some(id) = step.known.filter(|id| self.mutes.iter().any(|m| m == id)) {
            *self.muted.entry(id).or_default() += 1;
            trace(&format!(
                "FZ706-MUTED {id} seed={seed} i={index} shape={}",
                step.shape
            ));
            return None;
        }
        trace(&format!(
            "FZ706-STEP seed={seed} i={index} shape={} {}",
            step.shape, step.what
        ));
        let mut words = step.what.split(' ');
        if words.next() == Some("restore") {
            let target = words.next().map(|n| self.path(n));
            let source = words.nth(1).map(|n| self.path(n));
            if let (Some(target), Some(source)) = (target, source) {
                step.restore_expect = Some(RestoreExpect::capture(target, source));
            }
        }
        step.outcome = self.apply_tracked(&step.what);
        trace(&format!(
            "FZ706-DONE seed={seed} i={index} {}",
            step.outcome
        ));
        Some(step)
    }

    /// Oracles that look at the files, not the live connection.
    fn check_files(&self, step: &Step) -> Option<(&'static str, String)> {
        let mut words = step.what.split(' ');
        let verb = words.next().unwrap_or_default();
        let target = words.next().map(|n| self.path(n));
        match (verb, target) {
            ("restore", Some(_)) => {
                let e = step.restore_expect.as_ref()?;
                let id = step.known.unwrap_or("new-restore-mismatch");
                let now = |path: &Path| std::fs::read(path).ok();
                if step.outcome == "ok" {
                    // The backup's bytes in the target, the target's old bytes
                    // in the safety copy.
                    let target_now = now(&e.target);
                    if target_now != e.backup_before {
                        return Some((
                            id,
                            format!(
                                "restore said ok, {} holds {}, the backup held {}",
                                self.name(&e.target),
                                size_of(target_now.as_ref()),
                                size_of(e.backup_before.as_ref())
                            ),
                        ));
                    }
                    let safety_now = now(&e.safety);
                    if e.target_before.is_some() && safety_now != e.target_before {
                        return Some((
                            id,
                            format!(
                                "restore said ok, the safety copy {} holds {}, the project held {}",
                                self.name(&e.safety),
                                size_of(safety_now.as_ref()),
                                size_of(e.target_before.as_ref())
                            ),
                        ));
                    }
                    return None;
                }
                // Refused or failed: nothing it could touch has changed.
                [
                    ("project", &e.target, &e.target_before),
                    ("backup", &e.backup, &e.backup_before),
                    ("safety copy", &e.safety, &e.safety_before),
                ]
                .into_iter()
                .find_map(|(role, path, before)| {
                    let after = now(path);
                    (after != *before).then(|| {
                        (
                            id,
                            format!(
                                "restore said `{}`, but the {role} {} went from {} to {}",
                                step.outcome,
                                self.name(path),
                                size_of(before.as_ref()),
                                size_of(after.as_ref())
                            ),
                        )
                    })
                })
            }
            ("list-backups", Some(t)) => {
                let lists_itself = recovery::list_backups(t.clone())
                    .iter()
                    .any(|b| same_file(Path::new(&b.path), &t));
                (lists_itself && !self.mutes.iter().any(|m| m == "F3-backup-lists-itself")).then(
                    || {
                        (
                            "F3-backup-lists-itself",
                            format!("{} is offered as its own backup", self.name(&t)),
                        )
                    },
                )
            }
            _ => None,
        }
    }

    fn name(&self, path: &Path) -> String {
        path.strip_prefix(self.dir.path())
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Carry out a step described by `what` (the same string the trace shows,
    /// so a failing seed can be read straight off the log).
    fn apply(&self, what: &str) -> String {
        let mut words = what.split(' ');
        let verb = words.next().unwrap_or_default();
        let a = words.next().map(|n| self.path(n));
        let b = words.nth(1).map(|n| self.path(n));
        let state = &self.state;
        match (verb, a, b) {
            ("edit", _, _) => brief_result(&state.execute(Command::CreateSection {
                name: None,
                surveyor: None,
                date: None,
                color: "#aa5500".to_string(),
                direction: None,
                shots: vec![ShotData {
                    length: 4.0,
                    depth_in: 6.0,
                    depth_out: 6.5,
                    ..Default::default()
                }],
            })),
            ("save", _, _) => brief_result(&project::save_project_impl(state, None)),
            ("save-as", Some(p), _) => brief_result(&project::save_project_impl(state, Some(p))),
            ("reopen", Some(p), _) => match state.do_load_project_full(p) {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("err {}", e.code),
            },
            ("close", _, _) => brief_result(&state.execute(Command::CloseProject)),
            ("export-csv", Some(p), _) => brief_string(export::export_csv_impl(state, p)),
            ("export-json", Some(p), _) => brief_string(export::export_json_impl(state, p)),
            ("export-tml", Some(p), _) => {
                brief_string(export::export_tml_impl(state, p).map(|_| ()))
            }
            ("restore", Some(target), Some(source)) => {
                brief_string(recovery::restore_from_backup_impl(state, target, source).map(|_| ()))
            }
            ("repair", Some(p), _) => brief_string(recovery::attempt_repair(p).map(|_| ())),
            ("salvage", Some(p), _) => {
                brief_string(recovery::export_salvageable_data(p).map(|_| ()))
            }
            ("merge", Some(source), Some(out)) => {
                let mut sources = vec![source];
                if let Some(open) = self.open_path() {
                    sources.push(open);
                }
                brief_result(&project::merge_projects_impl(
                    state,
                    sources,
                    out,
                    "Merged".to_string(),
                    false,
                ))
            }
            ("create", Some(p), _) => {
                match state.do_create_project_full("Fresh".to_string(), None, None, Some(p), false)
                {
                    Ok(_) => "ok".to_string(),
                    Err(e) => format!("err {}", e.code),
                }
            }
            ("delete", Some(p), _) => brief_result(&project::delete_project_impl(state, p)),
            ("list-backups", Some(p), _) => format!("{} offered", recovery::list_backups(p).len()),
            ("ext-unlink", Some(p), _) => brief_io(std::fs::remove_file(p)),
            ("ext-rename-away", Some(p), _) => {
                let away = p.with_extension("moved");
                brief_io(std::fs::rename(p, away))
            }
            ("ext-replace", Some(p), _) => {
                // What a sync client does: write a full copy next to the
                // target, then rename it over the target.
                let staged = p.with_extension("sync-tmp");
                brief_io(
                    std::fs::copy(&self.spare, &staged).and_then(|_| std::fs::rename(&staged, &p)),
                )
            }
            ("ext-truncate", _, _) => match self.open_path() {
                Some(open) => brief_io(
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&open)
                        .and_then(|f| f.set_len(0)),
                ),
                None => "skip no open project".to_string(),
            },
            _ => format!("skip unparsed {what}"),
        }
    }

    /// Read every page through the live connection. Returns a finding detail
    /// when an APP step left the live view unreadable or corrupt.
    fn probe(&mut self, seed: u64, index: u32, actor: Actor) -> Option<String> {
        trace(&format!("FZ706-PROBE seed={seed} i={index}"));
        let Some(scan) = self
            .state
            .with_open_database_for_tests(Database::scan_live_for_tests)
        else {
            self.tainted = false;
            return None;
        };
        let detail = match scan {
            Ok(scan) => {
                if scan.mmap_size > 0 {
                    self.mapped_probes += 1;
                }
                if scan.quick_check == "ok" {
                    self.tainted = false;
                    return None;
                }
                format!("quick_check={} mmap={}", scan.quick_check, scan.mmap_size)
            }
            Err(e) => format!("probe error: {e}"),
        };
        trace(&format!(
            "FZ706-PROBE-BAD seed={seed} i={index} tainted={} {detail}",
            self.tainted
        ));
        if actor == Actor::External {
            self.tainted = true;
        }
        (actor == Actor::App && !self.tainted).then_some(detail)
    }
}

fn brief_result(result: &crate::core::CommandResult) -> String {
    if result.success {
        "ok".to_string()
    } else {
        format!(
            "err {}",
            result
                .error
                .as_deref()
                .unwrap_or("?")
                .chars()
                .take(80)
                .collect::<String>()
        )
    }
}

fn brief_string(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("err {}", e.chars().take(80).collect::<String>()),
    }
}

fn brief_io<T>(result: std::io::Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("err {:?}", e.kind()),
    }
}

/// Run one seed; returns the first finding line, if any.
fn run_seed(seed: u64, steps: u32, hazard: bool, focus: bool, mutes: &[&str]) -> Option<String> {
    crate::test_helpers::with_isolated_config(|| {
        let mut rng = Rng::new(seed);
        let mutes = mutes.iter().map(ToString::to_string).collect();
        let mut world = World::new(&mut rng, hazard, mutes);
        world.focus_restore = focus;
        trace(&format!("FZ706-SEED seed={seed} begin"));
        let first = world.pool[0].clone();
        let opened = world.apply_tracked(&format!("reopen {}", world.name(&first)));
        trace(&format!("FZ706-DONE seed={seed} i=0 open a.stalx {opened}"));
        let mut finding = None;
        for index in 1..=steps {
            let Some(step) = world.step(&mut rng, seed, index) else {
                continue;
            };
            let bad = world.check_files(&step).or_else(|| {
                world
                    .probe(seed, index, step.actor)
                    .map(|d| (step.known.unwrap_or("new-torn"), d))
            });
            if let Some((id, detail)) = bad {
                let line = format!(
                    "FZ706-FINDING seed={seed} i={index} id={id} shape={} op={} detail={detail}",
                    step.shape, step.what
                );
                trace(&line);
                finding = Some(line);
                break;
            }
        }
        for (id, n) in &world.muted {
            trace(&format!("FZ706-MUTED-COUNT seed={seed} {id} x{n}"));
        }
        trace(&format!(
            "FZ706-SEED seed={seed} end mapped_probes={} finding={}",
            world.mapped_probes,
            finding.is_some()
        ));
        finding
    })
}

/// What the live connection reads now, or `None` with no project open.
fn live_digest(state: &AppState) -> Option<Result<u64, String>> {
    state
        .with_open_database_for_tests(Database::scan_live_for_tests)
        .map(|scan| scan.map(|s| s.digest).map_err(|e| e.to_string()))
}

/// On-demand campaign: run with `--ignored`, seed windows via `FZ706_FIRST` /
/// `FZ706_SEEDS`.
#[test]
#[ignore = "fuzz campaign (#706): run with --ignored, seeds via FZ706_FIRST/FZ706_SEEDS"]
fn fz706_campaign() {
    let first = knob("FZ706_FIRST", 0);
    let seeds = knob("FZ706_SEEDS", 64);
    let steps = u32::try_from(knob("FZ706_STEPS", 40)).unwrap_or(40);
    let hazard = std::env::var("FZ706_HAZARD").is_ok_and(|v| v == "1");
    let focus = std::env::var("FZ706_FOCUS").is_ok_and(|v| v == "restore");
    let mute_env = std::env::var("FZ706_MUTE").unwrap_or_default();
    let mutes: Vec<&str> = mute_env
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .collect();
    let findings: Vec<String> = (first..first.saturating_add(seeds))
        .filter_map(|seed| run_seed(seed, steps, hazard, focus, &mutes))
        .collect();
    trace(&format!(
        "FZ706-SUMMARY first={first} seeds={seeds} steps={steps} hazard={hazard} focus={focus} mutes={} findings={}",
        mutes.join(","),
        findings.len()
    ));
    assert!(findings.is_empty(), "{}", findings.join("\n"));
}

/// A few seeds on every test run, so the harness itself cannot rot. Nothing is
/// muted: every finding it knows is fixed (#720).
#[test]
fn fz706_smoke() {
    for seed in 0..3 {
        assert_eq!(run_seed(seed, 15, false, false, &[]), None, "seed {seed}");
    }
}

/// The same with half of the steps restores (`FZ706_FOCUS=restore`).
#[test]
fn fz706_restore_smoke() {
    for seed in 100..103 {
        assert_eq!(run_seed(seed, 20, false, true, &[]), None, "seed {seed}");
    }
}

/// F1 (#720): a restore aimed at the open project is refused, and the live
/// connection reads what it read before. It used to copy over the mapped file
/// in place: the app went on showing the old project, and an empty backup
/// crashed the next read with SIGBUS.
#[test]
fn f1_a_restore_never_rewrites_the_open_project_under_its_live_connection() {
    crate::test_helpers::with_isolated_config(|| {
        let mut rng = Rng::new(7);
        let world = World::new(&mut rng, false, Vec::new());
        let (open, backup) = (world.pool[0].clone(), world.pool[1].clone());
        world
            .state
            .do_load_project_full(open.clone())
            .expect("open the project");
        let before = (live_digest(&world.state), std::fs::read(&open).ok());
        let result = recovery::restore_from_backup_impl(&world.state, open.clone(), backup);
        let after = (live_digest(&world.state), std::fs::read(&open).ok());
        assert_eq!(
            result,
            Err(
                "Not restored: a.stalx is open in Stalagmight. Close it, then restore.".to_string()
            )
        );
        assert_eq!(after, before);
    });
}

/// F1 under another name (#720): another process renamed the open file, and
/// the restore targets the new name. The app's open-project path no longer
/// names it, so the restore goes ahead; it renames the backup into place, and
/// the live connection keeps the inode it maps.
#[test]
fn f1_b_a_restore_never_rewrites_the_open_file_after_it_was_renamed() {
    crate::test_helpers::with_isolated_config(|| {
        let mut rng = Rng::new(7);
        let world = World::new(&mut rng, false, Vec::new());
        let (open, backup) = (world.pool[0].clone(), world.pool[1].clone());
        world
            .state
            .do_load_project_full(open.clone())
            .expect("open the project");
        let before = live_digest(&world.state);
        let renamed = open.with_extension("moved");
        std::fs::rename(&open, &renamed).expect("another process renames the open file");
        let backup_bytes = std::fs::read(&backup).expect("backup");
        recovery::restore_from_backup_impl(&world.state, renamed.clone(), backup)
            .expect("the renamed file is not the open project's path");
        assert_eq!(
            live_digest(&world.state),
            before,
            "the live inode is untouched"
        );
        assert_eq!(std::fs::read(&renamed).expect("restored"), backup_bytes);
    });
}

/// F4 (#720): with the safety copy an earlier restore left open, the next
/// restore of its project is refused. It used to rewrite the open file in
/// place with the new safety copy.
#[test]
fn f4_the_safety_copy_never_rewrites_the_open_project() {
    crate::test_helpers::with_isolated_config(|| {
        let mut rng = Rng::new(11);
        let world = World::new(&mut rng, false, Vec::new());
        let project = world.pool[0].clone();
        recovery::restore_from_backup_impl(&world.state, project.clone(), world.pool[1].clone())
            .expect("the first restore");
        let safety = safety_copy_of(&project);
        world
            .state
            .do_load_project_full(safety.clone())
            .expect("open the safety copy the first restore left");
        let before = (live_digest(&world.state), std::fs::read(&safety).ok());
        let result =
            recovery::restore_from_backup_impl(&world.state, project, world.pool[2].clone());
        assert!(
            result
                .as_ref()
                .is_err_and(|e| e.contains("a.stalx.pre-restore is open")),
            "{result:?}"
        );
        assert_eq!(
            (live_digest(&world.state), std::fs::read(&safety).ok()),
            before
        );
    });
}

/// F5 (#720): restoring the safety copy undoes the restore. It used to copy the
/// project over the safety copy first, which lost the original and reported
/// success.
#[test]
fn f5_restoring_the_safety_copy_brings_the_original_back() {
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().expect("app state");
        let dir = TempDir::new().expect("temp dir");
        let project = dir.path().join("cave.stalx");
        let backup = dir.path().join("cave.stalx.backup");
        write_project(&project, &synthetic_project(&mut Rng::new(3), "cave"));
        write_project(&backup, &synthetic_project(&mut Rng::new(4), "older"));
        let original = std::fs::read(&project).expect("saved project");
        let older = std::fs::read(&backup).expect("saved backup");
        recovery::restore_from_backup_impl(&state, project.clone(), backup)
            .expect("restore the backup");
        let safety = safety_copy_of(&project);

        recovery::restore_from_backup_impl(&state, project.clone(), safety.clone())
            .expect("undo the restore");

        assert_eq!(std::fs::read(&project).ok(), Some(original));
        assert_eq!(std::fs::read(&safety).ok(), Some(older));
    });
}

/// F2 (#720): the project picked as its own backup is refused. It used to come
/// out as 0 bytes.
#[test]
fn f2_restoring_a_project_from_itself_leaves_it_intact() {
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().expect("app state");
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("cave.stalx");
        write_project(&path, &synthetic_project(&mut Rng::new(1), "cave"));
        let before = std::fs::read(&path).expect("saved project");
        let result = recovery::restore_from_backup_impl(&state, path.clone(), path.clone());
        assert_eq!(
            result,
            Err("Not restored: the backup you picked is cave.stalx itself.".to_string())
        );
        assert_eq!(std::fs::read(&path).ok(), Some(before));
        assert!(
            !safety_copy_of(&path).exists(),
            "refused before the safety copy"
        );
    });
}

/// F3 (#720): a project whose name contains `.backup` is not offered as its
/// own backup (which is how F2 was reachable from the recovery dialog).
#[test]
fn f3_a_project_is_never_offered_as_its_own_backup() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("cave.backup.stalx");
    write_project(&path, &synthetic_project(&mut Rng::new(2), "cave"));
    let offered: Vec<String> = recovery::list_backups(path.clone())
        .into_iter()
        .map(|b| b.path)
        .filter(|b| same_file(Path::new(b), &path))
        .collect();
    assert_eq!(offered, Vec::<String>::new());
}
