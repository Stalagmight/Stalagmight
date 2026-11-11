//! The macOS menu bar's Help menu (#665).
//!
//! When an app sets no menu of its own, Tauri installs its default macOS menu,
//! whose Help submenu is empty: opening it shows only the system search field.
//! This keeps that default menu (the predefined Edit items are what make
//! copy/paste work in text fields) and fills Help with three entries.
//!
//! Windows and Linux get no menu bar from Tauri, so this is macOS-only.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, HELP_SUBMENU_ID};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_opener::OpenerExt;

/// The published user guide (source: `docs/user-guide/`).
const USER_GUIDE_URL: &str = "https://stalagmight.com/docs/";
const REPORT_ISSUE_URL: &str = "https://github.com/Stalagmight/Stalagmight/issues/new";

/// Asks the frontend to open the keyboard shortcuts dialog. The listener is
/// `src/hooks/useHelpMenu.ts`, which keeps the same name.
const SHOW_SHORTCUTS_EVENT: &str = "menu:keyboard-shortcuts";

const ID_USER_GUIDE: &str = "help.user-guide";
const ID_SHORTCUTS: &str = "help.keyboard-shortcuts";
const ID_REPORT_ISSUE: &str = "help.report-issue";

/// Tauri's default menu with the Help submenu filled in.
pub(crate) fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    let Some(help) = menu
        .get(HELP_SUBMENU_ID)
        .and_then(|item| item.as_submenu().cloned())
    else {
        tracing::warn!("Default menu has no Help submenu; leaving it as Tauri built it");
        return Ok(menu);
    };
    help.append_items(&[
        &MenuItem::with_id(app, ID_USER_GUIDE, "Stalagmight Help", true, None::<&str>)?,
        &MenuItem::with_id(app, ID_SHORTCUTS, "Keyboard Shortcuts", true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(
            app,
            ID_REPORT_ISSUE,
            "Report an Issue...",
            true,
            None::<&str>,
        )?,
    ])?;
    Ok(menu)
}

/// Handles clicks on the entries `build` adds. Every other menu event (the
/// predefined items) is handled by the OS and ignored here.
pub(crate) fn on_event<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    match event.id().as_ref() {
        ID_USER_GUIDE => open_in_browser(app, USER_GUIDE_URL),
        ID_REPORT_ISSUE => open_in_browser(app, REPORT_ISSUE_URL),
        ID_SHORTCUTS => {
            if let Err(error) = app.emit(SHOW_SHORTCUTS_EVENT, ()) {
                tracing::warn!(%error, "Could not ask the window to show keyboard shortcuts");
            }
        }
        _ => {}
    }
}

fn open_in_browser<R: Runtime>(app: &AppHandle<R>, url: &str) {
    if let Err(error) = app.opener().open_url(url, None::<&str>) {
        tracing::warn!(%error, url, "Could not open a Help menu link");
    }
}
