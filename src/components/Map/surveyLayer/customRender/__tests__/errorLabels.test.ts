import { afterEach, describe, expect, it } from 'vitest';
import { formatClosureErrorLabel, formatShotErrorLabel, closureErrorLabelStyle } from '../errorLabelText';
import { severityTextColor } from '../../issueColor';
import { buildLabelDescriptors, type LabelBuildContext } from '../labelTessellate';
import { LABEL_KIND, LABEL_FLAG } from '../labelShaders';
import { LABEL_KIND_LAYOUT, shapeLabel, shapedExtent } from '../labelLayout';
import { GlyphAtlas, SDF_BUFFER, type GlyphRasterizer, type RasterizedGlyph } from '../glyphAtlas';
import { packCssColor } from '../colorPack';
import { resolveIssueRender, type ErrorLabelFormat, type ErrorLabelInput } from '../../issueScore';
import { buildSurveyData, oneShot, warn, type ShotReadings } from '../../__tests__/surveyQualityFixture';
import { getMapThemeColors } from '../../../../../utils/themeColors';
import { shotKindToFlags, type SurveyData } from '../../../../../types/survey-data';
import type { LoopClosure } from '../../../../../types/project';
import type { ProjectWarning } from '../../../../../types/commands';
import { formatFixed, setNumberFormatLocale } from '../../../../../i18n/format';

const theme = getMapThemeColors();
const T = { azimuthDiff: 20, pitchDiff: 10, inclinationDepthDiff: 15 };
const FORMAT: ErrorLabelFormat = {
  precision: { length: 1, coordinates: 2, depth: 1, azimuth: 1, inclination: 1, gps: 6 },
  lengthUnit: 'meters',
  abbrev: { az: 'Δaz', inc: 'Δinc', dep: 'Δi/d', len: 'ΔL', loop: 'Loop' },
};
const NO_CLOSURES: LoopClosure[] = [];

function labelInput(
  data: SurveyData,
  opts: { warnings?: ProjectWarning[]; format?: ErrorLabelFormat; closures?: LoopClosure[] } = {},
): ErrorLabelInput {
  const render = resolveIssueRender(
    data,
    { color: false, labels: opts.format ?? FORMAT, warnings: opts.warnings ?? [], thresholds: T },
    opts.closures ?? NO_CLOSURES,
  );
  return render!.labels!;
}

const shotLabel = (r: ShotReadings, opts: Parameters<typeof labelInput>[1] = {}) => {
  const data = oneShot(r);
  return formatShotErrorLabel(labelInput(data, opts), 0);
};

describe('formatShotErrorLabel', () => {
  it('puts every disagreement of the shot in ONE label, in a fixed order', () => {
    expect(
      shotLabel({ azimuthIn: 10, azimuthOut: 33.4, length: 10, depthIn: 0, depthOut: 5, pitchIn: 18, pitchOut: 22 }),
    ).toBe('Δaz 23.4° Δinc 4.0° Δi/d -10.0° ΔL -4.6m');
  });

  it('has no label for a shot whose readings agree', () => {
    expect(shotLabel({ azimuthIn: 90, azimuthOut: 90 })).toBeNull();
    expect(shotLabel({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 30, pitchOut: 30 })).toBeNull();
  });

  it('drops a part that rounds to zero at the user precision', () => {
    expect(shotLabel({ azimuthIn: 90, azimuthOut: 90.04 })).toBeNull();
    expect(shotLabel({ azimuthIn: 90, azimuthOut: 90.06 })).toBe('Δaz 0.1°');
    const whole = { ...FORMAT, precision: { ...FORMAT.precision!, azimuth: 0 } };
    expect(shotLabel({ azimuthIn: 90, azimuthOut: 90.4 }, { format: whole })).toBeNull();
  });

  it('signs the inclination-vs-depth and length deltas, never the in/out ones', () => {
    expect(shotLabel({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 40, pitchOut: 40 })).toBe('Δi/d +10.0° ΔL +2.2m');
    expect(shotLabel({ azimuthIn: 33, azimuthOut: 10 })).toBe('Δaz 23.0°');
  });

  it('drops the parts of an ignored warning', () => {
    const data = oneShot({ azimuthIn: 10, azimuthOut: 40, length: 10, depthIn: 0, depthOut: 5, pitchIn: 40, pitchOut: 40 });
    const ignoredAz = labelInput(data, { warnings: [warn(1, 101, 'warning', { code: 'AZIMUTH_DEVIATION', isResolved: true })] });
    expect(formatShotErrorLabel(ignoredAz, 0)).toBe('Δi/d +10.0° ΔL +2.2m');
    const ignoredDepth = labelInput(data, {
      warnings: [warn(1, 101, 'warning', { code: 'INCLINATION_DEPTH_MISMATCH', isResolved: true })],
    });
    expect(formatShotErrorLabel(ignoredDepth, 0)).toBe('Δaz 30.0°');
  });

  it('converts the length delta to feet', () => {
    const feet = { ...FORMAT, lengthUnit: 'feet' as const };
    expect(shotLabel({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 40, pitchOut: 40 }, { format: feet })).toBe(
      'Δi/d +10.0° ΔL +7.3ft',
    );
  });

  describe('in a number locale whose minus sign is not ASCII (sv-SE)', () => {
    afterEach(() => setNumberFormatLocale('locale'));
    it('still writes an ASCII hyphen', () => {
      setNumberFormatLocale('sv-SE');
      const ch = (cp: number) => String.fromCharCode(cp);
      const nonAsciiDash = new RegExp(`[${ch(0x2010)}-${ch(0x2015)}${ch(0x2212)}]`);
      expect(formatFixed(-1, undefined, 1)).toMatch(nonAsciiDash); // the hazard is real
      const text = shotLabel({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 20, pitchOut: 20 }) ?? '';
      expect(text).toBe('Δi/d -10,0° ΔL -4,6m');
      expect(text).not.toMatch(nonAsciiDash);
    });
  });
});

