//! Registry coverage tests that read APP-side sources - `src/lib.rs`'s
//! `generate_handler![]` block, the frontend `src/` tree (both resolved
//! relative to this crate's `CARGO_MANIFEST_DIR`) - or enumerate the
//! app-level `tile_providers`. Relocated from `telemetry::registry` ahead of
//! the crate split: the files they open stay with the app crate, and
//! `tile_providers` sits above `telemetry`.

use std::collections::HashSet;

use crate::telemetry::registry::{EVENT_NAMES, SOURCE_NAMES};

/// Bare names of commands removed from the app but kept in the vocabulary.
/// They intentionally have no live handler and no frontend tracking, so the
/// `command.*` and `undo-after.*` coverage checks skip them.
const REMOVED_COMMANDS: &[&str] = &[
    "adjust_loop_closures",
    "resolve_all_warnings",
    // SpeleoDB commands with no caller, removed in the dead-code cleanup (#670, #671).
    "speleodb_get_project_metadata",
];

/// Dialog keys removed from `App.tsx` whose registry slots are likewise
/// retained. The `action.dialog.*` / `dialog-cancel.*` / `dwell.dialog.*`
/// coverage check skips them.
// Dialogs whose 4 event slots are retained but excluded from the
// `activeDialog`-derived stale check: either genuinely removed
// (`adjust-closures`) or relocated OUT of the single `activeDialog` slot into an
// independent overlay (`held-lock-resume`, #201 - so it can layer over another
// dialog during login), or BORN as one (`speleodb-dump-picker`, #258 - the
// ambiguous-dump question is raised from four surfaces, three of which already
// hold the slot themselves). The slots are kept for event-numbering stability.
const REMOVED_DIALOGS: &[&str] = &[
    "adjust-closures",
    "held-lock-resume",
    "speleodb-dump-picker",
    // Folded into the SpeleoDB exit prompt, an independent overlay that layers
    // over whatever dialog raised it instead of evicting it from the slot.
    "speleodb-close",
    "speleodb-quit",
];

/// `action.toggle.*` events that are intentionally NOT derived from
/// `DISPLAY_DEFAULTS`. `geometryMode` is an enum display mode tracked via an
/// explicit `trackAction('toggle.geometryMode')`, not a boolean default.
const EXTRA_TOGGLES: &[&str] = &["action.toggle.geometryMode"];

/// Every tile provider must have its per-provider consumption events registered
/// (#31), so tile counts land as registered events rather than the overflow map.
/// Catches "added a provider but forgot the telemetry events".
#[test]
fn every_tile_provider_has_consumption_events() {
    for info in crate::tile_providers::get_all_provider_info() {
        for kind in ["fetch", "revalidate", "cache"] {
            let event = format!("tile.{kind}.{}", info.name);
            assert!(
                EVENT_NAMES.iter().any(|&e| e == event),
                "provider '{}' missing telemetry event '{event}' - add it to EVENT_NAMES \
                 here and in telemetry-worker/src/registry.ts",
                info.name
            );
        }
    }
}

/// Commands registered in `generate_handler![]` that are excluded from
/// the `command.*` exhaustiveness check (they are telemetry internals).
const TELEMETRY_COMMANDS: &[&str] = &[
    "track_events",
    "flush_telemetry",
    "is_telemetry_available",
    "set_telemetry_enabled",
];

#[test]
fn registry_covers_all_commands() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let lib_path = format!("{manifest_dir}/src/lib.rs");
    let lib_src = std::fs::read_to_string(&lib_path)
        .unwrap_or_else(|e| panic!("Failed to read {lib_path}: {e}"));

    // Extract the generate_handler![...] block. The bracket-counting helper
    // (shared with app_contract_tests) survives attributes inside the block,
    // which a first-`]` slice would truncate at.
    let block = super::app_contract_tests::generate_handler_block(&lib_src);

    // Parse command names from each line
    let mut handler_commands: HashSet<String> = HashSet::new();
    for line in block.lines() {
        let trimmed = line.trim();
        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        // Extract the last :: segment, e.g. "commands::project::create_project,"
        let name = trimmed
            .rsplit("::")
            .next()
            .unwrap()
            .trim_end_matches(',')
            .trim();
        if !TELEMETRY_COMMANDS.contains(&name) {
            handler_commands.insert(format!("command.{name}"));
        }
    }

    // Build set of command.* entries from EVENT_NAMES, excluding the retained
    // slots of removed commands (which have no live handler by design).
    let removed: HashSet<String> = REMOVED_COMMANDS
        .iter()
        .map(|c| format!("command.{c}"))
        .collect();
    let registry_commands: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("command."))
        .map(|n| (*n).to_string())
        .filter(|n| !removed.contains(n))
        .collect();

    let missing: Vec<_> = handler_commands.difference(&registry_commands).collect();
    let stale: Vec<_> = registry_commands.difference(&handler_commands).collect();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "EVENT_NAMES command.* entries out of sync with generate_handler![]:\n  \
         Missing from registry: {missing:?}\n  \
         Stale in registry: {stale:?}"
    );
}

