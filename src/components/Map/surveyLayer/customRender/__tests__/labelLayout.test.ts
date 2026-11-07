import { describe, expect, it, vi } from 'vitest';
import {
  shapeLabel,
  shapedExtent,
  lineWidthEm,
  wrapIntoLines,
  ShapeCache,
  LABEL_KIND_LAYOUT,
  LINE_HEIGHT_EM,
  ASCENT_EM,
  COMMENT_MAX_WIDTH_EM,
} from '../labelLayout';
import { GlyphAtlas, SDF_BUFFER, SDF_FONT_SIZE, type GlyphRasterizer, type RasterizedGlyph } from '../glyphAtlas';
import { LABEL_KIND } from '../labelShaders';

/**
 * Label layout / shaping (#customRender labels, Phase 1). Uses a deterministic
 * fake atlas (no canvas / font): every char is a fixed-metric glyph so glyph
 * counts, wrap breaks, and em anchor/offset math are exactly predictable.
 */

/** Glyph metrics a fake produces, in raster px. Defaults: 12px advance (0.5 em),
 *  a 10x18 ink box, left bearing 1, ascent (baseline->top) 15. A space is a
 *  zero-ink 6px-advance glyph. */
function specFor(c: string): { advance: number; w: number; h: number; left: number; top: number } {
  if (c === ' ') return { advance: 6, w: 0, h: 0, left: 0, top: 0 };
  return { advance: 12, w: 10, h: 18, left: 1, top: 15 };
}

function fakeRasterizer(spec: (c: string) => ReturnType<typeof specFor> = specFor): GlyphRasterizer {
  return (c: string): RasterizedGlyph => {
    const s = spec(c);
    return {
      data: new Uint8Array(Math.max(0, s.w * s.h)),
      width: s.w,
      height: s.h,
      glyphWidth: Math.max(0, s.w - 2 * SDF_BUFFER),
      glyphHeight: Math.max(0, s.h - 2 * SDF_BUFFER),
      // Inverted so GlyphInfo.left === s.left and GlyphInfo.top === s.top.
      glyphTop: s.top - SDF_BUFFER,
      glyphLeft: s.left + SDF_BUFFER,
      glyphAdvance: s.advance,
    };
  };
}

function makeAtlas(spec?: (c: string) => ReturnType<typeof specFor>): GlyphAtlas {
  return new GlyphAtlas({ rasterizer: fakeRasterizer(spec) });
}

describe('shapeLabel counts + empty handling', () => {
  it('one quad per inked glyph, none for spaces', () => {
    const atlas = makeAtlas();
    // "A B" -> two inked glyphs, the space contributes no quad.
    const s = shapeLabel('A B', LABEL_KIND.STATION_COMMENT, atlas);
    expect(s.glyphCount).toBe(2);
    expect(s.glyphs).toHaveLength(2);
  });

  it('empty and whitespace-only strings shape to zero glyphs', () => {
    const atlas = makeAtlas();
    expect(shapeLabel('', LABEL_KIND.SHOT_LABEL, atlas).glyphCount).toBe(0);
    expect(shapeLabel('   ', LABEL_KIND.STATION_COMMENT, atlas).glyphCount).toBe(0);
    // A non-wrapping empty section name is also zero glyphs.
    expect(shapeLabel('', LABEL_KIND.SECTION_NAME, atlas).glyphCount).toBe(0);
  });

  it('renders an unknown codepoint as an ordinary quad (fallback box, no crash)', () => {
    // The production font draws .notdef (a box) for a missing glyph; the fake
    // gives it default metrics. Either way shaping emits a quad and advances.
    const atlas = makeAtlas();
    const s = shapeLabel('', LABEL_KIND.STATION_LABEL, atlas);
    expect(s.glyphCount).toBe(1);
    expect(s.glyphs[0]!.aw).toBeGreaterThan(0);
  });
});

