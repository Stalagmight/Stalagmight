import { useEffect } from 'react';
import { useUIStore } from '../store/uiStore';

/**
 * Register this component as "occupying the screen" for as long as `active`
 * is true. Feeds `isScreenBusy()` (the single source of truth other code
 * checks before doing something that assumes nothing else owns the screen,
 * e.g. the launch-file drain deciding whether to raise a dialog) - so the
 * NEXT independent top-layer overlay (one rendered outside the single
 * `activeDialog` slot, like TelemetryOptInDialog, HeldLockResumeDialog,
 * SpeleoDumpPickerDialog today) is covered by
 * calling this hook rather than by some other file remembering to enumerate
 * it (#445 review).
 *
 * Balanced by construction: the effect increments once on the transition to
 * `true` and decrements on the transition to `false` OR on unmount, so a
 * component that toggles `active` repeatedly, or unmounts while still
 * active, can never leave the counter positive with nothing on screen.
 */
export function useScreenOccupant(active: boolean): void {
  useEffect(() => {
    if (!active) return;
    useUIStore.getState().addScreenOccupant();
    return () => useUIStore.getState().removeScreenOccupant();
  }, [active]);
}
