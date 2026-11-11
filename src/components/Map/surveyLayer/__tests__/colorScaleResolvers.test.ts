/**
 * The user's palette + Levels range through the map's colour paths: the depth
 * resolvers (shots, endpoints, stations, labels), the distance overlay, the
 * interact overlay's MapLibre expression, and the incremental gate. The default
 * (no scale, or the Theme palette over an auto range) must stay byte-identical.
 */
import { describe, expect, it } from 'vitest';
import {
  resolveShotDisplayColor,
  resolveShotEndpointColors,
  resolveStationDisplayColor,
  type DisplayColorContext,
  type OverlayColorInput,
} from '../displayColor';
import { makeOverlayColor } from '../customRender/overlayColor';
import { sameColorGlobals } from '../customRender/incremental';
import { labelColorContextFrom, resolveStationLabelColor } from '../customRender/labelText';
import { surveyDataFromLegacy } from '../../../../types/survey-data';
import { getMapThemeColors } from '../../../../utils/themeColors';
import { distanceColorExpression, DISTANCE_FAR, DISTANCE_NEAR } from '../../../../utils/distanceGradient';
import { levelsT, resolveChoice, THEME_PALETTE_ID, type ColorScaleInput } from '../../../../utils/palettes';
import type { Section } from '../../../../types/ipc/Section';
import type { Station } from '../../../../types/ipc/Station';

const theme = getMapThemeColors();
const depthRange = { min: -10, max: 0 };

function surveyData() {
  const shot = (id: number, depth_in: number, depth_out: number) => ({
    id, azimuth_in: 0, azimuth_out: 0, length: 5, depth_in, depth_out, pitch_in: null, pitch_out: null,
    left: null, right: null, up: null, down: null, comment: null, kind: 'Normal' as const,
    marker_index: null, temperature: null,
  });
  const sections: Section[] = [
    {
      id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000', direction: 'in', quality: 1,
      next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(101, 0, -4), shot(102, -4, -10)],
    },
  ];
  const st = (i: number, z: number): Station => ({
    station_ref: { section_id: 1, station_index: i }, coordinates: { x: i * 5, y: 0, z },
    is_zero_datum: false, gps_coordinates: null, merged_with: [], closures_with: [],
  });
  return surveyDataFromLegacy(sections, [st(0, 0), st(1, -4), st(2, -10)]);
}

const ctx = (depthScale: ColorScaleInput | null = null): DisplayColorContext => ({
  showDepthGradient: true, depthRange, themeColors: theme, showIssueSeverity: false, depthScale,
});

const themeInput = (kind: 'depth' | 'distance'): ColorScaleInput => ({
  palette: resolveChoice({ id: THEME_PALETTE_ID, reversed: false }, kind, theme),
  scale: null,
});
const viridis = resolveChoice({ id: 'mpl.viridis', reversed: false }, 'depth', theme);

describe('depth colours through a colour scale', () => {
  const data = surveyData();

  it('Theme over an auto range is byte-identical to no scale at all', () => {
    for (let g = 0; g < data.shotCount; g++) {
      expect(resolveShotDisplayColor(data, '#ff0000', g, ctx(themeInput('depth')))).toBe(
        resolveShotDisplayColor(data, '#ff0000', g, ctx()),
      );
      expect(resolveShotEndpointColors(data, '#ff0000', g, ctx(themeInput('depth')))).toEqual(
        resolveShotEndpointColors(data, '#ff0000', g, ctx()),
      );
    }
    for (let g = 0; g < data.stationCount; g++) {
      expect(resolveStationDisplayColor(data, '#ff0000', g, ctx(themeInput('depth')))).toBe(
        resolveStationDisplayColor(data, '#ff0000', g, ctx()),
      );
    }
  });

  it('maps the scale ends to the palette ends and clamps beyond them', () => {
    const scaled = ctx({ palette: viridis, scale: { min: -8, max: -2, gamma: 1 } });
    // Station z: 0 (above max -> end), -4 (inside), -10 (below min -> start).
    expect(resolveStationDisplayColor(data, '#ff0000', 0, scaled)).toBe(viridis.hex(1));
    expect(resolveStationDisplayColor(data, '#ff0000', 2, scaled)).toBe(viridis.hex(0));
    expect(resolveStationDisplayColor(data, '#ff0000', 1, scaled)).toBe(viridis.hex(4 / 6));
  });

  it('applies gamma like Levels', () => {
    const scale = { min: -10, max: 0, gamma: 2 };
    const scaled = ctx({ palette: viridis, scale });
    expect(resolveStationDisplayColor(data, '#ff0000', 1, scaled)).toBe(viridis.hex(levelsT(-4, scale)));
    expect(levelsT(-4, scale)).toBeCloseTo(Math.sqrt(0.6), 12);
  });

  it('colours labels from the same scale', () => {
    const scale = { min: -10, max: 0, gamma: 1 };
    const labelCtx = labelColorContextFrom(true, depthRange, theme, { palette: viridis, scale });
    expect(resolveStationLabelColor(-10, labelCtx)).toBe(viridis.hex(0));
    expect(resolveStationLabelColor(-10, labelColorContextFrom(true, depthRange, theme))).toBe(
      resolveStationDisplayColor(data, '#ff0000', 2, ctx()),
    );
  });
});

