/**
 * Determinism of label SHAPING - wrap, glyph quads, extents and the shared
 * `ShapeCache` (#576 fuzzers, FZ-DET-TS). See `src/test/determinism.ts`.
 *
 * Asserted, per seed, over random label / comment texts (spaces, runs of
 * whitespace, authored `\n`, words wider than the wrap width):
 *  - the same text shaped twice, on atlases with the same history, is identical;
 *  - the em geometry (wrapped lines, quad corners, ink size, extent) does not
 *    depend on the atlas's history - an atlas pre-warmed with other glyphs in a
 *    random order shapes the same quads;
 *  - `ShapeCache` is transparent: any request sequence (repeats, a cap small
 *    enough to evict) returns exactly what direct shaping returns, and two caches
 *    fed the same requests in different orders agree on every key.
 *
 * NOT asserted: the atlas pixel rect (`ax` / `ay`) across atlases with different
 * histories - the shelf packer places a glyph where the cursor is on first use
 * (glyphAtlas.ts 121-134), so placement order is the atlas's state, not a leak.
 */
import { describe, expect, it } from 'vitest';
import { ShapeCache, shapeLabel, shapedExtent, wrapIntoLines, LABEL_KIND_LAYOUT, type ShapedLabel } from '../labelLayout';
import { LABEL_KIND, type LabelKind } from '../labelShaders';
import type { GlyphAtlas } from '../glyphAtlas';
import {
  determinismSeeds,
  fakeGlyphAtlas,
  mulberry32,
  pick,
  randInt,
  seededDiff,
  shuffled,
  type Rng,
} from '../../../../../test/determinism';

const SEEDS = determinismSeeds();
const KINDS: readonly LabelKind[] = [
  LABEL_KIND.SHOT_LABEL,
  LABEL_KIND.STATION_LABEL,
  LABEL_KIND.SECTION_NAME,
  LABEL_KIND.STATION_COMMENT,
  LABEL_KIND.SHOT_COMMENT,
];
const WORDS = ['CaveA', 'StationA1', 'sump', 'restriction', 'halocline', 'tie-in', 'x', 'Pozo', 'Ãrbol', '12.5m'];

function randomText(rng: Rng): string {
  const parts: string[] = [];
  const n = randInt(rng, 0, 14);
  for (let i = 0; i < n; i++) {
    parts.push(pick(rng, WORDS));
    parts.push(pick(rng, [' ', ' ', '  ', '\n', ' \n ', '\t']));
  }
  if (rng() < 0.2) parts.push('Supercalifragilisticexpialidocious');
  return parts.join('');
}

/** Everything about a shape that is atlas-history independent. */
function geometry(s: ShapedLabel) {
  return {
    glyphCount: s.glyphCount,
    quads: s.glyphs.map((g) => [g.xL, g.xR, g.yTop, g.yBot, g.aw, g.ah]),
    extent: shapedExtent(s),
  };
}

function warmedAtlas(rng: Rng): GlyphAtlas {
  const atlas = fakeGlyphAtlas();
  const chars = shuffled([...new Set(WORDS.join('') + 'Supercalifragilisticexpialidocious0123456789.-')], rng);
  for (const c of chars.slice(0, randInt(rng, 0, chars.length))) atlas.getGlyph(c);
  return atlas;
}

describe('label shaping is deterministic (FZ-DET-TS)', () => {
  it('same text twice on same-history atlases is identical; em geometry ignores atlas history', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      for (let i = 0; i < 20; i++) {
        const text = randomText(rng);
        const kind = pick(rng, KINDS);
        const a = shapeLabel(text, kind, fakeGlyphAtlas());
        const b = shapeLabel(text, kind, fakeGlyphAtlas());
        expect(seededDiff(seed, `shape #${i} ${JSON.stringify(text)}`, a, b)).toBeNull();
        const warmed = shapeLabel(text, kind, warmedAtlas(rng));
        expect(seededDiff(seed, `em geometry #${i} ${JSON.stringify(text)}`, geometry(a), geometry(warmed))).toBeNull();
        const width = LABEL_KIND_LAYOUT[kind].maxWidthEm;
        const lines = (atlas: GlyphAtlas) => wrapIntoLines(text, width, atlas);
        expect(seededDiff(seed, `wrap #${i}`, lines(fakeGlyphAtlas()), lines(warmedAtlas(rng)))).toBeNull();
      }
    }
  });

  it('ShapeCache is transparent under any request order, with evictions', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const pool = Array.from({ length: 30 }, () => ({ kind: pick(rng, KINDS), text: randomText(rng) }));
      // One atlas that already holds every glyph, so uv rects are fixed and shapes
      // from either path or either cache must agree byte for byte.
      const atlas = fakeGlyphAtlas();
      for (const p of pool) shapeLabel(p.text, p.kind, atlas);
      const requests = Array.from({ length: 120 }, () => pick(rng, pool));
      const small = new ShapeCache(randInt(rng, 1, 12));
      for (const [i, r] of requests.entries()) {
        const viaCache = small.get(r.kind, r.text, atlas);
        expect(seededDiff(seed, `request #${i}`, shapeLabel(r.text, r.kind, atlas), viaCache)).toBeNull();
      }
      const a = new ShapeCache();
      const b = new ShapeCache();
      for (const r of requests) a.get(r.kind, r.text, atlas);
      for (const r of shuffled(requests, rng)) b.get(r.kind, r.text, atlas);
      for (const [i, p] of pool.entries()) {
        expect(seededDiff(seed, `cache key #${i}`, a.get(p.kind, p.text, atlas), b.get(p.kind, p.text, atlas))).toBeNull();
      }
    }
  });
});
