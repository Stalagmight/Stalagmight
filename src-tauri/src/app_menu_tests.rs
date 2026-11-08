//! [MAC-CMDQ-D1] The app menu: Tauri's default macOS menu, with a Quit that asks
//! the main window to close instead of terminating the process.
//!
//! Driven on the `tauri::test` mock runtime, so it runs on every platform. What
//! only a macOS build can show (that AppKit gives Cmd+Q to this item, that Copy
//! and Paste reach a focused text field, the prompts themselves) is on the
//! checklist in docs/macos-build.md.

use std::sync::mpsc;

use muda::accelerator::{Accelerator, Code, CMD_OR_CTRL};
use tauri::menu::{MenuEvent, MenuId, MenuItemKind, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID};
use tauri::test::{mock_app, MockRuntime};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::app_menu::{
    build_app_menu, on_menu_event, Item, SubmenuSpec, Title, MACOS_APP_MENU, QUIT_ACCELERATOR,
    QUIT_MENU_ID,
};

/// The macOS branch of Tauri 2.9.5's `Menu::default` (`menu/menu.rs:150-249`),
/// written out item for item, with the one change: `Item::Quit` where it has
/// `PredefinedMenuItem::quit`. Recheck it against that function when Tauri moves.
#[test]
fn the_layout_is_the_default_macos_menu_with_only_quit_replaced() {
    use Item::{
        About, CloseWindow, Copy, Cut, Fullscreen, Hide, HideOthers, Maximize, Minimize, Paste,
        Quit, Redo, SelectAll, Separator, Services, Undo,
    };
    let tauri_default_with_guarded_quit = [
        SubmenuSpec {
            id: None,
            title: Title::AppName,
            items: &[
                About, Separator, Services, Separator, Hide, HideOthers, Separator, Quit,
            ],
        },
        SubmenuSpec {
            id: None,
            title: Title::Text("File"),
            items: &[CloseWindow],
        },
        SubmenuSpec {
            id: None,
            title: Title::Text("Edit"),
            items: &[Undo, Redo, Separator, Cut, Copy, Paste, SelectAll],
        },
        SubmenuSpec {
            id: None,
            title: Title::Text("View"),
            items: &[Fullscreen],
        },
        SubmenuSpec {
            id: Some(WINDOW_SUBMENU_ID),
            title: Title::Text("Window"),
            items: &[Minimize, Maximize, Separator, CloseWindow],
        },
        SubmenuSpec {
            id: Some(HELP_SUBMENU_ID),
            title: Title::Text("Help"),
            items: &[],
        },
    ];
    let quits = MACOS_APP_MENU
        .iter()
        .flat_map(|spec| spec.items)
        .filter(|item| **item == Quit)
        .count();
    println!("[MAC-CMDQ-D1] app menu layout: {MACOS_APP_MENU:#?}");
    assert_eq!(MACOS_APP_MENU, tauri_default_with_guarded_quit);
    assert_eq!(quits, 1, "one Quit, at the end of the application submenu");
}

fn kind_name(kind: &MenuItemKind<MockRuntime>) -> &'static str {
    match kind {
        MenuItemKind::MenuItem(_) => "MenuItem",
        MenuItemKind::Submenu(_) => "Submenu",
        MenuItemKind::Predefined(_) => "Predefined",
        MenuItemKind::Check(_) => "Check",
        MenuItemKind::Icon(_) => "Icon",
    }
}