/// Helper: read a frontend source file relative to the project root.
fn read_frontend_file(rel_path: &str) -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/../{rel_path}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"))
}

#[test]
fn registry_covers_all_tools() {
    let src = read_frontend_file("src/store/uiStore.ts");

    // Extract ToolType union members
    let start = src
        .find("type ToolType =")
        .expect("ToolType not found in uiStore.ts");
    let block = &src[start..];
    let mut tools: HashSet<String> = HashSet::new();
    for line in block.lines().skip(1) {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            break;
        }
        if let Some(q1) = trimmed.find('\'') {
            if let Some(q2) = trimmed[q1 + 1..].find('\'') {
                tools.insert(trimmed[q1 + 1..q1 + 1 + q2].to_string());
            }
        }
    }
    assert!(!tools.is_empty(), "Failed to parse any ToolType variants");

    let code_action: HashSet<String> = tools.iter().map(|t| format!("action.tool.{t}")).collect();
    let code_dwell: HashSet<String> = tools.iter().map(|t| format!("dwell.tool.{t}")).collect();

    let reg_action: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("action.tool."))
        .map(|n| (*n).to_string())
        .collect();
    let reg_dwell: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("dwell.tool."))
        .map(|n| (*n).to_string())
        .collect();

    // Tools removed from the UI whose event names are kept in the vocabulary
    // (their events simply never fire anymore). The registry is allowed to
    // retain these even though they're gone from ToolType - but every
    // *current* tool must still be covered.
    const RETIRED_TOOLS: &[&str] = &["draw", "delete"];
    let retired_action: HashSet<String> = RETIRED_TOOLS
        .iter()
        .map(|t| format!("action.tool.{t}"))
        .collect();
    let retired_dwell: HashSet<String> = RETIRED_TOOLS
        .iter()
        .map(|t| format!("dwell.tool.{t}"))
        .collect();

    let missing_action: Vec<_> = code_action.difference(&reg_action).collect();
    let stale_action: Vec<_> = reg_action
        .difference(&code_action)
        .filter(|n| !retired_action.contains(*n))
        .collect();
    let missing_dwell: Vec<_> = code_dwell.difference(&reg_dwell).collect();
    let stale_dwell: Vec<_> = reg_dwell
        .difference(&code_dwell)
        .filter(|n| !retired_dwell.contains(*n))
        .collect();

    assert!(
        missing_action.is_empty()
            && stale_action.is_empty()
            && missing_dwell.is_empty()
            && stale_dwell.is_empty(),
        "Tool registry out of sync with ToolType:\n  \
         Missing action.tool.*: {missing_action:?}\n  \
         Stale action.tool.*: {stale_action:?}\n  \
         Missing dwell.tool.*: {missing_dwell:?}\n  \
         Stale dwell.tool.*: {stale_dwell:?}"
    );
}

#[test]
fn registry_covers_all_undoable_commands() {
    let src = read_frontend_file("src/utils/telemetry.ts");

    // Extract UNDOABLE_COMMANDS set members
    let start = src
        .find("UNDOABLE_COMMANDS")
        .expect("UNDOABLE_COMMANDS not found");
    let bracket = src[start..]
        .find('[')
        .expect("[ not found after UNDOABLE_COMMANDS");
    let end_bracket = src[start + bracket..]
        .find(']')
        .expect("] not found after UNDOABLE_COMMANDS [");
    let block = &src[start + bracket..start + bracket + end_bracket];

    let mut commands: HashSet<String> = HashSet::new();
    for cap in block.split('\'').collect::<Vec<_>>().chunks(2) {
        if cap.len() == 2 && !cap[1].is_empty() {
            // cap[1] is the string between quotes
            commands.insert(cap[1].to_string());
        }
    }
    // Better approach: extract all single-quoted strings
    let mut commands2: HashSet<String> = HashSet::new();
    let mut chars = block.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            let s: String = chars.by_ref().take_while(|&ch| ch != '\'').collect();
            if !s.is_empty() {
                commands2.insert(s);
            }
        }
    }
    let commands = commands2;
    assert!(
        !commands.is_empty(),
        "Failed to parse any UNDOABLE_COMMANDS"
    );

    let code_undo: HashSet<String> = commands.iter().map(|c| format!("undo-after.{c}")).collect();

    // Exclude the retained undo-after slots of removed commands.
    let removed: HashSet<String> = REMOVED_COMMANDS
        .iter()
        .map(|c| format!("undo-after.{c}"))
        .collect();
    let reg_undo: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("undo-after."))
        .map(|n| (*n).to_string())
        .filter(|n| !removed.contains(n))
        .collect();

    let missing: Vec<_> = code_undo.difference(&reg_undo).collect();
    let stale: Vec<_> = reg_undo.difference(&code_undo).collect();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "undo-after.* registry out of sync with UNDOABLE_COMMANDS:\n  \
         Missing: {missing:?}\n  \
         Stale: {stale:?}"
    );
}

