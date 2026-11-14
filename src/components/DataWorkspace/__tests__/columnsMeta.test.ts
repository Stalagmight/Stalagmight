/**
 * Default column widths fit their header labels, and the columns the Survey
 * data table also has carry the same header text there and here.
 *
 * The header is `px-2` (8px each side) + a 1px right border, and a sortable
 * column reserves the sort button after the label (18px + 2px margin) even
 * while it is hidden, so the label gets `width - 37` px. The grid sets headers
 * in 13px Arial (SpreadsheetGrid.css); the advance widths below are Arial's
 * (Liberation Sans, its Linux stand-in, is metric-compatible), in 1/1000 em.
 */
import { describe, expect, it } from 'vitest';
import en from '../../../i18n/locales/en.json';
import { DATA_COLUMNS, DATA_DEFAULT_HIDDEN } from '../columnsMeta';
import type { DataColumnKey } from '../types';

const ARIAL: Record<string, number> = {
  ' ': 278, '!': 278, '(': 333, ')': 333, '/': 278, '.': 278, '-': 333,
  A: 667, B: 667, C: 722, D: 722, E: 667, F: 611, G: 778, H: 722, I: 278, J: 500, K: 667, L: 556, M: 833,
  N: 722, O: 778, P: 667, Q: 778, R: 722, S: 667, T: 611, U: 722, V: 667, W: 944, X: 667, Y: 667, Z: 611,
  a: 556, b: 556, c: 500, d: 556, e: 556, f: 278, g: 556, h: 556, i: 222, j: 222, k: 500, l: 222, m: 833,
  n: 556, o: 556, p: 556, q: 556, r: 333, s: 500, t: 278, u: 556, v: 500, w: 722, x: 500, y: 500, z: 500,
};
const HEADER_FONT_PX = 13;
/** A glyph Arial lacks (the "→" of From → To) is drawn by a fallback font:
 *  budget a full em for it. */
const UNKNOWN_GLYPH = 1000;

const textPx = (text: string) =>
  ([...text].reduce((sum, ch) => sum + (ARIAL[ch] ?? (/\d/.test(ch) ? 556 : UNKNOWN_GLYPH)), 0) * HEADER_FONT_PX) / 1000;

const PADDING_AND_BORDER = 8 + 8 + 1;
const SORT_BUTTON = 18 + 2;

const headers = en.dataWorkspace.headers as Record<DataColumnKey, string>;

describe('Data workspace default column widths', () => {
  it.each(DATA_COLUMNS.map((c) => [c.key, c] as const))('%s fits its header label in full', (_key, col) => {
    const label = headers[col.key];
    expect(label, `missing dataWorkspace.headers.${col.key}`).toBeTruthy();
    const needed = textPx(label) + PADDING_AND_BORDER + (col.sortable ? SORT_BUTTON : 0);
    expect(col.width, `"${label}" needs ${needed.toFixed(1)}px`).toBeGreaterThanOrEqual(Math.ceil(needed));
  });

  it('every default-visible column is listed (the check above covers them all)', () => {
    const visible = DATA_COLUMNS.filter((c) => !DATA_DEFAULT_HIDDEN.includes(c.key)).map((c) => c.key);
    expect(visible).toEqual(expect.arrayContaining(['length', 'azimuth', 'depth_in', 'depth_out', 'kind', 'links']));
  });

  it('hides the station depth by default (the shots carry depth in / out)', () => {
    expect(DATA_DEFAULT_HIDDEN).toContain('depth');
  });

  it('gives the Links column room for a closure chip with its misclosure', () => {
    expect(DATA_COLUMNS.find((c) => c.key === 'links')!.width).toBeGreaterThanOrEqual(320);
  });
});

describe('Data workspace headers match the Survey data table', () => {
  const dt = en.dataTable.columns;
  const meters = en.common.units.meters;
  const survey: Partial<Record<DataColumnKey, string>> = {
    issues: dt.warningsShort,
    length: dt.length.replace('{{unit}}', meters),
    azimuth: dt.azimuthShort,
    azimuth_in: dt.azimuthInShort,
    azimuth_out: dt.azimuthOutShort,
    inclination: dt.inclinationShort,
    pitch_in: dt.pitchInShort,
    pitch_out: dt.pitchOutShort,
    depth_in: dt.depthIn,
    depth_out: dt.depthOut,
    left: dt.leftShort,
    right: dt.rightShort,
    up: dt.upShort,
    down: dt.downShort,
    kind: dt.referenceShort,
    comment: dt.comment,
  };
  it.each(Object.entries(survey))('%s reads "%s" in both tables', (key, text) => {
    expect(headers[key as DataColumnKey]).toBe(text);
  });
});
