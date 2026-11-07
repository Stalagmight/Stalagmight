//! [#576 RV5-RL-6] Every way to construct a `Database` is in the busy-timeout test.
//!
//! `busy_timeout_tests` pins the 5000 ms busy timeout on each open path, and it
//! lists those paths by hand: a new constructor was covered by nothing, and
//! nothing said so. This tripwire finds the constructors in the source - every
//! function that builds the struct (`Self { conn` / `Database { conn`; the field is
//! private to `sqlite`, so the struct cannot be built anywhere else) and every
//! function whose signature returns `Self` or `Database` in a `Result` or plainly -
//! and fails for any that `busy_timeout_tests.rs` never calls.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Constructors the busy-timeout test cannot reach, with why that is safe. Keep it
/// this short: anything here is a construction no test pins the timeout of.
const NOT_AN_OPEN_PATH: &[(&str, &str)] = &[
    (
        "open_with",
        "private to open.rs: the one body of open and open_scratch (#576 W25-NF1), \
         which are its only callers and are both opened in the busy-timeout test",
    ),
    (
        "open_existing_with",
        "private to open.rs: the one body of open_existing and \
         open_existing_refusing_upgrade (#576 RV4-R-M3-OPEN), which are its only callers \
         and are both opened in the busy-timeout test",
    ),
    (
        "finish_open_existing",
        "private to open.rs: the shared tail of open_existing_with and \
         open_existing_untrusted (#608), its only callers, both opened in the \
         busy-timeout test; it takes their connection and opens nothing itself",
    ),
    (
        "run_existing_open_checks",
        "private to open.rs: the checks finish_open_existing runs on the handle it \
         was given (#608); it opens nothing itself",
    ),
    (
        "upgraded_copy_in_memory",
        "private to open.rs: the older-file branch of open_existing_to_read (#576 \
         RV6b-R1-D1), its only caller, which the busy-timeout test opens on an older file",
    ),
];

/// The production `.rs` files of the `sqlite` module: `sqlite.rs` and every file
/// under `sqlite/`, at any depth (#576 RV6a-14: a constructor in a future
/// `sqlite/<sub>/x.rs` is found too), test modules (`*tests.rs`) excluded. Each is
/// named by its path below `persistence/`.
fn sqlite_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            let is_rust = path.extension().is_some_and(|ext| ext == "rs");
            let is_test = path
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().ends_with("tests"));
            if is_rust && !is_test {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/persistence");
    let mut files = vec![root.join("sqlite.rs")];
    walk(&root.join("sqlite"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (name, std::fs::read_to_string(&path).unwrap())
        })
        .collect()
}

