/**
 * The toast stack: bottom-left, newest nearest the bottom edge, older ones
 * pushed upward, capped at whatever fits in 20% of the window.
 *
 * Split of responsibilities with `uiStore`:
 *   - the store owns the DATA (push / de-dup-refresh / evict / dismiss). It runs
 *     no timers, so it stays deterministic and trivial to unit-test.
 *   - this component owns the CLOCK and the GEOMETRY. Pausing is a DOM concern
 *     (hover / focus) and so is "how tall is a toast", neither of which the store
 *     can see, so the dwell-then-fade lifecycle lives here and calls
 *     `removeToast` when a toast's life is over, and the measured capacity is fed
 *     back to the store through `setToastCapacity`.
 *
 * LAYOUT. The column is position-driven, not flex-driven: every toast is
 * absolutely positioned on the same bottom anchor and lifted into its slot with
 * `translateY(-offset)`, where the offset is the summed height of everything
 * newer than it. A flex column cannot animate an insertion or a removal - the
 * siblings jump - whereas a changed transform is transitioned for free, which is
 * what makes the three movements (a new toast rising in, the stack being pushed
 * up to make room, a dying toast drifting up and out) read as motion rather than
 * as a re-layout. Heights are MEASURED, never assumed: a toast wraps to two or
 * three lines depending on the message and the UI scale.
 *
 * Always mounted (App renders it unconditionally) so the screen-reader live
 * regions exist BEFORE their content changes - a live region injected at the same
 * moment as its text is unreliably announced.
 */
import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState } from 'react';
import { useLatest } from '../../hooks/useLatest';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { useUIStore, type ToastItem, type ToastKind } from '../../store/uiStore';
import {
  fitToastCapacity,
  toastStackHeight,
  toastStackOffsets,
  usableToastHeight,
} from '../../utils/toastCapacity';
import { UI_SCALE_EVENT } from '../../utils/uiScale';
import { AlertCircleIcon, AlertTriangleIcon, CheckCircleIcon, CloseIcon, InfoIcon } from './icons';

// ---------------------------------------------------------------------------
// Timing. One place, named, so the lifecycle is readable and tunable.
// ---------------------------------------------------------------------------

/** Time at full opacity before a toast starts fading out. */
const DWELL_MS = 5000;
/**
 * Errors get a longer read: they carry the most consequence, they are the ones a
 * user is most likely to glance away from and miss, and unlike a success there is
 * usually a decision to make about them.
 */
const ERROR_DWELL_MS = 9000;
/** Warnings sit between the two - actionable, but rarely a dead end. */
const WARNING_DWELL_MS = 7000;
/** How long the fade from full to gone takes once the dwell is over. */
const FADE_MS = 1500;
/**
 * Lifecycle clock resolution. ~30 opacity steps across FADE_MS reads as a smooth
 * fade, and the interval only runs while at least one toast is on screen.
 */
const TICK_MS = 50;
/**
 * Enter / push / exit duration. Skipped entirely under prefers-reduced-motion,
 * where every toast simply appears in its slot.
 */
const TRANSITION_MS = 180;
/** How far BELOW its slot a toast starts its entrance, in px. */
const ENTER_RISE_PX = 12;
/** How far ABOVE its slot a leaving toast drifts while it fades, in px. */
const EXIT_RISE_PX = 12;
/**
 * Opacity below which a toast counts as GONE for hit-testing.
 *
 * Opacity has no effect on hit-testing, and this stack portals into a
 * `fixed inset-0 z-[99999]` layer that sits above every dialog (`DialogWrapper`
 * is `z-50`). The dismiss button is the one `pointer-events-auto` element in the
 * whole stack, so leaving it permanently armed left a live ~24px target in the
 * bottom-left corner for the entire 180ms exit AND the 1.5s fade tail - long
 * after there was anything to see. A click aimed at a modal underneath it was
 * swallowed, with no cue that anything had intercepted it. Under this threshold
 * the button drops out of hit-testing so the click lands where it was aimed.
 *
 * It costs nothing in reachability. The pause is hit-tested on POINTER POSITION
 * (see the hover effect below), so a pointer anywhere over the column snaps every
 * toast back to full opacity before a click can land: a toast under the cursor is
 * always fully interactive, and keyboard focus is unaffected by `pointer-events`
 * either way.
 */
const INTERACTIVE_OPACITY_MIN = 0.1;

/** Full lifetime of a toast of a given kind, dwell plus fade. */
function lifetimeOf(kind: ToastKind): number {
  return dwellOf(kind) + FADE_MS;
}

/** A toast's dwell: its kind's, or forever for a persistent one (it leaves only
 *  when dismissed - see `ToastOptions.persistent`). */
function itemDwell(item: ToastItem): number {
  return item.persistent ? Infinity : dwellOf(item.kind);
}

function itemLifetime(item: ToastItem): number {
  return item.persistent ? Infinity : lifetimeOf(item.kind);
}

