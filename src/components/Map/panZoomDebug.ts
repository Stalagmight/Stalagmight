// Optional MapLibre / handler instrumentation for the macOS lockup
// investigation. Off by default - installPanZoomDebug() early-returns unless
// the build is run with VITE_DEBUG_MACOS_LOCKUP=true. See
// docs/debugging-macos-pan-zoom.md for the protocol this enables.

import type * as maplibregl from 'maplibre-gl';

const ENABLED = import.meta.env.VITE_DEBUG_MACOS_LOCKUP === 'true';

type RefLike<T> = { current: T };

interface DebugRefs {
  customPanActive: RefLike<boolean>;
  ctrlPanActive: RefLike<boolean>;
  placementJustConfirmed: RefLike<boolean>;
  ctrlPanMouseEventTime: RefLike<number>;
}

const HANDLER_NAMES = [
  'dragPan',
  'scrollZoom',
  'boxZoom',
  'dragRotate',
  'touchZoomRotate',
  'keyboard',
  'doubleClickZoom',
  'touchPitch',
] as const;

const LIFECYCLE_EVENTS = [
  'movestart',
  'moveend',
  'zoomstart',
  'zoomend',
  'rotatestart',
  'rotateend',
  'dragstart',
  'dragend',
  'boxzoomstart',
  'boxzoomend',
  'boxzoomcancel',
] as const;

const RAW_EVENTS = [
  'pointerdown',
  'pointerup',
  'pointercancel',
  'wheel',
  'gesturestart',
  'gesturechange',
  'gestureend',
] as const;

// Walk an instance + its prototype chain, calling cb for every (owner, name, fn)
// where fn is a function. Stops at Object.prototype.
function forEachMethod(
  obj: unknown,
  cb: (owner: object, name: string, fn: (...args: unknown[]) => unknown) => void,
): void {
  if (!obj || typeof obj !== 'object') return;
  const visited = new Set<string>();
  let cur: object | null = obj as object;
  while (cur && cur !== Object.prototype) {
    for (const name of Object.getOwnPropertyNames(cur)) {
      if (visited.has(name) || name === 'constructor') continue;
      visited.add(name);
      try {
        const v = (obj as Record<string, unknown>)[name];
        if (typeof v === 'function') {
          cb(obj as object, name, v as (...args: unknown[]) => unknown);
        }
      } catch {
        // Some getters throw when accessed off-instance - ignore.
      }
    }
    cur = Object.getPrototypeOf(cur);
  }
}

// Known "currently running" flag names across MapLibre / TaskQueue variants.
// On catching a re-entry throw we reset any of these that exist on the owner,
// since the throw means a previous run() leaked the flag.
const RUNNING_FLAG_CANDIDATES = [
  '_currentlyRunning',
  '_running',
  'running',
  '_isRunning',
  '_inRun',
  '_renderInProgress',
];

// Snapshot primitive-typed own properties of an object so we can see which
// flag is leaked.
function snapshotPrimitives(owner: object): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const k of Object.keys(owner)) {
    try {
      const v = (owner as Record<string, unknown>)[k];
      const t = typeof v;
      if (t === 'boolean' || t === 'number' || t === 'string' || v == null) {
        out[k] = v;
      } else if (Array.isArray(v)) {
        out[k] = `Array(${v.length})`;
      } else if (t === 'object') {
        out[k] = `<${(v as { constructor?: { name?: string } })?.constructor?.name ?? 'object'}>`;
      }
    } catch {
      // Ignore getter throws.
    }
  }
  return out;
}

