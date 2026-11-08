# macOS Build

Two scripts handle building Stalagmight locally on macOS.

## Quick Start

```bash
# One-time setup (idempotent)
bash scripts/mac-setup.sh

# Build + test
bash scripts/mac-build.sh

# Build + test with Playwright UI tests (headless)
bash scripts/mac-build.sh --test-ui

# Build + test with WebDriverIO E2E tests
bash scripts/mac-build.sh --test-e2e
```

Build artifacts land under `src-tauri/target/release/bundle/`.

## Scripts Overview

| Script | Purpose |
|--------|---------|
| `scripts/mac-setup.sh` | Install dev tools on macOS (Xcode CLI, Homebrew, Node from `.nvmrc`, pnpm from `packageManager`, Rust, tauri-wd) |
| `scripts/mac-build.sh` | Run the full build pipeline (deps, tsc, cargo test, tauri build) |

## `mac-setup.sh` - Dev Tool Installer

Installs all required development tools, skipping anything already present. Safe to run repeatedly.

### Tools installed

1. **Xcode Command Line Tools** - headless install via `softwareupdate`
2. **Homebrew** - standard installer if sudo is available, otherwise git-clones to `~/.homebrew`
3. **Node.js** - major read from `.nvmrc` (currently 24), via `brew install node@<major>`, with PATH fix for keg-only installs
4. **pnpm** - exact version read from `packageManager` in `package.json` (currently 11.18.0, integrity-pinned), via `npm install -g pnpm@<version>`, with CA cert fix for Homebrew's OpenSSL. An existing install is accepted if its major matches, since any pnpm 10+ self-switches to the pin
5. **Rust stable** - via rustup
6. **tauri-wd** - via `cargo install --git https://github.com/FumblingBarbell/tauri-webdriver --branch fix/resolve-element-refs-in-execute-sync tauri-webdriver-automation` (WebDriver for macOS E2E tests; a fix branch until the change lands in a crates.io release)

### Homebrew PATH detection

The script checks three Homebrew locations in order:

| Location | When used |
|----------|-----------|
| `/opt/homebrew/bin/brew` | Standard Apple Silicon install |
| `/usr/local/bin/brew` | Standard Intel install |
| `~/.homebrew/bin/brew` | No-sudo fallback install |

## `mac-build.sh` - Build Pipeline

Runs the complete build and test pipeline. Assumes `mac-setup.sh` has already been run.

### Options

| Flag | Description |
|------|-------------|
| `--project-dir <path>` | Override project root (defaults to parent of `scripts/`) |
| `--test-ui` | Install Playwright browsers + run headless UI tests |
| `--test-ui-headed` | Install Playwright browsers + run headed UI tests (needs display) |
| `--test-e2e` | Build debug binary with e2e feature + run WebDriverIO E2E tests |

### Build steps

