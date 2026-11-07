# Tier 2: grant-gated OS input (macOS)

Real trusted input and real pixels, for exactly the class the WebDriver tiers
structurally cannot reach. Design doc: `docs/plans/2026-08-05_macos-agent-harness.md`
section 5.

| | Input grade | Pixels | Permissions | Runs unattended |
|---|---|---|---|---|
| Playwright + mock IPC | synthetic DOM | real (Chromium) | none | yes |
| Tier 1: MacSession | synthetic DOM, real backend | DOM-only | none | yes |
| **Tier 2: os_\* tools** | **real (CGEvent, trusted)** | **real (`screencapture`)** | Accessibility + Screen Recording, per-session grant | **no** |

Tier 1 drives the webview with `dispatchEvent`: `isTrusted: false`, so native
menu accelerators (Cmd+S) never fire, and its screenshots serialize the DOM,
so the survey map renders blank. Tier 2 exists for those two gaps and costs a
human at the keyboard, so reach for it only when the finding depends on real
input or real pixels. **Label every finding with the tier that produced it**: a
Tier 1 pass on a drag or a shortcut is evidence about DOM handlers, not about
real input.

## Layout

```
e2e/agent/os/
  helper/         Rust bin crate "stalagmight-oshelper" (standalone: NOT a
                  src-tauri workspace member, so outside the app's fmt/clippy
                  CI scope; its own path-filtered macOS workflow,
                  .github/workflows/os-helper.yml, runs fmt --check, clippy
                  -D warnings, its tests and a release build)
  helper.ts       ensureHelperBuilt() + runHelper(args) -> parsed JSON
  permissions.ts  per-process grant flag + TCC preflight
  input.ts        osClick / osKeys, the three gates, screen-point resolution
  screenshot.ts   window id by pid -> `screencapture` of that window
```

Every entry point in `input.ts` and `screenshot.ts` takes its helper runner and
its `osascript` / `screencapture` execs as an optional injected `deps`
argument, which is how the unit tests beside them cover the gates without
posting a single real event. `menu.ts` is deferred along with `os_menu`.

## MCP tools

Registered on darwin only, in `e2e/agent/mcp/server.ts`:

| Tool | Does |
|---|---|
| `os_grant` | opens the grant for this MCP server process |
| `os_click` | real CGEvent click, by screen point or by testid |
| `os_keys` | real key event with real modifier flags, so Cmd+S reaches the menu bar |
| `os_screenshot` | `screencapture -x -l <windowid>`: real pixels, occlusion-immune |

- **Grant flow.** `os_grant` is deliberately never added to a tool allowlist, so
  invoking it raises a real harness permission prompt: the human decides that
  this session may drive the desktop. The grant lasts until the MCP server
  exits. See "The grant model" below.
- **Evidence contract.** `frontmost_ok` and `permissions` ride every os_\*
  result. `frontmost_ok: false` means discard the evidence, not investigate the
  app. See "Evidence-quality contract" below. Note the asymmetry: a CAPTURE
  proceeds and labels itself, but INPUT refuses to post at all, because a chord
  sent to a window that stole focus is a real mutation in someone else's app,
  not merely a bad screenshot. Pass `force: true` to override.
- **`os_menu` is DEFERRED.** The helper's `menu` subcommand is a stub that
  returns `unimplemented` naming the AX menu-bar walk it needs, so menu
  accelerators go through `os_keys`.

## Permissions: who grants what

Two TCC permissions, both granted in System Settings > Privacy & Security:

| Permission | Needed by | Silent-failure mode if denied |
|---|---|---|
| Accessibility | `click`, `move`, `drag`, `keys` | events post "successfully" and are dropped; the app looks like it ignored the click |
| Screen Recording | `screencapture` (os_screenshot) | exits 0 and writes a desktop-wallpaper image with no app window in it |

Both failure modes produce evidence that downstream analysis would score as
real, which is why **preflight is mandatory** and rides every os_\* result. The
helper refuses input outright when Accessibility is denied, rather than posting
events into the void.

**The grant belongs to the process that spawns the helper** (your terminal, or
the editor running the MCP server), never to the helper binary and never to the
Stalagmight app: TCC attributes a CLI to its responsible process. So rebuilding
the helper does not invalidate anything, and after granting you must restart the
terminal or editor for it to take effect.

Check the current state at any time (read-only, never prompts):

```bash
cargo run --manifest-path e2e/agent/os/helper/Cargo.toml -- preflight
# {"ok":true,"accessibility":true,"screenRecording":true}
```

## The grant model

`os_grant` flips a flag that lives in the MCP server process until it exits.
It is **deliberately never added to any tool allowlist**, so invoking it always
raises a real harness permission prompt: the human, not the model, decides that
this session may drive the desktop. `requireGrant()` throws an instructive
refusal before anything touches the machine, and a fresh server starts
ungranted. Preflight runs BEFORE the flag is set, so a grant that reports
failure never leaves the gate open.

**The in-process flag is soft, and the real enforcement is not in this repo.**
Nothing here can observe (let alone enforce) a client's permission config: an
MCP client set to auto-approve tools, a fork that edits `permissions.ts`, or a
user who allowlists `mcp__stalagmight-app__*` wholesale all void the prompt,
and the server cannot tell. Treat the un-allowlisted `os_grant` as a convention
that makes consent explicit for a cooperating client, not as a security
boundary. The boundary that does hold is macOS TCC: without Accessibility, the
helper refuses outright and the OS drops the events anyway.

The focus check is re-run **immediately before the event is posted**, not only
on entry: resolving a testid costs a window lookup plus a webview round-trip,
and focus can move during it. Input also requires a live session (`ready` /
`busy`) and a still-running app pid, because a stopped session keeps its
recorder - and therefore remembers a pid the OS may since have reused.