function installSwallowForRunThrow(
  map: maplibregl.Map,
  log: (msg: string) => void,
): void {
  const NEEDLE = 'already running';
  const wrapped: string[] = [];

  const wrapOne = (owner: object, name: string, label: string): boolean => {
    const fn = (owner as Record<string, unknown>)[name] as
      | ((...args: unknown[]) => unknown)
      | undefined;
    if (typeof fn !== 'function') return false;
    if ((fn as { __debugWrapped?: boolean }).__debugWrapped) return false;
    let src = '';
    try { src = fn.toString(); } catch { return false; }
    if (!src.includes(NEEDLE)) return false;

    const orig = fn.bind(owner);
    const wrapper = function (this: unknown, ...args: unknown[]): unknown {
      try {
        return orig(...args);
      } catch (e: unknown) {
        const msg = (e as { message?: string })?.message ?? String(e);
        if (!msg.includes(NEEDLE)) throw e;

        // The "already running" guard fired. The previous call leaked its
        // running flag (TaskQueue.run has no try/finally, so any throwing
        // callback in the queue leaks the flag permanently).
        const before = snapshotPrimitives(owner);
        const reset: string[] = [];
        for (const k of RUNNING_FLAG_CANDIDATES) {
          if (k in (owner as object)) {
            try {
              (owner as Record<string, unknown>)[k] = false;
              reset.push(k);
            } catch {
              // Ignore non-writable.
            }
          }
        }
        log(
          `[swallow] ${label}.${name} re-entry caught; owner state was ${JSON.stringify(before)}; reset ${reset.length ? reset.join(',') : 'NONE'}`,
        );
        if (reset.length === 0) {
          log(`[swallow] no known running-flag found to reset; tell me which property on ${label} should be false`);
          return undefined;
        }
        try {
          return orig(...args);
        } catch (e2: unknown) {
          log(
            `[swallow] ${label}.${name} retry after reset failed: ${String((e2 as { message?: string })?.message ?? e2)}`,
          );
          return undefined;
        }
      }
    };
    (wrapper as { __debugWrapped?: boolean }).__debugWrapped = true;
    // Define on the instance, shadowing the prototype method if needed.
    try {
      (owner as Record<string, unknown>)[name] = wrapper;
      wrapped.push(`${label}.${name}`);
      return true;
    } catch {
      return false;
    }
  };

  // Roots to search: map itself, plus every object-valued own property of the
  // map instance one level deep. This auto-discovers internals like
  // _renderTaskQueue, _handlerManager, handlers, painter, etc. without
  // needing to hard-code names per MapLibre version.
  const roots: Array<[unknown, string]> = [[map, '__map']];
  const seenRoot = new WeakSet<object>([map as object]);
  for (const k of Object.getOwnPropertyNames(map)) {
    try {
      const v = (map as unknown as Record<string, unknown>)[k];
      if (v && typeof v === 'object' && !seenRoot.has(v as object)) {
        roots.push([v, `__map.${k}`]);
        seenRoot.add(v as object);
      }
    } catch {
      // Skip getters that throw on access.
    }
  }

  for (const [root, label] of roots) {
    forEachMethod(root, (owner, name) => {
      wrapOne(owner, name, label);
    });
  }

  if (wrapped.length === 0) {
    log(
      '[swallow] NO method containing "already running" found on map/handlers/subsystems - re-entry throws WILL still escape and kill rAF',
    );
  } else {
    log(`[swallow] wrapped ${wrapped.length} re-entry-throwing method(s): ${wrapped.join(', ')}`);
  }
}

