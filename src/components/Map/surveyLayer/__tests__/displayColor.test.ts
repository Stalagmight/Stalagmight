/**
 * Shared display-color resolver tests.
 *
 * These helpers (moved here from the deleted stock `features.ts`) are the single
 * source of per-feature colors for the custom color resolvers (colorResolvers.ts)
 * and the interact-overlay highlights. The tests pin the exact color each mode
 * (section / depth / severity / zero-datum) produces so the custom path can never
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
import { surveyDataFromLegacy } from '../../../../types/survey-data';
import { getMapThemeColors } from '../../../../utils/themeColors';
import { getDepthColor } from '../../../../utils/colors';
import type { Section } from '../../../../types/ipc/Section';
import type { Station } from '../../../../types/ipc/Station';
import { makeStation } from '../../../../test/factories/survey';

const theme = getMapThemeColors();
const depthRange = { min: -10, max: 0 };

/** 2 sections (distinct colors) + stations, with depths for gradient tests. */
function fixture(): { sections: Section[]; stations: Station[] } {
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
  const sections: Section[] = [
    {
      id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(101, 0, -4, 8), shot(102, -4, -10, 12)],
    },
    {
      id: 2, name: 'B', surveyor: null, date: null, color: '#00ff00',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(201, -2, -2, 6)],
    },
  ];
  const st = (
    section_id: number, station_index: number, x: number, y: number, z: number,
    is_zero_datum = false,
  ): Station =>
    makeStation({
      station_ref: { section_id, station_index },
      coordinates: { x, y, z },
      is_zero_datum,
      closures_with: [],
    });
  const stations: Station[] = [
    st(1, 0, 0, 0, 0, true), // zero datum
    st(1, 1, 8, 0, -4),
    st(1, 2, 8, 8, -10),
    st(2, 0, 20, 0, -2),
    st(2, 1, 26, 0, -2),
  ];
  return { sections, stations };
}

function sectionColorOf(sections: Section[], id: number): string {
  return sections.find((s) => s.id === id)!.color;
}

function findShotGlobal(
  data: ReturnType<typeof surveyDataFromLegacy>,
  sectionId: number,
  shotId: number,
): number {
  for (let i = 0; i < data.shotCount; i++) {
    if (data.shotSectionIds[i] === sectionId && data.shotIds[i] === shotId) return i;
  }
  throw new Error(`shot ${sectionId}:${shotId} not found`);
}

function findStationGlobal(
  data: ReturnType<typeof surveyDataFromLegacy>,
  sectionId: number,
  stationIndex: number,
): number {
  for (let g = 0; g < data.stationCount; g++) {
    if (data.stationSectionIds[g] === sectionId && data.stationIndices[g] === stationIndex) return g;
  }
  throw new Error(`station ${sectionId}:${stationIndex} not found`);
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
    const { sections, stations } = fixture();
    const data = surveyDataFromLegacy(sections, stations);
    const ctx = ctxFor(true);
    // shot 101: depth_in 0, depth_out -4.
    const gIdx = findShotGlobal(data, 1, 101);
    const [from, to] = resolveShotEndpointColors(data, sectionColorOf(sections, 1), gIdx, ctx);
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
    const { sections, stations } = fixture();
    const data = surveyDataFromLegacy(sections, stations);
    // shot 201: depth_in -2, depth_out -2 (flat).
    const gIdx = findShotGlobal(data, 2, 201);
    const [from, to] = resolveShotEndpointColors(data, sectionColorOf(sections, 2), gIdx, ctxFor(true));
    expect(from).toBe(to);
  });

  it('section-color mode: both endpoints equal the flat display color', () => {
    const { sections, stations } = fixture();
    const data = surveyDataFromLegacy(sections, stations);
    const ctx = ctxFor(false);
    const gIdx = findShotGlobal(data, 1, 101);
    const base = resolveShotDisplayColor(data, sectionColorOf(sections, 1), gIdx, ctx);
    const [from, to] = resolveShotEndpointColors(data, sectionColorOf(sections, 1), gIdx, ctx);
    expect(from).toBe(base);
    expect(to).toBe(base);
    expect(base).toBe(sectionColorOf(sections, 1));
  });
});

describe('resolveStationDisplayColor / resolveStationStrokeColor', () => {
  const ctxFor = (showDepthGradient: boolean): DisplayColorContext => ({
    showDepthGradient,
    depthRange,
    themeColors: theme,
    showIssueSeverity: false,
  });

  it('flat mode: non-zero-datum station takes its section color; zero-datum takes the zero-datum color', () => {
    const { sections, stations } = fixture();
    const data = surveyDataFromLegacy(sections, stations);
    const ctx = ctxFor(false);
    const plain = findStationGlobal(data, 1, 1);
    expect(resolveStationDisplayColor(data, sectionColorOf(sections, 1), plain, ctx)).toBe(
      sectionColorOf(sections, 1),
    );
    const zero = findStationGlobal(data, 1, 0); // marked zero-datum in the fixture
    expect(resolveStationDisplayColor(data, sectionColorOf(sections, 1), zero, ctx)).toBe(theme.zeroDatum);
  });

  it('depth mode: station fill is the ramp color at its z', () => {
    const { sections, stations } = fixture();
    const data = surveyDataFromLegacy(sections, stations);
    const ctx = ctxFor(true);
    const gIdx = findStationGlobal(data, 1, 2); // z = -10
    expect(resolveStationDisplayColor(data, sectionColorOf(sections, 1), gIdx, ctx)).toBe(
      getDepthColor(-10, theme.depthShallow, theme.depthDeep, depthRange.min, depthRange.max),
    );
  });

  it('stroke: theme stroke in flat mode; zero-datum ring in depth mode', () => {
    expect(resolveStationStrokeColor(false, ctxFor(false))).toBe(theme.strokeColor);
    expect(resolveStationStrokeColor(true, ctxFor(false))).toBe(theme.strokeColor);
    expect(resolveStationStrokeColor(true, ctxFor(true))).toBe(theme.zeroDatum);
    expect(resolveStationStrokeColor(false, ctxFor(true))).toBe(theme.strokeColor);
  });
});
