import { useCallback, useRef } from 'react';
import { useUIStore } from '../store/uiStore';

/**
 * Keep the toast stack above a bar along the window's bottom edge (#576 W70).
 *
 * The stack is anchored `bottom-6` from the window's bottom edge, so a toast covered
 * the status bar of the project list and the project view. Attach the returned
 * callback ref to such a bar: while it is mounted its height is registered as a toast
 * bottom offset under `owner` (re-measured when it resizes, dropped when it unmounts),
 * and the stack keeps its usual gap above the bar instead of covering it - the same
 * clearance mechanism the DMP wizard footer uses (`registerToastBottomOffset`).
 *
 * A callback ref, not an effect on a ref object: the project view renders its status
 * bar only once a project is open, so the bar can mount after the component did.
 */
export function useToastBottomClearance(owner: string): (el: HTMLElement | null) => void {
  const registerToastBottomOffset = useUIStore((s) => s.registerToastBottomOffset);
  const unregisterToastBottomOffset = useUIStore((s) => s.unregisterToastBottomOffset);
  const release = useRef<(() => void) | null>(null);
  return useCallback(
    (el: HTMLElement | null) => {
      release.current?.();
      release.current = null;
      if (!el) return;
      // offsetHeight: CSS px, the unit of the stack's `bottom` (as `--statusbar-h`).
      const report = () => registerToastBottomOffset(owner, el.offsetHeight);
      report();
      const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(report);
      observer?.observe(el);
      release.current = () => {
        observer?.disconnect();
        unregisterToastBottomOffset(owner);
      };
    },
    [owner, registerToastBottomOffset, unregisterToastBottomOffset],
  );
}
