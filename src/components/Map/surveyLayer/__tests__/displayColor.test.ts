/**
 * Shared display-color resolver tests.
 *
 * These helpers (moved here from the deleted stock `features.ts`) are the single
 * source of per-feature colors for the custom color resolvers (colorResolvers.ts)
 * and the interact-overlay highlights. The tests pin the exact color each mode
 * (line / depth / severity / zero-datum) produces so the custom path can never
 * drift from what the resolvers return.
 */
import { describe, expect, it } from 'vitest';
import {
  normalizeDepthRange,
  resolveShotDisplayColor,
  resolveShotEndpointColors,
  resolveStationDisplayColor,
  resolveStationStrokeColor,
  type DisplayColorContext,
} from '../displayColor';
import { closureErrorScore, issueScoreColor } from '../issueColor';
import { defaultIssueLevels } from '../issueScale';
import {
  STATION_FLAG_IS_GPS_FIX,
  STATION_FLAG_IS_ZERO_DATUM,
  surveyDataFromLegacy,
  withGpsFixFlags,
} from '../../../../types/survey-data';
import type { DatumRole } from '../../../../types/ipc/DatumRole';
import { makeColorResolvers } from '../customRender/colorResolvers';
import { packCssColor } from '../customRender/colorPack';
import { getMapThemeColors } from '../../../../utils/themeColors';
import { getDepthColor } from '../../../../utils/colors';
import type { Line } from '../../../../types/ipc/Line';
import type { Station } from '../../../../types/ipc/Station';

const theme = getMapThemeColors();
const depthRange = { min: -10, max: 0 };

/** 2 lines (distinct colors) + stations, with depths for gradient tests. */
function fixture(): { lines: Line[]; stations: Station[] } {
  const shot = (id: number, depth_in: number, depth_out: number, len: number) => ({
    id,
    azimuth_in: 0,
    azimuth_out: 0,
    length: len,
    depth_in,
    depth_out,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal' as const,
    marker_index: null,
    temperature: null,
  });
  const lines: Line[] = [
    {
      id: 1, name: 'A', color: '#ff0000',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(101, 0, -4, 8), shot(102, -4, -10, 12)],
    },
    {
      id: 2, name: 'B', color: '#00ff00',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(201, -2, -2, 6)],
    },
  ];
  const st = (
    line_id: number, station_index: number, x: number, y: number, z: number,
    is_zero_datum = false,
  ): Station => ({
    station_ref: { line_id, station_index },
    coordinates: { x, y, z },
    is_zero_datum,
    gps_coordinates: null,
    merged_with: [], id: 0, name: '',
    closures_with: [],
  });
  const stations: Station[] = [
    st(1, 0, 0, 0, 0, true), // zero datum
    st(1, 1, 8, 0, -4),
    st(1, 2, 8, 8, -10),
    st(2, 0, 20, 0, -2),
    st(2, 1, 26, 0, -2),
  ];
  return { lines, stations };
}

function lineColorOf(lines: Line[], id: number): string {
  return lines.find((s) => s.id === id)!.color;
}

function findShotGlobal(
  data: ReturnType<typeof surveyDataFromLegacy>,
  lineId: number,
  shotId: number,
): number {
  for (let i = 0; i < data.shotCount; i++) {
    if (data.shotLineIds[i] === lineId && data.shotIds[i] === shotId) return i;
  }
  throw new Error(`shot ${lineId}:${shotId} not found`);
}

function findStationGlobal(
  data: ReturnType<typeof surveyDataFromLegacy>,
  lineId: number,
  stationIndex: number,
): number {
  for (let g = 0; g < data.stationCount; g++) {
    if (data.stationLineIds[g] === lineId && data.stationIndices[g] === stationIndex) return g;
  }
  throw new Error(`station ${lineId}:${stationIndex} not found`);
}

describe('normalizeDepthRange', () => {
  it('orders min/max ascending', () => {
    expect(normalizeDepthRange({ min: 0, max: -10 })).toEqual({ min: -10, max: 0 });
    expect(normalizeDepthRange({ min: -3, max: 5 })).toEqual({ min: -3, max: 5 });
  });

  it('substitutes defaults for non-finite bounds', () => {
    expect(normalizeDepthRange({ min: NaN, max: NaN })).toEqual({ min: 0, max: 50 });
    expect(normalizeDepthRange({ min: Infinity, max: -Infinity })).toEqual({ min: 0, max: 50 });
  });
});

