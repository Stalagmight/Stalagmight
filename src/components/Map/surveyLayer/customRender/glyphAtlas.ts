/**
 * SDF glyph atlas for the custom-render LABEL pass (#customRender labels,
 * Phase 0).
 *
 * The first texture subsystem in `customRender/` (every other layer is pure
 * vector SDF math). Bundles Open Sans Regular locally and rasterizes each glyph
 * ON DEMAND into a single-channel (R8) shelf-packed atlas via TinySDF, so the
 * app needs NO network glyph endpoint (the remote
 * `fonts.openmaptiles.org/{fontstack}/{range}.pbf` dependency this renderer
 * deletes) and rasterization stays synchronous canvas work off the critical
 * path. The charset is tiny (digits, `°`, `m`, `.`, space, line-name /
 * comment letters - a few hundred glyphs), so one 1024-wide page holds
 * everything in practice.
 *
 * PURITY / ENVIRONMENT: this module owns only CPU-side data (the R8 byte grid +
 * per-glyph metrics + dirty-region list). It never touches a `WebGL*` context -
 * the label GL layer (Phase 2) uploads `data` / consumes {@link GlyphAtlas.takeUploads}.
 * The rasterizer is INJECTABLE so the `node` unit tests can drive packing/growth
 * with a deterministic fake and no canvas: {@link createTinySdfRasterizer}
 * (browser only) is the production one; {@link GlyphAtlas} with no rasterizer is
 * inert (returns zero-advance fallback glyphs rather than throwing).
 *
 * SDF PARAMS (must agree with `labelShaders.ts`): `fontSize 24, buffer 3,
 * radius 8, cutoff 0.25`, so the glyph edge lands at byte `191` (normalized
 * `SDF_EDGE = 0.75`) exactly as MapLibre's own glyph shader expects. See the
 * `labelShaders.ts` header for why 0.75 (not 0.5).
 */
import TinySDF from '@mapbox/tiny-sdf';
// Vite resolves this asset import to a served URL string (dev + build + vitest).
// The bundled font is Open Sans Regular (SIL OFL 1.1) under src/assets/fonts/.
import openSansUrl from '../../../../assets/fonts/OpenSans-Regular.ttf';

/** TinySDF font size in px (the raster em). Layout scales advances/offsets by
 *  `renderedSizePx / SDF_FONT_SIZE`. */
export const SDF_FONT_SIZE = 24;
/** Padding around each glyph's ink box, in px - the halo headroom. */
export const SDF_BUFFER = 3;
/** SDF encode radius in px (distance range around the edge). */
export const SDF_RADIUS = 8;
/** SDF cutoff - fraction of the radius inside the edge; `1 - cutoff = 0.75` is
 *  the normalized edge value the shader reads (MapLibre's `192/256`). */
export const SDF_CUTOFF = 0.25;

/** Font-family label registered via FontFace and passed to TinySDF. Distinct so
 *  it never collides with a page/system font of the same name. */
export const LABEL_FONT_FAMILY = 'CaveLabelSans';

/** Atlas page width (fixed); growth doubles HEIGHT only, so a glyph's atlas
 *  origin never moves on grow. */
export const ATLAS_WIDTH = 1024;
/** Initial atlas height; doubles on demand up to {@link maxHeight}. */
export const ATLAS_INITIAL_HEIGHT = 1024;
/** Default height cap (WebGL2 guarantees MAX_TEXTURE_SIZE >= 2048). The GL layer
 *  passes the queried `MAX_TEXTURE_SIZE` to cap lower on a constrained GPU. */
export const ATLAS_MAX_HEIGHT = 2048;
/** 1 px gutter between packed glyphs so bilinear sampling can't bleed a
 *  neighbour into a glyph's halo band. */
const GLYPH_GUTTER = 1;

/** The raw output of a rasterizer for one character - the exact shape of
 *  `TinySDF.draw()`. `data` is `width*height` R8 SDF bytes. */
export interface RasterizedGlyph {
  data: Uint8ClampedArray | Uint8Array;
  /** Bitmap dims (ink box + 2*buffer). */
  width: number;
  height: number;
  /** Ink box dims (0 for whitespace / empty glyphs). */
  glyphWidth: number;
  glyphHeight: number;
  /** Distance from the baseline UP to the top of the ink box, in px. */
  glyphTop: number;
  /** Ink box left edge relative to the pen origin, in px (can be negative). */
  glyphLeft: number;
  /** Pen advance width, in px. */
  glyphAdvance: number;
}

/** Rasterize one character to an SDF bitmap + metrics. */
export type GlyphRasterizer = (char: string) => RasterizedGlyph;

/** Per-glyph atlas placement + layout metrics. All px are in the 24 px raster
 *  space; `labelLayout.ts` converts to em by dividing by {@link SDF_FONT_SIZE}. */
