//! [#576 RV5-RL-1] Every ts-rs export test writes into `src-tauri/bindings/`,
//! and the app crate hosts none.
//!
//! A `#[ts(export)]` derive makes ts-rs emit an `export_bindings_*` test that
//! writes the type's `.ts` file to its `export_to`, resolved against the
//! DECLARING crate's manifest directory. Two app-crate exports said
//! `export_to = "../../src/types/ipc/"`, which from `src-tauri/` is the parent of
//! the checkout: every `cargo test` wrote `MemoryStats.ts` and
//! `ProcessMemory.ts` outside the repository (into `.claude/worktrees/src/` for a
//! worktree), and under the CI's unprivileged test user that write failed the
//! job. The rule (docs/ipc-typegen.md) is that every exporting type lives in a
//! member crate with `export_to = "../../bindings/"`, and `pnpm gen:ipc-types`
//! alone writes `src/types/ipc/`.
//!
//! A source scan, so it also covers a crate whose tests did not run: it reads
//! every `#[ts(...)]` attribute - also one a `#[cfg_attr(.., ts(...))]` carries
//! (#576 RV6a-9) - under `src-tauri/src` and
//! `src-tauri/crates/*/src`, keeps the ones that generate an export test (a bare
//! `export` among the arguments) and resolves their target the way ts-rs does.

