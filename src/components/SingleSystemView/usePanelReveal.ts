import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { PANEL_FIELDS, useUIStore, type PanelId } from '../../store/uiStore';
import { PANEL_CLOSE_GRACE_MS } from './layoutConstants';

export interface PanelReveal {
  /** Pinned and open → rendered in the flex layout (reflows the map). */
  isPinnedOpen: boolean;
  /** Hover mode and currently hovered → rendered as a floating overlay. */
  isFloatingOpen: boolean;
  /** Either kind of open. */
  isOpen: boolean;
  /** Attach to the bar, the tab, and the floating panel: together they form one
   *  hover region, so moving between them keeps a hover-revealed panel open. */
  onRegionEnter: () => void;
  onRegionFocus: () => void;
  onRegionBlur: () => void;
  onRegionLeave: () => void;
  /** Schedule a close after `ms` (cancelled by onRegionEnter). Used to give a
   *  longer grace after a resize so the user can move back over the panel. */
  scheduleClose: (ms?: number) => void;
  /** Click the closed bar/tab: pin mode opens pinned; hover mode reveals it. */
  openClick: () => void;
  revealTemporarily: () => void;
  /** Pin the panel open (hover → pin). */
  pin: () => void;
  /** Un-pin to hover mode; stays visible while the pointer remains over it. */
  unpin: () => void;
  /** Close a pinned panel (stays in pin mode). */
  close: () => void;
}

export interface PanelRevealOpts {
  /** Viewport-driven collapse (#260): the window is too narrow to keep this panel
   *  pinned open AND leave the map its minimum width. Behaves exactly like hover
   *  mode - the panel un-reflows to its opener bar/tab and can still be floated
   *  over the map - WITHOUT touching the persisted mode/open, so the user's real
   *  layout returns untouched as soon as there is room again. */
  forcedClosed?: boolean;
}

/**
 * Drives the two-mode (hover/pin) reveal behavior for one in-project panel.
 * Persisted `mode`/`open` live in the UI store; transient hover visibility is
 * local React state with a short close-grace timer. See the panel-reveal plan.
 */
