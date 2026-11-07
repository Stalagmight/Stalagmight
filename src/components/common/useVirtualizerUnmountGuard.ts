import { useLayoutEffect } from 'react';
import type { Virtualizer } from '@tanstack/react-virtual';

/**
 * [#576 NF-W8E3-1] Make a measured virtual list's pending `scrollToIndex` frame
 * harmless once the list unmounts.
 *
 * In dynamic-size mode (rows measured through `measureElement`), TanStack
 * Virtual 3.13's `scrollToIndex` schedules a frame that calls
 * `this.targetWindow.requestAnimationFrame(verify)` without checking
 * `targetWindow`, and the virtualizer's own unmount cleanup sets it to null. A
 * reveal made in the frame before the list goes away - select a shot, then
 * switch to Plan mode, which unmounts the Sections panel - therefore throws
 * "Cannot read properties of null (reading 'requestAnimationFrame')" out of that
 * frame, an uncaught page error.
 *
 * Emptying the measured-element cache on unmount takes the pending frame off
 * that branch: with no measured element the virtualizer is not in dynamic mode,
 * so the frame verifies in place, and its retry path does check `targetWindow`.
 * The instance is discarded with the component, so nothing reads the cache again.
 *
 * Only a measured list that reveals rows needs it: `scrollToIndex` is the one
 * caller of `targetWindow.requestAnimationFrame` in virtual-core 3.13. The
 * Sections panel reveals rows and uses it. DetailPanel's warnings list is
 * measured too but never reveals a row, so it has no such frame (#576 RV9b-8);
 * a list that starts calling `scrollToIndex` takes this hook with it.
 */
export function useVirtualizerUnmountGuard<S extends Element | Window, I extends Element>(
  virtualizer: Virtualizer<S, I>,
): void {
  useLayoutEffect(() => () => virtualizer.elementsCache.clear(), [virtualizer]);
}