use std::path::{Component, Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("a source directory is readable");
    for entry in entries {
        let path = entry.expect("a directory entry is readable").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `path` with `.` and `..` folded away, without touching the filesystem (the
/// target directory may not exist).
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The index just past the bracket that closes the one at the start of `text`
/// (`(` or `[`), skipping string literals; `None` if it never closes.
fn closing(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `text` split at its top-level commas (not inside brackets or strings).
fn top_level_items(text: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with(['(', '[', '"']) {
            i += if let Some(string) = rest.strip_prefix('"') {
                // A string on its own: close it like a one-character bracket pair.
                string.find('"').map_or(rest.len(), |n| n + 2)
            } else {
                closing(rest).unwrap_or(rest.len())
            };
            continue;
        }
        if rest.starts_with(',') {
            items.push(&text[start..i]);
            start = i + 1;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    items.push(&text[start..]);
    items
}

/// The argument lists of every `ts(...)` an attribute's content applies: the
/// attribute itself, or each attribute a `cfg_attr(predicate, ..)` carries (at
/// any depth).
fn ts_arguments(attribute: &str) -> Vec<&str> {
    let attribute = attribute.trim();
    let inner = |name: &str| {
        attribute
            .strip_prefix(name)
            .map(str::trim_start)
            .filter(|rest| rest.starts_with('('))
            .and_then(|rest| closing(rest).map(|end| &rest[1..end - 1]))
    };
    if let Some(args) = inner("ts") {
        return vec![args];
    }
    if let Some(items) = inner("cfg_attr") {
        return top_level_items(items)
            .into_iter()
            .skip(1)
            .flat_map(ts_arguments)
            .collect();
    }
    Vec::new()
}

/// Whether `prefix` (the text of a line before an attribute) holds nothing but
/// whitespace and complete attributes - so the attribute is code, not a mention
/// in a comment or a string.
fn only_attributes(prefix: &str) -> bool {
    let mut rest = prefix.trim_start();
    while !rest.is_empty() {
        let Some(attribute) = rest.strip_prefix('#') else {
            return false;
        };
        let Some(end) = closing(attribute).filter(|_| attribute.starts_with('[')) else {
            return false;
        };
        rest = attribute[end..].trim_start();
    }
    true
}

/// Every attribute in `source` that generates a ts-rs export test - a `#[ts(..)]`
/// or a `#[cfg_attr(.., ts(..))]` (#576 RV6a-9) whose arguments include a bare
/// `export` - as `(line, export_to)`, `None` when it takes ts-rs's default
/// (`bindings/`).
fn exporting_attributes(source: &str) -> Vec<(usize, Option<String>)> {
    let mut found = Vec::new();
    for (at, _) in source.match_indices("#[") {
        // An attribute starts its line (after any other attributes); a mention in
        // a comment or a string does not.
        let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
        if !only_attributes(&source[line_start..at]) {
            continue;
        }
        let Some(end) = closing(&source[at + 1..]) else {
            continue;
        };
        let content = &source[at + 2..at + end];
        let line = source[..at].matches('\n').count() + 1;
        for args in ts_arguments(content) {
            let items = top_level_items(args);
            if items.iter().any(|arg| arg.trim() == "export") {
                let export_to = items
                    .iter()
                    .map(|arg| arg.trim())
                    .find(|arg| arg.starts_with("export_to"))
                    .and_then(|arg| arg.split('"').nth(1))
                    .map(str::to_owned);
                found.push((line, export_to));
            }
        }
    }
    found
}

#[test]
fn every_ts_export_test_writes_into_the_bindings_tree() {
    let app_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bindings = lexical(&app_root.join("bindings"));
    let mut crate_roots = vec![app_root.clone()];
    let members = std::fs::read_dir(app_root.join("crates")).expect("src-tauri/crates is readable");
    for member in members {
        let member = member.expect("a member entry is readable").path();
        if member.join("Cargo.toml").is_file() {
            crate_roots.push(member);
        }
    }
    crate_roots.sort();

    let mut exports = 0;
    let mut violations = Vec::new();
    for root in &crate_roots {
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        files.sort();
        for file in files {
            // This module's own fixtures are not exports.
            if file.ends_with("src/ts_export_paths_tests.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("a source file is readable");
            for (line, export_to) in exporting_attributes(&source) {
                exports += 1;
                let target = export_to.as_deref().unwrap_or("bindings/");
                let resolved = lexical(&root.join(target));
                let dir = if target.ends_with(".ts") {
                    resolved.parent().map(Path::to_path_buf).unwrap_or_default()
                } else {
                    resolved
                };
                let at = format!(
                    "{}:{line}",
                    file.strip_prefix(&app_root).unwrap_or(&file).display()
                );
                if dir != bindings {
                    violations.push(format!(
                        "{at}: export_to {target:?} writes into {} (outside src-tauri/bindings)",
                        dir.display()
                    ));
                }
                if root == &app_root {
                    violations.push(format!(
                        "{at}: the app crate hosts an export test; move the type to a member \
                         crate (docs/ipc-typegen.md)"
                    ));
                }
            }
        }
    }
    println!(
        "ts-rs export tests found: {exports} across {} crates; violations: {}",
        crate_roots.len(),
        violations.len()
    );
    for violation in &violations {
        println!("  {violation}");
    }
    assert!(
        exports >= 50,
        "only {exports} exporting attributes found - did the scan stop reading?"
    );
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn the_attribute_scan_reads_export_and_export_to() {
    let source = "#[derive(TS)]\n#[ts(export, export_to = \"../../bindings/\")]\nstruct A;\n\
                  #[ts(export_to = \"bindings/B.ts\")]\nstruct B;\n    #[ts(rename = \"C\", export)]\n\
                  struct C;\n/// see `#[ts(export)]`\nstruct D;\n";
    assert_eq!(
        exporting_attributes(source),
        vec![(2, Some("../../bindings/".to_owned())), (6, None)]
    );
    assert_eq!(
        lexical(Path::new("/a/src-tauri/../../src/types/ipc/")),
        PathBuf::from("/src/types/ipc")
    );
}

#[test]
fn the_attribute_scan_reads_an_export_inside_cfg_attr() {
    // [#576 RV6a-9] `cfg_attr` keeps an export test that cargo fmt leaves on one
    // line, and two attributes can share a line; a mention in a comment is still
    // not an attribute.
    let source = "#[cfg_attr(test, ts(export, export_to = \"../../../../y/\"))]\nstruct E;\n\
                  #[cfg_attr(feature = \"x\", derive(TS), ts(export))]\nstruct F;\n\
                  #[derive(TS)] #[ts(export)]\nstruct G;\n\
                  #[cfg_attr(test, ts(export_to = \"bindings/H.ts\"))]\nstruct H;\n\
                  /// see #[cfg_attr(test, ts(export))]\nstruct I;\n";
    let found = exporting_attributes(source);
    println!("exports found in the cfg_attr fixture: {found:?}");
    assert_eq!(
        found,
        vec![(1, Some("../../../../y/".to_owned())), (3, None), (5, None)]
    );
}
