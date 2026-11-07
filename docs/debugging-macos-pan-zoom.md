# macOS UI Lockup - Postmortem and Re-enabling the Debug Toolkit

## What happened

On macOS only, after the first interactive action in a project (any tool
click, any zoom, any pan), every subsequent action stopped working until the
project was closed and reopened. Linux was unaffected.

**Root cause:** `SurveyLayer.handleZoomEndTelemetry` /
`handleMoveEndTelemetry` (then in `src/components/Map/SurveyLayer.tsx`, now
`src/components/Map/surveyLayer/navTelemetry.ts`) used
`orig instanceof TouchEvent`. On macOS desktop Safari/WKWebView, `TouchEvent`
is not exposed as a global by default (it's iOS-only unless touch events are
explicitly enabled at the engine level), so the *reference* threw
`ReferenceError: Can't find variable: TouchEvent` before `instanceof` could
evaluate.

These callbacks are wired to MapLibre's `zoomend` / `moveend` events, so the
throw fired on every user-initiated zoom or pan. It propagated up through
MapLibre's dispatch path:
`fire → _afterEase → _stop → TaskQueue.run`. MapLibre's `TaskQueue.run()` has
no `try/finally` around its `_currentlyRunning` flag, so the leaked flag
wedged the queue permanently. From that point on, every subsequent
`TaskQueue.run()` hit the re-entry guard and threw
`"Attempting to run(), but is already running."`, the rAF render loop died,
and even programmatic calls like `__map.panBy()` produced no movement.

**Fix:** guard the references with `typeof TouchEvent !== 'undefined' && ...`.
Two-line change in `src/components/Map/SurveyLayer.tsx` -
`handleZoomEndTelemetry` and `handleMoveEndTelemetry` (today
`trackZoomEndNav` / `trackMoveEndNav` in `surveyLayer/navTelemetry.ts`).

## When to re-enable the debug toolkit

If a similar lockup, render-loop wedge, or "everything stops working after one
action" symptom appears on macOS (or any other platform), this instrumentation
will likely identify it faster than reading code. The same scaffolding works
for IPC saturation, stuck React refs, and uncaught errors thrown inside
MapLibre's event dispatch chain.

The toolkit is inert at runtime by default. To enable it:

```bash
VITE_DEBUG_MACOS_LOCKUP=true pnpm tauri dev
```

(Or set the same env var in `.env.local`.) Vite tree-shakes the gated bodies
out of bundles built without the flag, so there is no production cost.

## What gets enabled

### `src/utils/debugLockup.ts`

- Wraps Tauri IPC calls (via `trackInvoke`, threaded through `invokeIpc` in
  `src/utils/ipc.ts`; `consoleCapture.ts`'s own `push_frontend_log` invoke is
  deliberately left unwrapped - `console.error` is monkey-patched there to
  call it, so tracking that invoke would recurse on IPC errors). Logs every
  call with id, duration, and a running in-flight count. Logs a `HANG`
  warning for any call still pending after 3 s.
- Captures `window.onerror` and `unhandledrejection` with full stack traces.
- Logs every document-level click in capture phase (target, testid, text,
  `defaultPrevented`).
- Subscribes to `useUIStore.activeTool` and logs each transition with a stack.
- Exposes `window.__lockupSnapshot()` and `window.__getActiveTool()`.

### `src/components/Map/panZoomDebug.ts`

- Exposes `window.__map`, `window.__panRefs`.
- Wraps every MapLibre handler subsystem's `enable()`/`disable()` with short
  stack traces.
- Logs MapLibre lifecycle events (`movestart`, `zoomend`, …) and raw canvas
  pointer/wheel/gesture events.
- **Auto-discovers and wraps** any method on the map instance whose source
  contains the string `"already running"` - that's how it found
  `_renderTaskQueue.run` during this investigation, without knowing the
  property name in advance. On catch, the wrap dumps the owner's primitive
  state and force-resets known "currently running" flag names
  (`_currentlyRunning`, `_running`, etc.), then retries. This keeps the
  render loop alive across the kind of leak that bit us here, so the *next*
  throw in the chain becomes visible instead of being masked by the cascade.
- Exposes `window.__debugSnapshot()` for handler-enabled state, internal map
  flags, MapLibre HandlerManager internals, and the custom-pan ref values.

## Diagnostic protocol

With the flag set, reproduce the bug, then in the Web Inspector console:

```js
__lockupSnapshot()
__debugSnapshot()
```

The two snapshots together cover:

| Layer | Where to look |
|---|---|
| IPC saturation | `__lockupSnapshot().inFlightCount` and `inFlightByCommand` |
| MapLibre handler enable-state | `__debugSnapshot().handlerEnabled` |
| MapLibre internal flags (`_running`, `_currentlyRunning`, …) | `__debugSnapshot().hmInternals` |
| Custom-pan refs (`MapCanvas.tsx`) | `__debugSnapshot().refs` |
| Uncaught throws | console - search for `[debug-lockup] window.onerror` |
| Stuck render-loop callbacks | console - search for `[swallow] re-entry caught` |
| Which click → state-change path fired | console - `[debug-lockup] click` then `[debug-lockup] activeTool` |

When `[swallow] re-entry caught` appears, the line includes the owner's full
primitive state at the moment of the throw, and the next throw in the chain
(now no longer cascaded) becomes visible via `window.onerror` - that's how
the `TouchEvent` ReferenceError surfaced once `_currentlyRunning` stopped
leaking.

## Notes for future investigators

- Don't monkey-patch `window.__TAURI_INTERNALS__.invoke` directly. Its
  internals invoke is callback-shaped in Tauri v2; the public `invoke()` in
  `@tauri-apps/api/core` is the Promise-shaped one. Patching at the internals
  level breaks app startup. Use the `trackInvoke(command, promise)`
  pass-through helper from `debugLockup.ts` instead and wire it in at known
  call sites (`src/utils/ipc.ts`) - not `src/utils/consoleCapture.ts`, whose
  `push_frontend_log` invoke must stay unwrapped for the reason above.
- `installConsoleCapture` (in `main.tsx`) pipes every `console.*` call to an
  IPC `push_frontend_log` invoke. If you add new debug logs from within
  debugLockup itself, cache `console.log.bind(console)` *before*
  `installConsoleCapture` runs (which the current code does) - otherwise your
  logs recurse back through the IPC layer you're measuring.
- The lockup symptom can present in three layers: events not reaching the
  canvas (DOM/CSS), state stuck in a React ref or Zustand store, or a leaked
  MapLibre internal flag. Run all three snapshots before forming a
  hypothesis - the lockup we just fixed looked like #2 in the early
  snapshots and only revealed itself as #3 once we dumped MapLibre's
  internals.