describe('formatClosureErrorLabel', () => {
  const closure = (pct: number, dist: number) =>
    ({ error_percentage: pct, misclosure_distance: dist }) as unknown as LoopClosure;
  it('shows the stored error percentage and misclosure distance', () => {
    expect(formatClosureErrorLabel(closure(2.14, 0.84), 'Loop', 'meters', 2)).toBe('Loop 2.1% 0.84m');
    expect(formatClosureErrorLabel(closure(2.14, 0.84), 'Loop', 'feet', 1)).toBe('Loop 2.1% 2.8ft');
  });
  it('has no label for non-finite metrics', () => {
    expect(formatClosureErrorLabel(closure(NaN, 0.84), 'Loop', 'meters')).toBeNull();
  });
  it('is coloured and scored like its closure line', () => {
    const dark = { ...theme, background: '#101418' };
    expect(closureErrorLabelStyle(closure(2, 1), dark)).toEqual({ score: 1.5, color: '#eab308' });
    expect(closureErrorLabelStyle(closure(7, 1), dark)).toEqual({ score: 3, color: '#ef4444' });
  });
  it('darkens the text colour on a light map so yellow and green stay readable', () => {
    const light = { ...theme, background: '#f4efe6' };
    expect(closureErrorLabelStyle(closure(2, 1), light).color).toBe('#816204');
    expect(severityTextColor('#22c55e', light)).toBe('#136c34');
    expect(severityTextColor('#22c55e', { ...theme, background: '#101418' })).toBe('#22c55e');
    expect(severityTextColor('#22c55e', { ...theme, background: 'rgb(255, 255, 255)' })).toBe('#22c55e');
  });
});

/** Fixed-metric fake rasterizer (12px advance) so text shapes deterministically. */
function fakeAtlas(): GlyphAtlas {
  const raster: GlyphRasterizer = (c: string): RasterizedGlyph => {
    const empty = c === ' ';
    const w = empty ? 0 : 10;
    const h = empty ? 0 : 18;
    return {
      data: new Uint8Array(Math.max(0, w * h)),
      width: w,
      height: h,
      glyphWidth: Math.max(0, w - 2 * SDF_BUFFER),
      glyphHeight: Math.max(0, h - 2 * SDF_BUFFER),
      glyphTop: 15,
      glyphLeft: 1,
      glyphAdvance: empty ? 6 : 12,
    };
  };
  return new GlyphAtlas({ rasterizer: raster });
}