export interface GlyphInfo {
  /** Atlas pixel rect of the SDF bitmap (top-left origin, `u_atlas`-relative). */
  ax: number;
  ay: number;
  aw: number;
  ah: number;
  /** Pen advance width (px). */
  advance: number;
  /** x offset from the pen origin to the bitmap's left edge (px; `glyphLeft -
   *  buffer`, may be negative). */
  left: number;
  /** Distance from the baseline UP to the bitmap's top edge (px; `glyphTop +
   *  buffer`). Positive = above the baseline. */
  top: number;
  /** Bitmap width/height (px) == `aw`/`ah`. Zero for whitespace (no quad). */
  width: number;
  height: number;
}

/** A dirty sub-rect the GL layer must `texSubImage2D` after a new glyph packs. */
export interface DirtyRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Result of {@link GlyphAtlas.takeUploads}: whether the backing store was
 *  reallocated (grew -> full re-upload) and the per-glyph dirty rects. */
export interface AtlasUploads {
  grew: boolean;
  rects: DirtyRect[];
}

/** A zero-size, zero-advance fallback glyph (used when no rasterizer is present
 *  in an inert atlas). Emits no quad and does not advance the pen. */
const INERT_GLYPH: GlyphInfo = { ax: 0, ay: 0, aw: 0, ah: 0, advance: 0, left: 0, top: 0, width: 0, height: 0 };

/**
 * A growing, shelf-packed R8 glyph atlas. Lazily rasterizes + packs a glyph on
 * first {@link getGlyph} and caches it forever (glyphs are immutable once
 * placed; the atlas only ever appends).
 *
 * SHELF (next-fit) PACKING: glyphs flow left-to-right along the current shelf;
 * when the next glyph would overflow the width, a new shelf opens below the
 * tallest glyph of the previous one. This tolerates any glyph height (unlike a
 * fixed row height), which matters because accented caps / descenders can exceed
 * the nominal `fontSize + 2*buffer`. When a new shelf would overflow the current
 * height, the atlas doubles its height (up to {@link maxHeight}); a doubling
 * keeps every existing glyph's `(ax, ay)` valid (row-major, width unchanged), so
 * only the `u_atlas_size` uniform and a full re-upload are needed - no re-layout.
 */
export class GlyphAtlas {
  readonly width = ATLAS_WIDTH;
  height: number;
  readonly maxHeight: number;
  /** R8 pixels, row-major, `width * height` bytes. */
  data: Uint8Array;

  private readonly rasterizer: GlyphRasterizer | null;
  private readonly glyphs = new Map<string, GlyphInfo>();
  // Shelf cursor. Starts one gutter in so nothing hugs the page edge.
  private shelfX = GLYPH_GUTTER;
  private shelfY = GLYPH_GUTTER;
  private shelfHeight = 0;
  private grew = false;
  private dirty: DirtyRect[] = [];

  constructor(opts: { rasterizer?: GlyphRasterizer | null; initialHeight?: number; maxHeight?: number } = {}) {
    this.height = opts.initialHeight ?? ATLAS_INITIAL_HEIGHT;
    this.maxHeight = Math.max(this.height, opts.maxHeight ?? ATLAS_MAX_HEIGHT);
    this.data = new Uint8Array(this.width * this.height);
    this.rasterizer = opts.rasterizer ?? null;
  }

  /** How many distinct glyphs are cached (packing/growth test hook). */
  get glyphCount(): number {
    return this.glyphs.size;
  }

  /**
   * Metrics + atlas placement for `char`, rasterizing + packing on first use.
   * Returns a cached {@link GlyphInfo} thereafter. In an inert atlas (no
   * rasterizer), or if the glyph cannot fit even after growth, returns a
   * zero-advance fallback so callers never crash (the "tofu"-free degenerate
   * case - a missing glyph simply contributes nothing).
   */
  getGlyph(char: string): GlyphInfo {
    const cached = this.glyphs.get(char);
    if (cached) return cached;
    if (!this.rasterizer) return INERT_GLYPH;

    const r = this.rasterizer(char);
    const info = this.pack(r);
    this.glyphs.set(char, info);
    return info;
  }

  /**
   * Consume the pending texture uploads and reset the dirty state. The GL layer
   * calls this after `getGlyph`s for a build: on `grew` it re-uploads the whole
   * (reallocated) `data`; otherwise it `texSubImage2D`s each rect.
   */
  takeUploads(): AtlasUploads {
    const out: AtlasUploads = { grew: this.grew, rects: this.dirty };
    this.grew = false;
    this.dirty = [];
    return out;
  }