describe('resolveShotEndpointColors (custom-render depth gradient)', () => {
  const ctxFor = (showDepthGradient: boolean): DisplayColorContext => ({
    showDepthGradient,
    depthRange,
    themeColors: theme,
    showIssueSeverity: false,
  });

  it('depth mode: endpoints are the in/out depth-ramp colors (stock gradient endpoints)', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    const ctx = ctxFor(true);
    // shot 101: depth_in 0, depth_out -4.
    const gIdx = findShotGlobal(data, 1, 101);
    const [from, to] = resolveShotEndpointColors(data, lineColorOf(lines, 1), gIdx, ctx);
    expect(from).toBe(
      getDepthColor(0, theme.depthShallow, theme.depthDeep, depthRange.min, depthRange.max),
    );
    expect(to).toBe(
      getDepthColor(-4, theme.depthShallow, theme.depthDeep, depthRange.min, depthRange.max),
    );
    // The two endpoints differ (there is a real ramp along this shot).
    expect(from).not.toBe(to);
  });

  it('depth mode: a flat shot has equal endpoints', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    // shot 201: depth_in -2, depth_out -2 (flat).
    const gIdx = findShotGlobal(data, 2, 201);
    const [from, to] = resolveShotEndpointColors(data, lineColorOf(lines, 2), gIdx, ctxFor(true));
    expect(from).toBe(to);
  });

  it('line-color mode: both endpoints equal the flat display color', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    const ctx = ctxFor(false);
    const gIdx = findShotGlobal(data, 1, 101);
    const base = resolveShotDisplayColor(data, lineColorOf(lines, 1), gIdx, ctx);
    const [from, to] = resolveShotEndpointColors(data, lineColorOf(lines, 1), gIdx, ctx);
    expect(from).toBe(base);
    expect(to).toBe(base);
    expect(base).toBe(lineColorOf(lines, 1));
  });
});

describe('resolveStationDisplayColor / resolveStationStrokeColor', () => {
  const ctxFor = (showDepthGradient: boolean): DisplayColorContext => ({
    showDepthGradient,
    depthRange,
    themeColors: theme,
    showIssueSeverity: false,
  });

  it('flat mode: non-zero-datum station takes its line color; zero-datum takes the zero-datum color', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    const ctx = ctxFor(false);
    const plain = findStationGlobal(data, 1, 1);
    expect(resolveStationDisplayColor(data, lineColorOf(lines, 1), plain, ctx)).toBe(
      lineColorOf(lines, 1),
    );
    const zero = findStationGlobal(data, 1, 0); // marked zero-datum in the fixture
    expect(resolveStationDisplayColor(data, lineColorOf(lines, 1), zero, ctx)).toBe(theme.zeroDatum);
  });

  it('depth mode: station fill is the ramp color at its z', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    const ctx = ctxFor(true);
    const gIdx = findStationGlobal(data, 1, 2); // z = -10
    expect(resolveStationDisplayColor(data, lineColorOf(lines, 1), gIdx, ctx)).toBe(
      getDepthColor(-10, theme.depthShallow, theme.depthDeep, depthRange.min, depthRange.max),
    );
  });

  it('stroke: theme stroke in flat mode; zero-datum ring in depth mode', () => {
    expect(resolveStationStrokeColor(0, ctxFor(false))).toBe(theme.strokeColor);
    expect(resolveStationStrokeColor(STATION_FLAG_IS_ZERO_DATUM, ctxFor(false))).toBe(theme.strokeColor);
    expect(resolveStationStrokeColor(STATION_FLAG_IS_ZERO_DATUM, ctxFor(true))).toBe(theme.zeroDatum);
    expect(resolveStationStrokeColor(0, ctxFor(true))).toBe(theme.strokeColor);
  });
});

