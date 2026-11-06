//! [#576 FZ-PERSIST A-fs] The write-policy fuzzer under each platform's rules,
//! through the pure path model, on every host.
//!
//! The policy's walk (`check_write_path_under` with `Walk { rules, disk }`) asks
//! a model of the disk (`LinkLookup`); a reference resolver written here,
//! independently of the policy, says where a write lands: Win32 semantics under
//! Windows rules (`.` and `..` folded by name first, a link's target taken from
//! the link's folder and folded again, 63 links in all, names matched without
//! case), POSIX under macOS and Linux rules (`..` from the folder reached, 40
//! links; macOS matches names without case). Random trees of folders, files and
//! links (chains up to and past each cap, loops, dangling, `..` in targets, case
//! variants) and random paths through them; the verdict oracle is the disk
//! fuzzer's (`write_path_fuzz_tests::judge`).
//!
//! Windows-only syntax - drive letters, `\\?\` and `\\?\UNC\`, `\\.\`, UNC
//! shares, mixed and repeated separators, letter case - cannot be walked on a
//! host whose `Path` does not parse it, so it is fuzzed through the pure
//! comparison (`path_is_within`): one place spelled several ways, against the
//! place the generator knows it is.
//!
//! All paths are synthetic; nothing touches the disk.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use super::write_path_fuzz_tests::{has_extension, judge, Expect, Kind, Rng, Said, Stats, Why};
use crate::commands::write_path::{
    check_write_path_under, path_is_within, test_system_dirs, AppDirs, LinkLookup, PathRules, Walk,
};

const LINUX: PathRules = PathRules {
    windows: false,
    fold_case: false,
};
const MACOS: PathRules = PathRules {
    windows: false,
    fold_case: true,
};
const WINDOWS: PathRules = PathRules {
    windows: true,
    fold_case: true,
};

/// Random paths per platform per seed (each judged as every file type).
const PROBES: usize = 30;
/// Spelling pairs per platform per seed for `path_is_within`.
const SPELLINGS: usize = 60;

/// An entry of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Folder,
    File,
    Link(PathBuf),
}

/// One step of a path, owned: the root (and a Windows drive prefix) as it is,
/// `..`, or a name.
enum Part {
    Head(OsString),
    Up,
    Name(OsString),
}

fn parts(path: &Path) -> Vec<Part> {
    path.components()
        .filter_map(|c| match c {
            Component::Prefix(_) | Component::RootDir => Some(Part::Head(c.as_os_str().into())),
            Component::ParentDir => Some(Part::Up),
            Component::Normal(name) => Some(Part::Name(name.to_os_string())),
            Component::CurDir => None,
        })
        .collect()
}

/// `path` with `.` and `..` folded by name.
fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in parts(path) {
        match part {
            Part::Head(head) => out.push(head),
            Part::Up => {
                out.pop();
            }
            Part::Name(name) => out.push(name),
        }
    }
    out
}

/// Where a write lands in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lands {
    /// The place, and whether something is there.
    At(PathBuf, bool),
    /// No write gets through (a file used as a folder, past 40 links on POSIX).
    NoWrite,
    /// Past the 63 links Windows follows.
    TooMany,
}

/// A disk: entries by name (letter case folded where the rules fold it), each
/// with its own spelling.
struct Model {
    rules: PathRules,
    nodes: BTreeMap<Vec<String>, (PathBuf, Node)>,
}

impl Model {
    fn key(&self, path: &Path) -> Vec<String> {
        path.components()
            .map(|c| {
                let text = c.as_os_str().to_string_lossy();
                if self.rules.fold_case {
                    text.to_lowercase()
                } else {
                    text.into_owned()
                }
            })
            .collect()
    }

    fn get(&self, path: &Path) -> Option<&(PathBuf, Node)> {
        self.nodes.get(&self.key(path))
    }

    /// Add an entry (its folder must be there); false when the name is taken.
    fn add(&mut self, path: &Path, node: Node) -> bool {
        let key = self.key(path);
        if self.nodes.contains_key(&key) {
            return false;
        }
        self.nodes.insert(key, (path.to_path_buf(), node));
        true
    }

