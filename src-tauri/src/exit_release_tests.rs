//! [MAC-CMDQ-D1] The exit-time lock release runs at every exit stage, once.
//!
//! On macOS `terminate:` (the Dock's Quit, logout, restart, shutdown, and Cmd+Q
//! before the app menu replaced Tauri's predefined Quit) ends the process through
//! `RunEvent::Exit` with no `RunEvent::ExitRequested` before it, and the release
//! used to run on `ExitRequested` only. These tests drive the app's run-loop
//! handler (`on_run_event`) on the `tauri::test` mock runtime: with `Exit` alone,
//! the release runs; with the ordinary `ExitRequested` then `Exit`, it runs once.
//!
//! What they observe is the release itself running (`ExitLockRelease::runs`, a
//! test-only count kept where the release is called). That the call hands back a
//! held lock is pinned in the core crate (`speleodb_release_held_lock_bounded`,
//! `sdb_silent_release_claim_tests.rs`).

use std::sync::mpsc;

use tauri::test::{mock_app, MockRuntime};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

use crate::{on_run_event, ExitLockRelease};
use stalagmight_core::test_fixtures::state_with_project;

/// How many times the exit-time release has run in `app`.
fn releases(app: &AppHandle<MockRuntime>) -> usize {
    app.try_state::<ExitLockRelease>()
        .map_or(0, |latch| latch.runs())
}

/// `terminate:` on macOS: `Exit`, never preceded by `ExitRequested`. The release
/// runs there, and a later exit stage does not run it again.
#[test]
fn an_exit_that_never_asked_first_still_releases_the_held_lock_once() {
    let app = mock_app();
    app.manage(state_with_project("CaveExit"));
    let handle = app.handle();

    let before = releases(handle);
    on_run_event(handle, &RunEvent::Exit);
    let after_exit = releases(handle);
    on_run_event(handle, &RunEvent::Exit);
    let after_second = releases(handle);
    println!(
        "[MAC-CMDQ-D1] Exit with no ExitRequested: releases before {before}, after Exit \
         {after_exit}, after a second Exit {after_second}"
    );
    assert_eq!(before, 0, "premise: nothing has released yet");
    assert_eq!(
        after_exit, 1,
        "Exit alone (the terminate: path) releases the held lock"
    );
    assert_eq!(after_second, 1, "and never twice");
}

/// The ordinary exit, through the mock runtime's own run loop: the last window
/// goes, the runtime asks (`ExitRequested`) and then exits (`Exit`). The release
/// runs at the first and not again at the second.
#[test]
fn an_exit_that_asked_first_releases_once_and_not_again_at_exit() {
    let app = mock_app();
    app.manage(state_with_project("CaveAsked"));
    let _window = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
        .build()
        .expect("window");
    let (tx, rx) = mpsc::channel();

    app.run_return(move |handle, event| {
        let stage = match &event {
            RunEvent::Ready => {
                // What the frontend does once its close guard lets the quit go.
                handle
                    .get_webview_window("main")
                    .expect("the main window")
                    .destroy()
                    .expect("destroy the window");
                "ready"
            }
            RunEvent::ExitRequested { .. } => "exit requested",
            RunEvent::Exit => "exit",
            _ => return,
        };
        on_run_event(handle, &event);
        tx.send((stage, releases(handle)))
            .expect("record the stage");
    });

    let seen: Vec<(&str, usize)> = rx.try_iter().collect();
    println!("[MAC-CMDQ-D1] (stage, releases so far): {seen:?}");
    assert_eq!(
        seen,
        [("ready", 0), ("exit requested", 1), ("exit", 1)],
        "ExitRequested releases; the Exit backstop after it does not release again"
    );
}

/// The updater's Windows installer hand-over releases on its own
/// (`on_before_exit`, `commands::app_update`); an exit stage after it does not
/// release a second time.
#[test]
fn the_exit_backstop_does_not_repeat_the_installer_hand_over_release() {
    let app = mock_app();
    app.manage(state_with_project("CaveInstall"));
    let handle = app.handle();

    crate::release_held_lock_on_exit(handle);
    let after_hand_over = releases(handle);
    on_run_event(handle, &RunEvent::Exit);
    let after_exit = releases(handle);
    println!(
        "[MAC-CMDQ-D1] installer hand-over then Exit: releases {after_hand_over}, then \
         {after_exit}"
    );
    assert_eq!(after_hand_over, 1);
    assert_eq!(after_exit, 1);
}
