import { describe, expect, it } from 'vitest';
import { getDepthColor } from '../../colors';
import { DISTANCE_FAR, DISTANCE_NEAR } from '../../distanceGradient';
import { packRampColor } from '../../../components/Map/surveyLayer/customRender/overlayColor';
import { packCssColor } from '../../../components/Map/surveyLayer/customRender/colorPack';
import { themeTokens } from '../../../theme/tokens';
import {
  ALL_PALETTES,
  PALETTE_GROUPS,
  autoScale,
  bakeLut,
  clampGamma,
  cssGradient,
  findPalette,
  gammaFromFraction,
  getPalette,
  levelsT,
  midFraction,
  midValue,
  paletteMatches,
  parsePaletteQuery,
  resolveChoice,
  THEME_PALETTE_ID,
} from '..';

const themes = Object.entries(themeTokens).map(([name, tokens]) => ({
  name,
  endpoints: {
    depthShallow: (tokens as Record<string, string>)['--cc-depth-shallow']!,
    depthDeep: (tokens as Record<string, string>)['--cc-depth-deep']!,
  },
}));

describe('levels mapping', () => {
  it('is linear and clamped at gamma 1', () => {
    const s = autoScale({ min: 10, max: 30 });
    expect(levelsT(10, s)).toBe(0);
    expect(levelsT(20, s)).toBe(0.5);
    expect(levelsT(30, s)).toBe(1);
    expect(levelsT(-5, s)).toBe(0);
    expect(levelsT(99, s)).toBe(1);
  });

  it('bends the middle like Photoshop Levels: the midpoint handle maps to 0.5', () => {
    for (const gamma of [0.1, 0.35, 1, 2, 4.5, 9.99]) {
      const s = { min: 0, max: 100, gamma };
      expect(levelsT(midValue(s), s)).toBeCloseTo(0.5, 9);
    }
    expect(midFraction(1)).toBe(0.5);
    expect(midFraction(2)).toBe(0.25); // handle moves left as gamma grows
  });

  it('round-trips a handle position through gamma, clamped to 0.10..9.99', () => {
    expect(gammaFromFraction(0.5)).toBe(1);
    expect(gammaFromFraction(midFraction(2))).toBe(2);
    expect(gammaFromFraction(0.9999)).toBe(0.1);
    expect(gammaFromFraction(0.0001)).toBe(9.99);
    expect(clampGamma(Number.NaN)).toBe(1);
    expect(clampGamma(1.234)).toBe(1.23);
  });
});

describe('Theme palette is byte-identical to the pre-palette colours (#380)', () => {
  it('depth: matches getDepthColor across every theme, value and range', () => {
    for (const { endpoints } of themes) {
      const pal = resolveChoice({ id: THEME_PALETTE_ID, reversed: false }, 'depth', endpoints);
      for (const [min, max] of [[0, 50], [-3.2, 117.7], [12.5, 13.0]] as const) {
        const scale = autoScale({ min, max });
        for (let i = -20; i <= 520; i++) {
          const v = min + ((max - min) * i) / 500;
          expect(pal.hex(levelsT(v, scale))).toBe(
            getDepthColor(v, endpoints.depthShallow, endpoints.depthDeep, min, max),
          );
        }
      }
      // A flat range: getDepthColor returns the shallow colour string itself.
      expect(pal.flatHex()).toBe(getDepthColor(5, endpoints.depthShallow, endpoints.depthDeep, 5, 5));
    }
  });

  it('distance: matches packRampColor (MapLibre floor packing) across the ramp', () => {
    const pal = resolveChoice({ id: THEME_PALETTE_ID, reversed: false }, 'distance', themes[0]!.endpoints);
    const near = packCssColor(DISTANCE_NEAR);
    const far = packCssColor(DISTANCE_FAR);
    const grey = packCssColor('#808080');
    for (const max of [1, 37.25, 1234.5]) {
      const scale = autoScale({ min: 0, max });
      for (let i = 0; i <= 1100; i++) {
        const v = (max * i) / 1000;
        const want = packRampColor(v, max, near, far, grey);
        const got = pal.rgb(levelsT(v, scale))!;
        expect([...got, 255]).toEqual([...want]);
      }
    }
  });
});

