//! [MAC-CMDQ-D1] The macOS application menu, with a Quit that goes through the
//! app's exit guards.
//!
//! When an app sets no menu, Tauri installs a default one on macOS (tauri 2.9.5
//! `app.rs:2097-2102`, `menu/menu.rs:150-249`), and its Quit is
//! `PredefinedMenuItem::quit`. muda gives that item the `terminate:` action
//! (muda 0.17.1 `platform_impl/macos/mod.rs:911`), and tao implements no
//! `applicationShouldTerminate:`, so Cmd+Q ended the process through
//! `applicationWillTerminate:` and `RunEvent::Exit` alone: no window close request
//! and no `RunEvent::ExitRequested`. The frontend's close guard (the
//! unsaved-changes prompt, the SpeleoDB quit prompt, the wait for queued edits,
//! the hold during an update install) never ran, and neither did the exit-time
//! lock release.
//!
//! [`build_app_menu`] builds that default menu item for item (the layout is
//! [`MACOS_APP_MENU`]) with one change: Quit is an ordinary item with the same
//! label and Cmd+Q, and [`on_menu_event`] answers it by asking the main window to
//! close ([`request_guarded_quit`]). `Window::close` emits `CloseRequested` like
//! the window's own close button, which `run()` prevents and hands to the
//! frontend, so Cmd+Q and the close button run the same guard.
//!
//! `run()` sets the menu on macOS only: Tauri sets no menu on Linux or Windows,
//! and one there would add a menu bar to the window. The code itself is
//! platform-neutral, so it compiles and is tested on every platform.
//!
//! The Dock's Quit and a logout, restart or shutdown still send `terminate:`, so
//! they still skip the guard. `RunEvent::Exit` releases a held lock for them
//! (`crate::on_run_event`). See docs/macos-build.md.

use tauri::menu::{
    AboutMetadata, Menu, MenuEvent, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu,
    HELP_SUBMENU_ID, WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Manager, Runtime};

/// The id of the guarded Quit item.
pub(crate) const QUIT_MENU_ID: &str = "stalagmight-quit";

/// Cmd+Q on macOS, where `CmdOrCtrl` is Cmd: the key equivalent of the
/// predefined Quit this item replaces (muda 0.17.1 `items/predefined.rs:344`).
/// Tauri drops an accelerator string it cannot parse without an error
/// (`menu/normal.rs:34`), so a test pins that this one parses to that key.
pub(crate) const QUIT_ACCELERATOR: &str = "CmdOrCtrl+Q";

/// The window whose close button the guard answers, and where its prompts open.
const MAIN_WINDOW: &str = "main";

/// One item of the app menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    About,
    Separator,
    Services,
    Hide,
    HideOthers,
    /// The guarded Quit: an ordinary item, NOT `PredefinedMenuItem::quit`.
    Quit,
    CloseWindow,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Fullscreen,
    Minimize,
    Maximize,
}

/// A submenu's title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Title {
    /// The product name, which is how Tauri titles the application submenu.
    AppName,
    Text(&'static str),
}

/// One submenu of the app menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubmenuSpec {
    /// Tauri's `WINDOW_SUBMENU_ID` or `HELP_SUBMENU_ID`, which make AppKit use
    /// the submenu as the Window or Help menu (tauri 2.9.5 `app.rs:2341-2349`).
    pub(crate) id: Option<&'static str>,
    pub(crate) title: Title,
    pub(crate) items: &'static [Item],
}

/// Tauri 2.9.5's default macOS menu (`menu/menu.rs:150-249`, the items its
/// `cfg(target_os = "macos")` arms keep), with [`Item::Quit`] in place of
/// `PredefinedMenuItem::quit`. Every other item stays predefined, so AppKit
/// handles it as before: Copy and Paste send `copy:` and `paste:` to the
/// focused text field, Close Window sends `performClose:`, and so on.
pub(crate) const MACOS_APP_MENU: [SubmenuSpec; 6] = [
    SubmenuSpec {
        id: None,
        title: Title::AppName,
        items: &[
            Item::About,
            Item::Separator,
            Item::Services,
            Item::Separator,
            Item::Hide,
            Item::HideOthers,
            Item::Separator,
            Item::Quit,
        ],
    },
    SubmenuSpec {
        id: None,
        title: Title::Text("File"),
        items: &[Item::CloseWindow],
    },
    SubmenuSpec {
        id: None,
        title: Title::Text("Edit"),
        items: &[
            Item::Undo,
            Item::Redo,
            Item::Separator,
            Item::Cut,
            Item::Copy,
            Item::Paste,
            Item::SelectAll,
        ],
    },
    SubmenuSpec {
        id: None,
        title: Title::Text("View"),
        items: &[Item::Fullscreen],
    },
    SubmenuSpec {
        id: Some(WINDOW_SUBMENU_ID),
        title: Title::Text("Window"),
        items: &[
            Item::Minimize,
            Item::Maximize,
            Item::Separator,
            Item::CloseWindow,
        ],
    },
    SubmenuSpec {
        id: Some(HELP_SUBMENU_ID),
        title: Title::Text("Help"),
        items: &[],
    },
];

