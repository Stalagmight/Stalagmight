import { useEffect, type RefObject } from 'react';
import { useLatest } from './useLatest';
import { attachLongPressTouch } from '../utils/longPressTouch';

/**
 * A finger's way into a context menu only a right click can open (#338).
 *
 * Bound once to a container, not to each of its descendants: the hold
 * synthesises the `contextmenu` event on whatever the finger came down on, and
 * the React `onContextMenu` handlers already there do the rest - target
 * resolution, moving the selection, focus. Full right-click parity, with no
 * second copy of those rules to keep in step.
 *
 * `dispatchEvent` returning false (the event was prevented) is what "a menu
 * opened" means here: a handler that opens one prevents the browser's own menu,
 * and a handler with nothing to show does not. That boolean is exactly what
 * `attachLongPressTouch` wants, so the tap ending the press is swallowed only
 * when there is a menu for it to dismiss.
 *
 * Nothing here blocks touchstart or touchmove - see `longPressTouch` - because
 * the container this hangs off is usually the scroller, and scrolling has to
 * keep working through a press that goes nowhere.
 */
export function useTouchContextMenu(
  ref: RefObject<HTMLElement | null>,
  options?: { enabled?: boolean },
): void {
  const enabledRef = useLatest(options?.enabled ?? true);

  useEffect(() => {
    const root = ref.current;
    if (!root) return undefined;

    return attachLongPressTouch(root, {
      isEnabled: () => enabledRef.current,
      onLongPress: ({ clientX, clientY, target }) => {
        if (!(target instanceof Element) || !root.contains(target)) return false;
        // Text entry keeps its native long press: the caret/paste gesture is
        // what the user is reaching for inside a field, not our menu.
        if (target.closest('input, textarea, select, [contenteditable="true"]')) return false;

        const menuEvent = new MouseEvent('contextmenu', {
          bubbles: true,
          cancelable: true,
          clientX,
          clientY,
          button: 2,
          buttons: 2,
          // No `view`: nothing on the contextmenu path reads it, and the
          // ambient window is not always the document's own Window (a test
          // realm is not), which makes passing one a portability hazard for
          // no gain.
        });
        return !target.dispatchEvent(menuEvent);
      },
    });
  }, [ref, enabledRef]);
}
