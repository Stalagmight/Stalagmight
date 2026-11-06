//! [#576 FZ-PERSIST A-fs] A randomized fuzzer for the file-system write policy
//! (`commands::write_path`).
//!
//! Each seed builds a random tree in a temp dir: folders, files, project files
//! and symlinks (absolute and relative, chains, loops, dangling, `..` in their
//! targets, case-variant names, links into and out of the protected folders and
//! at the names a write keeps beside its file), with a simulated app folder
//! (config, data and cache as `AppDirs::internal`, the cache's `speleodb` as the
//! managed-project cache) and a simulated system folder
//! (`write_path::test_system_dirs`). Random paths through that tree go to every
//! caller of the policy, and each caller really attempts its write when the
//! policy lets it. Two oracles:
//!
//! - the verdict: an independent resolver (`disk::kernel_landing`: lstat and
//!   readlink name by name, the way the Linux kernel walks a path, folders that
//!   do not exist yet made as `create_dir_all` makes them) says where the write
//!   lands. The policy must refuse a name or a link target of the wrong type and
//!   a write that lands in a protected folder, allow one that lands nowhere
//!   protected, and name the real reason;
//! - the effect: every entry of the temp tree (kind, size, content hash, link
//!   target) is recorded before and after each attempt. An allowed write changes
//!   nothing in a protected folder (a project write into the managed cache
//!   excepted, by design); a refused one changes nothing at all.
//!
//! Windows and macOS rules run through the pure path model
//! (`write_path_fuzz_model_tests`), on every host.
//!
//! CI runs `write_path_fuzz_gate` (fixed seeds, seconds). The long run is the
//! ignored `write_path_fuzz_sweep`: `STALAGMIGHT_RUST_PERSIST_SEEDS` seeds from
//! `STALAGMIGHT_RUST_PERSIST_SEED_START` (default 1), on
//! `STALAGMIGHT_RUST_PERSIST_THREADS` threads (default 1).
//!
//! All data is synthetic, built in code; every file lives in a `TempDir`.

#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              lines are the evidence a run captures"
)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use super::write_path_fuzz_model_tests::model_seed;
use crate::commands::write_path::WriteTarget;
use crate::export::{ImageFormat, ProfileFormat};

/// Probes (random paths, each put to one caller) per seed of the disk fuzzer.
pub(super) const PROBES_PER_SEED: usize = 12;

/// The seeds the CI gate runs: a fixed spread, then every seed a sweep found
/// failing (regression seeds, with the finding they caught).
const GATE_SEEDS: &[u64] = &[
    2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
    // FZ-PERSIST-fs-1: a project path spelled inside the managed cache, through a
    // link there into the settings (1, Windows rules) or data (4, Linux rules)
    // folder, was allowed.
    1, 4,
    // FZ-PERSIST-fs-2: Save As over a link that sits in a protected folder -
    // `<settings>/CAVE.stalx` into the managed cache (55), one in the system
    // folder reached through a link (784) - replaced the link there.
    55, 784,
    // FZ-PERSIST-fs-4: a create at `<cache>/nope/../speleodb/...` made
    // `<cache>/nope`, a folder in the app's own cache.
    1801, 1808,
];