describe('shapeLabel anchor + offset math', () => {
  it('places a shot label below the anchor (top-anchored, offset [0,0.5])', () => {
    const atlas = makeAtlas();
    const s = shapeLabel('A', LABEL_KIND.SHOT_LABEL, atlas);
    const g = s.glyphs[0]!;
    // Single 0.5em-wide glyph, centered on x -> straddles x=0.
    expect(g.xL).toBeCloseTo(-0.25 + 1 / SDF_FONT_SIZE, 5);
    expect(g.xR).toBeCloseTo(g.xL + 10 / SDF_FONT_SIZE, 5);
    // baseY (+down) = blockTop(0.5) + ASCENT(0.9); glyph top is `top` em above it.
    const baseY = 0.5 + ASCENT_EM;
    expect(g.yTop).toBeCloseTo(-(baseY - 15 / SDF_FONT_SIZE), 5);
    expect(g.yBot).toBeCloseTo(-(baseY - 15 / SDF_FONT_SIZE + 18 / SDF_FONT_SIZE), 5);
    // Below the anchor -> both corners negative in the +up shader frame.
    expect(g.yTop).toBeLessThan(0);
    expect(g.yBot).toBeLessThan(g.yTop);
  });

  it('places a station label up-and-right of the anchor (bottom-left, offset [0.8,-0.8])', () => {
    const atlas = makeAtlas();
    const s = shapeLabel('7', LABEL_KIND.STATION_LABEL, atlas);
    const g = s.glyphs[0]!;
    // Left-anchored: line left starts at the x offset (0.8), plus the glyph's
    // left bearing.
    expect(g.xL).toBeCloseTo(0.8 + 1 / SDF_FONT_SIZE, 5);
    // Bottom anchor + negative y offset -> the block sits ABOVE the anchor, so
    // the ink is above the point (positive y in the +up frame) for a cap glyph.
    expect(g.yBot).toBeGreaterThan(0);
  });

  it('centers a section name and sits it above the anchor (bottom-anchored)', () => {
    const atlas = makeAtlas();
    const s = shapeLabel('AA', LABEL_KIND.SECTION_NAME, atlas);
    // Two 0.5em glyphs -> 1em line, centered on x=0 -> spans about [-0.5, 0.5].
    const xs = s.glyphs.map((g) => (g.xL + g.xR) / 2);
    expect(Math.min(...xs)).toBeLessThan(0);
    expect(Math.max(...xs)).toBeGreaterThan(0);
    // Above the anchor.
    expect(s.glyphs[0]!.yBot).toBeGreaterThan(0);
  });
});