    /// The entries in `folder` (its own spelling).
    fn children(&self, folder: &Path) -> Vec<PathBuf> {
        let key = self.key(folder);
        self.nodes
            .iter()
            .filter(|(k, _)| k.len() == key.len() + 1 && k.starts_with(&key))
            .map(|(_, (path, _))| path.clone())
            .collect()
    }

    /// [Reference] Where a write to `path` lands under these rules.
    fn walk(&self, path: &Path) -> Lands {
        if self.rules.windows {
            self.walk_win32(path)
        } else {
            self.walk_posix(path)
        }
    }

    /// Win32: the path is folded by name before anything is looked up; a link
    /// is replaced by its target - taken from the link's folder when relative -
    /// with the rest of the path after it, folded again, and the lookup starts
    /// over; 63 links in all.
    fn walk_win32(&self, path: &Path) -> Lands {
        let mut current = fold(path);
        let mut links = 0;
        'restart: loop {
            let steps = parts(&current);
            let mut at = PathBuf::new();
            let mut missing = false;
            for (i, part) in steps.iter().enumerate() {
                let name = match part {
                    Part::Head(head) => {
                        at.push(head);
                        continue;
                    }
                    Part::Up => {
                        at.pop();
                        continue;
                    }
                    Part::Name(name) => name,
                };
                let next = at.join(name);
                if missing {
                    at = next;
                    continue;
                }
                match self.get(&next) {
                    None => {
                        at = next;
                        missing = true;
                    }
                    Some((real, Node::Folder)) => at.clone_from(real),
                    Some((real, Node::File)) => {
                        if i + 1 < steps.len() {
                            return Lands::NoWrite;
                        }
                        at.clone_from(real);
                    }
                    Some((_, Node::Link(target))) => {
                        links += 1;
                        if links > 63 {
                            return Lands::TooMany;
                        }
                        let mut next_path = at.join(target);
                        for rest in &steps[i + 1..] {
                            if let Part::Name(rest) = rest {
                                next_path.push(rest);
                            }
                        }
                        current = fold(&next_path);
                        continue 'restart;
                    }
                }
            }
            return Lands::At(at, !missing);
        }
    }

    /// POSIX: name by name; a link's target is walked in its place (from the
    /// link's folder when relative); `..` steps back from the folder reached;
    /// 40 links in all.
    fn walk_posix(&self, path: &Path) -> Lands {
        let mut todo: VecDeque<Part> = parts(path).into();
        let mut at = PathBuf::new();
        let mut missing = false;
        let mut links = 0;
        while let Some(part) = todo.pop_front() {
            match part {
                Part::Head(head) => {
                    at.push(head);
                    missing = false;
                }
                Part::Up => {
                    at.pop();
                    missing = self.get(&at).is_none();
                }
                Part::Name(name) => {
                    let next = at.join(&name);
                    if missing {
                        at = next;
                        continue;
                    }
                    match self.get(&next) {
                        None => {
                            at = next;
                            missing = true;
                        }
                        Some((real, Node::Folder)) => at.clone_from(real),
                        Some((real, Node::File)) => {
                            if !todo.is_empty() {
                                return Lands::NoWrite;
                            }
                            at.clone_from(real);
                        }
                        Some((_, Node::Link(target))) => {
                            links += 1;
                            if links > 40 {
                                return Lands::NoWrite;
                            }
                            for part in parts(target).into_iter().rev() {
                                todo.push_front(part);
                            }
                        }
                    }
                }
            }
        }
        Lands::At(at, !missing)
    }
}

/// What the policy's walk asks: `canonicalize` (every link followed, the place
/// there) and `read_link`, under the model's rules.
impl LinkLookup for Model {
    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        if path.components().any(|c| c == Component::ParentDir) {
            return None;
        }
        match self.walk(path) {
            Lands::At(real, true) => Some(real),
            _ => None,
        }
    }

    fn link_target(&self, path: &Path) -> Option<PathBuf> {
        let folder = self.canonical(path.parent()?)?;
        match self.get(&folder.join(path.file_name()?)) {
            Some((_, Node::Link(target))) => Some(target.clone()),
            _ => None,
        }
    }
}

/// An absolute root on this host, with nothing touched: `C:\` on Windows.
fn model_root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" }).join("fzmodel")
}