**Blast radius is the whole desktop, not the app window.** CGEvent input goes
wherever focus is, and `os_click {x, y}` is never clamped to the app's bounds -
a granted session can click or type into any window on the screen. Two things
keep that from being an accident rather than a choice: the app must be
frontmost before any input is posted (input ABORTS otherwise; `force: true` is
the explicit opt-out), and the human grants per server process, in person.

Permission (can the machine do this?) and grant (may we do this now?) are
independent locks and both must be open.

## Evidence-quality contract

- **Every os_\* result carries `{frontmost_ok, permissions: {accessibility, screenRecording}}`.**
  `frontmost_ok: false` means *discard the evidence*, not *investigate the app*:
  the click landed in whatever window the human brought forward.
- **At most one activate attempt per action.** Repeatedly stealing focus back
  from a human is always wrong.
- **A hover move precedes every press** (120 ms settle), presses and releases
  are 60 ms apart, and drags interpolate. Toolkits that track enter/leave ignore
  a press with no prior hover, so this is correctness, not politeness.
- **Preflight before capture, every time.** A capture with Screen Recording
  denied is not an error, it is a wrong picture.

## Coordinate spaces

Everything the helper accepts and returns is in **screen points**, top-left
origin of the main display: the space CGEvent posts in and the space
`kCGWindowBounds` reports in. No flip and no scaling is needed between them.

Retina 2x matters only when **interpreting captured pixels**: a `screencapture`
of a 1000x700 point window is a 2000x1400 pixel image. Divide by the backing
scale before turning a pixel coordinate into a click target, never multiply a
point coordinate before posting. That division is only sound because the capture
passes `-o`: with the window shadow included the image would carry transparent
margins the window never occupied, and every pixel-derived point would be off.

Element coordinates come from the app in CSS points via `evalInApp`, so the
composition is: element rect (CSS points, window-relative) + window content
origin (screen points, from `window-id --pid N`) -> screen points.

## The helper binary

One JSON object on stdout per run. Success is `{"ok":true,...}` with exit 0;
every failure, including a usage error, is `{"ok":false,"error":"..."}` with
exit 1. `--help` is the one exception and prints plain text.

| Subcommand | Result |
|---|---|
| `preflight` | `{ok,accessibility,screenRecording}` |
| `click X Y [--right] [--double]` | `{ok,action,x,y,button,clickCount}` |
| `move X Y` | `{ok,action,x,y}` |
| `drag X1 Y1 X2 Y2 [--duration-ms N]` | `{ok,action,from,to,durationMs,steps}` |
| `keys <spec>` | `{ok,action,spec,key,keyCode,modifiers}` |
| `window-id --pid N` | `{ok,windowId,pid,app,bounds:{x,y,w,h}}` |
| `frontmost` | `{ok,pid,app,windowId}` |
| `menu --pid N "File>Save"` | **unimplemented**: returns an error naming the AX menu-bar walk it needs. Use `keys` for menu accelerators. |

Key spec grammar (also in `--help`, and pinned by unit tests next to the
keycode table in `helper/src/keyspec.rs`):

```
[modifier+]... key            case-insensitive, e.g. cmd+s, cmd+shift+z, escape
modifiers  cmd|command|meta|super|win, shift, alt|opt|option, ctrl|control, fn
keys       a-z  0-9  f1-f12
           return|enter, escape|esc, tab, space, delete|backspace, forwarddelete,
           up, down, left, right, home, end, pageup, pagedown, help, keypadenter,
           minus, equal, comma, period, slash, backslash, semicolon, quote,
           leftbracket, rightbracket, grave|backtick
```

`+` is always the separator, so the literal plus key is `shift+equal`. Keycodes
are US-ANSI **physical positions**: on another layout `keys a` presses the key
where a US keyboard has A, not the key that prints "a".

Modifier state rides the key event's flags (what AppKit reads when matching a
menu key equivalent) rather than separate modifier key-downs, so a crash
mid-sequence cannot leave a modifier stuck down.

## Building and testing

`ensureHelperBuilt()` runs `cargo build --release` into the harness cargo target
dir (`CARGO_TARGET_DIR`, defaulting to `~/.cargo_build` like the rest of
`e2e/lib/env.ts`) and stamps the binary mtime beside it, mirroring
`e2e/agent/lib/build.ts`. It rebuilds when a source file, `Cargo.toml` or
`Cargo.lock` is newer than the binary, or when the stamp says something else
rebuilt it. Cargo output goes to stderr, never stdout, because under the MCP
server stdout is the JSON-RPC stream - a line at a time with the checkout folded
to `<repo>` and the home directory to `~`, as the app build's is (#576 W51,
`e2e/agent/lib/foldedLines.ts`).

```bash
cargo test --manifest-path e2e/agent/os/helper/Cargo.toml     # unit tests
STALAGMIGHT_OSHELPER_LIVE=1 cargo test --manifest-path e2e/agent/os/helper/Cargo.toml
```

The unit tests cover the pure layers (key-spec parsing, argument parsing, JSON
shapes) plus `preflight`, which is read-only and never prompts. Tests that need
a real desktop session are gated on `STALAGMIGHT_OSHELPER_LIVE=1` and return
early otherwise, so an unattended run never fails on a machine that is doing
nothing wrong. Nothing in the test suite posts input events: that is verified by
hand at the machine, per the design doc's Tier 2 gate (grant flow, a testid
click toggling real state, Cmd+S firing the real menu accelerator, a screenshot
showing real map pixels, and the revoked-Screen-Recording path failing loudly).