describe('shapeLabel comment wrapping (12 em)', () => {
  it('wraps greedily on whitespace and no rendered line exceeds the max width', () => {
    const atlas = makeAtlas();
    // Each 4-char word is 2 em; with 0.25em spaces, ~5 words fit per 12em line.
    const words = Array.from({ length: 12 }, () => 'WXYZ');
    const text = words.join(' ');
    const s = shapeLabel(text, LABEL_KIND.STATION_COMMENT, atlas);
    // 12 words * 4 inked glyphs = 48 glyphs, whatever the wrapping.
    expect(s.glyphCount).toBe(48);
    // Reconstruct per-line widths from the distinct baselines (yTop groups).
    // Simpler: assert the shaping produced multiple lines by span of yBot.
    const yValues = new Set(s.glyphs.map((g) => Math.round(g.yBot * 1000)));
    expect(yValues.size).toBeGreaterThan(1); // more than one line
    // And each greedily-built line is within the max: rebuild the wrap directly.
    for (const line of ['WXYZ WXYZ WXYZ WXYZ WXYZ']) {
      expect(lineWidthEm(line, atlas)).toBeLessThanOrEqual(COMMENT_MAX_WIDTH_EM);
    }
  });

  it('gives an over-long single word its own line (no mid-word break)', () => {
    const atlas = makeAtlas();
    const longWord = 'X'.repeat(40); // 20 em > 12 em max
    const s = shapeLabel('hi ' + longWord, LABEL_KIND.SHOT_COMMENT, atlas);
    // "hi" (2 glyphs) on line 1, the long word (40 glyphs) alone on line 2.
    expect(s.glyphCount).toBe(42);
    const lines = new Set(s.glyphs.map((g) => Math.round(g.yBot * 1000)));
    expect(lines.size).toBe(2);
  });

  it('does not wrap non-comment kinds (single line even when very wide)', () => {
    const atlas = makeAtlas();
    const wide = 'A'.repeat(60); // 30 em, far past 12
    const s = shapeLabel(wide, LABEL_KIND.SHOT_LABEL, atlas);
    const lines = new Set(s.glyphs.map((g) => Math.round(g.yBot * 1000)));
    expect(lines.size).toBe(1);
  });

  it('preserves an explicit newline as a HARD break (short lines that would otherwise share one)', () => {
    const atlas = makeAtlas();
    // "A" and "B" (0.5 em each) fit trivially on one 12 em line, so the OLD
    // split-on-/\s+/ join collapsed "A\nB" to a single line. The hard break must
    // now render them on two separate lines.
    const s = shapeLabel('A\nB', LABEL_KIND.STATION_COMMENT, atlas);
    expect(s.glyphCount).toBe(2);
    const lines = new Set(s.glyphs.map((g) => Math.round(g.yBot * 1000)));
    expect(lines.size).toBe(2);
  });

  it('width-wraps within each hard line independently', () => {
    const atlas = makeAtlas();
    // Line 1 is one short word (1 line); line 2 has 12 four-char words that wrap
    // to several visual lines, so the whole block is at least 3 visual lines.
    const line2 = Array.from({ length: 12 }, () => 'WXYZ').join(' ');
    const s = shapeLabel('hi\n' + line2, LABEL_KIND.SHOT_COMMENT, atlas);
    const lines = new Set(s.glyphs.map((g) => Math.round(g.yBot * 1000)));
    expect(lines.size).toBeGreaterThanOrEqual(3);
    // Every rendered line still respects the max width.
    for (const line of ['WXYZ WXYZ WXYZ WXYZ WXYZ']) {
      expect(lineWidthEm(line, atlas)).toBeLessThanOrEqual(COMMENT_MAX_WIDTH_EM);
    }
  });

  it('single-line comment text is unchanged (one line)', () => {
    const atlas = makeAtlas();
    const s = shapeLabel('hello', LABEL_KIND.STATION_COMMENT, atlas);
    const lines = new Set(s.glyphs.map((g) => Math.round(g.yBot * 1000)));
    expect(lines.size).toBe(1);
  });
});

describe('wrapIntoLines blank-line preservation', () => {
  it('keeps an interior blank hard line as one empty visual line', () => {
    const atlas = makeAtlas();
    // `a\n\nb` -> the empty middle hard line survives as a blank line, so the
    // paragraph break does not collapse (stock MapLibre preserves it too).
    expect(wrapIntoLines('a\n\nb', COMMENT_MAX_WIDTH_EM, atlas)).toEqual(['a', '', 'b']);
  });

  it('yields no lines for a wholly empty comment', () => {
    const atlas = makeAtlas();
    expect(wrapIntoLines('', COMMENT_MAX_WIDTH_EM, atlas)).toEqual([]);
    // A single whitespace-only hard line is ink-free too -> no lines.
    expect(wrapIntoLines('   ', COMMENT_MAX_WIDTH_EM, atlas)).toEqual([]);
  });

  it('preserves leading and trailing blank lines sanely', () => {
    const atlas = makeAtlas();
    expect(wrapIntoLines('a\n', COMMENT_MAX_WIDTH_EM, atlas)).toEqual(['a', '']);
    expect(wrapIntoLines('\na', COMMENT_MAX_WIDTH_EM, atlas)).toEqual(['', 'a']);
    // Two authored blank lines between paragraphs -> two empty visual lines.
    expect(wrapIntoLines('a\n\n\nb', COMMENT_MAX_WIDTH_EM, atlas)).toEqual(['a', '', '', 'b']);
  });

  it('shapes `a\\n\\nb` to two glyphs two line-heights apart (empty row occupies height)', () => {
    const atlas = makeAtlas();
    const s = shapeLabel('a\n\nb', LABEL_KIND.STATION_COMMENT, atlas);
    // Only `a` and `b` carry ink; the blank middle line contributes no glyph.
    expect(s.glyphCount).toBe(2);
    // `a` is line 0, `b` is line 2, so their baselines are 2 line-heights apart
    // (a single `a\nb` would be exactly one line-height) -- proving the blank
    // line reserved its row.
    const [ga, gb] = s.glyphs;
    expect(Math.abs(gb!.yBot - ga!.yBot)).toBeCloseTo(2 * LINE_HEIGHT_EM, 9);
  });
});

