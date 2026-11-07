import { useRef, useCallback, useEffect } from 'react';

interface LongPressOptions {
  /** Delay in ms before firing (default 500) */
  delay?: number;
  /** Max movement in px before cancelling (default 10) */
  moveThreshold?: number;
  /** Whether the hook is enabled (default true) */
  enabled?: boolean;
}

interface LongPressHandlers {
  onPointerDown: (e: React.PointerEvent) => void;
  onPointerMove: (e: React.PointerEvent) => void;
  onPointerUp: (e: React.PointerEvent) => void;
  onPointerCancel: (e: React.PointerEvent) => void;
}

/** How long the compatibility-event suppressor stays armed after a lift. */
const COMPAT_EVENT_WINDOW_MS = 700;

/** Tear-down for the armed suppressor, or null while none is armed. */
let disarmCompatEvents: (() => void) | null = null;

/**
 * Swallow the compatibility mouse events the lift synthesises.
 *
 * A touch device fires `mousedown`/`mouseup`/`click` (and sometimes
 * `contextmenu`) from the finger LIFT, not from the moment the hold completes -
 * so this is armed in `onPointerUp`, not when the timer fires. Arming it at
 * fire time would let any hold longer than the window leak its tap into the
 * menu the hold just opened.
 *
 * The listeners sit on `document` in the CAPTURE phase because that is the only
 * position that beats both React's root delegation and the bubble-phase
 * document `mousedown` that dismissable layers close themselves on. The
 * pointerdown would be the spec's suppression point, but preventing it there
 * kills the compat `mousedown` of every ordinary tap, and tap-to-select rides
 * on that.
 *
 * A platform that suppresses the compat events itself sends none of them, so
 * the window has to close on its own: on the first `click`, on the `pointerdown`
 * that starts the user's NEXT gesture (a real gesture always begins with one,
 * while a compat sequence synthesised from a lift never does - so this can
 * never disarm ahead of the events it exists to eat), or when it times out.
 *
 * `contextmenu` and `click` are prevented as well as stopped: stopping
 * propagation alone leaves the browser's own default live, which for a compat
 * `contextmenu` is a NATIVE menu over the one the press just opened.
 */
function armCompatEventSuppressor(): void {
  // Re-arm rather than stack, so a second long press just restarts the window.
  disarmCompatEvents?.();

  let timer: ReturnType<typeof setTimeout> | null = null;

  const swallow = (e: Event) => {
    e.stopPropagation();
  };
  const swallowDefault = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
  };
  const swallowClick = (e: Event) => {
    swallowDefault(e);
    disarm();
  };
  // A new gesture: everything from here on is the user's, not the lift's.
  const disarmForNewGesture = () => disarm();

  const disarm = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    document.removeEventListener('mousedown', swallow, true);
    document.removeEventListener('mouseup', swallow, true);
    document.removeEventListener('contextmenu', swallowDefault, true);
    document.removeEventListener('click', swallowClick, true);
    document.removeEventListener('pointerdown', disarmForNewGesture, true);
    if (disarmCompatEvents === disarm) disarmCompatEvents = null;
  };

  document.addEventListener('mousedown', swallow, true);
  document.addEventListener('mouseup', swallow, true);
  document.addEventListener('contextmenu', swallowDefault, true);
  document.addEventListener('click', swallowClick, true);
  document.addEventListener('pointerdown', disarmForNewGesture, true);
  timer = setTimeout(disarm, COMPAT_EVENT_WINDOW_MS);
  disarmCompatEvents = disarm;
}

/**
 * What the finger landed on, as an `HTMLElement`.
 *
 * The press target is the deepest node under the finger, and that is routinely
 * not HTML: a press on a row's icon lands on an `SVGPathElement` whose parent
 * is an `<svg>`, itself not an `HTMLElement`. So walk up until HTML appears -
 * the chain ends at the document, which is not one, so an unrooted or
 * non-element target simply yields null and the press stays unarmed.
 */
function pressTarget(raw: EventTarget | null): HTMLElement | null {
  let node: Node | null = raw instanceof Node ? raw : null;
  while (node !== null && !(node instanceof HTMLElement)) {
    node = node.parentNode;
  }
  return node instanceof HTMLElement ? node : null;
}

/**
 * Hook for long-press detection on touch devices.
 * Only activates for `pointerType === 'touch'` - mouse events pass through untouched.
 * After firing, the compatibility mouse events the lift synthesises are swallowed
 * (see `armCompatEventSuppressor`), so the menu the press opened survives its own tap.
 * A callback that returns `false` says it opened nothing, and gets no swallow.
 */
export function useLongPress(
  callback: (position: { x: number; y: number }, target: HTMLElement) => boolean | void,
  options?: LongPressOptions,
): LongPressHandlers {
  const { delay = 500, moveThreshold = 10, enabled = true } = options ?? {};

  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startPos = useRef<{ x: number; y: number } | null>(null);
  const firedRef = useRef(false);

  const clear = useCallback(() => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    startPos.current = null;
  }, []);

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (!enabled) return;
      if (e.pointerType !== 'touch') return;

      const target = pressTarget(e.target);
      if (!target) return;

      firedRef.current = false;
      startPos.current = { x: e.clientX, y: e.clientY };

      const x = e.clientX;
      const y = e.clientY;

      // A second finger restarts the hold. Overwriting `timerRef` alone would
      // orphan the first finger's timer where no lift or cancel can reach it,
      // and it would fire after both fingers are gone.
      if (timerRef.current !== null) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        // Only an explicit false counts as "nothing opened": there is then no
        // menu for the trailing tap to disturb, so it is left alone.
        firedRef.current = callback({ x, y }, target) !== false;
      }, delay);
    },
    [enabled, delay, callback],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (e.pointerType !== 'touch') return;
      if (!startPos.current || timerRef.current === null) return;

      const dx = e.clientX - startPos.current.x;
      const dy = e.clientY - startPos.current.y;
      if (dx * dx + dy * dy > moveThreshold * moveThreshold) {
        clear();
      }
    },
    [moveThreshold, clear],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent) => {
      if (e.pointerType !== 'touch') return;
      clear();
      if (firedRef.current) {
        firedRef.current = false;
        // Belt-and-braces: some engines honour a prevented pointerup and skip
        // the compat events entirely. The suppressor covers the ones that do not.
        e.preventDefault();
        armCompatEventSuppressor();
      }
    },
    [clear],
  );

  const onPointerCancel = useCallback(
    (e: React.PointerEvent) => {
      if (e.pointerType !== 'touch') return;
      clear();
      firedRef.current = false;
    },
    [clear],
  );

  // An armed window outlives the element that armed it - the menu a press opens
  // can unmount the row underneath it. Nothing left listening on `document`
  // once this consumer is gone, and no pending press left to fire for it.
  useEffect(
    () => () => {
      clear();
      disarmCompatEvents?.();
    },
    [clear],
  );

  return { onPointerDown, onPointerMove, onPointerUp, onPointerCancel };
}