const FOLDER_NAMES: &[&str] = &["Docs", "sub", "a", "Speleodb", "Config", "x.stalx", "deep"];
const STEMS: &[&str] = &["Cave", "cave", "x", "notes", "settings"];
const EXTENSIONS: &[&str] = &[
    "stalx", "STALX", "json", "csv", "tml", "tmlu", "png", "svg", "pdf", "xml", "txt", "toml", "",
];

fn file_name(rng: &mut Rng, kind: Option<Kind>) -> String {
    let stem = *rng.pick(STEMS);
    let ext = match kind {
        Some(kind) if rng.chance(60) => (*rng.pick(kind.extensions())).to_owned(),
        _ => (*rng.pick(EXTENSIONS)).to_owned(),
    };
    if ext.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{ext}")
    }
}

/// `text` with the case of some letters flipped.
fn recase(rng: &mut Rng, text: &str) -> String {
    text.chars()
        .map(|c| {
            if rng.chance(40) {
                if c.is_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            } else {
                c
            }
        })
        .collect()
}

/// `to` relative to the folder `from`.
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

/// One random model, its app folders and its system folder.
struct World {
    name: &'static str,
    model: Model,
    root: PathBuf,
    folders: Vec<PathBuf>,
    guarded_folders: Vec<PathBuf>,
    files: Vec<PathBuf>,
    links: Vec<PathBuf>,
    dirs: AppDirs,
    system_given: PathBuf,
    app: Vec<Vec<PathBuf>>,
    managed: Vec<PathBuf>,
    system: Vec<Vec<PathBuf>>,
}

impl World {
    fn build(rng: &mut Rng, name: &'static str, rules: PathRules) -> Self {
        let root = model_root();
        let mut model = Model {
            rules,
            nodes: BTreeMap::new(),
        };
        for ancestor in root.ancestors() {
            model.add(ancestor, Node::Folder);
        }
        let app = root.join("Users").join("me").join("AppData");
        let (config, data, cache) = (app.join("Config"), app.join("Data"), app.join("Cache"));
        let managed = cache.join("speleodb");
        let system = root.join("Windows");
        let user = root.join("Users").join("me").join("Documents");
        let mut folders: Vec<PathBuf> = Vec::new();
        for folder in [
            &config,
            &data,
            &managed,
            &system,
            &user,
            &root.join("Shared"),
        ] {
            for ancestor in folder.ancestors().filter(|a| a.starts_with(&root)) {
                model.add(ancestor, Node::Folder);
                if !folders.iter().any(|f| f == ancestor) {
                    folders.push(ancestor.to_path_buf());
                }
            }
        }
        let mut world = Self {
            name,
            model,
            root,
            folders,
            guarded_folders: Vec::new(),
            files: Vec::new(),
            links: Vec::new(),
            dirs: AppDirs {
                internal: Vec::new(),
                managed_projects: None,
            },
            system_given: system.clone(),
            app: Vec::new(),
            managed: Vec::new(),
            system: Vec::new(),
        };
        for _ in 0..3 + rng.below(8) {
            let folder = rng.pick(&world.folders).join(rng.pick(FOLDER_NAMES));
            if world.model.add(&folder, Node::Folder) {
                world.folders.push(folder);
            }
        }
        world.guarded_folders = world
            .folders
            .iter()
            .filter(|f| f.starts_with(&app) || f.starts_with(&system))
            .cloned()
            .collect();
        for _ in 0..3 + rng.below(8) {
            let file = rng.pick(&world.folders).join(file_name(rng, None));
            if world.model.add(&file, Node::File) {
                world.files.push(file);
            }
        }
        for _ in 0..8 + rng.below(16) {
            world.add_link(rng);
        }
        if rng.chance(35) {
            world.add_chain(rng);
        }
        let config_given = world.given(rng, &config);
        let data_given = world.given(rng, &data);
        let cache_given = world.given(rng, &cache);
        world.system_given = world.given(rng, &system);
        let managed_given = cache_given.join("speleodb");
        world.dirs = AppDirs {
            internal: vec![config_given, data_given, cache_given],
            managed_projects: Some(managed_given.clone()),
        };
        world.app = world.dirs.internal.iter().map(|d| world.forms(d)).collect();
        world.managed = world.forms(&managed_given);
        world.system = vec![world.forms(&world.system_given)];
        world
    }