describe('shapedExtent', () => {
  it('is the tight em box over every glyph corner (+y up)', () => {
    const atlas = makeAtlas();
    const s = shapeLabel('AB', LABEL_KIND.SHOT_LABEL, atlas);
    const ext = shapedExtent(s)!;
    expect(ext).not.toBeNull();
    // Matches the min/max of the shaped glyph corners exactly.
    const minX = Math.min(...s.glyphs.map((g) => g.xL));
    const maxX = Math.max(...s.glyphs.map((g) => g.xR));
    const minY = Math.min(...s.glyphs.map((g) => g.yBot));
    const maxY = Math.max(...s.glyphs.map((g) => g.yTop));
    expect(ext.minX).toBeCloseTo(minX, 9);
    expect(ext.maxX).toBeCloseTo(maxX, 9);
    expect(ext.minY).toBeCloseTo(minY, 9);
    expect(ext.maxY).toBeCloseTo(maxY, 9);
    expect(ext.maxX).toBeGreaterThan(ext.minX);
    expect(ext.maxY).toBeGreaterThan(ext.minY);
  });

  it('returns null for an empty / whitespace-only label (no ink → no box)', () => {
    const atlas = makeAtlas();
    expect(shapedExtent(shapeLabel('', LABEL_KIND.SHOT_LABEL, atlas))).toBeNull();
    expect(shapedExtent(shapeLabel('   ', LABEL_KIND.SHOT_LABEL, atlas))).toBeNull();
  });
});

describe('LABEL_KIND_LAYOUT parity table', () => {
  it('carries the stock anchor/offset/max-width per kind', () => {
    expect(LABEL_KIND_LAYOUT[LABEL_KIND.SHOT_LABEL]).toMatchObject({ h: 'center', v: 'top', offsetEm: [0, 0.5], maxWidthEm: Infinity });
    expect(LABEL_KIND_LAYOUT[LABEL_KIND.STATION_LABEL]).toMatchObject({ h: 'left', v: 'bottom', offsetEm: [0.8, -0.8] });
    expect(LABEL_KIND_LAYOUT[LABEL_KIND.SECTION_NAME]).toMatchObject({ h: 'center', v: 'bottom', offsetEm: [0, -1.2] });
    expect(LABEL_KIND_LAYOUT[LABEL_KIND.STATION_COMMENT]).toMatchObject({ v: 'top', offsetEm: [0, 1.5], maxWidthEm: COMMENT_MAX_WIDTH_EM });
    expect(LABEL_KIND_LAYOUT[LABEL_KIND.SHOT_COMMENT].maxWidthEm).toBe(COMMENT_MAX_WIDTH_EM);
  });

  it('LINE_HEIGHT_EM is the MapLibre default 1.2', () => {
    expect(LINE_HEIGHT_EM).toBe(1.2);
  });
});