function dwellOf(kind: ToastKind): number {
  if (kind === 'error') return ERROR_DWELL_MS;
  if (kind === 'warning') return WARNING_DWELL_MS;
  return DWELL_MS;
}

// ---------------------------------------------------------------------------
// Appearance. Accents come from the theme tokens (--cc-success / -warning /
// -error) via their Tailwind aliases, never from hardcoded colour.
//
// Every box keeps the opaque `bg-panel` base it has always had - the stack floats
// over the map, and a 10%-alpha accent alone would be unreadable there. The tint
// is a nested layer that composites over that opaque base.
//
// Icons are deliberately drawn from `./icons`, which is FILLED throughout: WebKit
// fattens stroked geometry into blobs under the native page-zoom UI scale.
// ---------------------------------------------------------------------------

interface KindStyle {
  /** Tint + left accent rail, layered over the opaque panel background. */
  body: string;
  /** Accent colour for the leading icon. */
  icon: string;
  Icon: typeof InfoIcon;
}

const KIND_STYLES: Record<ToastKind, KindStyle> = {
  info: { body: 'border-border', icon: 'text-text-secondary', Icon: InfoIcon },
  success: { body: 'bg-success-10 border-success', icon: 'text-success', Icon: CheckCircleIcon },
  warning: { body: 'bg-warning-10 border-warning', icon: 'text-warning', Icon: AlertTriangleIcon },
  error: { body: 'bg-error-10 border-error', icon: 'text-error', Icon: AlertCircleIcon },
};

/** Which live region a kind is announced through. */
function isUrgent(kind: ToastKind): boolean {
  return kind === 'error' || kind === 'warning';
}

// ---------------------------------------------------------------------------

/** Live `(prefers-reduced-motion: reduce)`, defaulting to false where unsupported. */
export function usePrefersReducedMotion(): boolean {
  const query = useCallback((): MediaQueryList | null => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return null;
    try {
      return window.matchMedia('(prefers-reduced-motion: reduce)');
    } catch {
      return null;
    }
  }, []);

  const [reduced, setReduced] = useState(() => query()?.matches ?? false);

  useEffect(() => {
    const mq = query();
    if (!mq) return;
    const onChange = () => setReduced(mq.matches);
    onChange();
    if (typeof mq.addEventListener === 'function') {
      mq.addEventListener('change', onChange);
      return () => mq.removeEventListener('change', onChange);
    }
    // Old WebKit (Safari <14, and the WKWebView the macOS build runs in) exposes
    // only the deprecated pair, so SUBSCRIBE through it rather than just guarding
    // against it: falling through to no listener at all left the start-up reading
    // frozen, and toggling Reduce Motion while the app was running changed nothing
    // until a reload. Chromium has both, so this gap is invisible to the browser
    // tests and only ever showed on the shipped mac build.
    const legacy = mq as Partial<Pick<MediaQueryList, 'addListener' | 'removeListener'>>;
    if (typeof legacy.addListener === 'function') {
      legacy.addListener(onChange);
      return () => legacy.removeListener?.(onChange);
    }
    return undefined;
  }, [query]);

  return reduced;
}

function readViewportHeight(): number {
  if (typeof window === 'undefined') return 0;
  // innerHeight can read 0 in a webview that has not sized itself yet; fall back
  // to the layout viewport before giving up, so a transient 0 does not become the
  // number the capacity is computed from.
  return window.innerHeight || document.documentElement?.clientHeight || 0;
}

/**
 * Window height in CSS px, kept current across ordinary window resizes AND the
 * app's UI-scale control.
 *
 * The scale control is native webview zoom (`utils/uiScale`), which changes the
 * CSS viewport and should therefore surface as a plain `resize`. Should is doing
 * work in that sentence - it depends on the platform webview - so this listens on
 * three independent sources rather than betting on one: `resize`, a
 * ResizeObserver on the document element (fires for any layout-viewport change),
 * and the app's own `ui-scale-change` event. `setZoom` is async, so the
 * ui-scale reading is taken again on the next frame; whichever source is right on
 * a given platform wins, and the duplicates cost nothing because setting state to
 * the same number does not re-render.
 */
function useViewportHeight(): number {
  const [height, setHeight] = useState(readViewportHeight);

  useEffect(() => {
    if (typeof window === 'undefined') return undefined;
    const sync = () => setHeight(readViewportHeight());
    const syncSoon = () => {
      sync();
      if (typeof requestAnimationFrame === 'function') requestAnimationFrame(sync);
    };

    window.addEventListener('resize', sync);
    window.addEventListener(UI_SCALE_EVENT, syncSoon);
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== 'undefined' && document.documentElement) {
      observer = new ResizeObserver(sync);
      observer.observe(document.documentElement);
    }
    sync();

    return () => {
      window.removeEventListener('resize', sync);
      window.removeEventListener(UI_SCALE_EVENT, syncSoon);
      observer?.disconnect();
    };
  }, []);

  return height;
}