    /// A protected folder as handed to the policy: as it is, spelled in
    /// another letter case (where names match without case), through a link
    /// to it, or with a `..` in it.
    fn given(&mut self, rng: &mut Rng, real: &Path) -> PathBuf {
        match rng.below(6) {
            0 if self.model.rules.fold_case => {
                let text = real.to_string_lossy().into_owned();
                PathBuf::from(recase(rng, &text))
            }
            1 => {
                let alias = self
                    .root
                    .join("Shared")
                    .join(format!("alias{}", rng.below(1000)));
                let target = if rng.chance(50) {
                    real.to_path_buf()
                } else {
                    relative(&self.root.join("Shared"), real)
                };
                if self.model.add(&alias, Node::Link(target)) {
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

    /// A link: to a folder, a file, a name not there (in a folder that may not
    /// be there either), another link, itself, a case variant; absolute or
    /// relative; sometimes with `name/..` in its target, where Windows and
    /// POSIX part ways when the name is a link.
    fn add_link(&mut self, rng: &mut Rng) {
        let folder = rng.pick(&self.folders).clone();
        let at = match rng.below(4) {
            0 => folder.join(format!("{}.stalx", rng.pick(STEMS))),
            1 => folder.join(rng.pick(FOLDER_NAMES)),
            _ => folder.join(file_name(rng, None)),
        };
        let pick_guarded = |rng: &mut Rng, w: &Self| {
            if rng.chance(55) && !w.guarded_folders.is_empty() {
                rng.pick(&w.guarded_folders).clone()
            } else {
                rng.pick(&w.folders).clone()
            }
        };
        let to = match rng.below(16) {
            0..=3 => pick_guarded(rng, self),
            4 | 5 if !self.files.is_empty() => rng.pick(&self.files).clone(),
            6..=9 => {
                let kind = rng.chance(60).then_some(Kind::Project);
                pick_guarded(rng, self).join(file_name(rng, kind))
            }
            10 => pick_guarded(rng, self)
                .join("not-yet")
                .join("planted.stalx"),
            11 | 12 if !self.links.is_empty() => rng.pick(&self.links).clone(),
            13 => at.clone(),
            _ => {
                let base = pick_guarded(rng, self);
                let text = base.to_string_lossy().into_owned();
                PathBuf::from(recase(rng, &text))
            }
        };
        let mut target = if rng.chance(50) {
            to
        } else {
            relative(&folder, &to)
        };
        if rng.chance(25) {
            let steps: Vec<Component<'_>> = target.components().collect();
            let at_step = steps.len().saturating_sub(1 + rng.below(2));
            let names = self.model.children(&folder);
            let detour = if names.is_empty() || rng.chance(30) {
                OsString::from("nope")
            } else {
                rng.pick(&names)
                    .file_name()
                    .unwrap_or_default()
                    .to_os_string()
            };
            let mut noisy = PathBuf::new();
            for (i, step) in steps.iter().enumerate() {
                if i == at_step {
                    noisy.push(&detour);
                    noisy.push("..");
                }
                noisy.push(step);
            }
            target = noisy;
        }
        if self.model.add(&at, Node::Link(target)) {
            self.links.push(at);
        }
    }

    /// A chain of links that lead nowhere yet, around each platform's cap.
    fn add_chain(&mut self, rng: &mut Rng) {
        let n = *rng.pick(&[39, 40, 41, 62, 63, 64, 65]);
        let user = self.root.join("Users").join("me").join("Documents");
        let end = if rng.chance(60) && !self.guarded_folders.is_empty() {
            rng.pick(&self.guarded_folders).join("planted.stalx")
        } else {
            user.join("mine.stalx")
        };
        let tag = rng.below(1000);
        let link = |i: usize| user.join(format!("c{tag}-{i}.stalx"));
        for i in 1..=n {
            let next = if i == n { end.clone() } else { link(i + 1) };
            if self.model.add(&link(i), Node::Link(next)) {
                self.links.push(link(i));
            }
        }
    }

    /// A folder as the oracle compares against it: as given (folded) and where
    /// it is.
    fn forms(&self, dir: &Path) -> Vec<PathBuf> {
        let mut forms = vec![fold(dir)];
        if let Lands::At(real, true) = self.model.walk(dir) {
            forms.push(real);
        }
        forms
    }

    /// `path` is `dir` or inside it, name by name (without case where the
    /// rules say so).
    fn within(&self, path: &Path, dirs: &[PathBuf]) -> bool {
        let path = self.model.key(path);
        dirs.iter()
            .any(|dir| path.starts_with(&self.model.key(dir)))
    }

    /// [Oracle] The answer for `path` written as `kind` (the disk fuzzer's
    /// rules, on the model).
    fn expect(&self, path: &Path, kind: Kind) -> Expect {
        if !has_extension(path, kind) {
            return Expect::Refuse(vec![Why::Extension]);
        }
        let land = match self.model.walk(path) {
            Lands::At(land, _) => land,
            Lands::NoWrite | Lands::TooMany => return Expect::Any,
        };
        let lex = fold(path);
        // Each place on its own: one in the app's folders is protected unless a
        // project is written into the managed cache there.
        let in_app = |p: &Path| {
            self.app.iter().any(|d| self.within(p, d))
                && !(kind == Kind::Project && self.within(p, &self.managed))
        };
        let in_sys = |p: &Path| self.system.iter().any(|d| self.within(p, d));
        // Where a write really lands: the place the name leads to, and the entry
        // the name is (its folder walked, the name not followed).
        let entry = match (path.parent(), path.file_name()) {
            (Some(folder), Some(name)) => match self.model.walk(folder) {
                Lands::At(at, _) => Some(at.join(name)),
                Lands::NoWrite | Lands::TooMany => None,
            },
            _ => None,
        };
        // Under POSIX rules a create also makes the name before each `..` where
        // the walk reaches it (`create_dir_all`); Windows folds the `..` first.
        let parts: Vec<Component<'_>> = path.components().collect();
        let made: Vec<PathBuf> = (1..parts.len())
            .filter(|&i| !self.model.rules.windows && parts[i] == Component::ParentDir)
            .filter_map(
                |i| match self.model.walk(&parts[..i].iter().collect::<PathBuf>()) {
                    Lands::At(at, false) => Some(at),
                    _ => None,
                },
            )
            .collect();
        let real: Vec<&Path> = std::iter::once(land.as_path())
            .chain(entry.as_deref())
            .chain(made.iter().map(PathBuf::as_path))
            .collect();
        let real_app = real.iter().any(|p| in_app(p));
        let real_sys = real.iter().any(|p| in_sys(p));
        let wrong_type = !has_extension(&land, kind);
        let mut facts = Vec::new();
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

    /// A random path: from the root (or the start of a chain), names of the
    /// folder reached (or case variants), `..`, `.`, a name not there, then a
    /// file name.
    fn probe(&self, rng: &mut Rng) -> PathBuf {
        if rng.chance(10) && !self.links.is_empty() {
            return rng.pick(&self.links).clone();
        }
        let mut path = if rng.chance(50) {
            self.root.clone()
        } else {
            rng.pick(&self.folders).clone()
        };
        for _ in 0..rng.below(5) {
            let names = match self.model.walk(&path) {
                Lands::At(folder, true) => self.model.children(&folder),
                _ => Vec::new(),
            };
            let step = match rng.below(12) {
                0 => "..".to_owned(),
                1 => ".".to_owned(),
                2 => "nope".to_owned(),
                3..=4 if !names.is_empty() => {
                    let name = rng
                        .pick(&names)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    recase(rng, &name)
                }
                _ if !names.is_empty() => rng
                    .pick(&names)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                _ => (*rng.pick(FOLDER_NAMES)).to_owned(),
            };
            path.push(step);
        }
        let kind = rng.chance(50).then_some(Kind::Project);
        path.join(file_name(rng, kind))
    }

    fn show(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).map_or_else(
            |_| path.display().to_string(),
            |rest| format!("<m>/{}", rest.display()),
        )
    }

    /// Where the reference says `path` lands, for a report.
    fn show_landing(&self, path: &Path) -> String {
        match self.model.walk(path) {
            Lands::At(land, there) => format!("{} (there: {there})", self.show(&land)),
            other => format!("{other:?}"),
        }
    }

    /// The links named on `path` as spelled (folded), with their targets, for a
    /// report.
    fn links_on(&self, path: &Path) -> Vec<String> {
        let mut at = PathBuf::new();
        let mut found = Vec::new();
        for part in fold(path).components() {
            at.push(part);
            if let Some((real, Node::Link(target))) = self.model.get(&at) {
                found.push(format!("{} -> {}", self.show(real), self.show(target)));
            }
        }
        found
    }
}

fn as_str(answer: &Result<(), String>) -> Result<(), &str> {
    match answer {
        Ok(()) => Ok(()),
        Err(message) => Err(message.as_str()),
    }
}

/// A place on a Windows machine, as the generator knows it.
#[derive(Debug, Clone)]
enum Root {
    Drive(char),
    Unc(String, String),
    /// The current drive's root (`\a`).
    Rooted,
}

#[derive(Debug, Clone)]
struct Place {
    root: Root,
    names: Vec<String>,
}

/// [Reference] `path` is `dir` or inside it: the same root and `dir`'s names
/// leading `path`'s, compared without case when `fold`.
fn place_within(path: &Place, dir: &Place, fold: bool) -> bool {
    let same = |a: &str, b: &str| {
        if fold {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let roots = match (&path.root, &dir.root) {
        (Root::Drive(a), Root::Drive(b)) => a.eq_ignore_ascii_case(b),
        (Root::Unc(s1, h1), Root::Unc(s2, h2)) => same(s1, s2) && same(h1, h2),
        (Root::Rooted, Root::Rooted) => true,
        _ => false,
    };
    roots
        && path.names.len() >= dir.names.len()
        && path.names.iter().zip(&dir.names).all(|(a, b)| same(a, b))
}

const PLACE_NAMES: &[&str] = &[
    "Windows", "System32", "AppData", "App", "Apps", "Users", "me", "x",
];

fn random_place(rng: &mut Rng, windows: bool) -> Place {
    let root = if windows {
        match rng.below(5) {
            0 | 1 => Root::Drive(*rng.pick(&['C', 'D'])),
            2 | 3 => Root::Unc((*rng.pick(&["srv", "nas"])).to_owned(), "share".to_owned()),
            _ => Root::Rooted,
        }
    } else {
        Root::Rooted
    };
    let names = (0..rng.below(4))
        .map(|_| (*rng.pick(PLACE_NAMES)).to_owned())
        .collect();
    Place { root, names }
}

/// `dir` and a place to compare with it: inside it, or one step away from it
/// (another drive or share, a name with `dir`'s last name as its prefix, a
/// shorter path).
fn place_pair(rng: &mut Rng, windows: bool) -> (Place, Place) {
    let dir = random_place(rng, windows);
    let mut path = dir.clone();
    match rng.below(6) {
        0 => path = random_place(rng, windows),
        1 => {
            if let Some(last) = path.names.last_mut() {
                last.push('s');
            }
        }
        2 => {
            path.names.pop();
        }
        _ => {
            for _ in 0..rng.below(3) {
                path.names.push((*rng.pick(PLACE_NAMES)).to_owned());
            }
        }
    }
    (path, dir)
}

/// `place` spelled as Win32 reads it: plain, `\\?\` verbatim (backslashes and
/// names only: nothing in it is normalized), or `\\.\`; `/` or `\` between
/// names, repeated separators, `.` steps and a trailing separator where Win32
/// folds them; letter case varied when `fold`.
fn spell_windows(rng: &mut Rng, place: &Place, fold: bool) -> String {
    let case = |rng: &mut Rng, text: &str| {
        if fold {
            recase(rng, text)
        } else {
            text.to_owned()
        }
    };
    let verbatim = rng.chance(30);
    let sep = |rng: &mut Rng| -> String {
        if verbatim {
            "\\".to_owned()
        } else {
            let one = if rng.chance(50) { "\\" } else { "/" };
            if rng.chance(15) {
                one.repeat(2)
            } else {
                one.to_owned()
            }
        }
    };
    let mut out = match &place.root {
        Root::Drive(letter) => {
            let drive = case(rng, &format!("{letter}:"));
            if verbatim {
                format!(r"\\?\{drive}")
            } else if rng.chance(20) {
                format!(r"\\.\{drive}")
            } else {
                drive
            }
        }
        Root::Unc(server, share) => {
            let (server, share) = (case(rng, server), case(rng, share));
            if verbatim {
                format!(r"\\?\UNC\{server}\{share}")
            } else {
                format!(r"\\{server}\{share}")
            }
        }
        Root::Rooted => String::new(),
    };
    if place.names.is_empty() {
        out.push('\\');
    }
    for name in &place.names {
        if out.is_empty() {
            // A rooted path's first separator stays single: `\\` starts a UNC path.
            out.push(if rng.chance(50) { '\\' } else { '/' });
        } else {
            out.push_str(&sep(rng));
        }
        if !verbatim && rng.chance(10) {
            out.push('.');
            out.push_str(&sep(rng));
        }
        out.push_str(&case(rng, name));
    }
    // A separator after a bare root would make `\\` (a UNC path) of it.
    if !verbatim && !place.names.is_empty() && rng.chance(20) {
        out.push_str(&sep(rng));
    }
    out
}

/// `place` (a `Rooted` one) spelled as POSIX reads it: `/` between names,
/// repeated, `.` steps, a trailing `/`; letter case varied when `fold`.
fn spell_posix(rng: &mut Rng, place: &Place, fold: bool) -> String {
    let mut out = String::new();
    if place.names.is_empty() {
        out.push('/');
    }
    for name in &place.names {
        out.push_str(if rng.chance(15) { "//" } else { "/" });
        if rng.chance(10) {
            out.push_str("./");
        }
        out.push_str(&if fold {
            recase(rng, name)
        } else {
            name.clone()
        });
    }
    if !place.names.is_empty() && rng.chance(20) {
        out.push('/');
    }
    out
}

/// One seed of the model fuzzer: under each platform's rules, a random model
/// and `PROBES` paths judged as every file type, then `SPELLINGS` pairs of
/// spellings through `path_is_within`.
pub(super) fn model_seed(seed: u64) -> Result<Stats, String> {
    let mut stats = Stats::default();
    for (index, (name, rules)) in [("windows", WINDOWS), ("macos", MACOS), ("linux", LINUX)]
        .into_iter()
        .enumerate()
    {
        let mut rng = Rng::new(seed.wrapping_mul(3).wrapping_add(index as u64 + 0x1000));
        let world = World::build(&mut rng, name, rules);
        let _system = test_system_dirs::add(vec![world.system_given.clone()]);
        let walk = Walk {
            rules,
            disk: &world.model,
        };
        for i in 0..PROBES {
            let path = world.probe(&mut rng);
            for kind in Kind::ALL {
                let got = check_write_path_under(&path, kind.target(), &world.dirs, walk);
                judge(
                    &path,
                    &world.expect(&path, kind),
                    as_str(&got),
                    Said::Policy,
                )
                .map_err(|why| {
                    format!(
                        "model {} probe {i} {} as {kind:?}: {why}; lands {}; links on the way {:?}",
                        world.name,
                        world.show(&path),
                        world.show_landing(&path),
                        world.links_on(&path)
                    )
                })?;
                stats.bump(&format!("model.{name}.verdicts"));
            }
        }
        for i in 0..SPELLINGS {
            let (path, dir) = place_pair(&mut rng, rules.windows);
            let (path_text, dir_text) = if rules.windows {
                (
                    spell_windows(&mut rng, &path, rules.fold_case),
                    spell_windows(&mut rng, &dir, rules.fold_case),
                )
            } else {
                (
                    spell_posix(&mut rng, &path, rules.fold_case),
                    spell_posix(&mut rng, &dir, rules.fold_case),
                )
            };
            let want = place_within(&path, &dir, rules.fold_case);
            let got = path_is_within(&path_text, &dir_text, rules);
            if got != want {
                return Err(format!(
                    "model {name} spelling {i}: path_is_within({path_text:?}, {dir_text:?}) = \
                     {got}, but {path:?} is{} inside {dir:?}",
                    if want { "" } else { " not" }
                ));
            }
            stats.bump(&format!("model.{name}.spellings"));
        }
    }
    Ok(stats)
}