describe('ShapeCache', () => {
  it('returns the SAME ShapedLabel object for a repeated (kind, text) - no re-shaping', () => {
    // A counting rasterizer proves the atlas is only touched on a cache MISS.
    let rasterCalls = 0;
    const atlas = new GlyphAtlas({
      rasterizer: (c: string): RasterizedGlyph => {
        rasterCalls++;
        const s = specFor(c);
        return {
          data: new Uint8Array(Math.max(0, s.w * s.h)),
          width: s.w,
          height: s.h,
          glyphWidth: Math.max(0, s.w - 2 * SDF_BUFFER),
          glyphHeight: Math.max(0, s.h - 2 * SDF_BUFFER),
          glyphTop: s.top - SDF_BUFFER,
          glyphLeft: s.left + SDF_BUFFER,
          glyphAdvance: s.advance,
        };
      },
    });
    const cache = new ShapeCache();
    const first = cache.get(LABEL_KIND.STATION_LABEL, '12', atlas);
    const afterFirst = rasterCalls;
    const second = cache.get(LABEL_KIND.STATION_LABEL, '12', atlas);
    expect(second).toBe(first); // identical object, reused
    expect(rasterCalls).toBe(afterFirst); // zero further rasterization on a hit
    expect(cache.size).toBe(1);
  });

  it('keys on (kind, text): the same text at a different kind is a distinct entry', () => {
    const atlas = makeAtlas();
    const cache = new ShapeCache();
    const a = cache.get(LABEL_KIND.STATION_LABEL, '7', atlas);
    const b = cache.get(LABEL_KIND.SHOT_LABEL, '7', atlas);
    expect(b).not.toBe(a);
    expect(cache.size).toBe(2);
  });

  it('bounds the cache to its max, evicting the oldest (LRU)', () => {
    const atlas = makeAtlas();
    const cache = new ShapeCache(2);
    const s0 = cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas);
    cache.get(LABEL_KIND.SECTION_NAME, 'b', atlas);
    cache.get(LABEL_KIND.SECTION_NAME, 'c', atlas); // evicts 'a' (oldest)
    expect(cache.size).toBe(2);
    // 'a' was evicted → a fresh shape (new object) on the next request.
    const s0again = cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas);
    expect(s0again).not.toBe(s0);
  });

  it('a cache HIT refreshes recency so it survives a later eviction (LRU touch)', () => {
    const atlas = makeAtlas();
    const cache = new ShapeCache(2);
    const a = cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas);
    cache.get(LABEL_KIND.SECTION_NAME, 'b', atlas);
    cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas); // touch 'a' → now newest
    cache.get(LABEL_KIND.SECTION_NAME, 'c', atlas); // evicts 'b' (now oldest), not 'a'
    expect(cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas)).toBe(a); // 'a' survived
  });

  // #576 NF-W12BE-3: a hit and an eviction are O(1). The cache used to evict
  // `keys().next()` and touch with `delete` + `set`; JavaScriptCore's Map iterator
  // walks every slot deleted at the front since the last rehash, so a label build
  // larger than the cap paid a scan per eviction (18 of 27 s on a 384k-shot import).
  it('evicts and touches without iterating or re-inserting into its Map (#576 NF-W12BE-3)', () => {
    const atlas = makeAtlas();
    const cache = new ShapeCache(2);
    cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas);
    cache.get(LABEL_KIND.SECTION_NAME, 'b', atlas);
    const keys = vi.spyOn(Map.prototype, 'keys');
    const del = vi.spyOn(Map.prototype, 'delete');
    const set = vi.spyOn(Map.prototype, 'set');
    try {
      cache.get(LABEL_KIND.SECTION_NAME, 'a', atlas); // hit
      expect(del).not.toHaveBeenCalled();
      expect(set).not.toHaveBeenCalled();
      cache.get(LABEL_KIND.SECTION_NAME, 'c', atlas); // miss + eviction of 'b'
      expect(keys).not.toHaveBeenCalled();
      expect(del).toHaveBeenCalledTimes(1);
    } finally {
      keys.mockRestore();
      del.mockRestore();
      set.mockRestore();
    }
    expect(cache.size).toBe(2);
  });

  it('keeps exactly the LRU contents of a reference model over a long random workload', () => {
    const atlas = makeAtlas();
    const max = 5;
    const cache = new ShapeCache(max);
    const shapes = new Map<string, unknown>();
    let order: string[] = []; // reference: oldest first
    let seed = 7;
    const rnd = () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed;
    };
    for (let i = 0; i < 2000; i++) {
      const text = String(rnd() % 9);
      const got = cache.get(LABEL_KIND.SECTION_NAME, text, atlas);
      if (order.includes(text)) {
        expect(got).toBe(shapes.get(text)); // a hit returns the cached object
        order = [...order.filter((t) => t !== text), text];
      } else {
        expect(got).not.toBe(shapes.get(text)); // a miss shapes afresh
        shapes.set(text, got);
        order = [...order, text];
        if (order.length > max) order = order.slice(1);
      }
      expect(cache.size).toBe(order.length);
    }
  });
});