export function usePanelReveal(panel: PanelId, opts: PanelRevealOpts = {}): PanelReveal {
  const forcedClosed = opts.forcedClosed ?? false;
  const mode = useUIStore((s) => s[PANEL_FIELDS[panel].mode]);
  const open = useUIStore((s) => s[PANEL_FIELDS[panel].open]);
  const setPanelMode = useUIStore((s) => s.setPanelMode);
  const setPanelOpen = useUIStore((s) => s.setPanelOpen);

  const [hovering, setHovering] = useState(false);
  const [programmatic, setProgrammatic] = useState(false);
  const [focused, setFocused] = useState(false);
  const focusedRef = useRef(false);
  const graceRef = useRef<number | null>(null);
  const graceDeadlineRef = useRef(0);

  const clearGrace = useCallback(() => {
    if (graceRef.current !== null) {
      clearTimeout(graceRef.current);
      graceRef.current = null;
    }
    graceDeadlineRef.current = 0;
  }, []);

  const onRegionFocus = useCallback(() => {
    focusedRef.current = true;
    clearGrace();
    setFocused(true);
  }, [clearGrace]);
  const onRegionBlur = useCallback(() => { focusedRef.current = false; setFocused(false); }, []);

  useEffect(() => clearGrace, [clearGrace]);

  // Drop the transient hover state whenever the panel is (re-)pinned - by the
  // user, or programmatically (section-focus, data-table fullscreen, keyboard /
  // menu toggles all force mode='pin'). Otherwise a `hovering=true` captured
  // before the pin would survive a later return to hover mode and float the panel
  // open with no pointer over it (e.g. exiting section-focus restores 'hover').
  // Skipped while force-closed: the panel is behaving as a hover panel there, so
  // clearing `hovering` on its (still 'pin') persisted mode would snap a panel the
  // user just floated open shut again.
  useEffect(() => {
    if (mode === 'pin' && !forcedClosed && !programmatic) {
      setHovering(false);
      clearGrace();
    }
  }, [mode, forcedClosed, programmatic, clearGrace]);

  // Programmatic reveal (e.g. "jump to this warning" from the data table) is
  // driven by the one panel that needs it (Details) via onRegionEnter +
  // scheduleClose in SingleSystemView, so this hook carries no reveal-nonce
  // subscription for any panel. See SingleSystemView's detailRevealNonce effect.

  const onRegionEnter = useCallback(() => {
    clearGrace();
    setHovering(true);
  }, [clearGrace]);

  const scheduleClose = useCallback(
    (ms: number = PANEL_CLOSE_GRACE_MS) => {
      // Don't let a shorter grace shorten a longer one already pending - e.g. the
      // reconciling mouseleave fired on resize pointer-capture release must not
      // clobber the generous post-resize grace down to the default.
      const deadline = Date.now() + ms;
      if (graceRef.current !== null && graceDeadlineRef.current > deadline) return;
      clearGrace();
      graceDeadlineRef.current = deadline;
      graceRef.current = window.setTimeout(() => {
        graceRef.current = null;
        graceDeadlineRef.current = 0;
        setHovering(false);
        if (!focusedRef.current) setProgrammatic(false);
      }, ms);
    },
    [clearGrace],
  );

  const onRegionLeave = useCallback(() => scheduleClose(), [scheduleClose]);

  // A force-closed panel never reflows the map (that is the whole point), but it
  // can still be floated over it - so it reads as hover mode for both booleans.
  const isPinnedOpen = mode === 'pin' && open && !forcedClosed;
  const isFloatingOpen = ((mode === 'hover' || forcedClosed) || (programmatic && !isPinnedOpen)) && (hovering || focused);
  const revealTemporarily = useCallback(() => { clearGrace(); setProgrammatic(true); setHovering(true); }, [clearGrace]);
  const isOpen = isPinnedOpen || isFloatingOpen;

  const openClick = useCallback(() => {
    if (mode === 'pin' && !forcedClosed) {
      setPanelOpen(panel, true);
    } else {
      // Force-closed lands here too: persisted `open` is already true, so writing
      // it again would be a no-op AND a pointless config write. Float it instead.
      clearGrace();
      setHovering(true);
    }
  }, [mode, forcedClosed, panel, setPanelOpen, clearGrace]);

  const pin = useCallback(() => {
    setPanelMode(panel, 'pin');
  }, [panel, setPanelMode]);

  const unpin = useCallback(() => {
    setPanelMode(panel, 'hover');
    // The pointer is over the tab when un-pinning, so keep it floating-open until
    // the user actually leaves the region.
    clearGrace();
    setHovering(true);
  }, [panel, setPanelMode, clearGrace]);

  const close = useCallback(() => {
    setPanelOpen(panel, false);
    setProgrammatic(false);
    focusedRef.current = false;
    setFocused(false);
    clearGrace();
    setHovering(false);
  }, [panel, setPanelOpen, clearGrace]);

  // Memoize so the returned object keeps a stable identity across renders where no
  // field changed (e.g. the ~60×/s store-width updates during a sibling's resize
  // drag don't touch mode/open/hovering). The callbacks are all stable, so this
  // object's identity flips only when a reveal boolean does - letting React.memo'd
  // <Dock>s that take `reveal` skip re-rendering during unrelated resize ticks.
  return useMemo(
    () => ({
      isPinnedOpen,
      isFloatingOpen,
      isOpen,
      onRegionEnter,
      onRegionFocus,
      onRegionBlur,
      onRegionLeave,
      scheduleClose,
      openClick,
      revealTemporarily,
      pin,
      unpin,
      close,
    }),
    [isPinnedOpen, isFloatingOpen, isOpen, onRegionFocus, onRegionBlur, onRegionEnter, onRegionLeave, scheduleClose, openClick, revealTemporarily, pin, unpin, close],
  );
}