/// Build [`MACOS_APP_MENU`] for `app`.
pub(crate) fn build_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for spec in &MACOS_APP_MENU {
        let title = match spec.title {
            Title::AppName => app.package_info().name.clone(),
            Title::Text(text) => text.to_owned(),
        };
        let submenu = match spec.id {
            Some(id) => Submenu::with_id(app, id, title, true)?,
            None => Submenu::new(app, title, true)?,
        };
        for item in spec.items {
            submenu.append(&build_item(app, *item)?)?;
        }
        menu.append(&submenu)?;
    }
    Ok(menu)
}

fn build_item<R: Runtime>(app: &AppHandle<R>, item: Item) -> tauri::Result<MenuItemKind<R>> {
    let predefined = match item {
        Item::Quit => {
            // The label muda gives the predefined Quit on macOS ("Quit" and the
            // app's name, `platform_impl/macos/mod.rs:315`).
            let label = format!("Quit {}", app.package_info().name);
            return Ok(MenuItemKind::MenuItem(MenuItem::with_id(
                app,
                QUIT_MENU_ID,
                label,
                true,
                Some(QUIT_ACCELERATOR),
            )?));
        }
        Item::About => PredefinedMenuItem::about(app, None, Some(about_metadata(app)))?,
        Item::Separator => PredefinedMenuItem::separator(app)?,
        Item::Services => PredefinedMenuItem::services(app, None)?,
        Item::Hide => PredefinedMenuItem::hide(app, None)?,
        Item::HideOthers => PredefinedMenuItem::hide_others(app, None)?,
        Item::CloseWindow => PredefinedMenuItem::close_window(app, None)?,
        Item::Undo => PredefinedMenuItem::undo(app, None)?,
        Item::Redo => PredefinedMenuItem::redo(app, None)?,
        Item::Cut => PredefinedMenuItem::cut(app, None)?,
        Item::Copy => PredefinedMenuItem::copy(app, None)?,
        Item::Paste => PredefinedMenuItem::paste(app, None)?,
        Item::SelectAll => PredefinedMenuItem::select_all(app, None)?,
        Item::Fullscreen => PredefinedMenuItem::fullscreen(app, None)?,
        Item::Minimize => PredefinedMenuItem::minimize(app, None)?,
        Item::Maximize => PredefinedMenuItem::maximize(app, None)?,
    };
    Ok(MenuItemKind::Predefined(predefined))
}

/// The About panel's details, as Tauri's default menu fills them in
/// (`menu/menu.rs:151-159`).
fn about_metadata<R: Runtime>(app: &AppHandle<R>) -> AboutMetadata<'static> {
    let package = app.package_info();
    let bundle = &app.config().bundle;
    AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        copyright: bundle.copyright.clone(),
        authors: bundle.publisher.clone().map(|publisher| vec![publisher]),
        ..Default::default()
    }
}

/// The app menu's handler. Only Quit reaches it: every other item is
/// predefined, and AppKit acts on those itself.
pub(crate) fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    if event.id() == QUIT_MENU_ID {
        request_guarded_quit(app);
    }
}

/// Ask the main window to close, as its close button does. `Window::close`
/// emits `CloseRequested` first (tauri 2.9.5 `window/mod.rs:1779`); `run()`
/// prevents it and hands it to the frontend's close guard, which either closes
/// the window itself (the app then exits through `RunEvent::ExitRequested`) or
/// keeps it open behind a prompt.
///
/// The window is raised first. A prompt the guard opens in a minimized window
/// is one the user cannot see, and Cmd+Q would look like it did nothing.
pub(crate) fn request_guarded_quit<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        // No window means no frontend and no guard to run. Exit the ordinary
        // way, which still releases a held lock (`RunEvent::ExitRequested`).
        tracing::warn!(
            purpose = "quit from the app menu",
            "No main window to ask; exiting without the close guard"
        );
        app.exit(0);
        return;
    };
    if let Err(e) = window.unminimize() {
        tracing::warn!(
            error = %e,
            purpose = "show the quit prompt",
            "Could not unminimize the main window"
        );
    }
    if let Err(e) = window.set_focus() {
        tracing::warn!(
            error = %e,
            purpose = "show the quit prompt",
            "Could not focus the main window"
        );
    }
    if let Err(e) = window.close() {
        tracing::error!(
            error = %e,
            purpose = "quit from the app menu",
            "Could not ask the main window to close"
        );
    }
}
