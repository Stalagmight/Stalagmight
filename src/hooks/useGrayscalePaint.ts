import { createContext, useCallback, useContext } from 'react';
import { grayscaleColor } from '../utils/grayscaleTokens';

/**
 * True inside an element dimmed to gray without a CSS filter (#576 NF-W13FE-CSS-D1, #605):
 * the placement-mode dim of the docked panels. `useGrayscaleTokens` re-declares the colour
 * custom properties on the dimmed element, which grays everything painted from a token; a
 * colour that is not a token (a section's own colour on its swatch) reads this to gray itself.
 */
export const GrayscalePaintContext = createContext(false);

/**
 * `paint(color)`: `color` as the dimmed element paints it (desaturated the way the
 * `grayscale(1)` filter did, `../utils/grayscaleTokens.ts`) inside a `GrayscalePaintContext`
 * that is on, else `color` itself. A value that is not a plain colour is returned as is.
 */
export function useGrayscalePaint(): (color: string) => string {
  const gray = useContext(GrayscalePaintContext);
  return useCallback((color: string) => (gray ? (grayscaleColor(color) ?? color) : color), [gray]);
}