export function installPanZoomDebug(map: maplibregl.Map, refs: DebugRefs): void {
  if (!ENABLED) return;

  const w = window as unknown as Record<string, unknown>;
  w.__map = map;
  w.__panRefs = refs;

  const log = (...args: unknown[]) => console.log('[pan-zoom-debug]', ...args);
  const shortStack = () =>
    new Error().stack?.split('\n').slice(2, 5).map((s) => s.trim()).join(' <- ') ?? '';

  log('install: map created, instrumentation active');

  for (const name of HANDLER_NAMES) {
    const h = (map as unknown as Record<string, { enable?: () => void; disable?: () => void; isEnabled?: () => boolean }>)[name];
    if (!h || typeof h.enable !== 'function' || typeof h.disable !== 'function') continue;
    const origEnable = h.enable.bind(h);
    const origDisable = h.disable.bind(h);
    h.enable = () => {
      log(`${name}.enable()  was=${h.isEnabled?.()}  ${shortStack()}`);
      return origEnable();
    };
    h.disable = () => {
      log(`${name}.disable() was=${h.isEnabled?.()}  ${shortStack()}`);
      return origDisable();
    };
  }

  // Catch the MapLibre "Attempting to run(), but is already running." re-entry
  // throw so it does not escape up the rAF callback and kill the render loop.
  // The exact owner/method varies by MapLibre version, so we discover it: walk
  // own properties and prototype methods of map and its handler subsystems,
  // and wrap any function whose source contains the literal error message.
  installSwallowForRunThrow(map, log);

  for (const ev of LIFECYCLE_EVENTS) {
    map.on(ev as never, (e: unknown) => {
      const ee = e as { originalEvent?: { type?: string; button?: number } } | undefined;
      log(`map.${ev}`, {
        origType: ee?.originalEvent?.type,
        button: ee?.originalEvent?.button,
      });
    });
  }

  const canvas = map.getCanvas();
  for (const t of RAW_EVENTS) {
    canvas.addEventListener(
      t,
      (e: Event) => {
        const me = e as Partial<MouseEvent & WheelEvent & PointerEvent>;
        log(`canvas.${t}`, {
          button: me.button,
          buttons: me.buttons,
          ctrl: me.ctrlKey,
          meta: me.metaKey,
          pointerType: (me as PointerEvent).pointerType,
          deltaY: me.deltaY,
          defaultPrevented: e.defaultPrevented,
        });
      },
      { capture: true, passive: true },
    );
  }

  w.__debugSnapshot = () => {
    const handlerEnabled = Object.fromEntries(
      HANDLER_NAMES.map((n) => {
        const h = (map as unknown as Record<string, { isEnabled?: () => boolean }>)[n];
        return [n, h?.isEnabled?.()];
      }),
    );
    // Dump primitive-typed own properties of HandlerManager so we can see
    // its internal flags (e.g. _running, _started, _changes pending, etc.)
    // without knowing their names in advance.
    const hmObj = (map._handlers as unknown as Record<string, unknown> | undefined) ?? {};
    const hmInternals: Record<string, unknown> = {};
    for (const k of Object.keys(hmObj)) {
      const v = (hmObj as Record<string, unknown>)[k];
      const t = typeof v;
      if (t === 'boolean' || t === 'number' || t === 'string' || v == null) {
        hmInternals[k] = v;
      } else if (Array.isArray(v)) {
        hmInternals[k] = `Array(${v.length})`;
      } else if (t === 'object') {
        hmInternals[k] = `<${(v as { constructor?: { name?: string } })?.constructor?.name ?? 'object'}>`;
      } else if (t === 'function') {
        hmInternals[k] = '<fn>';
      }
    }
    // Internal flag candidates on the Map itself that may indicate a wedged
    // render loop.
    const mapPrivate = map as unknown as Record<string, unknown>;
    const mapInternals: Record<string, unknown> = {};
    for (const k of ['_loaded', '_styleDirty', '_sourcesDirty', '_repaint', '_frame', '_rendering', '_idleTriggered', '_painter']) {
      const v = mapPrivate[k];
      const t = typeof v;
      mapInternals[k] = (t === 'boolean' || t === 'number' || t === 'string' || v == null)
        ? v
        : `<${(v as { constructor?: { name?: string } })?.constructor?.name ?? t}>`;
    }
    const snapshot = {
      handlerEnabled,
      isMoving: map.isMoving(),
      isZooming: map.isZooming(),
      isRotating: map.isRotating(),
      zoom: map.getZoom(),
      center: map.getCenter().toArray(),
      cursor: map.getCanvas().style.cursor,
      canvasTouchAction: getComputedStyle(map.getCanvas()).touchAction,
      hmInternals,
      mapInternals,
      refs: {
        customPanActive: refs.customPanActive.current,
        ctrlPanActive: refs.ctrlPanActive.current,
        placementJustConfirmed: refs.placementJustConfirmed.current,
        ctrlPanMouseEventTime: refs.ctrlPanMouseEventTime.current,
        msSinceCtrlPanEvent: Date.now() - refs.ctrlPanMouseEventTime.current,
      },
    };
    console.table(handlerEnabled);
    console.log('[pan-zoom-debug] snapshot', JSON.stringify(snapshot, null, 2));
    return snapshot;
  };

  log('install: call window.__debugSnapshot() when reproducing the bug');
}