/** One line of a screen-reader live region. */
interface Announcement {
  /** `id:revision` - unique per announcement, so a refresh in place re-keys. */
  key: string;
  message: string;
}

/**
 * The lines one screen-reader live region should carry.
 *
 * LATCHED, not derived from the current stack. A live region announces whenever
 * its content CHANGES, so publishing "the newest message" re-read the PREVIOUS
 * toast every time the newest one expired: remove the newest, the one before it
 * becomes newest, the region's text changes, and assistive tech reads a message
 * the user was already told about - on every natural expiry, which for an info
 * toast is every five seconds.
 *
 * So the content only moves FORWARD: it is replaced by EVERY (id, revision) this
 * hook has not announced yet, oldest first, and holds otherwise. A refresh in
 * place (same id, bumped revision) is a new event and does announce - the message
 * can have changed. The latch is cleared once the stack empties, so an identical
 * message pushed later is still a change and therefore still announced.
 *
 * A QUEUE rather than "the newest item", because a render can bring in more than
 * one: two `showToast` calls in one event handler are a single React update, so
 * the first toast is never the newest in any render - and it never becomes the
 * newest later either, since same-kind toasts share a lifetime and a batch starts
 * their clocks together, so they expire on the same tick and the stack goes
 * straight from two to none. Sampling the newest dropped that message entirely.
 *
 * Each line is rendered as its own node, so a burst reads as separate messages in
 * the order they happened rather than as one run-on string.
 */
function useLiveRegionMessages(items: readonly ToastItem[]): readonly Announcement[] {
  /** id -> the revision of that toast this region has already announced. */
  const announced = useRef(new Map<number, number>());
  const [lines, setLines] = useState<readonly Announcement[]>([]);
  // No dependency list: the decision is about each item's (id, revision), which
  // is cheap to re-check, and the two writes below both bail out when nothing has
  // moved - the empty case deliberately re-uses the previous array rather than a
  // fresh one, which would be a new reference every pass and so a re-render loop.
  useEffect(() => {
    const seen = announced.current;
    if (items.length === 0) {
      seen.clear();
      setLines((prev) => (prev.length === 0 ? prev : []));
      return;
    }
    // Forget ids that have left, so the map cannot grow for the session's life.
    // Ids are monotonic, so a pruned id can never come back and be re-announced.
    for (const id of [...seen.keys()]) {
      if (!items.some((item) => item.id === id)) seen.delete(id);
    }
    // Oldest-first, matching the stack's own order.
    const unseen = items.filter((item) => seen.get(item.id) !== item.revision);
    if (unseen.length === 0) return;
    for (const item of unseen) seen.set(item.id, item.revision);
    setLines(unseen.map((item) => ({ key: `${item.id}:${item.revision}`, message: item.message })));
  });
  return lines;
}

/** Per-toast clock bookkeeping, keyed by toast id. */
interface Clock {
  /** The revision this clock belongs to; a refresh bumps it and resets `elapsed`. */
  revision: number;
  /** Unpaused milliseconds since the toast appeared (or was last refreshed). */
  elapsed: number;
  /** Milliseconds since the exit animation started. Only used while `leaving`. */
  leftFor: number;
}

/** Measured box of one toast, in CSS px. */
interface Size {
  height: number;
  width: number;
}

