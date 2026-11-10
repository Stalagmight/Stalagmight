import { useMemo } from 'react';
import { useUIStore } from '../../../store/uiStore';
import { isColorScaleLive, useColorScaleStore } from '../../../store/colorScaleStore';
import { useEffectivePalette } from '../../../store/colorScalePalette';
import { getMapThemeColors } from '../../../utils/themeColors';
import {
  THEME_PALETTE_ID,
  resolveChoice,
  type ColorScaleInput,
  type ColorScaleKind,
  type LevelsScale,
  type PaletteChoice,
} from '../../../utils/palettes';

/**
 * The colour-scale input for one kind, or null for the untouched default (Theme
 * palette, forward, auto range) so the renderer keeps its original code path.
 * Re-resolves on a theme switch: the Theme palette's colours are the theme's.
 */
export function colorScaleInputFor(
  kind: ColorScaleKind,
  choice: PaletteChoice,
  scale: LevelsScale | null,
): ColorScaleInput | null {
  if (choice.id === THEME_PALETTE_ID && !choice.reversed && scale === null) return null;
  return { palette: resolveChoice(choice, kind, getMapThemeColors()), scale };
}

/** Same, but always an input (legends and the editor need a palette to draw). */
export function colorScaleInputOrTheme(
  kind: ColorScaleKind,
  choice: PaletteChoice,
  scale: LevelsScale | null,
): ColorScaleInput {
  return { palette: resolveChoice(choice, kind, getMapThemeColors()), scale };
}

export interface MapColorScaleInputs {
  depth: ColorScaleInput | null;
  distance: ColorScaleInput | null;
  /** A palette preview or Levels drag is in progress (fast recolour path). */
  live: boolean;
}

/** The map's current depth and distance colour-scale inputs (hook). */
export function useMapColorScaleInputs(): MapColorScaleInputs {
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  const depthChoice = useEffectivePalette('depth');
  const distanceChoice = useEffectivePalette('distance');
  const mapScale = useColorScaleStore((s) => s.mapScale);
  const live = useColorScaleStore(isColorScaleLive);
  return useMemo(
    () => ({
      depth: colorScaleInputFor('depth', depthChoice, mapScale.depth),
      distance: colorScaleInputFor('distance', distanceChoice, mapScale.distance),
      live,
    }),
    // resolvedTheme: the Theme palette resolves from the live theme tokens.
    [depthChoice, distanceChoice, mapScale, live, resolvedTheme],
  );
}