/// The menu Tauri gets: one submenu per spec entry, with its title and id; the
/// Quit is an ordinary item with this module's id, enabled, with the label the
/// predefined Quit had; every other item is a predefined one (so AppKit keeps
/// handling Copy, Paste and the rest), and none of them is a Quit.
#[test]
fn the_built_menu_has_the_guarded_quit_and_keeps_every_predefined_item() {
    let app = mock_app();
    let menu = build_app_menu(app.handle()).expect("build the app menu");
    let submenus = menu.items().expect("the menu's submenus");
    assert_eq!(submenus.len(), MACOS_APP_MENU.len());

    let mut quit = None;
    let mut predefined_labels = Vec::new();
    for (built, spec) in submenus.iter().zip(&MACOS_APP_MENU) {
        let MenuItemKind::Submenu(submenu) = built else {
            panic!("a top-level item is a {}, not a submenu", kind_name(built));
        };
        let title = submenu.text().expect("submenu title");
        let expected_title = match spec.title {
            Title::AppName => app.package_info().name.clone(),
            Title::Text(text) => text.to_owned(),
        };
        assert_eq!(title, expected_title);
        if let Some(id) = spec.id {
            assert!(submenu.id() == id, "{title}: id {:?}", submenu.id());
        }
        let items = submenu.items().expect("submenu items");
        assert_eq!(items.len(), spec.items.len(), "{title}");
        let mut labels = Vec::new();
        for (built, wanted) in items.iter().zip(spec.items) {
            match (wanted, built) {
                (Item::Quit, MenuItemKind::MenuItem(item)) => {
                    quit = Some((
                        title.clone(),
                        item.id().clone(),
                        item.text().expect("quit label"),
                        item.is_enabled().expect("quit enabled"),
                    ));
                }
                (wanted, MenuItemKind::Predefined(item)) if *wanted != Item::Quit => {
                    // muda marks mnemonics with `&` on Linux and Windows and strips
                    // them on macOS.
                    labels.push(item.text().expect("label").replace('&', ""));
                }
                (wanted, built) => {
                    panic!("{title}: {wanted:?} was built as a {}", kind_name(built));
                }
            }
        }
        println!("[MAC-CMDQ-D1] {title}: predefined {labels:?}");
        predefined_labels.push((title, labels));
    }
    println!("[MAC-CMDQ-D1] quit item (submenu, id, label, enabled): {quit:?}");

    let (submenu, id, label, enabled) = quit.expect("a Quit item was built");
    assert_eq!(
        submenu,
        app.package_info().name,
        "Quit is in the application submenu"
    );
    assert!(id == QUIT_MENU_ID, "{id:?}");
    assert_eq!(label, format!("Quit {}", app.package_info().name));
    assert!(enabled);
    let edit = predefined_labels
        .iter()
        .find(|(title, _)| title == "Edit")
        .map(|(_, labels)| labels.as_slice());
    assert_eq!(
        edit,
        Some(&["Undo", "Redo", "", "Cut", "Copy", "Paste", "Select All"].map(String::from)[..]),
        "the Edit menu keeps the predefined clipboard items"
    );
    assert!(
        predefined_labels
            .iter()
            .flat_map(|(_, labels)| labels)
            .all(|label| !label.starts_with("Quit") && label != "Exit"),
        "no predefined Quit is left: {predefined_labels:?}"
    );
}

/// Tauri drops an accelerator it cannot parse without a word, which would leave
/// Quit with no key at all. This one parses to the key the predefined Quit had:
/// `CMD_OR_CTRL` + Q, which is Cmd+Q on macOS (muda `items/predefined.rs:344`).
#[test]
fn the_quit_accelerator_is_the_one_the_predefined_quit_had() {
    let parsed: Result<Accelerator, _> = QUIT_ACCELERATOR.parse();
    println!("[MAC-CMDQ-D1] {QUIT_ACCELERATOR:?} -> {parsed:?}");
    assert_eq!(
        parsed.expect("the accelerator parses"),
        Accelerator::new(Some(CMD_OR_CTRL), Code::KeyQ)
    );
}

/// The menu's Quit, through the mock runtime's run loop: it asks the main window
/// to close, which is the `CloseRequested` its close button sends, and it does
/// not exit on its own. While the guard keeps the window (what `run()` does with
/// every close request, then the frontend's prompt) the app keeps running; once
/// the frontend lets the quit go and destroys the window, the app exits.
#[test]
fn quit_asks_the_main_window_to_close_and_waits_for_the_guard() {
    let app = mock_app();
    let _window = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
        .build()
        .expect("window");
    let (tx, rx) = mpsc::channel::<String>();
    let mut close_requests = 0_u32;
    let mut ticks_since_close = None::<u32>;
    let mut ticks = 0_u32;

    app.run_return(move |handle, event| {
        let record = |what: String| tx.send(what).expect("record");
        match &event {
            RunEvent::Ready => {
                record("ready".to_owned());
                on_menu_event(
                    handle,
                    &MenuEvent {
                        id: MenuId::new(QUIT_MENU_ID),
                    },
                );
            }
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                close_requests += 1;
                record(format!("close requested: {label}"));
                // What `run()` does with every close request.
                api.prevent_close();
                ticks_since_close = Some(0);
            }
            RunEvent::MainEventsCleared => {
                ticks += 1;
                if let Some(since) = ticks_since_close.as_mut() {
                    *since += 1;
                    if *since == 2 {
                        // The guard has held the window for two loop turns; now
                        // the frontend lets the quit go.
                        record(format!(
                            "window kept: {}",
                            handle.get_webview_window("main").is_some()
                        ));
                        destroy_main(handle);
                    }
                } else if ticks == 5 {
                    // No close request came: end the loop so the test fails
                    // instead of hanging.
                    record("no close request".to_owned());
                    destroy_main(handle);
                }
            }
            RunEvent::ExitRequested { .. } => record("exit requested".to_owned()),
            RunEvent::Exit => record(format!("exit; close requests {close_requests}")),
            _ => {}
        }
    });

    let seen: Vec<String> = rx.try_iter().collect();
    println!("[MAC-CMDQ-D1] run loop after the menu's Quit: {seen:?}");
    assert_eq!(
        seen,
        [
            "ready",
            "close requested: main",
            "window kept: true",
            "exit requested",
            "exit; close requests 1",
        ]
    );
}

fn destroy_main(handle: &tauri::AppHandle<MockRuntime>) {
    handle
        .get_webview_window("main")
        .expect("the main window")
        .destroy()
        .expect("destroy the window");
}
