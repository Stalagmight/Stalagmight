import { describe, expect, it } from 'vitest';
import type { MapThemeColors } from '../../../../../utils/themeColors';
import { LABEL_KIND } from '../labelShaders';
import { packCssColor } from '../colorPack';
import {
  interpolateStops,
  resolveHaloColors,
  computeLabelUniforms,
  LABEL_KIND_STYLE,
  type LabelStyleInputs,
} from '../labelStyle';

/**
 * Per-kind label style + per-frame uniform math (#customRender labels, Phase 2).
 * Pure, node env. Locks the size / opacity / decimation / minzoom / halo numbers
 * to the stock symbol layers and the DPR + reference-hide wiring the shader reads.
 */

const THEME = { textHalo: '#111111', textHaloAlt: '#222222' } as unknown as MapThemeColors;

const S = LABEL_KIND.STATION_LABEL;
const SEC = LABEL_KIND.LINE_NAME;

/** All-enabled inputs at a given DPR with theme halo colors resolved. */
function inputs(over: Partial<LabelStyleInputs> = {}): LabelStyleInputs {
  return {
    dpr: 1,
    kindEnabled: [true, true, true, true, true],
    haloColors: resolveHaloColors(THEME),
    hideReference: false,
  hideJump: false,
    ...over,
  };
}

describe('interpolateStops', () => {
  it('clamps below the first and above the last stop', () => {
    const stops = [[14, 9], [18, 11]] as const;
    expect(interpolateStops(10, stops)).toBe(9);
    expect(interpolateStops(14, stops)).toBe(9);
    expect(interpolateStops(18, stops)).toBe(11);
    expect(interpolateStops(30, stops)).toBe(11);
  });

  it('linearly interpolates between stops', () => {
    expect(interpolateStops(16, [[14, 9], [18, 11]])).toBeCloseTo(10, 6);
    expect(interpolateStops(15, [[14, 0.7], [18, 1]])).toBeCloseTo(0.775, 6);
  });

  it('treats a single stop as a constant', () => {
    expect(interpolateStops(0, [[0, 0.85]])).toBe(0.85);
    expect(interpolateStops(99, [[0, 0.85]])).toBe(0.85);
  });
});

describe('resolveHaloColors', () => {
  it('uses textHalo for line names and textHaloAlt for every other kind', () => {
    const halos = resolveHaloColors(THEME);
    expect(halos[SEC]).toEqual(packCssColor('#111111'));
    expect(halos[LABEL_KIND.STATION_LABEL]).toEqual(packCssColor('#222222'));
    expect(halos[LABEL_KIND.SHOT_LABEL]).toEqual(packCssColor('#222222'));
    expect(halos[LABEL_KIND.STATION_COMMENT]).toEqual(packCssColor('#222222'));
  });
});