describe('GPS fix marker (#741)', () => {
  const ctxFor = (showDepthGradient: boolean): DisplayColorContext => ({
    showDepthGradient,
    depthRange,
    themeColors: theme,
    showIssueSeverity: false,
  });
  /** Two placed datums, 1:0 and 2:0; the backend's role for 2:0 is `role`. */
  const dataWith = (role: DatumRole) => {
    const { lines, stations } = fixture();
    stations[3] = { ...stations[3]!, is_zero_datum: true };
    return withGpsFixFlags(surveyDataFromLegacy(lines, stations), [
      { station_ref: { line_id: 1, station_index: 0 }, role: 'Placement' },
      { station_ref: { line_id: 2, station_index: 0 }, role },
    ]);
  };

  it('flags exactly the stations whose wire role is a GPS fix', () => {
    const fix = dataWith('GpsFix');
    const flagged = [...fix.stationFlags.keys()].filter((g) => (fix.stationFlags[g]! & STATION_FLAG_IS_GPS_FIX) !== 0);
    expect(flagged).toEqual([findStationGlobal(fix, 2, 0)]);
    const placed = dataWith('Placement');
    expect(placed.stationFlags.every((f) => (f & STATION_FLAG_IS_GPS_FIX) === 0)).toBe(true);
  });

  it('leaves the data untouched when nothing is a fix', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    expect(withGpsFixFlags(data, [{ station_ref: { line_id: 1, station_index: 0 }, role: 'Placement' }])).toBe(data);
  });

  for (const depth of [false, true]) {
    it(`draws a fix as a hollow zero-datum ring, unlike the placement datum (depth ${depth})`, () => {
      const fix = dataWith('GpsFix');
      const g = findStationGlobal(fix, 2, 0);
      const ctx = ctxFor(depth);
      expect(resolveStationDisplayColor(fix, '#00ff00', g, ctx)).toBe(theme.background);
      expect(resolveStationStrokeColor(fix.stationFlags[g]!, ctx)).toBe(theme.zeroDatum);
      // The renderer's packed bytes follow the same rule.
      const resolvers = makeColorResolvers(fix, ctx);
      expect(resolvers.fillForStation(g)).toEqual(packCssColor(theme.background));
      expect(resolvers.strokeForStation(g)).toEqual(packCssColor(theme.zeroDatum));

      // The same station as a placement datum is solid: never the background.
      const placed = dataWith('Placement');
      const fill = resolveStationDisplayColor(placed, '#00ff00', g, ctx);
      expect(fill).not.toBe(theme.background);
      if (!depth) expect(fill).toBe(theme.zeroDatum);
      expect(placed.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM).not.toBe(0);
    });
  }
});

describe('Survey Issues gradient (issue scores)', () => {
  it('ramps green -> yellow at the threshold -> red at 3x', () => {
    expect(issueScoreColor(theme, 0)).toBe('#22c55e');
    expect(issueScoreColor(theme, 0.5)).toBe('#86bc33');
    expect(issueScoreColor(theme, 1)).toBe('#eab308');
    expect(issueScoreColor(theme, 2)).toBe('#ed7c26');
    expect(issueScoreColor(theme, 3)).toBe('#ef4444');
  });

  it('clamps out-of-range and NaN scores', () => {
    expect(issueScoreColor(theme, 7)).toBe('#ef4444');
    expect(issueScoreColor(theme, -1)).toBe('#22c55e');
    expect(issueScoreColor(theme, NaN)).toBe('#22c55e');
  });

  it('quantises to hundredths, so at most 301 distinct colours exist', () => {
    const seen = new Set<string>();
    for (let s = 0; s <= 3; s += 0.0007) seen.add(issueScoreColor(theme, s));
    expect(seen.size).toBeLessThanOrEqual(301);
    expect(issueScoreColor(theme, 1.234)).toBe(issueScoreColor(theme, 1.23));
  });

  it('colours a shot and a station by their scores, flat along the shot', () => {
    const { lines, stations } = fixture();
    const data = surveyDataFromLegacy(lines, stations);
    const gShot = findShotGlobal(data, 1, 102);
    const shotScore = new Float32Array(data.shotCount);
    const stationScore = new Float32Array(data.stationCount);
    shotScore[gShot] = 2;
    stationScore[0] = 1;
    const ctx: DisplayColorContext = {
      showDepthGradient: true, // Survey Issues takes precedence over depth
      depthRange,
      themeColors: theme,
      showIssueSeverity: true,
      issueScores: { shotScore, stationScore },
    };
    expect(resolveShotDisplayColor(data, '#ff0000', gShot, ctx)).toBe('#ed7c26');
    expect(resolveShotEndpointColors(data, '#ff0000', gShot, ctx)).toEqual(['#ed7c26', '#ed7c26']);
    expect(resolveStationDisplayColor(data, '#ff0000', 0, ctx)).toBe('#eab308');
    expect(resolveStationDisplayColor(data, '#ff0000', 1, ctx)).toBe('#22c55e');
  });
});

describe('closureErrorScore', () => {
  const levels = defaultIssueLevels({ azimuthDiff: 1, pitchDiff: 1, inclinationDepthDiff: 1 }).closure;
  it('maps the closure colour band edges onto the shot score scale with the default levels', () => {
    expect(closureErrorScore(0, levels)).toBe(0);
    expect(closureErrorScore(0.5, levels)).toBe(0.5);
    expect(closureErrorScore(1, levels)).toBe(1);
    expect(closureErrorScore(3, levels)).toBe(2);
    expect(closureErrorScore(5, levels)).toBe(3);
    expect(closureErrorScore(40, levels)).toBe(3);
    expect(closureErrorScore(NaN, levels)).toBe(0);
  });
  it('follows the closure levels the user set', () => {
    expect(closureErrorScore(2, { min: 1, warn: 2, max: 4 })).toBe(1);
    expect(closureErrorScore(1, { min: 1, warn: 2, max: 4 })).toBe(0);
    expect(closureErrorScore(3, { min: 1, warn: 2, max: 4 })).toBe(2);
  });
});