export function ToastStack() {
  const { t } = useTranslation();
  const toasts = useUIStore((s) => s.toasts);
  const dismissToast = useUIStore((s) => s.dismissToast);
  const setToastCapacity = useUIStore((s) => s.setToastCapacity);
  // F8: the largest clearance any mounted owner has registered (e.g. the DMP
  // wizard's footer), added on top of the default `bottom-6` anchor so a
  // lingering toast never covers it. 0 when nothing has registered.
  const toastBottomOffsets = useUIStore((s) => s.toastBottomOffsets);
  const bottomOffsetPx = Math.max(0, ...Object.values(toastBottomOffsets));
  const reducedMotion = usePrefersReducedMotion();
  const viewportHeight = useViewportHeight();

  // Pause scope is the WHOLE stack, not the single hovered toast. Toast positions
  // are derived from stack order, so letting the neighbours expire while the
  // pointer rests on one would reshuffle the column underneath the cursor - the
  // toast being read (and its dismiss button) would slide out from under the
  // pointer mid-click. Freezing the whole stack keeps the geometry still, and
  // "I am reading this" almost always means the batch, not one line of it.
  //
  // The two REASONS to pause are tracked separately, because they end in
  // completely different ways and neither exit can be trusted to fire:
  //
  //   - hover ends on mouseleave, backed up by the pointermove heal below (the
  //     hovered node can be removed, and a removed node fires no leave).
  //   - focus ends on blur, backed up by a containment re-check on every tick.
  //     Pressing Enter on a toast's dismiss button UNMOUNTS the focused element,
  //     and browsers fire no focusout for a removed node - so `onBlurCapture`
  //     never runs. Left conflated, that stranded the whole stack frozen (and
  //     `pointer-events-auto` over the map) for the rest of the session, since
  //     the empty-stack un-pause needs the stack to actually empty and the
  //     pointermove heal needs a pointer a keyboard user never moves.
  //
  // Keeping them apart also stops one reason from cancelling the other: a
  // mouseleave used to clear a live keyboard focus pause, and vice versa.
  //
  // The REFS are the truth, and the state only mirrors them for rendering. The
  // tick both reads and clears the pause, so it cannot afford a value that only
  // catches up on the next render: React batches, and under fake timers a whole
  // run of ticks executes before any re-render, so a render-synced ref would read
  // stale for all of them (and re-run the clock reset on every one).
  const hoverPausedRef = useRef(false);
  const focusPausedRef = useRef(false);
  const [paused, setPaused] = useState(false);
  /** Re-derive the RENDERED pause from the two refs. Every begin/end calls it. */
  const syncPaused = useCallback(() => {
    setPaused(hoverPausedRef.current || focusPausedRef.current);
  }, []);

  const reducedMotionRef = useLatest(reducedMotion);

  const clocks = useRef(new Map<number, Clock>());
  /** The stack container, so the tick can re-check whether it is really hovered. */
  const stackRef = useRef<HTMLDivElement | null>(null);

  /**
   * Give every surviving toast its FULL dwell back.
   *
   * A reset rather than a resume: having read the stack, the user has spent the
   * time the dwell exists to provide, and dropping them back into the last
   * fraction of a fade means a toast they just looked at vanishes almost
   * immediately.
   *
   * Harmless to call while the other pause reason still holds - the clocks are
   * frozen anyway, and the toasts get the same full dwell whenever it lifts.
   */
  const resetClocks = useCallback(() => {
    for (const clock of clocks.current.values()) clock.elapsed = 0;
  }, []);

  // Both ends are idempotent - the tick may call `endFocus` repeatedly, and a
  // second call must not hand the toasts another full dwell.
  const beginHover = useCallback(() => {
    hoverPausedRef.current = true;
    syncPaused();
  }, [syncPaused]);

  const endHover = useCallback(() => {
    if (!hoverPausedRef.current) return;
    hoverPausedRef.current = false;
    resetClocks();
    syncPaused();
  }, [resetClocks, syncPaused]);

  const beginFocus = useCallback(() => {
    focusPausedRef.current = true;
    syncPaused();
  }, [syncPaused]);

  const endFocus = useCallback(() => {
    if (!focusPausedRef.current) return;
    focusPausedRef.current = false;
    resetClocks();
    syncPaused();
  }, [resetClocks, syncPaused]);

  /** Live DOM node per toast id, so heights can be re-measured on demand. */
  const nodes = useRef(new Map<number, HTMLElement>());
  /** Last measured box per toast id. */
  const sizes = useRef(new Map<number, Size>());
  /** Slot offset each toast last occupied - where a leaving toast drifts up FROM. */
  const lastOffsets = useRef(new Map<number, number>());
  /** Toasts that have had their first frame and so are no longer entering. */
  const settled = useRef(new Set<number>());
  const observer = useRef<ResizeObserver | null>(null);

  // Geometry and the opacity ramp are computed at render time from refs, so
  // anything that changes them has to ask for a re-render explicitly.
  const [, bump] = useReducer((n: number) => n + 1, 0);

  const measure = useCallback((el: HTMLElement): boolean => {
    const id = Number(el.dataset.toastId);
    if (!Number.isFinite(id)) return false;
    const rect = el.getBoundingClientRect();
    // getBoundingClientRect is the truth in a browser; offsetHeight/Width is the
    // fallback for environments that do not lay out (jsdom returns zeroes).
    const height = rect.height || el.offsetHeight || 0;
    const width = rect.width || el.offsetWidth || 0;
    const prev = sizes.current.get(id);
    if (prev && prev.height === height && prev.width === width) return false;
    sizes.current.set(id, { height, width });
    return true;
  }, []);

  // One ResizeObserver for the whole stack rather than one per toast: a toast can
  // change height without React re-rendering it (a font swap, a UI-scale change,
  // a wrap point moving), and that has to move every toast above it.
  useEffect(() => {
    if (typeof ResizeObserver === 'undefined') return undefined;
    const ro = new ResizeObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        if (measure(entry.target as HTMLElement)) changed = true;
      }
      if (changed) bump();
    });
    observer.current = ro;
    for (const el of nodes.current.values()) ro.observe(el);
    return () => {
      ro.disconnect();
      observer.current = null;
    };
  }, [measure]);

  // A single stable ref callback for every toast - the id is read back off the
  // element, so this never has to be re-created per item (which would detach and
  // re-attach every ref on every render).
  const attach = useCallback(
    (el: HTMLDivElement | null) => {
      if (!el) return;
      const id = Number(el.dataset.toastId);
      if (!Number.isFinite(id)) return;
      nodes.current.set(id, el);
      observer.current?.observe(el);
      measure(el);
    },
    [measure],
  );

  // React 18 ref callbacks cannot return a cleanup, so the per-id bookkeeping is
  // pruned here instead: anything not in the current list is gone for good.
  //
  // UNOBSERVING is not optional. This component is always mounted (App renders it
  // unconditionally), so the observer's disconnect-on-unmount never actually
  // runs - dropping the map entry without unobserving left every toast element
  // the session had ever shown pinned inside the ResizeObserver, and through it
  // in memory, for as long as the app was open.
  //
  // Keyed to `toasts`, and deliberately NOT re-measuring: `attach` measures each
  // element as it mounts, and the ResizeObserver above catches every later size
  // change (a font swap, a UI-scale change, a wrap point moving) - which is the
  // entire reason that observer exists. Running `getBoundingClientRect` over every
  // mounted toast on every render forced a pre-paint layout flush per toast per
  // render, over a live WebGL map, and could not tell the geometry anything those
  // two had not already recorded.
  useLayoutEffect(() => {
    const liveIds = new Set(toasts.map((item) => item.id));
    for (const [id, el] of [...nodes.current.entries()]) {
      if (liveIds.has(id)) continue;
      observer.current?.unobserve(el);
      nodes.current.delete(id);
    }
    for (const id of [...sizes.current.keys()]) if (!liveIds.has(id)) sizes.current.delete(id);
    for (const id of [...lastOffsets.current.keys()]) if (!liveIds.has(id)) lastOffsets.current.delete(id);
    for (const id of [...settled.current]) if (!liveIds.has(id)) settled.current.delete(id);
  }, [toasts]);

  const hasToasts = toasts.length > 0;
  useEffect(() => {
    if (!hasToasts) {
      clocks.current.clear();
      // Un-pause on the way to empty. Dismissing the last toast from under the
      // pointer (or with the keyboard) unmounts the container, so its
      // mouseleave/blur never fires - leaving `paused` stuck true would mean the
      // NEXT toast arrives pre-frozen and never dismisses itself.
      hoverPausedRef.current = false;
      focusPausedRef.current = false;
      setPaused(false);
      return undefined;
    }
    const handle = window.setInterval(() => {
      const store = useUIStore.getState();
      const live = store.toasts;
      const map = clocks.current;

      // Focus pause, re-derived rather than remembered: the element that pauses
      // the stack is usually a dismiss button, and dismissing REMOVES it, so the
      // blur that would have lifted the pause is never dispatched. Asking the
      // document where focus actually is cannot be stranded that way, and it
      // costs one `contains` per tick while a pause is live.
      if (focusPausedRef.current) {
        const el = stackRef.current;
        if (!el || !el.contains(document.activeElement)) endFocus();
      }

      for (const id of [...map.keys()]) {
        if (!live.some((item) => item.id === id)) map.delete(id);
      }

      // Did anything the RENDER reads actually move this tick? The clocks are
      // refs, so a change in them is invisible until someone asks for a frame -
      // but during the DWELL nothing rendered moves at all (opacity is a flat 1,
      // the offsets are unchanged), and a paused stack is frozen by definition.
      // Bumping unconditionally re-rendered the whole stack 20 times a second for
      // each toast's entire 6.5-10.5s life, over a live WebGL map, to paint the
      // frame that was already on screen.
      let dirty = false;

      for (const item of live) {
        let clock = map.get(item.id);
        if (!clock || clock.revision !== item.revision) {
          clock = { revision: item.revision, elapsed: 0, leftFor: 0 };
          map.set(item.id, clock);
        }
        if (item.leaving) {
          // An exit always runs to completion - it is already off the user's
          // reading path, and freezing it mid-slide on hover just leaves litter.
          // `leftFor` is bookkeeping only: nothing rendered reads it, and the
          // removal it leads to is a store write, which re-renders on its own.
          clock.leftFor += TICK_MS;
          if (clock.leftFor >= (reducedMotionRef.current ? 0 : TRANSITION_MS)) {
            store.removeToast(item.id);
          }
          continue;
        }
        if (hoverPausedRef.current || focusPausedRef.current) continue;
        clock.elapsed += TICK_MS;
        // Past the dwell the opacity ramp - and the exit rise it drives - moves on
        // every tick, so those ticks, and only those, need a frame. Under reduced
        // motion there is no ramp: full opacity until the toast is simply removed.
        if (!reducedMotionRef.current && clock.elapsed > itemDwell(item)) dirty = true;
        if (clock.elapsed >= itemLifetime(item)) store.removeToast(item.id);
      }

      // Backstop for the entrance flip below: a toast must never be stranded at
      // opacity 0 if the frame callback never runs (a backgrounded window). That
      // flip IS rendered, so it is one of the things worth a frame.
      for (const item of live) {
        if (settled.current.has(item.id)) continue;
        settled.current.add(item.id);
        dirty = true;
      }

      if (dirty) bump();
    }, TICK_MS);
    return () => window.clearInterval(handle);
  }, [hasToasts, endFocus, reducedMotionRef]);


  // The entrance. A toast renders its first frame below its slot at opacity 0,
  // then flips to its real offset on the next frame, and the CSS transition does
  // the rising. Both halves have to be committed to the DOM as separate frames or
  // the browser collapses them into no transition at all.
  const entering = toasts.some((item) => !settled.current.has(item.id));
  useEffect(() => {
    if (!entering) return undefined;
    const flush = () => {
      let changed = false;
      for (const item of useUIStore.getState().toasts) {
        if (!settled.current.has(item.id)) {
          settled.current.add(item.id);
          changed = true;
        }
      }
      if (changed) bump();
    };
    if (typeof requestAnimationFrame === 'function') {
      const frame = requestAnimationFrame(flush);
      return () => cancelAnimationFrame(frame);
    }
    const timer = window.setTimeout(flush, 0);
    return () => window.clearTimeout(timer);
  }, [entering]);

  // Geometry. Leaving toasts are excluded from the stack so the survivors close
  // the gap smoothly; each leaver keeps the offset it last held and drifts up
  // from there.
  //
  // `usableToastHeight`, not `?? ESTIMATED`: `measure` stores `rect.height ||
  // offsetHeight || 0`, so a zero-layout environment stores a real 0 that `??`
  // waves through - and a column of zeroes stacks at one gap per toast, i.e. all
  // of them on top of each other. The geometry module applies the same rule.
  const live = toasts.filter((item) => !item.leaving);
  const liveHeights = live.map((item) => usableToastHeight(sizes.current.get(item.id)?.height));
  const offsets = toastStackOffsets(liveHeights);
  live.forEach((item, index) => lastOffsets.current.set(item.id, offsets[index] ?? 0));
  const stackHeight = toastStackHeight(liveHeights);
  const stackWidth = live.reduce((widest, item) => Math.max(widest, sizes.current.get(item.id)?.width ?? 0), 0);

  // Capacity. The store owns eviction (so it stays deterministic and testable);
  // only the DOM knows how tall the window and the toasts actually are, so the
  // measurement is reported in and the store remains the single answer to "what
  // is on screen". Newest-first, because the newest toast is the last to be
  // given up. `setToastCapacity` is a no-op when the number has not moved, so
  // reporting on every render cannot loop.
  useEffect(() => {
    // F8: the stack's bottom anchor sits at `24 + bottomOffsetPx` px above the
    // window's bottom edge (see the container's `style.bottom` below), so the
    // headroom actually available for the column is `viewportHeight -
    // bottomOffsetPx`, not the raw window height - budgeting against the full
    // height let a full 5-toast stack overflow off the TOP of a short viewport
    // once a registered offset had already eaten into the room from the
    // bottom. `viewportHeight - bottomOffsetPx` (not `viewportHeight -
    // footerRect.top`): `bottomOffsetPx` is the exact quantity already driving
    // the anchor position below, so reusing it keeps position and capacity
    // answering the same question from the same number, and the registry only
    // ever stores a clearance height, not an owner's absolute screen position.
    setToastCapacity(fitToastCapacity(viewportHeight - bottomOffsetPx, [...liveHeights].reverse()));
  });

  /**
   * Hover detection by GEOMETRY rather than by `mouseenter`/`mouseleave`.
   *
   * The stack is deliberately click-through (`pointer-events-none`; only the
   * dismiss buttons opt back in), so it receives no pointer events of its own and
   * has no enter/leave pair to listen to. Hit-testing the pointer against the
   * measured box gives the same pause behaviour while a click on the map or a
   * panel underneath still lands where the user aimed it.
   *
   * It also subsumes the stranded-pause self-heal this replaces: `mouseleave` was
   * never guaranteed to arrive, because the node under the pointer can simply be
   * REMOVED (a toast finishing its exit, the stack reflowing, the container
   * unmounting) and a node that no longer exists fires no leave event - which left
   * `paused` stuck true and nothing ever dismissing again. A position test cannot
   * get stranded: no box, or the pointer outside it, ends the hover. A focus pause
   * is untouched - focus is held regardless of where the mouse is, and the tick
   * re-check above is what ends that one.
   *
   * The box is cached rather than measured per event: pointer moves arrive at
   * pointer rate, so reading `getBoundingClientRect` in the handler would force a
   * layout flush on every mouse move across the whole app, over a live WebGL map.
   * It only moves when the stack's own geometry or the viewport does, which is
   * exactly this effect's dependency list.
   */
  const hoverBox = useRef<DOMRect | null>(null);
  useEffect(() => {
    if (!hasToasts) {
      hoverBox.current = null;
      return undefined;
    }
    hoverBox.current = stackRef.current?.getBoundingClientRect() ?? null;
    const onMove = (e: MouseEvent) => {
      const box = hoverBox.current;
      const inside =
        !!box &&
        e.clientX >= box.left &&
        e.clientX <= box.right &&
        e.clientY >= box.top &&
        e.clientY <= box.bottom;
      if (inside) beginHover();
      else endHover();
    };
    // ONE listener, never a pointermove/mousemove pair. A pointer event is a
    // superset of the mouse event it is paired with (and fires for pen and touch
    // besides), so registering both ran this hit-test TWICE for every mouse move
    // anywhere in the app. `mousemove` stays as a genuine fallback for a runtime
    // without PointerEvent - which no webview this ships in is - rather than as a
    // second registration alongside it.
    const moveEvent = typeof PointerEvent === 'function' ? 'pointermove' : 'mousemove';
    document.addEventListener(moveEvent, onMove, true);
    return () => {
      document.removeEventListener(moveEvent, onMove, true);
    };
  }, [hasToasts, stackHeight, stackWidth, viewportHeight, beginHover, endHover]);

  const urgent = toasts.filter((item) => isUrgent(item.kind));
  const polite = toasts.filter((item) => !isUrgent(item.kind));
  const politeMessages = useLiveRegionMessages(polite);
  const urgentMessages = useLiveRegionMessages(urgent);

  return createPortal(
    <div className="fixed inset-0 pointer-events-none z-[99999]">
      {/*
        A single container cannot vary politeness per item, so announcement is
        split off from presentation: two permanently-mounted screen-reader regions
        carry the text (polite for info/success, assertive for error/warning),
        while the visible stack below marks its message `aria-hidden` so nothing is
        read twice. Each region carries every message it has not yet announced and
        HOLDS them (see useLiveRegionMessages) - reading the newest message
        straight off the stack re-announced the previous one on every removal, and
        lost every message but the last of a batch.

        One node per message, not one joined string: the regions are not atomic,
        so a burst arriving in a single update is read as the separate messages it
        is, in the order they happened.
      */}
      <div className="sr-only" role="status" aria-live="polite">
        {politeMessages.map((line) => (
          <div key={line.key}>{line.message}</div>
        ))}
      </div>
      <div className="sr-only" role="alert" aria-live="assertive">
        {urgentMessages.map((line) => (
          <div key={line.key}>{line.message}</div>
        ))}
      </div>

      {hasToasts && (
        <div
          data-testid="toast-stack"
          // Bottom-anchored, so the column grows UPWARD as toasts arrive: the
          // array is oldest-first and every toast shares this bottom edge, which
          // puts the newest at the bottom, nearest where the eye already is.
          //
          // Left-aligned rather than centered: a common left edge makes
          // differently-sized toasts read as one column instead of a ragged
          // centered pile.
          //
          // The box is sized from the measurement so it covers the whole column,
          // gaps included: hover pauses the stack, and a gap that was not part of
          // the hit area would un-pause every time the pointer crossed one.
          //
          // CLICK-THROUGH. The stack floats over the survey map and the side
          // panels, and it is transient feedback the user never asked for - so it
          // must not eat a click aimed at what is underneath. With
          // `pointer-events-auto` here, any click landing on the column (a station,
          // a shot, a panel row) was swallowed with no cue that anything had
          // intercepted it, and a pointer resting in that corner kept the dead zone
          // alive indefinitely by re-arming the dwell on every leave. Only the
          // dismiss buttons opt back in; the pause is hit-tested against this box
          // instead of relying on pointer events it no longer receives.
          className="absolute left-6 pointer-events-none"
          style={{
            bottom: 24 + bottomOffsetPx,
            height: stackHeight || undefined,
            width: stackWidth || undefined,
          }}
          ref={stackRef}
          onFocusCapture={beginFocus}
          onBlurCapture={endFocus}
        >
          {toasts.map((item) => (
            <Toast
              key={item.id}
              item={item}
              elapsed={clocks.current.get(item.id)?.elapsed ?? 0}
              offset={lastOffsets.current.get(item.id) ?? 0}
              entering={!reducedMotion && !settled.current.has(item.id)}
              paused={paused}
              reducedMotion={reducedMotion}
              measureRef={attach}
              dismissLabel={t('common.toast.dismiss', { message: item.message })}
              actionLabel={item.action ? t('common.toast.action', { action: item.action.label, message: item.message }) : ''}
              onDismiss={() => dismissToast(item.id)}
            />
          ))}
        </div>
      )}
    </div>,
    document.body,
  );
}