#[test]
fn registry_covers_all_display_toggles() {
    let src = read_frontend_file("src/store/displaySelectors.ts");

    // Extract DISPLAY_DEFAULTS keys
    let start = src
        .find("DISPLAY_DEFAULTS = {")
        .expect("DISPLAY_DEFAULTS not found");
    let brace = start + "DISPLAY_DEFAULTS = {".len();
    let end_brace = src[brace..]
        .find('}')
        .expect("} not found after DISPLAY_DEFAULTS");
    let block = &src[brace..brace + end_brace];

    let mut keys: HashSet<String> = HashSet::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim();
            if !key.is_empty() && !key.starts_with("//") {
                keys.insert(key.to_string());
            }
        }
    }
    assert!(
        !keys.is_empty(),
        "Failed to parse any DISPLAY_DEFAULTS keys"
    );

    let code_toggles: HashSet<String> = keys.iter().map(|k| format!("action.toggle.{k}")).collect();

    // Exclude toggles that are intentionally not DISPLAY_DEFAULTS booleans.
    let extra: HashSet<String> = EXTRA_TOGGLES.iter().map(|s| (*s).to_string()).collect();
    let reg_toggles: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("action.toggle."))
        .map(|n| (*n).to_string())
        .filter(|n| !extra.contains(n))
        .collect();

    let missing: Vec<_> = code_toggles.difference(&reg_toggles).collect();
    let stale: Vec<_> = reg_toggles.difference(&code_toggles).collect();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "action.toggle.* registry out of sync with DISPLAY_DEFAULTS:\n  \
         Missing: {missing:?}\n  \
         Stale: {stale:?}"
    );
}

#[test]
fn registry_covers_all_dialogs() {
    let src = read_frontend_file("src/App.tsx");

    // Extract all activeDialog === '...' patterns
    let re_pattern = "activeDialog === '";
    let mut dialogs: HashSet<String> = HashSet::new();
    for line in src.lines() {
        let mut search = line;
        while let Some(pos) = search.find(re_pattern) {
            let after = &search[pos + re_pattern.len()..];
            if let Some(end) = after.find('\'') {
                dialogs.insert(after[..end].to_string());
            }
            search = &search[pos + re_pattern.len()..];
        }
    }
    assert!(
        !dialogs.is_empty(),
        "Failed to parse any dialogs from App.tsx"
    );

    // Build expected sets
    let code_action: HashSet<String> = dialogs
        .iter()
        .map(|d| format!("action.dialog.{d}"))
        .collect();
    let code_cancel: HashSet<String> = dialogs
        .iter()
        .map(|d| format!("dialog-cancel.{d}"))
        .collect();
    let code_dwell: HashSet<String> = dialogs
        .iter()
        .map(|d| format!("dwell.dialog.{d}"))
        .collect();

    // Retained slots of removed dialogs are excluded from the stale checks.
    let removed_action: HashSet<String> = REMOVED_DIALOGS
        .iter()
        .map(|d| format!("action.dialog.{d}"))
        .collect();
    let removed_cancel: HashSet<String> = REMOVED_DIALOGS
        .iter()
        .map(|d| format!("dialog-cancel.{d}"))
        .collect();
    let removed_dwell: HashSet<String> = REMOVED_DIALOGS
        .iter()
        .map(|d| format!("dwell.dialog.{d}"))
        .collect();
    let reg_action: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("action.dialog."))
        .map(|n| (*n).to_string())
        .filter(|n| !removed_action.contains(n))
        .collect();
    let reg_cancel: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("dialog-cancel."))
        .map(|n| (*n).to_string())
        .filter(|n| !removed_cancel.contains(n))
        .collect();
    let reg_dwell: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("dwell.dialog."))
        .map(|n| (*n).to_string())
        .filter(|n| !removed_dwell.contains(n))
        .collect();

    let missing_action: Vec<_> = code_action.difference(&reg_action).collect();
    let stale_action: Vec<_> = reg_action.difference(&code_action).collect();
    let missing_cancel: Vec<_> = code_cancel.difference(&reg_cancel).collect();
    let stale_cancel: Vec<_> = reg_cancel.difference(&code_cancel).collect();
    let missing_dwell: Vec<_> = code_dwell.difference(&reg_dwell).collect();
    let stale_dwell: Vec<_> = reg_dwell.difference(&code_dwell).collect();

    assert!(
        missing_action.is_empty()
            && stale_action.is_empty()
            && missing_cancel.is_empty()
            && stale_cancel.is_empty()
            && missing_dwell.is_empty()
            && stale_dwell.is_empty(),
        "Dialog registry out of sync with App.tsx activeDialog values:\n  \
         Missing action.dialog.*: {missing_action:?}\n  \
         Stale action.dialog.*: {stale_action:?}\n  \
         Missing dialog-cancel.*: {missing_cancel:?}\n  \
         Stale dialog-cancel.*: {stale_cancel:?}\n  \
         Missing dwell.dialog.*: {missing_dwell:?}\n  \
         Stale dwell.dialog.*: {stale_dwell:?}"
    );
}