describe('distance overlay through a colour scale', () => {
  const base: OverlayColorInput = { mode: 'distance', shots: new Map(), stations: new Map(), max: 40 };

  it('Theme over an auto range matches the fixed ramp byte for byte', () => {
    const plain = makeOverlayColor(base, theme.grayLine);
    const themed = makeOverlayColor({ ...base, scale: themeInput('distance') }, theme.grayLine);
    for (const v of [-1, 0, 0.001, 3.3, 20, 39.999, 40, 55, Number.NaN]) {
      expect(themed(v)).toEqual(plain(v));
    }
  });

  it('keeps unreachable grey and recolours the reachable range', () => {
    const pal = resolveChoice({ id: 'mpl.magma', reversed: false }, 'distance', theme);
    const colour = makeOverlayColor({ ...base, scale: { palette: pal, scale: { min: 10, max: 30, gamma: 1 } } }, '#777777');
    expect(colour(-1)).toEqual([0x77, 0x77, 0x77, 255]);
    expect(colour(5)).toEqual([...pal.rgb(0)!, 255]);
    expect(colour(35)).toEqual([...pal.rgb(1)!, 255]);
  });

  it('builds the interact-overlay expression from the scale', () => {
    const plain = distanceColorExpression(DISTANCE_NEAR, DISTANCE_FAR, 40, '#777777');
    expect(JSON.stringify(plain)).toContain(DISTANCE_NEAR);
    const pal = resolveChoice({ id: 'mpl.viridis', reversed: false }, 'distance', theme);
    const scaled = distanceColorExpression(DISTANCE_NEAR, DISTANCE_FAR, 40, '#777777', undefined, {
      palette: pal,
      scale: null,
    }) as unknown[];
    const ramp = (scaled[3] as unknown[])[3] as unknown[];
    expect(ramp[0]).toBe('interpolate');
    expect(ramp.length).toBe(3 + 33 * 2);
    expect(ramp[3]).toBe(0);
    expect(ramp[4]).toBe(pal.hex(0));
    expect(ramp[ramp.length - 2]).toBe(40);
    const tab10 = resolveChoice({ id: 'mpl.tab10', reversed: false }, 'distance', theme);
    const stepped = distanceColorExpression(DISTANCE_NEAR, DISTANCE_FAR, 40, '#777777', undefined, {
      palette: tab10,
      scale: null,
    }) as unknown[];
    const step = (stepped[3] as unknown[])[3] as unknown[];
    expect(step[0]).toBe('step');
    expect(step[3]).toBe(4); // first band edge: 40 * (1/10)^1
  });
});

describe('incremental gate', () => {
  it('treats a palette or range change as a global colour change', () => {
    const a = ctx({ palette: viridis, scale: { min: -10, max: 0, gamma: 1 } });
    const same = ctx({ palette: viridis, scale: { min: -10, max: 0, gamma: 1 } });
    const otherRange = ctx({ palette: viridis, scale: { min: -10, max: 0, gamma: 1.5 } });
    const otherPalette = ctx({ palette: resolveChoice({ id: 'mpl.magma', reversed: false }, 'depth', theme), scale: null });
    expect(sameColorGlobals(a, same)).toBe(true);
    expect(sameColorGlobals(a, otherRange)).toBe(false);
    expect(sameColorGlobals(a, otherPalette)).toBe(false);
    expect(sameColorGlobals(ctx(), a)).toBe(false);
  });
});