/// The name of the last `fn NAME` that starts before byte `at` in `src`.
fn enclosing_fn(src: &str, at: usize) -> Option<String> {
    let start = src[..at].rfind("fn ")? + 3;
    let name: String = src[start..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// How the busy-timeout test has to reach a constructor for it to count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reached {
    /// An associated function: called by its path, `Database::NAME(`.
    ByPath,
    /// A method that consumes a `Database` (`into_read_only(self, ..)`): called as
    /// `.NAME(` on a call chain rooted at `Database::`.
    OnADatabase,
}

/// Whether `fn name` in `src` takes `self` (a method) or not (an associated
/// function).
fn reached_as(src: &str, name: &str) -> Reached {
    let takes_self = src
        .match_indices(&format!("fn {name}("))
        .filter_map(|(at, _)| {
            let params = &src[at..];
            let open = params.find('(')? + 1;
            let first = params[open..].split([',', ')']).next()?;
            let first = first.trim().trim_start_matches('&').trim_start();
            Some(first.trim_start_matches("mut ").trim_start() == "self")
        })
        .any(|is_method| is_method);
    if takes_self {
        Reached::OnADatabase
    } else {
        Reached::ByPath
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The root of the call chain whose next link is the `.` at byte `dot` of `code`:
/// `Database` for `Database::open_existing(&path).unwrap().into_read_only(`, the
/// variable for `created.into_read_only(`, `None` when there is no chain.
fn chain_root(code: &str, dot: usize) -> Option<&str> {
    let bytes = code.as_bytes();
    let mut at = dot;
    loop {
        let mut end = at;
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end > 0 && bytes[end - 1] == b'?' {
            end -= 1;
        }
        // A call: step back over its balanced argument list to the callee.
        if end > 0 && bytes[end - 1] == b')' {
            let mut depth = 0usize;
            let mut open = end;
            loop {
                open = open.checked_sub(1)?;
                match bytes[open] {
                    b')' => depth += 1,
                    b'(' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            end = open;
        }
        let mut start = end;
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        if start == end {
            return None;
        }
        let mut before = start;
        while before > 0 && bytes[before - 1].is_ascii_whitespace() {
            before -= 1;
        }
        if before > 0 && bytes[before - 1] == b'.' {
            at = before - 1;
            continue;
        }
        if before >= 2 && &code[before - 2..before] == "::" {
            let segment_end = before - 2;
            let mut segment_start = segment_end;
            while segment_start > 0 && is_ident_byte(bytes[segment_start - 1]) {
                segment_start -= 1;
            }
            return Some(&code[segment_start..segment_end]);
        }
        return Some(&code[start..end]);
    }
}

/// [#576 RV6a-14, NF-W8BE-1] Whether `covered` (the busy-timeout test's source)
/// calls the constructor `name`. Exact: it used to accept `Database::NAME(` or
/// `.NAME(` anywhere in the file, so an `.open(` on an `OpenOptions`, or the name
/// in a comment, counted as covering `Database::open`. Only code counts now, an
/// associated function only by its own `Database::` path (not `TestDatabase::`),
/// and a consuming method only on a chain rooted at `Database::`.
fn calls(covered: &str, name: &str, reached: Reached) -> bool {
    // [#576 RV9a-9] The shared blanking: block comments, raw strings and char
    // literals too, which the local one this used missed.
    let code = crate::models::source_scan::code_only(covered);
    match reached {
        Reached::ByPath => code
            .match_indices(&format!("Database::{name}("))
            .any(|(at, _)| at == 0 || !is_ident_byte(code.as_bytes()[at - 1])),
        Reached::OnADatabase => code
            .match_indices(&format!(".{name}("))
            .any(|(at, _)| chain_root(&code, at) == Some("Database")),
    }
}

/// Every constructor of `Database` in `src`, by the two rules in the module doc,
/// with how a call to it is recognised.
fn constructors(src: &str) -> BTreeMap<String, Reached> {
    let mut found = BTreeSet::new();
    for literal in ["Self { conn", "Database { conn"] {
        for (at, _) in src.match_indices(literal) {
            found.extend(enclosing_fn(src, at));
        }
    }
    for (at, _) in src.match_indices("fn ") {
        let header = &src[at..];
        let end = header.find(['{', ';']).unwrap_or(header.len());
        let signature: String = header[..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let returns_one = ["Result<Self>", "Result<Database>"]
            .iter()
            .any(|ret| signature.contains(ret))
            || signature.ends_with("-> Self")
            || signature.ends_with("-> Database");
        if returns_one {
            found.extend(enclosing_fn(src, at + 3));
        }
    }
    found
        .into_iter()
        .map(|name| {
            let reached = reached_as(src, &name);
            (name, reached)
        })
        .collect()
}

#[test]
fn every_database_constructor_is_in_the_busy_timeout_test() {
    let covered = include_str!("busy_timeout_tests.rs");
    let mut uncovered = Vec::new();
    for (file, src) in sqlite_sources() {
        let in_impl_database = src.contains("impl Database {");
        for (name, reached) in constructors(&src) {
            // A `Self`-returning fn outside `impl Database` constructs another type.
            if !in_impl_database && !src.contains("Database { conn") {
                continue;
            }
            let called = calls(covered, &name, reached);
            let excused = NOT_AN_OPEN_PATH.iter().find(|(n, _)| *n == name);
            println!(
                "{file}: constructor {name} ({reached:?}) - in the busy-timeout test: {called}{}",
                excused.map_or(String::new(), |(_, why)| format!(
                    " (not an open path: {why})"
                ))
            );
            if !called && excused.is_none() {
                uncovered.push(format!("{file}::{name}"));
            }
        }
    }
    assert!(
        uncovered.is_empty(),
        "these Database constructors are not checked by busy_timeout_tests.rs - add an \
         open through each to `every_open_path_waits_at_least_five_seconds_for_a_busy_file`: \
         {uncovered:?}"
    );
}

#[test]
fn only_a_real_call_of_the_constructor_counts_as_covering_it() {
    // [#576 RV6a-14, NF-W8BE-1] The match was `Database::NAME(` or `.NAME(` anywhere
    // in the file, so each of these counted as covering a constructor that the
    // busy-timeout test never opens a `Database` through.
    let not_a_call = [
        (
            "open",
            Reached::ByPath,
            "let f = OpenOptions::new().read(true).open(&path);",
        ),
        (
            "open",
            Reached::ByPath,
            "// Database::open(&path) is covered elsewhere",
        ),
        (
            "open",
            Reached::ByPath,
            "let label = \"Database::open(&path)\";",
        ),
        (
            "open",
            Reached::ByPath,
            "let db = TestDatabase::open(&path);",
        ),
        (
            "into_read_only",
            Reached::OnADatabase,
            "let db = other.into_read_only(&path);",
        ),
        (
            "into_read_only",
            Reached::OnADatabase,
            "// .into_read_only(&path)",
        ),
        // [#576 RV9a-9] The shapes the local blanking missed: a block comment, a
        // raw string, and a char literal `'"'` that opened a phantom string, so
        // the real string after it was read as code.
        ("open", Reached::ByPath, "/* Database::open(&path) */"),
        (
            "open",
            Reached::ByPath,
            "let label = r#\"Database::open(&path)\"#;",
        ),
        (
            "open",
            Reached::ByPath,
            "let q = '\"'; let label = \"Database::open(&path)\";",
        ),
    ];
    let mut miscounted = Vec::new();
    for (name, reached, src) in not_a_call {
        let counted = calls(src, name, reached);
        println!("{name} ({reached:?}) in `{src}`: counted as a call = {counted} (expected false)");
        if counted {
            miscounted.push(src);
        }
    }

    let a_call = [
        (
            "open",
            Reached::ByPath,
            "let db = Database::open(&path).unwrap();",
        ),
        (
            "open",
            Reached::ByPath,
            "(\"open\", crate::persistence::Database::open(&path).unwrap()),",
        ),
        (
            "into_read_only",
            Reached::OnADatabase,
            "Database::open_existing(&path)\n    .unwrap()\n    .into_read_only(&path)",
        ),
        (
            "into_read_only",
            Reached::OnADatabase,
            "Database::open_existing_refusing_upgrade(&path, &refusal)?.into_read_only(&path)",
        ),
    ];
    for (name, reached, src) in a_call {
        let counted = calls(src, name, reached);
        println!("{name} ({reached:?}) in `{src}`: counted as a call = {counted} (expected true)");
        if !counted {
            miscounted.push(src);
        }
    }
    assert!(
        miscounted.is_empty(),
        "RULE: only a call in code - an associated fn by its `Database::` path, a \
         consuming method on a chain rooted at `Database::` - covers a constructor; \
         miscounted: {miscounted:?}"
    );

    // The rule each constructor is held to follows its own signature.
    assert_eq!(
        reached_as(
            "pub fn into_read_only(self, path: &Path) -> Result<Self> {",
            "into_read_only"
        ),
        Reached::OnADatabase
    );
    assert_eq!(
        reached_as("pub fn open(path: &Path) -> Result<Self> {", "open"),
        Reached::ByPath
    );
}