#[test]
fn registry_covers_all_sources() {
    let src = read_frontend_file("src/utils/telemetry.ts");
    let sources = extract_type_union(&src, "SourceName");

    // SourceName excludes "" (no source) and "unknown" (default fallback)
    // which are registry-only entries. Add them to get the full set.
    let excluded: HashSet<&str> = ["", "unknown"].into();

    let reg_sources: HashSet<String> = SOURCE_NAMES
        .iter()
        .filter(|s| !excluded.contains(*s))
        .map(|s| (*s).to_string())
        .collect();

    let code_sources: HashSet<String> = sources.into_iter().collect();

    let missing: Vec<_> = code_sources.difference(&reg_sources).collect();
    let stale: Vec<_> = reg_sources.difference(&code_sources).collect();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "SOURCE_NAMES out of sync with SourceName type union:\n  \
         Missing from registry: {missing:?}\n  \
         Stale in registry: {stale:?}"
    );
}

/// Parse a TS type union like `type Foo = | 'a' | 'b' | 'c';`.
/// Extracts single-quoted strings from `|`-prefixed lines after
/// `type {name} =`.
fn extract_type_union(src: &str, name: &str) -> Vec<String> {
    let marker = format!("type {name} =");
    let start = src
        .find(&marker)
        .unwrap_or_else(|| panic!("type {name} not found in source"));
    let block = &src[start + marker.len()..];
    let mut items = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            if !items.is_empty() {
                break; // end of union
            }
            continue; // skip the `=` line itself if no `|` yet
        }
        if let Some(q1) = trimmed.find('\'') {
            if let Some(q2) = trimmed[q1 + 1..].find('\'') {
                items.push(trimmed[q1 + 1..q1 + 1 + q2].to_string());
            }
        }
    }
    assert!(
        !items.is_empty(),
        "Failed to parse any variants from type {name}"
    );
    items
}

#[test]
fn registry_covers_all_nav_events() {
    let src = read_frontend_file("src/utils/telemetry.ts");
    let navs = extract_type_union(&src, "NavigationAction");

    let code_action: HashSet<String> = navs.iter().map(|n| format!("action.nav.{n}")).collect();

    let reg_action: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("action.nav."))
        .map(|n| (*n).to_string())
        .collect();

    let missing: Vec<_> = code_action.difference(&reg_action).collect();
    let stale: Vec<_> = reg_action.difference(&code_action).collect();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "action.nav.* registry out of sync with NavigationAction:\n  \
         Missing: {missing:?}\n  \
         Stale: {stale:?}"
    );
}

#[test]
fn registry_covers_all_panel_events() {
    let src = read_frontend_file("src/utils/telemetry.ts");
    let panels = extract_type_union(&src, "PanelName");

    // Each panel emits open/close (visibility) and pin/unpin (mode) events.
    let verbs = ["open", "close", "pin", "unpin"];
    let mut missing: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    for verb in verbs {
        let prefix = format!("action.panel-{verb}.");
        let code: HashSet<String> = panels.iter().map(|p| format!("{prefix}{p}")).collect();
        let reg: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with(&prefix))
            .map(|n| (*n).to_string())
            .collect();
        missing.extend(code.difference(&reg).cloned());
        stale.extend(reg.difference(&code).cloned());
    }

    assert!(
        missing.is_empty() && stale.is_empty(),
        "Panel registry out of sync with PanelName:\n  \
         Missing: {missing:?}\n  \
         Stale: {stale:?}"
    );
}