describe('buildLabelDescriptors: Errors labels', () => {
  /** Two shots in one section: shot 0 disagrees on azimuth by 40 (score 2),
   *  shot 1 (a reference shot) by 10 (score 0.5); a closure between station 0
   *  and station 2. */
  function survey() {
    const data = buildSurveyData({
      sections: [{ id: 1, shotIds: [101, 102] }],
      stations: [
        { sectionId: 1, stationIndex: 0, x: 0, y: 0 },
        { sectionId: 1, stationIndex: 1, x: 10, y: 0 },
        { sectionId: 1, stationIndex: 2, x: 20, y: 4 },
      ],
      shots: { 0: { azimuthIn: 90, azimuthOut: 130 }, 1: { azimuthIn: 90, azimuthOut: 100 } },
    });
    data.shotFlags[1] = shotKindToFlags('Reference');
    const closure = {
      station_a: { section_id: 1, station_index: 0 },
      station_b: { section_id: 1, station_index: 2 },
      error_percentage: 4,
      misclosure_distance: 0.5,
    } as unknown as LoopClosure;
    return { data, closure };
  }

  const ctx = (errors: ErrorLabelInput | null): LabelBuildContext => ({
    colorCtx: { showDepthGradient: false, depthShallow: '#00bfff', depthDeep: '#ff4444', depthMin: 0, depthMax: 1, labelColor: '#e0e0e0' },
    errors,
    themeColors: theme,
  });

  it('builds nothing without the Errors input', () => {
    const { data } = survey();
    const kinds = buildLabelDescriptors(data, null, 0, 0, ctx(null)).map((d) => d.kind);
    expect(kinds).not.toContain(LABEL_KIND.SHOT_ERROR);
    expect(kinds).not.toContain(LABEL_KIND.CLOSURE_ERROR);
  });

  it('puts a shot error label at the shot midpoint, with its score, colour and virtual flag', () => {
    const { data, closure } = survey();
    const descs = buildLabelDescriptors(data, null, 0, 0, ctx(labelInput(data, { closures: [closure] })));
    const shotLabels = descs.filter((d) => d.kind === LABEL_KIND.SHOT_LABEL);
    const errs = descs.filter((d) => d.kind === LABEL_KIND.SHOT_ERROR);
    expect(errs.map((d) => d.text)).toEqual(['Δaz 40.0°', 'Δaz 10.0°']);
    expect(errs.map((d) => d.score)).toEqual([2, 0.5]);
    expect(errs[0]!.color).toEqual(packCssColor('#ed7c26'));
    expect(errs[1]!.flags & LABEL_FLAG.REFERENCE).toBeTruthy();
    // Same anchor as the shot data label (which sits below it).
    for (let i = 0; i < 2; i++) {
      expect(errs[i]!.anchorX).toBeCloseTo(shotLabels[i]!.anchorX, 12);
      expect(errs[i]!.anchorY).toBeCloseTo(shotLabels[i]!.anchorY, 12);
    }
  });

  it('puts one closure label at the midpoint of the closure stations', () => {
    const { data, closure } = survey();
    const descs = buildLabelDescriptors(data, null, 0, 0, ctx(labelInput(data, { closures: [closure] })));
    const stations = descs.filter((d) => d.kind === LABEL_KIND.STATION_LABEL);
    const [c] = descs.filter((d) => d.kind === LABEL_KIND.CLOSURE_ERROR);
    expect(c!.text).toBe('Loop 4.0% 0.5m');
    expect(c!.score).toBe(2.5);
    expect(c!.anchorX).toBeCloseTo((stations[0]!.anchorX + stations[2]!.anchorX) / 2, 12);
    expect(c!.anchorY).toBeCloseTo((stations[0]!.anchorY + stations[2]!.anchorY) / 2, 12);
    expect(c!.stationGlobal).toBe(0);
    expect(c!.pairStationGlobal).toBe(2);
  });

  it('skips a closure whose station is not in the survey', () => {
    const { data, closure } = survey();
    const stale = { ...closure, station_b: { section_id: 9, station_index: 0 } } as LoopClosure;
    const descs = buildLabelDescriptors(data, null, 0, 0, ctx(labelInput(data, { closures: [stale] })));
    expect(descs.some((d) => d.kind === LABEL_KIND.CLOSURE_ERROR)).toBe(false);
  });

  it('an Errors label is drawn entirely above its point; Shot data entirely below', () => {
    const atlas = fakeAtlas();
    const above = shapedExtent(shapeLabel('Δaz 40.0°', LABEL_KIND.SHOT_ERROR, atlas))!;
    const below = shapedExtent(shapeLabel('312° 4.0m 1.5m', LABEL_KIND.SHOT_LABEL, atlas))!;
    expect(above.minY).toBeGreaterThan(0);
    expect(below.maxY).toBeLessThan(0);
    expect(LABEL_KIND_LAYOUT[LABEL_KIND.SHOT_ERROR].maxWidthEm).toBe(Infinity);
  });
});