interface ToastProps {
  item: ToastItem;
  elapsed: number;
  /** Distance in px from the stack's bottom anchor to this toast's bottom edge. */
  offset: number;
  /** True for the one frame before the entrance transition starts. */
  entering: boolean;
  paused: boolean;
  reducedMotion: boolean;
  measureRef: (el: HTMLDivElement | null) => void;
  dismissLabel: string;
  /** Accessible name of the action button (unused without an action). */
  actionLabel: string;
  onDismiss: () => void;
}

function Toast({
  item,
  elapsed,
  offset,
  entering,
  paused,
  reducedMotion,
  measureRef,
  dismissLabel,
  actionLabel,
  onDismiss,
}: ToastProps) {
  const style = KIND_STYLES[item.kind];
  const { Icon } = style;

  // Opacity is driven off the tick rather than a CSS transition so that pausing
  // can freeze it and un-pausing can resume from where it stopped. Hover/focus
  // snaps straight back to full - "it lit back up" is the intended read.
  let opacity = 1;
  // How far through the fade-out this toast is, 0 (untouched) to 1 (gone). Also
  // drives the exit RISE below, so the drift happens WHILE it fades rather than
  // after: expiring naturally used to reach opacity 0 on the tick and only then
  // be marked `leaving`, so the upward travel was applied to an already-invisible
  // box and the toast simply faded in place.
  let fade = 0;
  if (item.leaving || entering) {
    opacity = 0;
    fade = item.leaving ? 1 : 0;
  } else if (!paused && !reducedMotion) {
    const overdue = elapsed - itemDwell(item);
    if (overdue > 0) {
      fade = Math.min(1, overdue / FADE_MS);
      opacity = 1 - fade;
    }
  }

  // Hit-testing follows what the user can SEE. `opacity` alone decides it, which
  // covers both invisible states without restating them: a leaving toast and a
  // toast on its entrance frame both render at 0 (see the branch above).
  const interactive = opacity >= INTERACTIVE_OPACITY_MIN;

  // Up is negative. A toast enters from below its slot, sits ON it, and leaves
  // above it. Under reduced motion there is no travel at all: it is placed in its
  // slot and taken away from it, with the same lifetime either way.
  let translate = -offset;
  if (!reducedMotion) {
    if (entering) translate = -(offset - ENTER_RISE_PX);
    else if (fade > 0) translate = -(offset + EXIT_RISE_PX * fade);
  }

  return (
    <div
      ref={measureRef}
      data-testid="toast"
      data-toast-id={item.id}
      data-toast-kind={item.kind}
      data-toast-key={item.key}
      data-toast-leaving={item.leaving ? 'true' : undefined}
      className={[
        // `w-max` so the box sizes to its own content rather than to the
        // measurement-sized container it lives in (which would be circular), and
        // still wraps at the cap.
        'absolute bottom-0 left-0 w-max bg-panel rounded-lg shadow-lg overflow-hidden max-w-[min(32rem,90vw)]',
        // Reduced motion: appear and disappear, no slide, no fade ramp. The total
        // lifetime is unchanged - only the movement goes away.
        reducedMotion ? '' : 'transition-[opacity,transform] ease-out',
      ]
        .filter(Boolean)
        .join(' ')}
      style={{
        opacity,
        transform: `translateY(${translate}px)`,
        transitionDuration: reducedMotion ? undefined : `${TRANSITION_MS}ms`,
      }}
    >
      <div className={`flex items-start gap-2 border-l-4 px-4 py-2 ${style.body}`}>
        <Icon size={16} className={`shrink-0 mt-0.5 ${style.icon}`} />
        {/* aria-hidden: the live regions above already announce this text. */}
        <span className="text-text-primary text-sm leading-5" aria-hidden="true">
          {item.message}
        </span>
        {/* The toast's one action (only the lost-WebGL-context toast has one,
            #576 D38-LOST). Hit-testable on the same terms as the dismiss button
            below; it runs the action, then dismisses the toast. */}
        {item.action && (
          <button
            type="button"
            aria-label={actionLabel}
            data-testid={item.action.testId}
            onClick={() => {
              item.action?.onAction();
              onDismiss();
            }}
            className={`${
              interactive ? 'pointer-events-auto' : 'pointer-events-none'
            } shrink-0 mt-0.5 rounded px-2 py-0.5 text-sm font-medium text-accent hover:bg-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-accent`}
          >
            {item.action.label}
          </button>
        )}
        {/* The one part of the stack that opts back INTO pointer events, and only
            while it is visible: the container is click-through so a toast never
            eats a click meant for the map or a panel, this button is the only
            thing in it the user can legitimately aim at, and an invisible button
            is not something anyone can aim at (see INTERACTIVE_OPACITY_MIN). */}
        <button
          type="button"
          aria-label={dismissLabel}
          data-testid={`toast-dismiss-${item.id}`}
          onClick={onDismiss}
          className={`${
            interactive ? 'pointer-events-auto' : 'pointer-events-none'
          } shrink-0 -mr-1 mt-0.5 rounded p-1 text-text-secondary hover:text-text-primary hover:bg-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-accent`}
        >
          <CloseIcon size={12} />
        </button>
      </div>
    </div>
  );
}

export default ToastStack;