describe('computeLabelUniforms', () => {
  it('interpolates station text size across the 14→9 / 18→11 band, scaled by DPR', () => {
    expect(computeLabelUniforms(14, inputs()).textSizePx[S]).toBeCloseTo(9, 6);
    expect(computeLabelUniforms(16, inputs()).textSizePx[S]).toBeCloseTo(10, 6);
    expect(computeLabelUniforms(18, inputs()).textSizePx[S]).toBeCloseTo(11, 6);
    // DPR 2 doubles the device-px size (viewport is device px too).
    expect(computeLabelUniforms(16, inputs({ dpr: 2 })).textSizePx[S]).toBeCloseTo(20, 6);
  });

  it('interpolates station opacity across 14→0.7 / 18→1', () => {
    expect(computeLabelUniforms(14, inputs()).opacity[S]).toBeCloseTo(0.7, 6);
    expect(computeLabelUniforms(18, inputs()).opacity[S]).toBeCloseTo(1, 6);
  });

  it('gates kind visibility on the toggle AND the static minzoom', () => {
    // Station minzoom 14: hidden below, shown at/above when enabled.
    expect(computeLabelUniforms(13.9, inputs()).kindVisible[S]).toBe(0);
    expect(computeLabelUniforms(14, inputs()).kindVisible[S]).toBe(1);
    // Toggle off collapses it even above minzoom.
    const off = inputs({ kindEnabled: [false, false, false, false, false] });
    expect(computeLabelUniforms(16, off).kindVisible[S]).toBe(0);
    // Line names have minzoom 10.
    expect(computeLabelUniforms(11, inputs()).kindVisible[SEC]).toBe(1);
    expect(computeLabelUniforms(9, inputs()).kindVisible[SEC]).toBe(0);
  });

  it('scales halo width by DPR and packs halo colors to 0..1', () => {
    const u = computeLabelUniforms(16, inputs({ dpr: 2 }));
    expect(u.haloWidthPx[S]).toBeCloseTo(LABEL_KIND_STYLE[S].haloWidth * 2, 6);
    // textHaloAlt '#222222' -> 0x22/255 per channel.
    expect(u.haloColor[S * 4]).toBeCloseTo(0x22 / 255, 6);
    expect(u.haloColor[S * 4 + 3]).toBeCloseTo(1, 6);
  });

  it('passes the reference-hide scalar through', () => {
    expect(computeLabelUniforms(16, inputs()).hideReference).toBe(0);
    expect(computeLabelUniforms(16, inputs({ hideReference: true })).hideReference).toBe(1);
  });
});

describe('computeLabelUniforms - all five kinds (stock parity)', () => {
  const SHOT = LABEL_KIND.SHOT_LABEL;
  const STC = LABEL_KIND.STATION_COMMENT;
  const SHC = LABEL_KIND.SHOT_COMMENT;

  it('shot labels: size 14→9 / 18→12, opacity 14→0.7 / 18→1, halo 2, minzoom 14', () => {
    expect(computeLabelUniforms(14, inputs()).textSizePx[SHOT]).toBeCloseTo(9, 6);
    expect(computeLabelUniforms(18, inputs()).textSizePx[SHOT]).toBeCloseTo(12, 6);
    expect(computeLabelUniforms(14, inputs()).opacity[SHOT]).toBeCloseTo(0.7, 6);
    expect(computeLabelUniforms(18, inputs()).opacity[SHOT]).toBeCloseTo(1, 6);
    expect(LABEL_KIND_STYLE[SHOT].haloWidth).toBe(2);
    expect(computeLabelUniforms(13.9, inputs()).kindVisible[SHOT]).toBe(0);
    expect(computeLabelUniforms(14, inputs()).kindVisible[SHOT]).toBe(1);
  });

  it('line names: size 12→11 / 18→14, opacity 1, halo 1.5, minzoom 10', () => {
    expect(computeLabelUniforms(12, inputs()).textSizePx[SEC]).toBeCloseTo(11, 6);
    expect(computeLabelUniforms(18, inputs()).textSizePx[SEC]).toBeCloseTo(14, 6);
    expect(computeLabelUniforms(15, inputs()).opacity[SEC]).toBeCloseTo(1, 6);
    expect(LABEL_KIND_STYLE[SEC].haloWidth).toBe(1.5);
  });

  it('comments: size 14→8 / 18→10, opacity 0.85 constant, halo 1.5, minzoom 14', () => {
    for (const K of [STC, SHC]) {
      expect(computeLabelUniforms(14, inputs()).textSizePx[K]).toBeCloseTo(8, 6);
      expect(computeLabelUniforms(18, inputs()).textSizePx[K]).toBeCloseTo(10, 6);
      expect(computeLabelUniforms(14, inputs()).opacity[K]).toBeCloseTo(0.85, 6);
      expect(computeLabelUniforms(18, inputs()).opacity[K]).toBeCloseTo(0.85, 6);
      expect(LABEL_KIND_STYLE[K].haloWidth).toBe(1.5);
      expect(computeLabelUniforms(13.9, inputs()).kindVisible[K]).toBe(0);
      expect(computeLabelUniforms(14, inputs()).kindVisible[K]).toBe(1);
    }
  });
});