1. `pnpm install` - install frontend dependencies
2. `pnpm tsc --noEmit` - TypeScript type check
3. `playwright install chromium` - install Playwright browsers *(only with `--test-ui*`)*
4. `pnpm test:ui` / `pnpm test:ui:headed` - Playwright tests *(only with `--test-ui*`)*
5. `cargo test` - Rust test suite (with `ulimit -n 4096`)
5a. `pnpm tauri build --debug --no-bundle -- --features e2e` - E2E debug binary *(only with `--test-e2e`)*
5b. `pnpm agent:journeys` - agent-harness journeys *(only with `--test-e2e`; the WDIO suite was retired in #549)*
6. `CI=true pnpm tauri build` - production build (`.app` bundle + `.dmg`)
6a. `./scripts/verify-macos-signature.sh` - checks the code signature of the app
    *inside* the built DMG, and **fails the build** if it is invalid. Tauri signs
    the app itself (ad-hoc, from `bundle.macOS.signingIdentity`), so this only
    verifies; it does not sign. An invalid signature is not a cosmetic problem:
    macOS reports such a bundle as "damaged" and refuses to open it at all, with
    no way for the user to click past it
    ([#350](https://github.com/Stalagmight/Stalagmight/issues/350)). The release
    workflow runs the same script.

### Build artifacts

| Artifact | Location |
|----------|----------|
| `.app` bundle | `src-tauri/target/release/bundle/macos/` |
| `.dmg` installer | `src-tauri/target/release/bundle/dmg/` |

### Playwright Tests

The `--test-ui` and `--test-ui-headed` flags add Playwright browser testing to the build pipeline. Only Chromium is installed (per `playwright.config.ts`).

| Mode | Flag | Display needed? | Notes |
|------|------|-----------------|-------|
| Headless | `--test-ui` | No | Works over SSH with no display server |
| Headed | `--test-ui-headed` | Yes | Requires a GUI session (e.g. a local display or a VNC session over SSH) |

Test results (screenshots, traces on failure) are saved to `test-results/`.

### WebDriverIO Tests

The `--test-e2e` flag runs WebDriverIO E2E tests against the Tauri app. On macOS, this uses `tauri-wd` (from the `tauri-webdriver-automation` crate) instead of `tauri-driver`, which only supports Linux/Windows.

#### How it works

1. The app is built with `--features e2e`, which compiles in `tauri-plugin-webdriver-automation` - a Tauri plugin that exposes W3C WebDriver protocol for WKWebView
2. `tauri-wd` is spawned as the WebDriver server (instead of `tauri-driver`)
3. WebDriverIO connects to `tauri-wd` and runs tests as normal

The `e2e` feature is gated behind a Cargo feature flag (`[features] e2e = [...]` in `Cargo.toml`), so it is **never compiled** in normal or release builds - zero impact on the production binary.

#### Prerequisites

- `tauri-wd` must be installed from the pinned fix branch (step 6 of `mac-setup.sh` above; the pinned command lives there)

#### Artifacts

| Artifact | Location |
|----------|----------|
| Journey logs + screenshots | `.agent-artifacts/sessions/<id>/` |

## App menu and Cmd+Q

Decision MAC-CMDQ-D1: Cmd+Q goes through the same exit guards as the window's
close button.

### Why the app sets its own menu

When an app sets no menu, Tauri installs a default one on macOS (tauri 2.9.5
`app.rs:2097-2102`, `menu/menu.rs:150-249`). Its Quit is a predefined item that
sends `terminate:` to `NSApplication`. tao implements no
`applicationShouldTerminate:`, so the process ended through
`applicationWillTerminate:` and `RunEvent::Exit` alone. There was no window close
request and no `RunEvent::ExitRequested`, so Cmd+Q skipped:

- the frontend's close guard (`useBeforeUnload` and `guardQuit`): the
  unsaved-changes prompt, the SpeleoDB quit prompt, the wait for queued edits,
  and the hold while an update installs;
- the exit-time SpeleoDB lock release, which ran on `ExitRequested` only.

### What the app does now

- `src-tauri/src/app_menu.rs` builds Tauri's default macOS menu item for item
  (application, File, Edit, View, Window and Help), with one change. Quit is an
  ordinary menu item with the same label ("Quit Stalagmight") and the same key
  (Cmd+Q). Every other item stays predefined, so AppKit still handles Copy,
  Paste, Hide, Close Window and the rest itself.
- Choosing Quit raises the main window and asks it to close (`Window::close`).
  That sends the same `CloseRequested` as the close button, which `run()`
  prevents and hands to the frontend. The guard then decides, exactly as for a
  click on the close button: it quits, prompts, waits for queued edits, or
  ignores the request while an update installs.
- `run()` sets this menu on macOS only. Tauri sets no menu on Linux or Windows,
  and one there would add a menu bar to the window. The code is
  platform-neutral, so every platform's build compiles it and the tests run on
  Linux CI.
- Backstop: `RunEvent::Exit` also runs the held-lock release
  (`on_run_event` in `src-tauri/src/lib.rs`). It runs once per process
  (`ExitLockRelease`). If `ExitRequested` already released the lock, `Exit`
  does not release it again.

These still send `terminate:`, so they still skip the frontend guard:

- Quit from the Dock icon's menu;
- logout, restart and shutdown;
- a quit Apple event, for example `osascript -e 'quit app "Stalagmight"'`.

For these, the backstop still releases a held SpeleoDB lock (best effort, with
a 5 second limit). Unsaved edits get no prompt. A Force Quit ends the process
before any of this runs.

Cmd+Q now depends on the frontend, as the close button always has. If the
frontend is not responding, Cmd+Q does nothing; the Dock's Quit or Force Quit
still end the app.

When Tauri is upgraded, check `Menu::default` again.
`app_menu_tests.rs` writes out the 2.9.5 default menu, and a new default item
would not be added here automatically.

### Verify on a Mac build

The Linux tests cover the menu's layout, the Quit item's id, label and key, the
close request it sends, and the backstop's single release. What only a macOS
build can show:

1. Cmd+Q with unsaved edits shows the unsaved-changes prompt. Cancel keeps the
   app open.
2. Cmd+Q while holding a SpeleoDB write lock shows the SpeleoDB quit prompt.
3. Cmd+Q with a clean project quits at once, with no prompt and one key press.
4. Choosing "Quit Stalagmight" in the application menu with the mouse does the
   same as Cmd+Q.
5. Minimize the window (Cmd+M) with unsaved edits, then press Cmd+Q. The window
   comes back and shows the prompt.
6. Cmd+Q again while a prompt is open does not open a second prompt.
7. Cmd+Q while an update installs does nothing. The app restarts when the
   install finishes.
8. Copy, Cut, Paste, Select All and Undo work in a text field, with both the
   keys (Cmd+C, Cmd+X, Cmd+V, Cmd+A, Cmd+Z) and the Edit menu.
9. The other menu items behave as before: About, Services, Hide (Cmd+H), Hide
   Others, Close Window (Cmd+W, which goes through the guard as before),
   Minimize, Zoom, Enter Full Screen, the Window menu's window list, and the
   Help menu's search field.
10. Quit from the Dock, and logout, quit with no prompt (expected). With a
    SpeleoDB lock held, the lock is released on the server afterwards.

## Known Issues & Workarounds

| Issue | Cause | Workaround |
|-------|-------|------------|
| No-sudo Homebrew | Some build hosts lack passwordless sudo | `mac-setup.sh` git-clones Homebrew to `~/.homebrew` |
| Node.js not on PATH | `node@<major>` is keg-only in Homebrew | Scripts read the major from `.nvmrc`, then detect and prepend `$(brew --prefix node@<major>)/bin` to PATH |
| npm SSL certificate errors | Homebrew's source-built OpenSSL lacks CA bundle | `mac-setup.sh` sets `npm config set cafile` to Homebrew's `ca-certificates` bundle |
| DMG bundling hangs over SSH | Tauri's DMG bundler uses AppleScript (needs Finder GUI) | `mac-build.sh` sets `CI=true` which skips the AppleScript step |
| macOS `ulimit -n 256` | Default is too low for the Rust test suite | `mac-build.sh` raises to 4096 (falls back to 1024) |
| `directories` crate ignores XDG on macOS | Hardcodes `~/Library/Application Support/` regardless of `XDG_CONFIG_HOME` | The XDG-dependent config/logging tests are gated to `#[cfg(target_os = "linux")]` |
| `pnpm --dir` broken for `exec` | `--dir` flag doesn't propagate to subcommands | Scripts use subshell `(cd $DIR && pnpm ...)` instead |
