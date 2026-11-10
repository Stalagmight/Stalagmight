import { describe, expect, it } from 'vitest';
import {
  formatShotLabel,
  formatStationLabel,
  formatStationNameLabel,
  STATION_NAME_LABEL_MAX_CHARS,
  formatSectionLabel,
  formatCommentLabel,
  resolveShotLabelColor,
  resolveStationLabelColor,
  resolveSectionLabelColor,
  type LabelColorContext,
} from '../labelText';
import type { DepthRange } from '../../displayColor';
import type { MapThemeColors } from '../../../../../utils/themeColors';

/**
 * Label TEXT + COLOR helper tests (#customRender labels, Phase 1). These are the
 * single source of shot/station/section label strings and colors for the custom
 * label renderer, so the tests pin the exact string + color each mode produces.
 */

const THEME = {
  stationDefault: '#ffffff',
  zeroDatum: '#00ffff',
  strokeColor: '#000000',
  depthShallow: '#00bfff',
  depthDeep: '#ff4444',
  labelColor: '#e0e0e0',
  severityError: '#ef4444',
  severityWarning: '#eab308',
  severityClear: '#22c55e',
  grayLine: '#808080',
} as unknown as MapThemeColors;

const DEPTH_RANGE: DepthRange = { min: -2, max: 0 };

function ctxFor(showDepthGradient: boolean): LabelColorContext {
  return {
    showDepthGradient,
    depthShallow: THEME.depthShallow,
    depthDeep: THEME.depthDeep,
    depthMin: DEPTH_RANGE.min,
    depthMax: DEPTH_RANGE.max,
    labelColor: THEME.labelColor,
  };
}

describe('labelText helpers (units)', () => {
  it('formats a shot label as "az° len m depth m"', () => {
    // Defaults: azimuth 0 dp, length + depth 1 dp.
    expect(formatShotLabel(312, 4.2, 1.8)).toBe('312° 4.2m 1.8m');
    expect(formatShotLabel(312, 4.2, -0.5, { azimuth: 0, length: 1, depth: 1 } as never)).toBe('312° 4.2m -0.5m');
  });

  it('formats a station name label: sanitized, trimmed, null when nothing is left', () => {
    expect(formatStationNameLabel('17')).toBe('17');
    expect(formatStationNameLabel('  A12 ')).toBe('A12');
    // Bidi overrides and control characters are stripped, as in the grid/detail panel.
    expect(formatStationNameLabel('1\u202e7\n')).toBe('17');
    // No name, an empty one, or one of only unsafe characters: no label at all.
    expect(formatStationNameLabel(undefined)).toBeNull();
    expect(formatStationNameLabel(null)).toBeNull();
    expect(formatStationNameLabel('')).toBeNull();
    expect(formatStationNameLabel('   ')).toBeNull();
    expect(formatStationNameLabel('\u202e\u2066\u0007')).toBeNull();
    // Same rules as the image export (image.rs station_name_label): the Arabic
    // letter mark goes, and a byte-order mark trims like whitespace.
    expect(formatStationNameLabel('\ufeff\u061c17\ufeff')).toBe('17');
    expect(formatStationNameLabel('\ufeff')).toBeNull();
  });

  it('caps a long station name by code point and ends it with an ellipsis', () => {
    const exact = 'b'.repeat(STATION_NAME_LABEL_MAX_CHARS);
    expect(formatStationNameLabel(exact)).toBe(exact);
    const capped = formatStationNameLabel('a'.repeat(40))!;
    expect(Array.from(capped)).toHaveLength(STATION_NAME_LABEL_MAX_CHARS);
    expect(capped.endsWith('\u2026')).toBe(true);
    // A cut right after a space does not keep the space before the ellipsis.
    expect(formatStationNameLabel(`${'c'.repeat(22)} ${'d'.repeat(10)}`)).toBe(`${'c'.repeat(22)}\u2026`);
    // Astral characters (surrogate pairs) count once and are never split.
    const emoji = formatStationNameLabel('\u{1F30A}'.repeat(30))!;
    expect(Array.from(emoji)).toHaveLength(STATION_NAME_LABEL_MAX_CHARS);
    expect(emoji).toBe(`${'\u{1F30A}'.repeat(STATION_NAME_LABEL_MAX_CHARS - 1)}\u2026`);
  });

  it('formats a station label as its index; section as its name (empty for null)', () => {
    expect(formatStationLabel(7)).toBe('7');
    expect(formatSectionLabel('Alpha')).toBe('Alpha');
    expect(formatSectionLabel(null)).toBe('');
    expect(formatCommentLabel('hi there')).toBe('hi there');
  });

  it('flat mode -> labelColor; section -> its own color', () => {
    const flat = ctxFor(false);
    expect(resolveShotLabelColor(1.5, flat)).toBe(THEME.labelColor);
    expect(resolveStationLabelColor(-1, flat)).toBe(THEME.labelColor);
    expect(resolveSectionLabelColor('#4ecdc4')).toBe('#4ecdc4');
  });

  it('depth mode -> the ramp color (a hex string), not labelColor', () => {
    const depth = ctxFor(true);
    const c = resolveShotLabelColor(-1, depth);
    expect(c).toMatch(/^#[0-9a-f]{6}$/i);
    expect(c).not.toBe(THEME.labelColor);
  });
});
