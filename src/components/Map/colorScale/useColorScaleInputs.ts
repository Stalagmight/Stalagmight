import { useEffect, useMemo } from 'react';
import { useUIStore } from '../../../store/uiStore';
import { isColorScaleLive, useColorScaleStore } from '../../../store/colorScaleStore';
import { currentMapPalette, useEffectivePalette } from '../../../store/colorScalePalette';
import { useProjectStore } from '../../../store/projectStore';
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

/** The minimal SurveyLayer surface the colour-scale driver needs. */
export interface ColorScaleTarget {
  setColorScales(depth: ColorScaleInput | null, distance: ColorScaleInput | null): void;
  previewColorScales(depth: ColorScaleInput | null, distance: ColorScaleInput | null): void;
}

/** Push the current colour scales to a layer: preview while live, else commit. */
export function applyColorScales(target: ColorScaleTarget): void {
  const cs = useColorScaleStore.getState();
  const persisted = currentMapPalette();
  const choice = (kind: ColorScaleKind): PaletteChoice =>
    cs.palettePreview && cs.palettePreview.kind === kind ? cs.palettePreview.choice : persisted[kind];
  const depth = colorScaleInputFor('depth', choice('depth'), cs.mapScale.depth);
  const distance = colorScaleInputFor('distance', choice('distance'), cs.mapScale.distance);
  if (isColorScaleLive(cs)) target.previewColorScales(depth, distance);
  else target.setColorScales(depth, distance);
}

/**
 * Drive a SurveyLayer from the colour-scale stores by subscription, outside
 * React rendering: a Levels drag updates the store on every pointer move, and
 * re-rendering MapCanvas for each would cost more than the recolour itself.
 * Re-applies on a theme switch (the Theme palette's colours are the theme's).
 */
export function useSurveyLayerColorScales(
  layerRef: { readonly current: ColorScaleTarget | null },
  mapLoaded: boolean,
): void {
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  useEffect(() => {
    if (!mapLoaded) return undefined;
    const apply = (): void => {
      if (layerRef.current) applyColorScales(layerRef.current);
    };
    apply();
    const offScale = useColorScaleStore.subscribe(apply);
    const offConfig = useProjectStore.subscribe((s, prev) => {
      if (s.config?.map_palette !== prev.config?.map_palette) apply();
    });
    return () => {
      offScale();
      offConfig();
    };
  }, [layerRef, mapLoaded, resolvedTheme]);
}
