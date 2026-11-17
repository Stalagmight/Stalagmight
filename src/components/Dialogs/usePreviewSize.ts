/**
 * Measures the wizard's map column and returns the preview size that FILLS it.
 *
 * Was `usePreviewSquareSize`, which returned the side of the largest square that
 * fit (#93). The square is what made the divider's upper range useless: the side
 * had to be the smaller of the two axes, so once the column was wider than the
 * band was tall, every extra pixel of column became dead space either side of a
 * centred square - on a 1024x640 window nothing past ~500px bought any map, yet
 * the divider still dragged to 900 (#567 item 1).
 *
 * The square was never a renderer requirement. `LinePreview` is a MapLibre
 * viewport that fits its bounds with padding taken from its container's own
 * clientWidth and clientHeight independently (LinePreview.tsx), so a
 * non-square viewport is drawn correctly - it just shows more cave. The only
 * square assumption was the single `size` prop, which now takes an optional
 * `height` beside it.
 *
 * Its own module because `DmpNewImportDialog` is far past the file-size ratchet's
 * hard budget - anything that can leave, must.
 */
import { useCallback, useRef, useState } from 'react';

/** Pixel size of the preview box. Square when the two are equal. */
export interface PreviewSize {
  width: number;
  height: number;
}

/**
 * Returns a callback ref + the pixel size of a preview that fills the ref'd
 * container (minus the preview panel's `p-4`), each axis independently clamped
 * to `[min, max]`. Without the clamp the preview overflows and is clipped by the
 * dialog's `overflow-hidden` when the window is near its 600px minimum height
 * (#93 review). Reacts to window/layout resizes via ResizeObserver, which is
 * also what carries a divider drag into the preview.
 */
export function usePreviewSize(
  max = 440,
  min = 240,
): [(el: HTMLDivElement | null) => void, PreviewSize] {
  // Seeded at the MINIMUM, not the max: the ref callback measures on attach, so
  // this value is on screen for at most one frame - and at `max` that frame is
  // a 900px-tall preview inside a ~600px column, i.e. a visible overflow before
  // the first measurement lands. Too small is invisible; too big is not.
  const [size, setSize] = useState<PreviewSize>({ width: min, height: min });
  const elRef = useRef<HTMLDivElement | null>(null);
  const observerRef = useRef<ResizeObserver | null>(null);

  const measure = useCallback(() => {
    const el = elRef.current;
    if (!el) return;
    // Preview panel adds p-4 (1rem on every side = 32px per axis) around the
    // preview; clientWidth/clientHeight include that padding, so subtracting it
    // gives the content box exactly.
    const fit = (available: number) => Math.max(min, Math.min(max, Math.floor(available)));
    const next = { width: fit(el.clientWidth - 32), height: fit(el.clientHeight - 32) };
    // Same numbers, same object: the observer fires for every drag frame and for
    // resizes on the other axis, and a fresh object each time would re-render the
    // preview (and its MapLibre subtree) for no change.
    setSize((prev) => (prev.width === next.width && prev.height === next.height ? prev : next));
  }, [max, min]);

  const setRef = useCallback(
    (el: HTMLDivElement | null) => {
      observerRef.current?.disconnect();
      elRef.current = el;
      if (el) {
        measure();
        observerRef.current = new ResizeObserver(measure);
        observerRef.current.observe(el);
      }
    },
    [measure],
  );

  return [setRef, size];
}