  /** Pack a rasterized glyph onto a shelf, blit its bytes, record metrics. */
  private pack(r: RasterizedGlyph): GlyphInfo {
    const w = r.width;
    const h = r.height;
    // Whitespace / empty ink: no bitmap to place, but the pen still advances.
    if (w <= 0 || h <= 0) {
      return { ax: 0, ay: 0, aw: 0, ah: 0, advance: r.glyphAdvance, left: 0, top: 0, width: 0, height: 0 };
    }
    // A single glyph wider than the whole page cannot be shelf-packed; drop its
    // bitmap but keep the advance (degrades to an invisible-but-spaced glyph).
    if (w + 2 * GLYPH_GUTTER > this.width) {
      return { ax: 0, ay: 0, aw: 0, ah: 0, advance: r.glyphAdvance, left: 0, top: 0, width: 0, height: 0 };
    }
    // Open a new shelf when this glyph would overflow the current one's width.
    if (this.shelfX + w + GLYPH_GUTTER > this.width) {
      this.shelfY += this.shelfHeight + GLYPH_GUTTER;
      this.shelfX = GLYPH_GUTTER;
      this.shelfHeight = 0;
    }
    // Grow (double height) until the shelf fits, or give up at the cap.
    while (this.shelfY + h + GLYPH_GUTTER > this.height) {
      if (!this.growHeight()) {
        // Cap reached (multi-page fallback is a designed-but-unexpected path;
        // not needed for the label charset). Emit an advance-only glyph.
        return { ax: 0, ay: 0, aw: 0, ah: 0, advance: r.glyphAdvance, left: 0, top: 0, width: 0, height: 0 };
      }
    }
    const ax = this.shelfX;
    const ay = this.shelfY;
    this.blit(r.data, w, h, ax, ay);
    this.shelfX += w + GLYPH_GUTTER;
    if (h > this.shelfHeight) this.shelfHeight = h;
    return {
      ax,
      ay,
      aw: w,
      ah: h,
      advance: r.glyphAdvance,
      left: r.glyphLeft - SDF_BUFFER,
      top: r.glyphTop + SDF_BUFFER,
      width: w,
      height: h,
    };
  }

  /** Double the atlas height (up to the cap). Returns false at the cap. Existing
   *  glyph placements survive unchanged (row-major, width fixed). */
  private growHeight(): boolean {
    if (this.height >= this.maxHeight) return false;
    const newHeight = Math.min(this.maxHeight, this.height * 2);
    const next = new Uint8Array(this.width * newHeight);
    next.set(this.data);
    this.data = next;
    this.height = newHeight;
    this.grew = true;
    return true;
  }

  /** Copy an `w*h` R8 bitmap into `data` at `(ax, ay)`, row by row, and record a
   *  dirty rect for the incremental upload. */
  private blit(src: Uint8ClampedArray | Uint8Array, w: number, h: number, ax: number, ay: number): void {
    const { data, width } = this;
    for (let row = 0; row < h; row++) {
      const dstOff = (ay + row) * width + ax;
      const srcOff = row * w;
      for (let col = 0; col < w; col++) data[dstOff + col] = src[srcOff + col]!;
    }
    // On a grow, the whole store is re-uploaded, so per-glyph rects would be
    // redundant; skip them until the next takeUploads clears `grew`.
    if (!this.grew) this.dirty.push({ x: ax, y: ay, w, h });
  }
}

// ─── Font loading + production rasterizer (browser only) ───────────────────

let fontPromise: Promise<boolean> | null = null;

/**
 * Idempotently load + register the bundled Open Sans via the FontFace API. The
 * returned promise resolves `true` once the font is ready (so the atlas
 * rasterizes with the exact letterforms the app shows), or `false` in a non-DOM
 * / unsupported env (tests, SSR) - callers then either inject their own
 * rasterizer or accept the inert atlas. Safe to call many times; the load
 * happens once. Pre-warm at app init so the font is ready before the first
 * label build (a raster requested before ready would fall back to system
 * `sans-serif`; bounded to boot).
 */
export function ensureLabelFont(): Promise<boolean> {
  if (fontPromise) return fontPromise;
  fontPromise = (async () => {
    if (typeof document === 'undefined' || typeof FontFace === 'undefined') return false;
    try {
      const face = new FontFace(LABEL_FONT_FAMILY, `url(${openSansUrl})`);
      await face.load();
      (document as unknown as { fonts: FontFaceSet }).fonts.add(face);
      return true;
    } catch (e) {
      console.warn('[customRender] label font load failed; falling back to system sans-serif:', e);
      return false;
    }
  })();
  return fontPromise;
}

/** True in a canvas-capable environment (browser / worker). TinySDF needs an
 *  OffscreenCanvas or a DOM `<canvas>`. */
export function canRasterize(): boolean {
  return typeof OffscreenCanvas !== 'undefined' || typeof document !== 'undefined';
}

/**
 * Build the production TinySDF rasterizer with the module's SDF params + the
 * bundled font family. Browser only (constructs a canvas). Returns null where no
 * canvas is available (the atlas then stays inert). Construct AFTER
 * {@link ensureLabelFont} resolves so glyphs use Open Sans, not the boot
 * fallback.
 */
export function createTinySdfRasterizer(): GlyphRasterizer | null {
  if (!canRasterize()) return null;
  const sdf = new TinySDF({
    fontSize: SDF_FONT_SIZE,
    buffer: SDF_BUFFER,
    radius: SDF_RADIUS,
    cutoff: SDF_CUTOFF,
    fontFamily: LABEL_FONT_FAMILY,
  });
  return (char: string): RasterizedGlyph => sdf.draw(char) as RasterizedGlyph;
}