/// A deterministic PRNG (SplitMix64): every decision of a seed comes from here,
/// so a seed replays exactly.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) const fn new(seed: u64) -> Self {
        Self(seed ^ 0x5eed_f00d_cafe_d00d)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..bound`; 0 when `bound` is 0.
    pub(super) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }

    /// True with probability `percent`/100.
    pub(super) fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }

    /// One of `items` (which must not be empty).
    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// What a seed exercised, by name: printed by the gate and the sweep so a run
/// shows what it covered.
#[derive(Default)]
pub(super) struct Stats(BTreeMap<String, u64>);

impl Stats {
    pub(super) fn bump(&mut self, what: &str) {
        *self.0.entry(what.to_owned()).or_default() += 1;
    }

    fn add(&mut self, other: &Self) {
        for (what, n) in &other.0 {
            *self.0.entry(what.clone()).or_default() += n;
        }
    }

    /// The counts of every name that starts with `prefix`, summed.
    #[cfg(target_os = "linux")]
    fn count(&self, prefix: &str) -> u64 {
        self.0
            .iter()
            .filter(|(what, _)| what.starts_with(prefix))
            .map(|(_, n)| n)
            .sum()
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown: Vec<String> = self.0.iter().map(|(k, v)| format!("{k}={v}")).collect();
        f.write_str(&shown.join(" "))
    }
}

/// One seed of the whole fuzzer - the disk fuzzer (Linux), then the path model
/// (every host). `Err` is a one-line reason; a panic is caught and reported as
/// one.
fn one_seed(seed: u64) -> Result<Stats, String> {
    let run = || -> Result<Stats, String> {
        let mut stats = Stats::default();
        #[cfg(target_os = "linux")]
        stats.add(&disk::disk_seed(seed, PROBES_PER_SEED)?);
        stats.add(&model_seed(seed)?);
        Ok(stats)
    };
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            Err(format!("panicked: {}", message.replace('\n', " | ")))
        }
    }
}

#[test]
fn write_path_fuzz_gate() {
    let started = Instant::now();
    let mut total = Stats::default();
    let mut failed = Vec::new();
    for &seed in GATE_SEEDS {
        match one_seed(seed) {
            Ok(stats) => total.add(&stats),
            Err(why) => failed.push(format!("seed={seed} {why}")),
        }
    }
    println!(
        "FZ-PERSIST GATE fs seeds={} failed={} secs={:.1} {total}",
        GATE_SEEDS.len(),
        failed.len(),
        started.elapsed().as_secs_f64()
    );
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Seeds that reproduce each finding reported and still open (its fix lies
/// outside this module), with its id: see `disk::known_open`.
#[cfg(target_os = "linux")]
const KNOWN_OPEN_SEEDS: &[(u64, &str)] = &[
    // FZ-PERSIST-fs-3: Save As into the managed cache, `notes.stalx.tmp` a
    // dangling link into the system folder: the save creates its temp file
    // through it.
    (437, "fs-3"),
];

/// The open findings still reproduce. When one no longer does - it was fixed -
/// remove its matcher from `disk::known_open` and move its seeds into
/// `GATE_SEEDS`.
#[cfg(target_os = "linux")]
#[test]
fn write_path_fuzz_known_open() {
    let mut gone = Vec::new();
    for &(seed, id) in KNOWN_OPEN_SEEDS {
        match one_seed(seed) {
            Ok(stats) => {
                let seen = stats.count(&format!("disk.known-open.{id}"));
                println!("FZ-PERSIST KNOWN-OPEN fs seed={seed} {id} seen={seen}");
                if seen == 0 {
                    gone.push(format!("seed={seed} no longer reproduces {id}"));
                }
            }
            Err(why) => gone.push(format!("seed={seed} {why}")),
        }
    }
    assert!(gone.is_empty(), "{gone:#?}");
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "long randomized run: STALAGMIGHT_RUST_PERSIST_SEEDS seeds from \
            STALAGMIGHT_RUST_PERSIST_SEED_START on STALAGMIGHT_RUST_PERSIST_THREADS threads"]
fn write_path_fuzz_sweep() {
    let count = env_u64("STALAGMIGHT_RUST_PERSIST_SEEDS", 200).max(1);
    let first = env_u64("STALAGMIGHT_RUST_PERSIST_SEED_START", 1);
    let threads = env_u64("STALAGMIGHT_RUST_PERSIST_THREADS", 1).clamp(1, 16);
    let last = first + count - 1;
    let next = AtomicU64::new(first);
    let total = Mutex::new(Stats::default());
    let failed = AtomicU64::new(0);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let seed = next.fetch_add(1, Ordering::Relaxed);
                if seed > last {
                    break;
                }
                match one_seed(seed) {
                    Ok(stats) => total.lock().expect("stats lock").add(&stats),
                    Err(why) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                        println!("FZ-PERSIST FAIL fs seed={seed} {why}");
                    }
                }
            });
        }
    });
    let secs = started.elapsed().as_secs_f64();
    let failed = failed.load(Ordering::Relaxed);
    println!("FZ-PERSIST STATS fs {}", total.lock().expect("stats lock"));
    println!(
        "FZ-PERSIST SUMMARY fs seeds={first}..{last} failed={failed} secs={secs:.0} \
         seeds_per_sec={:.2}",
        count as f64 / secs.max(0.001)
    );
    assert_eq!(
        failed, 0,
        "{failed} failing seeds; see the FZ-PERSIST FAIL lines"
    );
}

/// A file type a command writes, with the extensions the policy must accept for
/// it - written out here, not read from the policy, so the oracle stays
/// independent of what it checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Project,
    Csv,
    Json,
    Ariane,
    Png,
    Svg,
    Pdf,
    Xml,
    Txt,
}

impl Kind {
    pub(super) const ALL: [Self; 9] = [
        Self::Project,
        Self::Csv,
        Self::Json,
        Self::Ariane,
        Self::Png,
        Self::Svg,
        Self::Pdf,
        Self::Xml,
        Self::Txt,
    ];

    pub(super) fn target(self) -> WriteTarget {
        match self {
            Self::Project => WriteTarget::Project,
            Self::Csv => WriteTarget::Csv,
            Self::Json => WriteTarget::Json,
            Self::Ariane => WriteTarget::Ariane,
            Self::Png => WriteTarget::Image(ImageFormat::Png),
            Self::Svg => WriteTarget::Image(ImageFormat::Svg),
            Self::Pdf => WriteTarget::Image(ImageFormat::Pdf),
            Self::Xml => WriteTarget::DiveProfile(ProfileFormat::SubsurfaceXml),
            Self::Txt => WriteTarget::DiveProfile(ProfileFormat::PlainText),
        }
    }

    pub(super) fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Project => &["stalx"],
            Self::Csv => &["csv"],
            Self::Json => &["json"],
            Self::Ariane => &["tml", "tmlu"],
            Self::Png => &["png"],
            Self::Svg => &["svg"],
            Self::Pdf => &["pdf"],
            Self::Xml => &["xml"],
            Self::Txt => &["txt"],
        }
    }

    /// The type a file of this name is, by its extension.
    pub(super) fn of(path: &Path) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| has_extension(path, *kind))
    }
}

pub(super) fn has_extension(path: &Path, kind: Kind) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        kind.extensions()
            .iter()
            .any(|x| ext.eq_ignore_ascii_case(x))
    })
}

/// Why the policy refuses, read from its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Why {
    Extension,
    LinkType,
    AppFolder,
    SystemFolder,
    /// Past the 63 links Windows follows (Windows rules only).
    TooManyLinks,
}

pub(super) fn reason_of(message: &str) -> Option<Why> {
    if message.contains(": the file must end in ") {
        Some(Why::Extension)
    } else if message.contains(": it is a link to ") {
        Some(Why::LinkType)
    } else if message.contains("that is inside the app's own settings, data or cache folder") {
        Some(Why::AppFolder)
    } else if message.contains("that is inside a system folder") {
        Some(Why::SystemFolder)
    } else if message.contains("that is reached through more links than Windows follows") {
        Some(Why::TooManyLinks)
    } else {
        None
    }
}

/// What the policy must answer for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Expect {
    Allow,
    /// Refused, for one of these reasons.
    Refuse(Vec<Why>),
    /// The path as given lies in a protected folder, the write lands outside
    /// every one: refusing (for one of these reasons) is the documented
    /// conservative answer (`path_forms`), allowing is harmless.
    Either(Vec<Why>),
    /// No write gets through: any answer.
    Any,
}

/// How a message names the file: the policy's own refusal, or a load's
/// upgrade refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Said {
    Policy,
    Load,
}

/// The name a policy message calls `path` by.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// [Oracle] The policy's answer against the expected one. `Err` names the
/// finding: a policy hole, an over-refusal, a wrong reason.
pub(super) fn judge(
    path: &Path,
    expect: &Expect,
    got: Result<(), &str>,
    said: Said,
) -> Result<(), String> {
    if let Err(message) = got {
        let prefix = match said {
            Said::Policy => format!("Refused to change {}: ", name_of(path)),
            Said::Load => format!(
                "{} was saved by an older version of Stalagmight",
                name_of(path)
            ),
        };
        if !message.starts_with(&prefix) {
            return Err(format!("a refusal not naming the file: {message:?}"));
        }
    }
    match (expect, got) {
        (Expect::Any | Expect::Allow | Expect::Either(_), Ok(())) | (Expect::Any, Err(_)) => Ok(()),
        (Expect::Allow, Err(message)) => Err(format!(
            "over-refusal: {message:?} for a write that lands outside every protected folder \
             with the right extension"
        )),
        (Expect::Refuse(facts), Ok(())) => Err(format!(
            "policy hole: allowed, but the write is refused for {facts:?}"
        )),
        (Expect::Refuse(facts) | Expect::Either(facts), Err(message)) => match reason_of(message) {
            Some(why) if facts.contains(&why) => Ok(()),
            why => Err(format!(
                "wrong reason: {why:?} ({message:?}), the real one is one of {facts:?}"
            )),
        },
    }
}

/// The disk fuzzer: a real tree, real writes. Linux only: the oracle walks the
/// path as the Linux kernel does (40 links, case-sensitive names).
#[cfg(target_os = "linux")]
mod disk {
    use std::collections::{BTreeMap, VecDeque};
    use std::ffi::{OsStr, OsString};
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::os::unix::fs::symlink;
    use std::path::{Component, Path, PathBuf};
    use std::sync::OnceLock;

    use tempfile::TempDir;

    use super::{has_extension, judge, reason_of, Expect, Kind, Rng, Said, Stats, Why};
    use crate::commands::project::{create_project_full_checked, save_project_impl};
    use crate::commands::recovery::write_salvaged_data_with;
    use crate::commands::save_target::save_target_exists_with;
    use crate::commands::write_path::{
        check_write_path_with, load_upgrade_refusal_with, test_system_dirs, AppDirs,
    };
    use crate::core::loading::SpeleoManagedFile;
    use crate::core::{AppState, Command, CommandErrorCode};
    use crate::models::Project;
    use crate::persistence::{with_stalx_extension, Database};

    /// One step of a path, owned (a link's target is spliced into the walk).
    enum Part {
        Root,
        Up,
        Name(OsString),
    }

    fn parts(path: &Path) -> Vec<Part> {
        path.components()
            .filter_map(|c| match c {
                Component::RootDir => Some(Part::Root),
                Component::ParentDir => Some(Part::Up),
                Component::Normal(name) => Some(Part::Name(name.to_os_string())),
                Component::CurDir | Component::Prefix(_) => None,
            })
            .collect()
    }

    /// What is at the place a walk ends.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Found {
        Missing,
        File,
        Folder,
    }

    /// Where a write to a path lands.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Landing {
        At(PathBuf, Found),
        /// No write gets through (`ELOOP`, `ENOTDIR`, ...).
        NoWrite(&'static str),
        /// The walk leaves the temp tree: never attempted.
        Escapes,
    }

    /// The links one Linux path lookup follows (`MAXSYMLINKS`).
    const KERNEL_MAX_LINKS: usize = 40;

    /// [Oracle] Where a write to `path` (absolute) lands, walked the way the Linux
    /// kernel walks it, independently of the policy: `lstat` each name; a link is
    /// replaced by its target (`readlink`), taken from the link's folder when
    /// relative; `..` steps back from the folder reached; a name that is not
    /// there is a folder a write that makes missing folders (`create_dir_all`)
    /// would make, and everything below it is new too; the last name is opened
    /// with `O_CREAT`, which follows a link even to a file that does not exist
    /// yet. 40 links in all, then `ELOOP`. Every place the walk reaches must lie
    /// in `jail` (or be one of the folders above it on the way in).
    fn kernel_landing(path: &Path, jail: &Path) -> Landing {
        let bytes = path.as_os_str().as_encoded_bytes();
        let trailing = bytes.ends_with(b"/") || bytes.ends_with(b"/.");
        let mut todo: VecDeque<Part> = parts(path).into();
        let mut at = PathBuf::from("/");
        let mut missing = false;
        let mut found = Found::Folder;
        let mut links = 0;
        while let Some(part) = todo.pop_front() {
            match part {
                Part::Root => {
                    at = PathBuf::from("/");
                    missing = false;
                    found = Found::Folder;
                }
                Part::Up => {
                    at.pop();
                    missing = std::fs::symlink_metadata(&at).is_err();
                    found = if missing {
                        Found::Missing
                    } else {
                        Found::Folder
                    };
                }
                Part::Name(name) => {
                    let next = at.join(&name);
                    if missing {
                        at = next;
                    } else {
                        match std::fs::symlink_metadata(&next) {
                            Err(_) => {
                                at = next;
                                missing = true;
                                found = Found::Missing;
                            }
                            Ok(meta) if meta.file_type().is_symlink() => {
                                links += 1;
                                if links > KERNEL_MAX_LINKS {
                                    return Landing::NoWrite("more than 40 links (ELOOP)");
                                }
                                let Ok(target) = std::fs::read_link(&next) else {
                                    return Landing::NoWrite("an unreadable link");
                                };
                                for part in parts(&target).into_iter().rev() {
                                    todo.push_front(part);
                                }
                                continue;
                            }
                            Ok(meta) if meta.is_dir() => {
                                at = next;
                                found = Found::Folder;
                            }
                            Ok(_) => {
                                if !todo.is_empty() {
                                    return Landing::NoWrite("a file used as a folder (ENOTDIR)");
                                }
                                at = next;
                                found = Found::File;
                            }
                        }
                    }
                }
            }
            let above = jail.starts_with(&at) && !missing;
            if !at.starts_with(jail) && !above {
                return Landing::Escapes;
            }
        }
        if !at.starts_with(jail) || at == jail {
            return Landing::Escapes;
        }
        if trailing && found != Found::Folder {
            return Landing::NoWrite("a separator after a file name");
        }
        Landing::At(at, found)
    }

    /// `path` with `.` and `..` folded by name, without touching the disk.
    fn folded(path: &Path) -> PathBuf {
        let mut out = PathBuf::from("/");
        for part in parts(path) {
            match part {
                Part::Root => out = PathBuf::from("/"),
                Part::Up => {
                    out.pop();
                }
                Part::Name(name) => out.push(name),
            }
        }
        out
    }

    fn absolute(path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
        }
    }

    /// A protected folder as the oracle compares against it: the path as given
    /// (folded) and the folder it is on disk.
    fn forms(dir: &Path) -> Vec<PathBuf> {
        let mut forms = vec![folded(&absolute(dir))];
        if let Ok(real) = std::fs::canonicalize(dir) {
            if !forms.contains(&real) {
                forms.push(real);
            }
        }
        forms
    }

    fn within(path: &Path, forms: &[PathBuf]) -> bool {
        forms.iter().any(|dir| path.starts_with(dir))
    }

    /// The protected folders of one tree, for both oracles.
    struct Guarded {
        app: Vec<Vec<PathBuf>>,
        managed: Vec<PathBuf>,
        system: Vec<Vec<PathBuf>>,
        /// On disk, for the effect oracle.
        app_real: Vec<PathBuf>,
        managed_real: PathBuf,
        system_real: Vec<PathBuf>,
    }

    /// [Oracle] The answer for `path` written as `kind` (`None`: the location
    /// rules alone, as for a load's in-place upgrade). A project may land in the
    /// managed cache - judged by where it LANDS.
    fn expect(path: &Path, kind: Option<Kind>, guarded: &Guarded, jail: &Path) -> Expect {
        if let Some(kind) = kind {
            if !has_extension(path, kind) {
                return Expect::Refuse(vec![Why::Extension]);
            }
        }
        let project = kind.is_none_or(|k| k == Kind::Project);
        let abs = absolute(path);
        let Landing::At(land, _) = kernel_landing(&abs, jail) else {
            return Expect::Any;
        };
        let lex = folded(&abs);
        // Each place on its own: one in the app's folders is protected unless a
        // project is written into the managed cache there.
        let in_app = |p: &Path| {
            guarded.app.iter().any(|d| within(p, d)) && !(project && within(p, &guarded.managed))
        };
        let in_sys = |p: &Path| guarded.system.iter().any(|d| within(p, d));
        // Where a write really lands: the place the name leads to (opened, links
        // followed), the entry the name is (replaced by a rename over it), and
        // every folder a create makes on the way (`create_dir_all`).
        let entry = entry_of(&abs, jail);
        let made = made_folders(&abs, jail);
        let real: Vec<&Path> = std::iter::once(land.as_path())
            .chain(entry.as_deref())
            .chain(made.iter().map(PathBuf::as_path))
            .collect();
        let real_app = real.iter().any(|p| in_app(p));
        let real_sys = real.iter().any(|p| in_sys(p));
        let mut facts = Vec::new();
        let wrong_type = kind.is_some_and(|kind| !has_extension(&land, kind));
        if wrong_type {
            facts.push(Why::LinkType);
        }
        if real_app || in_app(&lex) {
            facts.push(Why::AppFolder);
        }
        if real_sys || in_sys(&lex) {
            facts.push(Why::SystemFolder);
        }
        if wrong_type || real_app || real_sys {
            Expect::Refuse(facts)
        } else if facts.is_empty() {
            Expect::Allow
        } else {
            Expect::Either(facts)
        }
    }

    /// One entry of the tree as the effect oracle records it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Shot {
        Folder,
        /// Size and content hash.
        File(u64, u64),
        Link(PathBuf),
        Other,
    }

    /// [Oracle] Every entry under `root`, links not followed.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Shot> {
        let mut out = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                let shot = if meta.file_type().is_symlink() {
                    Shot::Link(std::fs::read_link(&path).unwrap_or_default())
                } else if meta.is_dir() {
                    stack.push(path.clone());
                    Shot::Folder
                } else if meta.is_file() {
                    let mut hasher = DefaultHasher::new();
                    std::fs::read(&path).unwrap_or_default().hash(&mut hasher);
                    Shot::File(meta.len(), hasher.finish())
                } else {
                    Shot::Other
                };
                out.insert(path, shot);
            }
        }
        out
    }

    /// The entries made, removed or changed between two snapshots.
    fn changed(before: &BTreeMap<PathBuf, Shot>, after: &BTreeMap<PathBuf, Shot>) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = before
            .iter()
            .filter(|&(path, shot)| after.get(path) != Some(shot))
            .map(|(path, _)| path.clone())
            .collect();
        out.extend(
            after
                .keys()
                .filter(|path| !before.contains_key(*path))
                .cloned(),
        );
        out
    }

    /// The project files a tree is seeded with: one this build saved, and the same
    /// file stamped older (16) and unstamped (0) - `PRAGMA user_version` is the
    /// 4 bytes at offset 60 of the SQLite header - which a load upgrades or
    /// completes in place.
    struct Templates {
        current: Vec<u8>,
        older: Vec<u8>,
        unstamped: Vec<u8>,
    }

    fn templates() -> &'static Templates {
        static TEMPLATES: OnceLock<Templates> = OnceLock::new();
        TEMPLATES.get_or_init(|| {
            use crate::models::{SectionData, ShotData};
            let dir = TempDir::new().expect("template dir");
            let path = dir.path().join("CaveT.stalx");
            let mut project = Project::new("CaveT".to_owned(), None);
            project.add_section(SectionData {
                name: Some("SectionA".to_owned()),
                color: "#123456".to_owned(),
                shots: vec![ShotData {
                    length: 5.0,
                    ..Default::default()
                }],
                ..Default::default()
            });
            Database::open(&path)
                .expect("create the template")
                .save_project(&project)
                .expect("write the template");
            let names: Vec<OsString> = std::fs::read_dir(dir.path())
                .expect("template dir")
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(names.len(), 1, "a template is one file: {names:?}");
            let current = std::fs::read(&path).expect("read the template");
            let stamped = |version: u32| {
                let mut bytes = current.clone();
                bytes[60..64].copy_from_slice(&version.to_be_bytes());
                bytes
            };
            Templates {
                older: stamped(16),
                unstamped: stamped(0),
                current,
            }
        })
    }

    const FOLDER_NAMES: &[&str] = &[
        "a", "b", "sub", "Docs", "docs", "DOCS", "speleodb", "SpeleoDB", "config", "x.stalx",
        "deep", "Cave",
    ];
    const STEMS: &[&str] = &[
        "Cave", "cave", "CAVE", "Cave_1", "Cave_2", "x", "notes", "settings", "project",
    ];
    const EXTENSIONS: &[&str] = &[
        "stalx",
        "STALX",
        "Stalx",
        "json",
        "JSON",
        "csv",
        "tml",
        "tmlu",
        "png",
        "svg",
        "pdf",
        "xml",
        "txt",
        "toml",
        "db",
        "backup",
        "stalx.txt",
        "",
    ];
    /// What a write keeps beside its file, appended to the file's name: the save's
    /// temp file, SQLite's journals, the backups a load, a repair and a restore
    /// keep, a merge's archived source.
    const BESIDE: &[&str] = &[
        ".tmp",
        "-journal",
        "-wal",
        "-shm",
        ".backup",
        ".backup.1",
        ".pre-v16.backup",
        ".pre-restore.1",
        ".archived",
    ];

    /// A file name, of `kind` most of the time when one is given.
    fn file_name(rng: &mut Rng, kind: Option<Kind>) -> String {
        let stem = *rng.pick(STEMS);
        let ext = match kind {
            Some(kind) if rng.chance(65) => {
                let ext = *rng.pick(kind.extensions());
                if rng.chance(25) {
                    ext.to_uppercase()
                } else {
                    ext.to_owned()
                }
            }
            _ => (*rng.pick(EXTENSIONS)).to_owned(),
        };
        if ext.is_empty() {
            stem.to_owned()
        } else {
            format!("{stem}.{ext}")
        }
    }

    /// `name` with one letter's case flipped (a different name on Linux).
    fn case_variant(rng: &mut Rng, name: &OsStr) -> OsString {
        let text = name.to_string_lossy();
        let chars: Vec<char> = text.chars().collect();
        let i = rng.below(chars.len());
        let flipped: String = chars
            .iter()
            .enumerate()
            .map(|(j, c)| {
                if j != i {
                    *c
                } else if c.is_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect();
        OsString::from(flipped)
    }

    /// The names in a folder, sorted (a seed replays only with a fixed order).
    fn names_in(folder: &Path) -> Vec<OsString> {
        let mut names: Vec<OsString> = std::fs::read_dir(folder)
            .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// `to` relative to the folder `from` (both absolute, physical).
    fn relative(from: &Path, to: &Path) -> PathBuf {
        let from: Vec<Component<'_>> = from.components().collect();
        let to: Vec<Component<'_>> = to.components().collect();
        let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
        let mut out = PathBuf::new();
        for _ in common..from.len() {
            out.push("..");
        }
        for part in &to[common..] {
            out.push(part);
        }
        out
    }

    /// Where an entry of the tree is, for the effect oracle.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Place {
        Managed,
        App,
        System,
    }

    /// One random tree, and what is in it.
    struct Tree {
        jail: PathBuf,
        root: PathBuf,
        /// The root again through a link (as `/var` is `/private/var` on macOS).
        root_alias: Option<PathBuf>,
        folders: Vec<PathBuf>,
        files: Vec<PathBuf>,
        projects: Vec<PathBuf>,
        links: Vec<PathBuf>,
        dirs: AppDirs,
        system_given: PathBuf,
        guarded: Guarded,
    }

    impl Tree {
        fn build(rng: &mut Rng, jail: &Path) -> Self {
            let root = jail.join("r");
            let app = root.join("home").join("app");
            let (config, data, cache) = (app.join("config"), app.join("data"), app.join("cache"));
            let managed = cache.join("speleodb");
            let system = root.join("sys");
            let mut folders = vec![
                root.clone(),
                root.join("home"),
                app,
                config.clone(),
                data.clone(),
                cache.clone(),
                managed.clone(),
                system.clone(),
                root.join("user"),
                root.join("user2"),
            ];
            for folder in &folders {
                std::fs::create_dir_all(folder).expect("a folder of the tree");
            }
            let root_alias = rng.chance(30).then(|| {
                let alias = jail.join("rlink");
                symlink(&root, &alias).expect("the root's alias");
                alias
            });
            for _ in 0..4 + rng.below(8) {
                let folder = rng.pick(&folders).join(rng.pick(FOLDER_NAMES));
                if std::fs::symlink_metadata(&folder).is_err()
                    && std::fs::create_dir(&folder).is_ok()
                {
                    folders.push(folder);
                }
            }
            let mut files = Vec::new();
            for _ in 0..3 + rng.below(8) {
                let file = rng.pick(&folders).join(file_name(rng, None));
                if std::fs::symlink_metadata(&file).is_err() {
                    let bytes: Vec<u8> = (0..rng.below(48)).map(|_| rng.below(256) as u8).collect();
                    std::fs::write(&file, bytes).expect("a file of the tree");
                    files.push(file);
                }
            }
            let mut projects = Vec::new();
            let protected = [config.clone(), data.clone(), cache.clone(), system.clone()];
            for i in 0..3 + rng.below(4) {
                let folder = match i {
                    0 => managed.clone(),
                    1 => rng.pick(&protected).clone(),
                    2 => root.join("user"),
                    _ => rng.pick(&folders).clone(),
                };
                let ext = if rng.chance(15) { "STALX" } else { "stalx" };
                let file = folder.join(format!("{}.{ext}", rng.pick(STEMS)));
                if std::fs::symlink_metadata(&file).is_err() {
                    let t = templates();
                    let variants = [&t.current, &t.older, &t.unstamped];
                    std::fs::write(&file, rng.pick(&variants)).expect("a project of the tree");
                    projects.push(file);
                }
            }
            let mut tree = Self {
                jail: jail.to_path_buf(),
                root,
                root_alias,
                folders,
                files,
                projects,
                links: Vec::new(),
                dirs: AppDirs {
                    internal: Vec::new(),
                    managed_projects: None,
                },
                system_given: system.clone(),
                guarded: Guarded {
                    app: Vec::new(),
                    managed: Vec::new(),
                    system: Vec::new(),
                    app_real: vec![config.clone(), data.clone(), cache.clone()],
                    managed_real: managed,
                    system_real: vec![system.clone()],
                },
            };
            for _ in 0..8 + rng.below(16) {
                tree.add_link(rng);
            }
            let config_given = tree.given(rng, &config);
            let data_given = tree.given(rng, &data);
            let cache_given = tree.given(rng, &cache);
            tree.system_given = tree.given(rng, &system);
            tree.dirs = AppDirs {
                internal: vec![config_given, data_given, cache_given.clone()],
                managed_projects: Some(cache_given.join("speleodb")),
            };
            tree.guarded.app = tree.dirs.internal.iter().map(|d| forms(d)).collect();
            tree.guarded.managed = forms(&cache_given.join("speleodb"));
            tree.guarded.system = vec![forms(&tree.system_given)];
            tree
        }

        /// A protected folder as handed to the policy: as it is on disk, through
        /// the root's alias, through a link to it in the user's folder, or with a
        /// `..` in it.
        fn given(&mut self, rng: &mut Rng, real: &Path) -> PathBuf {
            match rng.below(6) {
                0 => match (&self.root_alias, real.strip_prefix(&self.root)) {
                    (Some(alias), Ok(rest)) => alias.join(rest),
                    _ => real.to_path_buf(),
                },
                1 => {
                    let name = real
                        .file_name()
                        .map_or_else(OsString::new, OsStr::to_os_string);
                    let mut alias = self.root.join("user").join("alias");
                    alias.as_mut_os_string().push(&name);
                    let target = if rng.chance(50) {
                        real.to_path_buf()
                    } else {
                        relative(&self.root.join("user"), real)
                    };
                    if symlink(&target, &alias).is_ok() {
                        self.links.push(alias.clone());
                        alias
                    } else {
                        real.to_path_buf()
                    }
                }
                2 => match (real.parent(), real.file_name()) {
                    (Some(parent), Some(name)) => parent
                        .join("..")
                        .join(parent.file_name().unwrap_or_default())
                        .join(name),
                    _ => real.to_path_buf(),
                },
                _ => real.to_path_buf(),
            }
        }

        /// A link somewhere in the tree: at a random name, a name a write keeps
        /// beside a file, or a project's name; to a folder, a file, a name that is
        /// not there (in a folder that may not be there either), another link
        /// (chains), itself or a link back (loops), or a case variant of a name;
        /// absolute or relative, sometimes with `name/..` on the way.
        fn add_link(&mut self, rng: &mut Rng) {
            let at = match rng.below(10) {
                0..=2 => {
                    let mut pool: Vec<PathBuf> = self.projects.clone();
                    pool.extend(self.files.iter().cloned());
                    let base = if pool.is_empty() || rng.chance(40) {
                        rng.pick(&self.folders)
                            .join(format!("{}.stalx", rng.pick(STEMS)))
                    } else {
                        rng.pick(&pool).clone()
                    };
                    let mut name = base.into_os_string();
                    name.push(*rng.pick(BESIDE));
                    PathBuf::from(name)
                }
                3..=4 => rng
                    .pick(&self.folders)
                    .join(format!("{}.stalx", rng.pick(STEMS))),
                5..=6 => rng.pick(&self.folders).join(rng.pick(FOLDER_NAMES)),
                _ => rng.pick(&self.folders).join(file_name(rng, None)),
            };
            if std::fs::symlink_metadata(&at).is_ok() {
                return;
            }
            let Some(folder) = at.parent().map(Path::to_path_buf) else {
                return;
            };
            let guarded_folders: Vec<PathBuf> = self
                .folders
                .iter()
                .filter(|f| self.place_of(f).is_some())
                .cloned()
                .collect();
            let to = match rng.below(20) {
                0..=4 => {
                    if rng.chance(50) && !guarded_folders.is_empty() {
                        rng.pick(&guarded_folders).clone()
                    } else {
                        rng.pick(&self.folders).clone()
                    }
                }
                5..=7 => {
                    let mut pool: Vec<PathBuf> = self.projects.clone();
                    pool.extend(self.files.iter().cloned());
                    if pool.is_empty() {
                        rng.pick(&self.folders).clone()
                    } else {
                        rng.pick(&pool).clone()
                    }
                }
                8..=11 => {
                    let folder = if rng.chance(60) && !guarded_folders.is_empty() {
                        rng.pick(&guarded_folders).clone()
                    } else {
                        rng.pick(&self.folders).clone()
                    };
                    let kind = rng.chance(60).then_some(Kind::Project);
                    folder.join(file_name(rng, kind))
                }
                12 => rng
                    .pick(&self.folders)
                    .join("not-yet")
                    .join(file_name(rng, Some(Kind::Project))),
                13..=15 if !self.links.is_empty() => rng.pick(&self.links).clone(),
                16 => at.clone(),
                17 => {
                    let back = folder.join(format!("loop{}.stalx", rng.below(100)));
                    if symlink(&at, &back).is_ok() {
                        self.links.push(back.clone());
                    }
                    back
                }
                _ => {
                    let base = rng.pick(&self.folders).clone();
                    let Ok(rest) = base.strip_prefix(&self.root) else {
                        return;
                    };
                    let mut varied = self.root.clone();
                    for part in rest.components() {
                        varied.push(if rng.chance(30) {
                            case_variant(rng, part.as_os_str())
                        } else {
                            part.as_os_str().to_os_string()
                        });
                    }
                    varied
                }
            };
            let mut target = if rng.chance(50) {
                let via_alias = match (&self.root_alias, to.strip_prefix(&self.root)) {
                    (Some(alias), Ok(rest)) if rng.chance(30) => Some(alias.join(rest)),
                    _ => None,
                };
                via_alias.unwrap_or(to)
            } else {
                relative(&folder, &to)
            };
            if rng.chance(20) {
                // `nope/..` (a name that is not there) before one of the last two
                // parts: inside the temp tree, so the walk stays in it.
                let parts: Vec<Component<'_>> = target.components().collect();
                let at_part = parts.len().saturating_sub(1 + rng.below(2));
                let mut noisy = PathBuf::new();
                for (i, part) in parts.iter().enumerate() {
                    if i == at_part {
                        noisy.push("nope");
                        noisy.push("..");
                    }
                    noisy.push(part);
                }
                target = noisy;
            }
            if symlink(&target, &at).is_ok() {
                self.links.push(at);
            }
        }

        /// Which protected folder an entry (a path on disk) is in.
        fn place_of(&self, path: &Path) -> Option<Place> {
            let g = &self.guarded;
            if path.starts_with(&g.managed_real) {
                Some(Place::Managed)
            } else if g.app_real.iter().any(|d| path.starts_with(d)) {
                Some(Place::App)
            } else if g.system_real.iter().any(|d| path.starts_with(d)) {
                Some(Place::System)
            } else {
                None
            }
        }

        /// A path for a report: the temp dir shown as `<t>`.
        fn show(&self, path: &Path) -> String {
            path.strip_prefix(&self.jail).map_or_else(
                |_| path.display().to_string(),
                |rest| format!("<t>/{}", rest.display()),
            )
        }

        /// A folder of the tree by a random route: as it is, through the root's
        /// alias, through a link that leads to it or above it, with `name/..` on
        /// the way (a name that is not there, a folder, a link), with a `.`, or
        /// relative to the working directory.
        fn route(&self, rng: &mut Rng, folder: &Path) -> PathBuf {
            let mut path = folder.to_path_buf();
            if rng.chance(45) {
                let via: Vec<PathBuf> = self
                    .links
                    .iter()
                    .filter_map(|link| {
                        let target = std::fs::canonicalize(link).ok()?;
                        let rest = folder.strip_prefix(&target).ok()?;
                        Some(link.join(rest))
                    })
                    .collect();
                if !via.is_empty() {
                    path = rng.pick(&via).clone();
                }
            } else if rng.chance(25) {
                if let (Some(alias), Ok(rest)) = (&self.root_alias, folder.strip_prefix(&self.root))
                {
                    path = alias.join(rest);
                }
            }
            if rng.chance(30) {
                let parts: Vec<Component<'_>> = path.components().collect();
                let depth = self.jail.components().count();
                let at_part = depth + rng.below(parts.len().saturating_sub(depth) + 1);
                let mut noisy = PathBuf::new();
                for (i, part) in parts.iter().enumerate() {
                    if i == at_part {
                        let names = names_in(&noisy);
                        let name = if names.is_empty() || rng.chance(30) {
                            OsString::from("nope")
                        } else {
                            rng.pick(&names).clone()
                        };
                        noisy.push(name);
                        noisy.push("..");
                    }
                    noisy.push(part);
                }
                path = noisy;
            }
            if rng.chance(8) {
                path.push(".");
            }
            if rng.chance(6) {
                if let Ok(cwd) = std::env::current_dir() {
                    let mut rel = PathBuf::new();
                    for _ in 1..cwd.components().count() {
                        rel.push("..");
                    }
                    if let Ok(rest) = path.strip_prefix("/") {
                        path = rel.join(rest);
                    }
                }
            }
            path
        }

        /// A walk from the root, a few names at a time: names in the folder
        /// reached (or case variants of them), `..`, `.`, a name not there.
        fn wander(&self, rng: &mut Rng) -> PathBuf {
            let mut path = match &self.root_alias {
                Some(alias) if rng.chance(25) => alias.clone(),
                _ => self.root.clone(),
            };
            for _ in 0..=rng.below(4) {
                let names = match kernel_landing(&path, &self.jail) {
                    Landing::At(folder, Found::Folder) => names_in(&folder),
                    _ => Vec::new(),
                };
                let step = match rng.below(12) {
                    0 => OsString::from(".."),
                    1 => OsString::from("."),
                    2 => OsString::from("nope"),
                    3 if !names.is_empty() => {
                        let name = rng.pick(&names).clone();
                        case_variant(rng, &name)
                    }
                    _ if !names.is_empty() => rng.pick(&names).clone(),
                    _ => OsString::from(*rng.pick(FOLDER_NAMES)),
                };
                path.push(step);
            }
            path
        }

        /// A folder of the tree, half the time a protected one.
        fn some_folder(&self, rng: &mut Rng) -> PathBuf {
            let guarded: Vec<&PathBuf> = self
                .folders
                .iter()
                .filter(|f| self.place_of(f).is_some())
                .collect();
            if rng.chance(50) && !guarded.is_empty() {
                (*rng.pick(&guarded)).clone()
            } else {
                rng.pick(&self.folders).clone()
            }
        }

        /// A path a write of `kind` is asked for: a folder by a route or a walk,
        /// and a file name (one of `kind` most of the time, or a name already
        /// there), sometimes ending in a separator, `.` or `..`.
        fn file_probe(&self, rng: &mut Rng, kind: Option<Kind>) -> PathBuf {
            if rng.chance(20) {
                return self.existing_probe(rng);
            }
            let folder = if rng.chance(70) {
                let folder = self.some_folder(rng);
                self.route(rng, &folder)
            } else {
                self.wander(rng)
            };
            let names = match kernel_landing(&absolute(&folder), &self.jail) {
                Landing::At(real, Found::Folder) => names_in(&real),
                _ => Vec::new(),
            };
            let name = if rng.chance(25) && !names.is_empty() {
                rng.pick(&names).clone()
            } else {
                OsString::from(file_name(rng, kind))
            };
            let mut path = folder.join(name);
            match rng.below(40) {
                0 => path.as_mut_os_string().push("/"),
                1 => path.push("."),
                2 => path.push(".."),
                _ => {}
            }
            path
        }

        /// A path to something already there: a project (or another file), or
        /// a link, by a route to its folder.
        fn existing_probe(&self, rng: &mut Rng) -> PathBuf {
            let pick = if rng.chance(65) && !self.projects.is_empty() {
                rng.pick(&self.projects).clone()
            } else if rng.chance(70) && !self.links.is_empty() {
                rng.pick(&self.links).clone()
            } else if !self.files.is_empty() {
                rng.pick(&self.files).clone()
            } else {
                return self.wander(rng);
            };
            match (pick.parent(), pick.file_name()) {
                (Some(folder), Some(name)) => self.route(rng, folder).join(name),
                _ => pick,
            }
        }

        /// A folder a create is asked to make its project in: one there, or a
        /// new one inside it.
        fn folder_probe(&self, rng: &mut Rng) -> PathBuf {
            let folder = if rng.chance(70) {
                let folder = self.some_folder(rng);
                self.route(rng, &folder)
            } else {
                self.wander(rng)
            };
            if rng.chance(30) {
                folder.join(rng.pick(FOLDER_NAMES))
            } else {
                folder
            }
        }
    }

    /// Every caller of the policy. Where the command reads the process-wide app
    /// folders (`AppDirs::current`, unset in a test), its testable core is driven
    /// with the tree's `AppDirs`, or the policy and the command's own write step
    /// are called in the command's order - see `attempt`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Caller {
        SaveAs,
        CreateInFolder,
        CreateAtFile,
        ExistenceProbe,
        Salvage,
        Load,
        Merge,
        MergeArchiving,
        Export(Kind),
        Delete,
        Rename,
        Repair,
        Restore,
        SpeleoRemoveLink,
    }

    impl Caller {
        fn pick(rng: &mut Rng) -> Self {
            match rng.below(22) {
                0..=2 => Self::SaveAs,
                3 => Self::CreateInFolder,
                4 => Self::CreateAtFile,
                5 | 6 => Self::ExistenceProbe,
                7 => Self::Salvage,
                8 | 9 => Self::Load,
                10 => Self::Merge,
                11 => Self::MergeArchiving,
                12..=15 => Self::Export(*rng.pick(&Kind::ALL[1..])),
                16 => Self::Delete,
                17 => Self::Rename,
                18 => Self::Repair,
                19 => Self::Restore,
                _ => Self::SpeleoRemoveLink,
            }
        }

        /// It writes a project (which may live in the managed cache).
        fn writes_project(self) -> bool {
            !matches!(self, Self::ExistenceProbe | Self::Salvage | Self::Export(_))
        }
    }

    /// The verdict oracle as an attempt sees it: the answer for a path as what
    /// (`None`: the location rules alone), on the tree as it is at that moment.
    type Oracle<'a> = dyn Fn(&Path, Option<Kind>) -> Expect + 'a;

    /// One question the policy was asked during an attempt, with the answer the
    /// oracle expected when it was asked (before anything was written).
    struct Asked {
        path: PathBuf,
        expected: Expect,
        answer: Result<(), String>,
        said: Said,
    }

    impl Asked {
        fn policy(path: &Path, kind: Kind, dirs: &AppDirs, oracle: &Oracle<'_>) -> Self {
            Self {
                path: path.to_path_buf(),
                expected: oracle(path, Some(kind)),
                answer: check_write_path_with(path, kind.target(), dirs),
                said: Said::Policy,
            }
        }
    }

    /// What one attempt did: what the policy was asked, and whether the caller
    /// refused before writing.
    struct Outcome {
        asked: Vec<Asked>,
        refused: bool,
    }

    /// An attempt planned before the tree is recorded: the caller, its path, and
    /// the paths it reads (merge sources, a restore's candidate, a salvage's
    /// damaged project).
    struct Plan {
        caller: Caller,
        path: PathBuf,
        others: Vec<PathBuf>,
    }

    fn plan(tree: &Tree, rng: &mut Rng) -> Plan {
        let caller = Caller::pick(rng);
        let (path, others) = match caller {
            Caller::SaveAs | Caller::CreateAtFile => {
                (tree.file_probe(rng, Some(Kind::Project)), Vec::new())
            }
            Caller::CreateInFolder => (tree.folder_probe(rng), Vec::new()),
            Caller::ExistenceProbe => {
                let kind = rng.chance(80).then(|| *rng.pick(&Kind::ALL));
                (tree.file_probe(rng, kind), Vec::new())
            }
            Caller::Salvage => (
                tree.file_probe(rng, Some(Kind::Json)),
                vec![tree.existing_probe(rng)],
            ),
            Caller::Load
            | Caller::Delete
            | Caller::Rename
            | Caller::Repair
            | Caller::SpeleoRemoveLink => {
                let path = if rng.chance(75) {
                    tree.existing_probe(rng)
                } else {
                    tree.file_probe(rng, Some(Kind::Project))
                };
                (path, Vec::new())
            }
            Caller::Merge | Caller::MergeArchiving => {
                let sources = (0..=rng.below(2))
                    .map(|_| tree.existing_probe(rng))
                    .collect();
                (tree.file_probe(rng, Some(Kind::Project)), sources)
            }
            Caller::Export(kind) => (tree.file_probe(rng, Some(kind)), Vec::new()),
            Caller::Restore => (tree.existing_probe(rng), vec![tree.existing_probe(rng)]),
        };
        Plan {
            caller,
            path,
            others,
        }
    }

    /// The open project the exports and a Save As need (a delete or a failed
    /// load may have closed it).
    fn ensure_open_project(state: &AppState) {
        if state.settled_snapshot().is_none() {
            let created = state.execute(Command::CreateProject {
                name: "CaveF".to_owned(),
                location: None,
            });
            assert!(created.success, "a project to export: {:?}", created.error);
        }
    }

    fn refused_by_policy(message: &str) -> bool {
        message.starts_with("Refused to change ")
    }

    /// Run the caller of `plan`: ask the policy as the command does and, where
    /// it allows, make the command's write. `Err` is a finding about the
    /// command itself (its own check answered otherwise, a create opened
    /// another file than it checked, ...).
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per caller of the policy, each a few lines; split up they say less"
    )]
    fn attempt(
        plan: &Plan,
        dirs: &AppDirs,
        state: &AppState,
        oracle: &Oracle<'_>,
    ) -> Result<Outcome, String> {
        let path = &plan.path;
        let project = |p: &Path| Asked::policy(p, Kind::Project, dirs, oracle);
        let gate = |asked: Vec<Asked>, write: &dyn Fn()| {
            let refused = asked.iter().any(|a| a.answer.is_err());
            if !refused {
                write();
            }
            Outcome { asked, refused }
        };
        Ok(match plan.caller {
            // `save_project_impl` itself, after the policy with the tree's app
            // folders (the command's own check sees the system folders alone).
            Caller::SaveAs => {
                let asked = project(&with_stalx_extension(path.clone()));
                let in_app = asked
                    .answer
                    .as_ref()
                    .err()
                    .map(String::as_str)
                    .and_then(reason_of)
                    == Some(Why::AppFolder);
                if in_app {
                    return Ok(Outcome {
                        asked: vec![asked],
                        refused: true,
                    });
                }
                ensure_open_project(state);
                let saved = save_project_impl(state, Some(path.clone()));
                let said = saved.error.as_deref().filter(|e| refused_by_policy(e));
                if said != asked.answer.as_ref().err().map(String::as_str) {
                    return Err(format!(
                        "Save As refused {said:?} where the policy answered {:?}",
                        asked.answer
                    ));
                }
                let refused = asked.answer.is_err();
                Outcome {
                    asked: vec![asked],
                    refused,
                }
            }
            // `create_project_full_checked` itself, the policy as its check.
            Caller::CreateInFolder | Caller::CreateAtFile => {
                let asked = std::cell::RefCell::new(Vec::new());
                let check = |p: &Path| {
                    let one = project(p);
                    let answer = one.answer.clone();
                    asked.borrow_mut().push(one);
                    answer
                };
                let (folder, file) = if plan.caller == Caller::CreateInFolder {
                    (Some(path.clone()), None)
                } else {
                    (None, Some(path.clone()))
                };
                let created = create_project_full_checked(
                    state,
                    &check,
                    "Cave".to_owned(),
                    None,
                    folder,
                    file,
                    false,
                );
                let asked = asked.into_inner();
                let last = asked.last().map(|a| (&a.path, a.answer.is_err()));
                let refused = match (&created, last) {
                    (Ok(c), Some((checked, false))) => {
                        if c.project.file_path.as_ref() != Some(checked) {
                            return Err(format!(
                                "the create opened {:?}, the policy judged {checked:?}",
                                c.project.file_path
                            ));
                        }
                        false
                    }
                    (Ok(c), _) => {
                        return Err(format!(
                            "the create opened {:?} with no allowed check",
                            c.project.file_path
                        ))
                    }
                    (Err(e), Some((_, true))) => {
                        if !refused_by_policy(&e.message) {
                            return Err(format!("a refused create said {:?}", e.message));
                        }
                        true
                    }
                    (Err(_), _) => false,
                };
                Outcome { asked, refused }
            }
            // `save_target_exists_with` itself; it never writes.
            Caller::ExistenceProbe => {
                let probed = save_target_exists_with(path, dirs);
                let Some(kind) = Kind::of(path) else {
                    let refused = probed.as_ref().is_err_and(|e| {
                        e.code == CommandErrorCode::WritePathRefused
                            && e.message.contains("writes no file of that type")
                    });
                    if !refused {
                        return Err(format!("probed a file of no type: {probed:?}"));
                    }
                    return Ok(Outcome {
                        asked: Vec::new(),
                        refused: true,
                    });
                };
                let answer = match &probed {
                    Err(e) if e.code == CommandErrorCode::WritePathRefused => {
                        Err(e.message.clone())
                    }
                    Ok(there) => {
                        if *there != std::fs::symlink_metadata(path).is_ok() {
                            return Err(format!("probed {there} for {path:?}"));
                        }
                        Ok(())
                    }
                    Err(_) => Ok(()),
                };
                let refused = answer.is_err();
                Outcome {
                    asked: vec![Asked {
                        path: path.clone(),
                        expected: oracle(path, Some(kind)),
                        answer,
                        said: Said::Policy,
                    }],
                    refused,
                }
            }
            // `write_salvaged_data_with` itself.
            Caller::Salvage => {
                let expected = oracle(path, Some(Kind::Json));
                let wrote = write_salvaged_data_with(&plan.others[0], path, dirs);
                let answer = match wrote {
                    Err(e) if refused_by_policy(&e.message) => Err(e.message),
                    _ => Ok(()),
                };
                let refused = answer.is_err();
                Outcome {
                    asked: vec![Asked {
                        path: path.clone(),
                        expected,
                        answer,
                        said: Said::Policy,
                    }],
                    refused,
                }
            }
            // `load_upgrade_refusal_with`, then the load the commands make with it.
            Caller::Load => {
                let expected = oracle(path, None);
                let refusal = load_upgrade_refusal_with(path, dirs);
                let _ = state.do_load_project_full_with(path.clone(), refusal.as_ref());
                if let Some(r) = &refusal {
                    if reason_of(&r.older_format) != reason_of(&r.unstamped) {
                        return Err(format!("the two load refusals differ: {r:?}"));
                    }
                }
                Outcome {
                    asked: vec![Asked {
                        path: path.clone(),
                        expected,
                        answer: refusal
                            .as_ref()
                            .map_or(Ok(()), |r| Err(r.older_format.clone())),
                        said: Said::Load,
                    }],
                    refused: refusal.is_some(),
                }
            }
            // The command's checks (the output, and the sources it moves), then
            // `Command::MergeProjects`.
            Caller::Merge | Caller::MergeArchiving => {
                let archiving = plan.caller == Caller::MergeArchiving;
                let mut asked = vec![project(&with_stalx_extension(path.clone()))];
                if archiving {
                    asked.extend(plan.others.iter().map(|s| project(s)));
                }
                gate(asked, &|| {
                    let _ = state.execute(Command::MergeProjects {
                        source_paths: plan.others.clone(),
                        output_path: path.clone(),
                        output_name: "Merged".to_owned(),
                        archive_originals: archiving,
                    });
                })
            }
            // The command's check, then its exporter (`export_csv`, `export_json`,
            // `export_tml`; the image and dive-profile exports write through
            // `write_atomic_bytes`).
            Caller::Export(kind) => gate(vec![Asked::policy(path, kind, dirs, oracle)], &|| {
                ensure_open_project(state);
                let Some(open) = state.settled_snapshot() else {
                    return;
                };
                let _ = match kind {
                    Kind::Csv => crate::export::export_csv(&open, path),
                    Kind::Json => crate::export::export_json(&open, path),
                    Kind::Ariane => crate::export::export_tml(&open, path).map(|_| ()),
                    _ => crate::export::write_atomic_bytes(path, b"synthetic export"),
                };
            }),
            Caller::Delete => gate(vec![project(path)], &|| {
                let _ = state.delete_project(path.clone());
            }),
            Caller::Rename => gate(vec![project(path)], &|| {
                let _ = state.set_project_name_for_path(path.clone(), "Renamed".to_owned());
            }),
            Caller::Repair => gate(vec![project(path)], &|| {
                let _ = Database::attempt_repair(path);
            }),
            Caller::Restore => gate(vec![project(path)], &|| {
                let _ = Database::restore_from_backup(path, &plan.others[0]);
            }),
            Caller::SpeleoRemoveLink => gate(vec![project(path)], &|| {
                let _ = state.speleodb_remove_link_at(
                    path.clone(),
                    SpeleoManagedFile {
                        path: path.display().to_string(),
                        project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
                        project_name: "CaveF".to_owned(),
                        endpoint: "https://example.invalid".to_owned(),
                        can_open_remote: false,
                    },
                );
            }),
        })
    }

    fn as_str(answer: &Result<(), String>) -> Result<(), &str> {
        match answer {
            Ok(()) => Ok(()),
            Err(message) => Err(message.as_str()),
        }
    }

    /// [Oracle] The entry a path names: its folder as the kernel reaches it, its
    /// own name not followed - where a write that replaces the name (a rename
    /// over it) lands.
    fn entry_of(path: &Path, jail: &Path) -> Option<PathBuf> {
        let abs = absolute(path);
        let (folder, name) = (abs.parent()?, abs.file_name()?);
        match kernel_landing(folder, jail) {
            Landing::At(at, Found::Folder | Found::Missing) => Some(at.join(name)),
            _ => None,
        }
    }

    /// [Oracle] The folders a write that makes missing folders (`create_dir_all`)
    /// makes for a name right before a `..` in `abs`: the walk reaches the name,
    /// makes it, and steps out of it.
    fn made_folders(abs: &Path, jail: &Path) -> Vec<PathBuf> {
        let parts: Vec<Component<'_>> = abs.components().collect();
        (1..parts.len())
            .filter(|&i| parts[i] == Component::ParentDir)
            .filter_map(
                |i| match kernel_landing(&parts[..i].iter().collect::<PathBuf>(), jail) {
                    Landing::At(at, Found::Missing) => Some(at),
                    _ => None,
                },
            )
            .collect()
    }

    /// The names a write keeps beside its file that this fuzzer links: the
    /// save's temp file (`with_extension("stalx.tmp")`), and the `BESIDE` names.
    fn beside_names(entry: &Path) -> Vec<(String, PathBuf)> {
        let mut names = vec![(".stalx.tmp".to_owned(), entry.with_extension("stalx.tmp"))];
        for suffix in BESIDE {
            let mut name = entry.as_os_str().to_os_string();
            name.push(suffix);
            names.push(((*suffix).to_owned(), PathBuf::from(name)));
        }
        names
    }

    /// [Known open] Which reported, still-open finding explains a change at
    /// `changed` in a protected folder, if one does: `fs-3.<name>`, a link at a
    /// name a write keeps beside the file it writes (the save's `.stalx.tmp`,
    /// ...), in that file's own folder, that leads to `changed` - the write
    /// follows it, and the policy judges only the file's own name.
    fn known_open(
        tree: &Tree,
        before: &BTreeMap<PathBuf, Shot>,
        plan: &Plan,
        changed: &Path,
    ) -> Option<String> {
        for written in walked(plan).iter().filter_map(|p| entry_of(p, &tree.jail)) {
            for (suffix, beside) in beside_names(&written) {
                if let Some(Shot::Link(target)) = before.get(&beside) {
                    let naive = beside.parent().map(|f| folded(&f.join(target)));
                    let lands = kernel_landing(&beside, &tree.jail);
                    let leads = naive.as_deref() == Some(changed)
                        || matches!(&lands, Landing::At(at, _) if at.as_path() == changed);
                    if leads {
                        return Some(format!("fs-3{suffix}"));
                    }
                }
            }
        }
        None
    }

    /// The links (as they were before an attempt) that lead to `entry`, for a
    /// report: `link -> target`.
    fn links_to(tree: &Tree, before: &BTreeMap<PathBuf, Shot>, entry: &Path) -> Vec<String> {
        before
            .iter()
            .filter_map(|(link, shot)| {
                let Shot::Link(target) = shot else {
                    return None;
                };
                let naive = folded(&link.parent()?.join(target));
                let lands = kernel_landing(link, &tree.jail);
                let leads = naive == entry
                    || matches!(&lands, Landing::At(land, _) if land.as_path() == entry);
                leads.then(|| format!("{} -> {}", tree.show(link), target.display()))
            })
            .collect()
    }

    /// Every path an attempt walks, for the escape guard.
    fn walked(plan: &Plan) -> Vec<PathBuf> {
        let mut walks = vec![plan.path.clone(), with_stalx_extension(plan.path.clone())];
        walks.push(plan.path.join("Cave.stalx"));
        walks.extend(plan.others.iter().cloned());
        walks
    }

    /// One seed of the disk fuzzer: a random tree, then `probes` attempts, each
    /// judged by both oracles. `Err` is the first finding, on one line.
    pub(super) fn disk_seed(seed: u64, probes: usize) -> Result<Stats, String> {
        let _config = crate::test_helpers::isolated_config();
        let mut rng = Rng::new(seed);
        let tmp = TempDir::new().map_err(|e| format!("temp dir: {e}"))?;
        let jail = std::fs::canonicalize(tmp.path()).map_err(|e| format!("temp dir: {e}"))?;
        let tree = Tree::build(&mut rng, &jail);
        let _system = test_system_dirs::add(vec![tree.system_given.clone()]);
        let state = stalagmight_core::test_fixtures::state_with_null_sink();
        ensure_open_project(&state);
        let mut stats = Stats::default();
        stats.bump("disk.seeds");
        for i in 0..probes {
            let plan = plan(&tree, &mut rng);
            let lands = match kernel_landing(&absolute(&plan.path), &jail) {
                Landing::At(land, found) => format!("{} ({found:?})", tree.show(&land)),
                other => format!("{other:?}"),
            };
            let fail = |why: String| {
                format!(
                    "probe {i} {:?} {} (others {:?}): {why}; landed (before) {lands}",
                    plan.caller,
                    tree.show(&plan.path),
                    plan.others.iter().map(|p| tree.show(p)).collect::<Vec<_>>(),
                )
            };
            if walked(&plan)
                .iter()
                .any(|p| kernel_landing(&absolute(p), &jail) == Landing::Escapes)
            {
                stats.bump("disk.probe-skipped-escapes");
                continue;
            }
            // [Verdict] The policy for every file type, and a load's refusal, on
            // this path - nothing written.
            for kind in Kind::ALL {
                let got = check_write_path_with(&plan.path, kind.target(), &tree.dirs);
                let expected = expect(&plan.path, Some(kind), &tree.guarded, &jail);
                judge(&plan.path, &expected, as_str(&got), Said::Policy)
                    .map_err(|why| fail(format!("as {kind:?}: {why}")))?;
                stats.bump("disk.verdicts");
            }
            let refusal = load_upgrade_refusal_with(&plan.path, &tree.dirs);
            let answer = refusal.map_or(Ok(()), |r| Err(r.older_format));
            let expected = expect(&plan.path, None, &tree.guarded, &jail);
            judge(&plan.path, &expected, as_str(&answer), Said::Load)
                .map_err(|why| fail(format!("load refusal: {why}")))?;
            // A load upgrades an older file in place: the project it opens is
            // made one first.
            if plan.caller == Caller::Load {
                if let Landing::At(real, Found::File) = kernel_landing(&absolute(&plan.path), &jail)
                {
                    if tree.projects.contains(&real) {
                        let t = templates();
                        let bytes = if rng.chance(50) {
                            &t.older
                        } else {
                            &t.unstamped
                        };
                        std::fs::write(&real, bytes).map_err(|e| fail(format!("re-stamp: {e}")))?;
                    }
                }
            }
            let oracle = |p: &Path, kind: Option<Kind>| expect(p, kind, &tree.guarded, &jail);
            let before = snapshot(&jail);
            let outcome = attempt(&plan, &tree.dirs, &state, &oracle).map_err(fail)?;
            let after = snapshot(&jail);
            for asked in &outcome.asked {
                judge(
                    &asked.path,
                    &asked.expected,
                    as_str(&asked.answer),
                    asked.said,
                )
                .map_err(|why| fail(format!("asked {}: {why}", tree.show(&asked.path))))?;
            }
            // [Effect] A refusal changed nothing; an allowed write changed nothing
            // in a protected folder (a project in the managed cache excepted).
            let changes = changed(&before, &after);
            let shown: Vec<String> = changes.iter().map(|p| tree.show(p)).collect();
            if outcome.refused && !changes.is_empty() {
                return Err(fail(format!("refused, yet changed {shown:?}")));
            }
            let protected: Vec<String> = changes
                .iter()
                .filter(|p| match tree.place_of(p) {
                    None => false,
                    Some(Place::Managed) => !plan.caller.writes_project(),
                    Some(Place::App | Place::System) => true,
                })
                .filter(|p| match known_open(&tree, &before, &plan, p) {
                    Some(id) => {
                        stats.bump(&format!("disk.known-open.{id}"));
                        false
                    }
                    None => true,
                })
                .map(|p| {
                    format!(
                        "{} ({:?} -> {:?}; links to it: {:?})",
                        tree.show(p),
                        before.get(p),
                        after.get(p),
                        links_to(&tree, &before, p)
                    )
                })
                .collect();
            if !protected.is_empty() {
                return Err(fail(format!(
                    "allowed, and changed {protected:?} in a protected folder"
                )));
            }
            let verdict = if outcome.refused {
                "refused"
            } else {
                "allowed"
            };
            stats.bump(&format!("disk.{:?}.{verdict}", plan.caller));
            if !outcome.refused && !changes.is_empty() {
                stats.bump("disk.attempts-that-wrote");
            }
            if changes
                .iter()
                .any(|p| tree.place_of(p) == Some(Place::Managed))
            {
                stats.bump("disk.managed-cache-writes");
            }
        }
        Ok(stats)
    }
}
