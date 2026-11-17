import { describe, expect, it } from 'vitest';
import {
  shotStationDecimationModulus,
  decimationModulusForKind,
  decimationTier,
  decimationAdmitZoom,
  isLabelDecimatedOut,
  DECIM_BAND_MID_ZOOM,
  DECIM_BAND_FINE_ZOOM,
} from '../labelDecimation';
import { LABEL_KIND } from '../labelShaders';

/**
 * Label decimation planning (#customRender labels, reveal-zoom rework). Locks the
 * NESTED per-kind zoom-band moduli (4 -> 2 -> 1, the monotone replacement for the
 * non-monotone stock 5 -> 2 -> 1), the decimation TIER + admission-zoom folded
 * into the reveal computation, and the nested-monotonicity property (the kept set
 * only grows as zoom rises).
 */

describe('shotStationDecimationModulus', () => {
  it('uses the NESTED bands (14->4, 16->2, 18->1) - the monotone replacement for stock 5->2->1', () => {
    expect(shotStationDecimationModulus(14)).toBe(4);
    expect(shotStationDecimationModulus(15.9)).toBe(4);
    expect(shotStationDecimationModulus(16)).toBe(2);
    expect(shotStationDecimationModulus(17.9)).toBe(2);
    expect(shotStationDecimationModulus(18)).toBe(1);
    expect(shotStationDecimationModulus(22)).toBe(1);
  });
});

describe('decimationModulusForKind', () => {
  it('decimates shot + station labels, never line names or comments', () => {
    expect(decimationModulusForKind(LABEL_KIND.SHOT_LABEL, 15)).toBe(4);
    expect(decimationModulusForKind(LABEL_KIND.STATION_LABEL, 16)).toBe(2);
    expect(decimationModulusForKind(LABEL_KIND.LINE_NAME, 15)).toBe(1);
    expect(decimationModulusForKind(LABEL_KIND.STATION_COMMENT, 15)).toBe(1);
    expect(decimationModulusForKind(LABEL_KIND.SHOT_COMMENT, 15)).toBe(1);
  });
});

describe('decimationTier', () => {
  it('tiers ordinals by the nested chain: %4==0 coarse (0), %2==0 mid (1), odd fine (2)', () => {
    expect(decimationTier(0)).toBe(0);
    expect(decimationTier(4)).toBe(0);
    expect(decimationTier(8)).toBe(0);
    expect(decimationTier(2)).toBe(1);
    expect(decimationTier(6)).toBe(1);
    expect(decimationTier(1)).toBe(2);
    expect(decimationTier(7)).toBe(2);
  });
});

describe('decimationAdmitZoom', () => {
  it('shot/station tiers admit at minzoom (coarse), z16 (mid), z18 (fine)', () => {
    const mz = 14;
    expect(decimationAdmitZoom(LABEL_KIND.SHOT_LABEL, 4, false, mz)).toBe(14); // %4 coarse
    expect(decimationAdmitZoom(LABEL_KIND.SHOT_LABEL, 2, false, mz)).toBe(DECIM_BAND_MID_ZOOM); // 16
    expect(decimationAdmitZoom(LABEL_KIND.SHOT_LABEL, 1, false, mz)).toBe(DECIM_BAND_FINE_ZOOM); // 18
    expect(decimationAdmitZoom(LABEL_KIND.STATION_LABEL, 6, false, mz)).toBe(DECIM_BAND_MID_ZOOM);
  });

  it('a first-in-line station admits at minzoom regardless of ordinal', () => {
    expect(decimationAdmitZoom(LABEL_KIND.STATION_LABEL, 7, true, 14)).toBe(14);
  });

  it('non-decimating kinds (line names, comments) admit at minzoom', () => {
    expect(decimationAdmitZoom(LABEL_KIND.LINE_NAME, 5, false, 10)).toBe(10);
    expect(decimationAdmitZoom(LABEL_KIND.STATION_COMMENT, 5, false, 14)).toBe(14);
  });
});

describe('isLabelDecimatedOut', () => {
  it('keeps every N-th ordinal, hides the rest', () => {
    // modulus 4: 0,4,8 shown; others hidden.
    expect(isLabelDecimatedOut(0, 4, false)).toBe(false);
    expect(isLabelDecimatedOut(4, 4, false)).toBe(false);
    expect(isLabelDecimatedOut(3, 4, false)).toBe(true);
    expect(isLabelDecimatedOut(6, 4, false)).toBe(true);
  });

  it('always shows a first-in-line label regardless of ordinal', () => {
    expect(isLabelDecimatedOut(3, 4, true)).toBe(false);
    expect(isLabelDecimatedOut(7, 2, true)).toBe(false);
  });

  it('shows all when the modulus is <= 1 (zoom >= 18)', () => {
    expect(isLabelDecimatedOut(3, 1, false)).toBe(false);
    expect(isLabelDecimatedOut(999, 1, false)).toBe(false);
  });
});

describe('nested-decimation monotonicity (the core stock-parity break)', () => {
  it('the kept set only GROWS as zoom rises (4 -> 2 -> 1 is nested)', () => {
    const kept = (zoom: number): Set<number> => {
      const mod = shotStationDecimationModulus(zoom);
      const s = new Set<number>();
      for (let o = 0; o < 200; o++) if (!isLabelDecimatedOut(o, mod, false)) s.add(o);
      return s;
    };
    const coarse = kept(15); // % 4
    const mid = kept(17); // % 2
    const fine = kept(19); // all
    // coarse ⊆ mid ⊆ fine - no ordinal ever leaves the set as zoom rises.
    for (const o of coarse) expect(mid.has(o)).toBe(true);
    for (const o of mid) expect(fine.has(o)).toBe(true);
    expect(fine.size).toBeGreaterThan(mid.size);
    expect(mid.size).toBeGreaterThan(coarse.size);
    // The stock 5->2->1 would FAIL this: ordinal 5 is kept at %5 but dropped at %2.
    expect(5 % 5).toBe(0); // kept under the old coarse band
    expect(isLabelDecimatedOut(5, 4, false)).toBe(true); // and correctly NOT kept under the nested coarse band
  });
});