#[test]
fn registry_covers_all_selections() {
    let src = read_frontend_file("src/utils/telemetry.ts");
    let types = extract_type_union(&src, "SelectionType");

    let code_select: HashSet<String> = types.iter().map(|t| format!("action.select.{t}")).collect();
    let code_add: HashSet<String> = types
        .iter()
        .map(|t| format!("action.select-add.{t}"))
        .collect();

    let reg_select: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("action.select."))
        .map(|n| (*n).to_string())
        .collect();
    let reg_add: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| n.starts_with("action.select-add."))
        .map(|n| (*n).to_string())
        .collect();

    let missing_select: Vec<_> = code_select.difference(&reg_select).collect();
    let stale_select: Vec<_> = reg_select.difference(&code_select).collect();
    let missing_add: Vec<_> = code_add.difference(&reg_add).collect();
    let stale_add: Vec<_> = reg_add.difference(&code_add).collect();

    assert!(
        missing_select.is_empty()
            && stale_select.is_empty()
            && missing_add.is_empty()
            && stale_add.is_empty(),
        "Selection registry out of sync with SelectionType:\n  \
         Missing select.*: {missing_select:?}\n  \
         Stale select.*: {stale_select:?}\n  \
         Missing select-add.*: {missing_add:?}\n  \
         Stale select-add.*: {stale_add:?}"
    );
}

#[test]
fn registry_covers_all_context_menu_actions() {
    let src = read_frontend_file("src/utils/telemetry.ts");
    let actions = extract_type_union(&src, "ContextMenuAction");

    let code_actions: HashSet<String> = actions.iter().map(|a| format!("action.{a}")).collect();

    let reg_actions: HashSet<String> = EVENT_NAMES
        .iter()
        .filter(|n| {
            n.starts_with("action.")
                && !n.starts_with("action.tool.")
                && !n.starts_with("action.nav.")
                && !n.starts_with("action.toggle.")
                && !n.starts_with("action.select")
                && !n.starts_with("action.dialog.")
                && !n.starts_with("action.panel-")
        })
        .map(|n| (*n).to_string())
        .collect();

    let missing: Vec<_> = code_actions.difference(&reg_actions).collect();
    let stale: Vec<_> = reg_actions.difference(&code_actions).collect();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "Context-menu action registry out of sync with ContextMenuAction:\n  \
         Missing: {missing:?}\n  \
         Stale: {stale:?}"
    );
}

/// Helper: walk directory recursively collecting .ts/.tsx files.
fn walk_ts_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_ts_files(&path, files);
            } else if let Some(ext) = path.extension() {
                if ext == "ts" || ext == "tsx" {
                    files.push(path);
                }
            }
        }
    }
}

#[test]
fn dialog_complete_calls_match_dialogs() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src_dir = std::path::Path::new(manifest_dir).join("../src");
    let mut ts_files = Vec::new();
    walk_ts_files(&src_dir, &mut ts_files);
    assert!(!ts_files.is_empty(), "No .ts/.tsx files found in src/");

    // Collect all dialog names from trackDialogComplete('...') calls
    let pattern = "trackDialogComplete('";
    let mut complete_names: HashSet<String> = HashSet::new();
    for file in &ts_files {
        let content = std::fs::read_to_string(file).unwrap();
        let mut search = content.as_str();
        while let Some(pos) = search.find(pattern) {
            let after = &search[pos + pattern.len()..];
            if let Some(end) = after.find('\'') {
                complete_names.insert(after[..end].to_string());
            }
            search = &search[pos + pattern.len()..];
        }
    }
    assert!(
        !complete_names.is_empty(),
        "No trackDialogComplete() calls found in src/"
    );

    // All dialog names from App.tsx (canonical list)
    let app_src = read_frontend_file("src/App.tsx");
    let dialog_pattern = "activeDialog === '";
    let mut app_dialogs: HashSet<String> = HashSet::new();
    let mut search = app_src.as_str();
    while let Some(pos) = search.find(dialog_pattern) {
        let after = &search[pos + dialog_pattern.len()..];
        if let Some(end) = after.find('\'') {
            app_dialogs.insert(after[..end].to_string());
        }
        search = &search[pos + dialog_pattern.len()..];
    }

    // Every trackDialogComplete name must be a real dialog
    let invalid: Vec<_> = complete_names.difference(&app_dialogs).collect();
    assert!(
        invalid.is_empty(),
        "trackDialogComplete() called with names not in App.tsx dialog list:\n  \
         {invalid:?}\n  \
         These would silently produce unregistered dialog-cancel events."
    );
}