describe('palette sampling', () => {
  const endpoints = themes[0]!.endpoints;

  it('hits a stops palette exactly at its ends and follows matplotlib in between', () => {
    const viridis = resolveChoice({ id: 'mpl.viridis', reversed: false }, 'depth', endpoints);
    expect(viridis.hex(0)).toBe('#440154');
    expect(viridis.hex(1)).toBe('#fde725');
    // matplotlib viridis(0.5) = #21918c; the simplified table stays within 2/255.
    const mid = viridis.rgb(0.5)!;
    [0x21, 0x91, 0x8c].forEach((c, k) => expect(Math.abs(mid[k]! - c)).toBeLessThanOrEqual(2));
  });

  it('reverses by flipping the position', () => {
    const fwd = resolveChoice({ id: 'mpl.magma', reversed: false }, 'depth', endpoints);
    const rev = resolveChoice({ id: 'mpl.magma', reversed: true }, 'depth', endpoints);
    for (const t of [0, 0.13, 0.5, 0.87, 1]) expect(rev.hex(t)).toBe(fwd.hex(1 - t));
    const theme = resolveChoice({ id: THEME_PALETTE_ID, reversed: true }, 'depth', endpoints);
    expect(theme.hex(0)).toBe(getDepthColor(0, endpoints.depthDeep, endpoints.depthShallow, 0, 1));
  });

  it('draws discrete palettes as equal hard bands', () => {
    const tab10 = resolveChoice({ id: 'mpl.tab10', reversed: false }, 'depth', endpoints);
    expect(tab10.bands).toBe(10);
    expect(tab10.hex(0)).toBe('#1f77b4');
    expect(tab10.hex(0.0999)).toBe('#1f77b4');
    expect(tab10.hex(0.1)).toBe('#ff7f0e');
    expect(tab10.hex(1)).toBe(tab10.hex(0.95));
    expect(cssGradient(tab10)).toContain('#1f77b4 10.00%, #ff7f0e 10.00%');
  });

  it('bakes a lookup table whose ends are the palette ends', () => {
    const pal = resolveChoice({ id: 'mpl.plasma', reversed: false }, 'depth', endpoints);
    const lut = bakeLut(pal, 64);
    expect(lut.length).toBe(256);
    expect([lut[0], lut[1], lut[2], lut[3]]).toEqual([...pal.rgb(0)!, 255]);
    expect([lut[252], lut[253], lut[254], lut[255]]).toEqual([...pal.rgb(1)!, 255]);
  });

  it('falls back to Theme for an unknown id', () => {
    expect(findPalette('mpl.does-not-exist')).toBeUndefined();
    expect(getPalette('mpl.does-not-exist').id).toBe(THEME_PALETTE_ID);
  });
});

describe('catalogue integrity', () => {
  it('has unique, config-safe ids', () => {
    const ids = ALL_PALETTES.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) expect(id).toMatch(/^[a-z0-9_.-]{1,64}$/);
  });

  it('puts every palette in a known group and leaves no group empty', () => {
    const groups = new Set<string>(PALETTE_GROUPS);
    for (const p of ALL_PALETTES) expect(groups.has(p.group), p.id).toBe(true);
    for (const g of PALETTE_GROUPS) {
      if (g === 'pinned') continue;
      expect(ALL_PALETTES.some((p) => p.group === g), g).toBe(true);
    }
  });

  it('has well-formed stop data', () => {
    for (const p of ALL_PALETTES) {
      if (p.kind === 'theme') continue;
      const width = p.kind === 'stops' ? 4 : 3;
      expect(p.data.length % width, p.id).toBe(0);
      expect(p.data.length / width, p.id).toBeGreaterThanOrEqual(2);
      for (let i = 0; i < p.data.length; i += width) {
        if (p.kind === 'stops') {
          expect(p.data[i], p.id).toBeGreaterThanOrEqual(i === 0 ? 0 : p.data[i - width]!);
          expect(p.data[i], p.id).toBeLessThanOrEqual(1);
        }
        for (let k = width - 3; k < width; k++) {
          const c = p.data[i + k]!;
          expect(Number.isInteger(c) && c >= 0 && c <= 255, p.id).toBe(true);
        }
      }
      if (p.kind === 'stops') {
        expect(p.data[0], p.id).toBe(0);
        expect(p.data[p.data.length - 4], p.id).toBe(1);
      }
    }
  });
});

describe('search', () => {
  it('reads matplotlib _r as "reversed"', () => {
    expect(parsePaletteQuery('  Viridis_r ')).toEqual({ text: 'viridis', reversed: true });
    expect(parsePaletteQuery('_r')).toEqual({ text: '_r', reversed: false });
    expect(parsePaletteQuery('blue')).toEqual({ text: 'blue', reversed: false });
  });

  it('matches every word against name, id and group label', () => {
    const p = getPalette('grad.deep-water');
    expect(paletteMatches(p, '', 'Blues')).toBe(true);
    expect(paletteMatches(p, 'deep', 'Blues')).toBe(true);
    expect(paletteMatches(p, 'water blues', 'Blues')).toBe(true);
    expect(paletteMatches(p, 'magma', 'Blues')).toBe(false);
  });
});
