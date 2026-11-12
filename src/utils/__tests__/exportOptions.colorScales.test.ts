import { describe, expect, it } from 'vitest';
import { exportScaleInputsFrom, withColorScales, withThemeDepthRamp, EXPORT_PALETTE_ENTRIES } from '../exportOptions';
import { DEFAULT_IMAGE_OPTIONS } from '../../store/exportStore';
import { resolveChoice } from '../palettes';
import { getMapThemeColors } from '../themeColors';

const AUTO = { depth: null, distance: null };
const THEME = { id: 'theme', reversed: false };

describe('withColorScales', () => {
  it('sends nothing extra for the default palette and range (export unchanged)', () => {
    const out = withColorScales(
      DEFAULT_IMAGE_OPTIONS,
      exportScaleInputsFrom({ depth: THEME, distance: THEME}, { exportScale: AUTO, palettePreview: null }),
    );
    expect(out).toEqual(withThemeDepthRamp(DEFAULT_IMAGE_OPTIONS));
    expect(out).not.toHaveProperty('depth_scale');
    expect(out).not.toHaveProperty('distance_scale');
  });

  it('bakes a chosen palette into a table and passes the export range', () => {
    const out = withColorScales(
      DEFAULT_IMAGE_OPTIONS,
      exportScaleInputsFrom(
        { depth: { id: 'mpl.viridis', reversed: false }, distance: THEME},
        { exportScale: { depth: null, distance: { min: 10, max: 90, gamma: 2 } }, palettePreview: null },
      ),
    );
    const viridis = resolveChoice({ id: 'mpl.viridis', reversed: false }, 'depth', getMapThemeColors());
    expect(out.depth_scale?.palette).toHaveLength(EXPORT_PALETTE_ENTRIES);
    expect(out.depth_scale?.palette?.[0]).toBe(viridis.hex(0));
    expect(out.depth_scale?.palette?.[EXPORT_PALETTE_ENTRIES - 1]).toBe(viridis.hex(1));
    expect(out.depth_scale).not.toHaveProperty('levels');
    expect(out.distance_scale).toEqual({ levels: { min: 10, max: 90, gamma: 2 } });
  });

  it("follows the picker's live preview for its kind", () => {
    const inputs = exportScaleInputsFrom(
      { depth: THEME, distance: THEME},
      { exportScale: AUTO, palettePreview: { kind: 'distance', choice: { id: 'mpl.magma', reversed: true } } },
    );
    expect(inputs.depth.choice).toEqual(THEME);
    expect(inputs.distance.choice).toEqual({ id: 'mpl.magma', reversed: true });
  });

  it('does not touch the caller options', () => {
    const options = { ...DEFAULT_IMAGE_OPTIONS };
    withColorScales(
      options,
      exportScaleInputsFrom({ depth: { id: 'mpl.jet', reversed: false }, distance: THEME}, { exportScale: AUTO, palettePreview: null }),
    );
    expect(options).toEqual(DEFAULT_IMAGE_OPTIONS);
  });
});
