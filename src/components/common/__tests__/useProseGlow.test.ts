/**
 * RS-NODOTS: a two-colour prose pill's glow per line. Each line's piece of
 * the pill is one capsule painted by its first word box, rounded at both
 * ends, its colour running across the arrow; a line break ends one piece
 * and starts the next, and a segment blends into its neighbour only on the
 * same line. Painting never moves a word: what a box takes as padding to
 * stretch over its piece it gives back as a negative margin.
 * Injections: painting every box on its own (one image per word) fails "one
 * image per piece"; blending a seam across a line break fails "a break at
 * the seam"; a stretch not given back fails "no word moves".
 */
import { describe, expect, it } from 'vitest';
import { glowStyles, type LaidBox } from '../useProseGlow';
import { GLOW, GLOW_REACH } from '../../../theme/proseGlow';
import { pillThemeOf, prosePillColorsOf, proseOverlayOf } from '../../../theme/pillColors';

const FONT = 14;
const panel = pillThemeOf('light').panel;
const A = prosePillColorsOf('#ffe600', 'light').bg;
const B = prosePillColorsOf('#1f4fff', 'light').bg;
const H = prosePillColorsOf('#101010', 'light').bg;
/** A box of segment `seg` on line `line`, its words from `left` to `right`. */
const box = (seg: number, line: number, left: number, right: number): LaidBox => ({
  seg,
  left,
  right,
  top: line * 20 - 6,
  bottom: line * 20 + 22,
});
const svgOf = (style: Record<string, string>) =>
  decodeURIComponent(/data:image\/svg\+xml,(.*)"\)$/.exec(style['--w-glow'] ?? '')?.[1] ?? '');
const stopsOf = (svg: string) => [...svg.matchAll(/stop-color='rgb\(([^)]+)\)' stop-opacity='([^']+)'/g)].map((m) => m[1]);
const wash = (target: string) => proseOverlayOf(target, panel).rgb.join(',');
const px = (v: string | undefined) => parseFloat(v ?? '0');
const room = (GLOW.padX + GLOW_REACH) * FONT;
const ends = (GLOW.padX + GLOW.marginX) * FONT;

// A from half (A), the arrow (A to B), the to half (B).
const SEGMENTS = [{ colour: A }, { colour: A, bridgeTo: B }, { colour: B }];

describe('glowStyles (RS-NODOTS)', () => {
  it('one image per piece: the first box paints the whole line, rounded at both ends', () => {
    const boxes = [box(0, 0, 0, 10), box(1, 0, 10, 30), box(2, 0, 30, 40)];
    const s = glowStyles(boxes, SEGMENTS, panel, FONT) as Array<Record<string, string>>;
    expect(s[0]!['--w-glow']).toMatch(/^url\("data:image\/svg\+xml,/);
    expect(s[1]!['--w-glow']).toBeUndefined();
    expect(s[2]!['--w-glow']).toBeUndefined();
    const svg = svgOf(s[0]!);
    // Stretched over the words and the room for both rounded ends.
    expect(svg).toContain(`width='${Math.round((40 + 2 * room) * 100) / 100}'`);
    // Its colour runs from the from half's to the to half's.
    const stops = stopsOf(svg);
    expect(stops[0]).toBe(wash(A));
    expect(stops[stops.length - 1]).toBe(wash(B));
  });

  it('no word moves: what a box takes to stretch it gives back, beyond what the CSS gives the pill ends', () => {
    const boxes = [box(0, 0, 0, 10), box(1, 0, 10, 30), box(2, 0, 30, 40)];
    const s = glowStyles(boxes, SEGMENTS, panel, FONT) as Array<Record<string, string>>;
    // The pill's first box: its left as the CSS has it (room, less what goes back).
    expect(px(s[0]!.paddingLeft) + px(s[0]!.marginLeft)).toBeCloseTo(ends, 1);
    // Its stretch to the piece's right end is all given back.
    expect(px(s[0]!.paddingRight) + px(s[0]!.marginRight)).toBeCloseTo(0, 1);
    expect(px(s[0]!.paddingRight)).toBeCloseTo(40 - 10 + room, 1);
  });

  it('a line break ends one piece and starts the next, each its own capsule', () => {
    // "Main Passage" wraps after its first word: the from half spans two lines.
    const boxes = [box(0, 0, 200, 240), box(0, 0, 240, 244), box(0, 1, 0, 50), box(1, 1, 50, 70), box(2, 1, 70, 80)];
    // The space at the break hangs at the line's end with no width.
    boxes[1] = { ...boxes[1]!, right: boxes[1]!.left };
    const s = glowStyles(boxes, SEGMENTS, panel, FONT) as Array<Record<string, string>>;
    expect(s[0]!['--w-glow']).toBeDefined();
    expect(s[1]!['--w-glow']).toBeUndefined();
    expect(s[2]!['--w-glow']).toBeDefined();
    // The first line's piece is the from half alone: one colour.
    expect(new Set(stopsOf(svgOf(s[0]!)))).toEqual(new Set([wash(A)]));
    // The second line's piece starts at a break: its room is given back in full.
    expect(px(s[2]!.paddingLeft)).toBeCloseTo(room, 1);
    expect(px(s[2]!.paddingLeft) + px(s[2]!.marginLeft)).toBeCloseTo(0, 1);
    // Its stretch over the line is given back; the pill's last box keeps its
    // right end as the CSS has it, untouched.
    expect(px(s[2]!.paddingRight) + px(s[2]!.marginRight)).toBeCloseTo(0, 1);
    expect(s[4]).toEqual({});
  });

  it('a break at the seam: a head on one line and the from half on the next do not blend', () => {
    // Head (H), from half (A), arrow, to half (B); the break falls after the head.
    const segments = [{ colour: H }, { colour: A }, { colour: A, bridgeTo: B }, { colour: B }];
    const boxes = [box(0, 0, 100, 150), box(1, 1, 0, 10), box(2, 1, 10, 30), box(3, 1, 30, 40)];
    const s = glowStyles(boxes, segments, panel, FONT) as Array<Record<string, string>>;
    expect(new Set(stopsOf(svgOf(s[0]!)))).toEqual(new Set([wash(H)]));
    expect(stopsOf(svgOf(s[1]!))[0]).toBe(wash(A));
    // On one line they blend: the seam's colour is neither end's.
    const joined = [box(0, 0, 0, 50), box(1, 0, 50, 60), box(2, 0, 60, 80), box(3, 0, 80, 90)];
    const t = glowStyles(joined, segments, panel, FONT) as Array<Record<string, string>>;
    const stops = stopsOf(svgOf(t[0]!));
    expect(stops[0]).toBe(wash(H));
    expect(stops.some((c) => c !== wash(H) && c !== wash(A) && c !== wash(B))).toBe(true);
  });

  it('every box paints nothing until a piece is laid out', () => {
    const s = glowStyles([{ ...box(0, 0, 0, 0) }], SEGMENTS, panel, FONT);
    expect(s).toEqual([{}]);
  });
});
