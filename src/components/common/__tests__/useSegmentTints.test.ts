/**
 * RS-BLEND: a two-tone prose pill's tint per line. A segment blends into its
 * neighbour only where the two share a line; a line break at the seam or
 * inside a segment leaves each fragment fading at that edge, as a
 * one-colour pill's fragments do, never a solid edge at a line's end.
 * Injections: blending at every seam whatever the lines fails "a break at
 * the seam"; one layer for a whole wrapped segment fails "a segment that
 * wraps inside".
 */
import { describe, expect, it } from 'vitest';
import { fragmentTints, type FragmentBox } from '../useSegmentTints';
import { prosePillColorsOf, pillThemeOf } from '../../../theme/pillColors';

const panel = pillThemeOf('light').panel;
const A = prosePillColorsOf('#101010', 'light');
const B = prosePillColorsOf('#f6f6f2', 'light');
const line = (n: number, width: number): FragmentBox => ({ top: n * 20, bottom: n * 20 + 16, width });
const svgs = (tint: unknown) =>
  String(tint)
    .split(', url(')
    .map((u) => decodeURIComponent(u.replace(/^url\("data:image\/svg\+xml,|"\)$/g, '').replace(/^"data:image\/svg\+xml,/, '')));
/** The centre of a fragment's fade: 1 = a seam at its right, 0 = at its left, 0.5 = none; null = seams both sides. */
const fadeCentre = (svg: string) => {
  const m = /radialGradient id='f' cx='([^']+)'/.exec(svg);
  return m ? Number(m[1]) : null;
};

describe('fragmentTints (RS-BLEND)', () => {
  it('blends two segments on one line at their seam', () => {
    const t = fragmentTints(
      [
        { colors: A, rects: [line(0, 120)] },
        { colors: B, rects: [line(0, 40)] },
      ],
      panel,
      13,
    )!;
    expect(svgs(t[0]!['--seg-tint' as keyof typeof t[0]]).map(fadeCentre)).toEqual([1]);
    expect(svgs(t[1]!['--seg-tint' as keyof typeof t[1]]).map(fadeCentre)).toEqual([0]);
  });

  it('a break at the seam: each side fades there, no solid edge at a line end', () => {
    const t = fragmentTints(
      [
        { colors: A, rects: [line(0, 120)] },
        { colors: B, rects: [line(1, 40)] },
      ],
      panel,
      13,
    )!;
    expect(svgs(t[0]!['--seg-tint' as keyof typeof t[0]]).map(fadeCentre)).toEqual([0.5]);
    expect(svgs(t[1]!['--seg-tint' as keyof typeof t[1]]).map(fadeCentre)).toEqual([0.5]);
  });

  it('a segment that wraps inside: one layer per line, blended only on the seam line', () => {
    const t = fragmentTints(
      [
        { colors: A, rects: [line(0, 150), line(1, 50)] },
        { colors: B, rects: [line(1, 40)] },
      ],
      panel,
      13,
    )!;
    const head = t[0] as Record<string, string>;
    expect(svgs(head['--seg-tint']).map(fadeCentre)).toEqual([0.5, 1]);
    // Over the strip of both fragments end to end (200 px): 75% then 25%.
    expect(head['--seg-tint-size']).toBe('75.0000% 100%, 25.0000% 100%');
    // The second layer's left edge at 150 px: 150 / (200 - 50) = 100%.
    expect(head['--seg-tint-pos']).toBe('0.0000% 0, 100.0000% 0');
  });

  it('a middle segment between two seams on its line fades only top and bottom', () => {
    const t = fragmentTints(
      [
        { colors: A, rects: [line(0, 30)] },
        { colors: B, rects: [line(0, 40)] },
        { colors: A, rects: [line(0, 30)] },
      ],
      panel,
      13,
    )!;
    expect(svgs((t[1] as Record<string, string>)['--seg-tint']).map(fadeCentre)).toEqual([null]);
  });

  it('is null until laid out', () => {
    expect(fragmentTints([{ colors: A, rects: [] }], panel, 13)).toBeNull();
  });
});
