/**
 * MapLibre `CustomLayerInterface` for the custom-render LABEL pass (#customRender
 * labels, Phase 2). Draws SDF glyph quads from typed-array buffers + a growing R8
 * glyph-atlas texture - the first sampler in this renderer. Sibling of
 * `surveyCustomLayer.ts` (geometry); same GL discipline (WebGL2 / GLSL ES 3.00,
 * premultiplied-alpha blend, first-frames `getError()` self-disable) and the same
 * anchor-relative-mercator + screen-px-offset primitive via `composeAnchorMatrix`.
 *
 * INDEPENDENCE: this layer fails on its own. A compile/link failure or a GL error
 * self-disables JUST the labels (the controller removes the dead layer and emits
 * a fatal - there is no stock fallback to restore),
 * leaving the geometry custom layer untouched - the design's "the two custom
 * layers can fail independently" (§8).
 *
 * PER-FRAME UNIFORMS: the layer holds no zoom-dependent state. Each frame it reads
 * the LIVE zoom (`map.getZoom()`), uploads it as `u_zoom`, and calls
 * {@link computeLabelUniforms} with the controller's style inputs, so `text-size`
 * / `text-opacity` interpolate continuously and per-label reveal-zoom admission
 * (`a_reveal_zoom` vs `u_zoom`) + minzoom gating track zoom with zero buffer
 * rebuild (§0/§3 of the design).
 */
import type { CustomLayerInterface, CustomRenderMethodInput, Map as MaplibreMap } from 'maplibre-gl';
import { composeAnchorMatrix } from './mercator';
import { LABEL_VERTEX_SRC, LABEL_FRAGMENT_SRC } from './labelShaders';
import { SDF_FONT_SIZE, SDF_RADIUS, type GlyphAtlas } from './glyphAtlas';
import { computeLabelUniforms, type LabelStyleInputs } from './labelStyle';
import type { LabelBuffers } from './labelTessellate';
import {
  type ProgramBundle,
  linkProgramBundle,
  deleteProgramBundle,
  bindVec2,
  bindFloat,
  bindColor,
  bindIntU8,
} from './glProgram';

/** See {@link import('./surveyCustomLayer').SelfDisableCallback}. */
export type LabelSelfDisableCallback = (reason: string) => void;

/** How many consecutive clean frames to validate with `gl.getError()` before
 *  trusting the layer (a `getError()` is a pipeline stall). */
const MAX_CHECKED_FRAMES = 5;

/** Everything the layer needs to draw one frame; swapped wholesale by update(). */
export interface LabelRenderData {
  anchorX: number;
  anchorY: number;
  buffers: LabelBuffers;
  /** The glyph atlas backing `a_uv` - the layer drains its pending texture
   *  uploads (grow → full re-upload; else per-rect `texSubImage2D`). */
  atlas: GlyphAtlas;
}

interface LabelGpu {
  anchor: WebGLBuffer;
  offsetEm: WebGLBuffer;
  uv: WebGLBuffer;
  color: WebGLBuffer;
  kind: WebGLBuffer;
  revealZoom: WebGLBuffer;
  flags: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

/** Uniform / attribute names of the label program, resolved once at link time
 *  ({@link ProgramBundle}). Each list must match the GLSL declarations in
 *  labelShaders.ts; `__tests__/programNames.test.ts` pins that. `u_text_size_px`
 *  and the other per-kind arrays are queried by their BARE name, exactly as the
 *  per-frame lookups did. */
export const LABEL_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_atlas_size',
  'u_font_size',
  'u_sdf_radius',
  'u_zoom',
  'u_text_size_px',
  'u_opacity',
  'u_kind_visible',
  'u_halo_color',
  'u_halo_width_px',
  'u_hide_reference',
  'u_hide_jump',
  'u_atlas',
] as const;

export const LABEL_ATTRIBS = [
  'a_anchor',
  'a_offset_em',
  'a_uv',
  'a_color',
  'a_kind',
  'a_reveal_zoom',
  'a_flags',
] as const;

function makeBuffer(gl: WebGL2RenderingContext, target: number, data: ArrayBufferView): WebGLBuffer {
  const buf = gl.createBuffer()!;
  gl.bindBuffer(target, buf);
  gl.bufferData(target, data, gl.STATIC_DRAW);
  return buf;
}

/** A neutral style default so a frame before the controller pushes inputs never
 *  reads undefined (everything hidden, opaque-black halo - draws nothing since
 *  kindVisible is 0). */
const DEFAULT_STYLE: LabelStyleInputs = {
  dpr: 1,
  kindEnabled: [false, false, false, false, false],
  haloColors: [],
  hideReference: false,
      hideJump: false,
};

export class SurveyCustomLabelLayer implements CustomLayerInterface {
  readonly id: string;
  readonly type = 'custom' as const;
  readonly renderingMode = '2d' as const;

  private map: MaplibreMap | null = null;
  private gl: WebGL2RenderingContext | null = null;
  private bundle: ProgramBundle<typeof LABEL_UNIFORMS, typeof LABEL_ATTRIBS> | null = null;
  private gpu: LabelGpu | null = null;
  private tex: WebGLTexture | null = null;
  /** Atlas dims currently allocated in `tex` (0 = not yet uploaded). */
  private texWidth = 0;
  private texHeight = 0;

  private disabled = false;
  private disabledReason: string | null = null;
  private readonly onSelfDisable: LabelSelfDisableCallback | null;
  private checkedFrames = 0;
  private firstFrameLogged = false;

  private data: LabelRenderData | null = null;
  private style: LabelStyleInputs = DEFAULT_STYLE;

  constructor(id: string, initial: LabelRenderData | null, onSelfDisable: LabelSelfDisableCallback | null = null) {
    this.id = id;
    this.data = initial;
    this.onSelfDisable = onSelfDisable;
  }

  isDisabled(): boolean {
    return this.disabled;
  }

  getDisabledReason(): string | null {
    return this.disabledReason;
  }

  /** Replace the controller-owned NON-zoom style inputs (theme halo, per-kind
   *  toggles, DPR, reference-hide). Cheap: no buffer touch, just a repaint by the
   *  caller. The layer folds these with the live zoom each frame. */
  setStyleInputs(style: LabelStyleInputs): void {
    this.style = style;
  }

  onAdd(map: MaplibreMap, gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    if (typeof WebGL2RenderingContext === 'undefined' || !(gl instanceof WebGL2RenderingContext)) {
      console.warn('[customRender] WebGL2 unavailable; disabling custom label layer');
      this.disabled = true;
      this.disabledReason = 'webgl2-unavailable';
      return;
    }
    this.map = map;
    this.gl = gl;
    this.disabled = false;
    this.disabledReason = null;
    // linkProgramBundle() already logs getShaderInfoLog / getProgramInfoLog ungated.
    this.bundle = linkProgramBundle(
      gl, LABEL_VERTEX_SRC, LABEL_FRAGMENT_SRC, LABEL_UNIFORMS, LABEL_ATTRIBS, '[customRender] label',
    );
    if (!this.bundle) {
      console.warn('[customRender] label program link failed; disabling custom label layer');
      this.disabled = true;
      this.disabledReason = 'program-link-failed';
      return;
    }
    // Reset the glyph texture handle so uploadAtlas does a fresh texImage2D on
    // every add, including the re-add after a style rebuild (styleRestore.ts).
    // A handle from an earlier add may belong to a dead context; without the
    // reset, uploadAtlas' `!this.tex` + `sizeChanged` guards would skip the
    // upload and the labels would silently fail to re-appear.
    this.tex = null;
    this.texWidth = 0;
    this.texHeight = 0;
    if (this.data) {
      this.uploadBuffers(this.data.buffers);
      this.uploadAtlas(this.data.atlas);
    }
  }

  /** Replace the glyph buffers + atlas; re-uploads if the context is live. */
  update(data: LabelRenderData): void {
    this.data = data;
    if (this.gl && !this.disabled) {
      this.uploadBuffers(data.buffers);
      this.uploadAtlas(data.atlas);
    }
  }

  /**
   * Re-upload ONLY the per-vertex flags buffer - the cheap DIM / drag-hide flip +
   * collision-rerun path (Phase 5). `flags.length` must equal the currently
   * uploaded glyph buffer's vertex count (the controller recomputes it from the
   * SAME shaped set via `buildLabelFlags`, so it always matches). `bufferSubData`
   * overwrites in place; positions / uv / color / indices are untouched. The
   * cached `data.buffers.flags` is kept coherent so a context-restore re-upload
   * carries the latest bits.
   */
  updateFlags(flags: Uint8Array): void {
    const gl = this.gl;
    if (!gl || this.disabled || !this.gpu) return;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.gpu.flags);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, flags);
    if (this.data) this.data.buffers.flags = flags;
  }

  /**
   * Re-upload ONLY the per-vertex reveal-zoom buffer - the cheap kind / reference
   * toggle path (rerun the reveal pass off the render loop, re-upload the
   * thresholds). Sibling of {@link updateFlags}: `revealZoom.length` must equal
   * the uploaded glyph buffer's vertex count (the controller recomputes it from
   * the SAME shaped set via `buildLabelRevealZooms`, so it always matches).
   * Positions / uv / color / flags / indices are untouched; the cached
   * `data.buffers.revealZoom` is kept coherent for a context-restore re-upload.
   */
  updateReveal(revealZoom: Float32Array): void {
    const gl = this.gl;
    if (!gl || this.disabled || !this.gpu) return;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.gpu.revealZoom);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, revealZoom);
    if (this.data) this.data.buffers.revealZoom = revealZoom;
  }

  private uploadBuffers(b: LabelBuffers): void {
    const gl = this.gl;
    if (!gl) return;
    this.freeGpu();
    if (b.glyphCount === 0) {
      this.gpu = null;
      return;
    }
    this.gpu = {
      anchor: makeBuffer(gl, gl.ARRAY_BUFFER, b.anchor),
      offsetEm: makeBuffer(gl, gl.ARRAY_BUFFER, b.offsetEm),
      uv: makeBuffer(gl, gl.ARRAY_BUFFER, b.uv),
      color: makeBuffer(gl, gl.ARRAY_BUFFER, b.color),
      kind: makeBuffer(gl, gl.ARRAY_BUFFER, b.kind),
      revealZoom: makeBuffer(gl, gl.ARRAY_BUFFER, b.revealZoom),
      flags: makeBuffer(gl, gl.ARRAY_BUFFER, b.flags),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, b.indices),
      count: b.indices.length,
    };
  }

  /**
   * Upload the atlas' pending texture changes. First time (or after a height
   * doubling) does a full `texImage2D` of the R8 grid; otherwise `texSubImage2D`s
   * only the per-glyph dirty rects the atlas recorded. R8 / RED / UNSIGNED_BYTE
   * with UNPACK_ALIGNMENT 1 (single-channel rows are not 4-aligned). No
   * UNPACK_FLIP_Y: `a_uv` pairs the screen-top glyph corner with atlas row `ay`
   * (the glyph's top), so data-row order matches sample order.
   */
  private uploadAtlas(atlas: GlyphAtlas): void {
    const gl = this.gl;
    if (!gl) return;
    if (!this.tex) {
      this.tex = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, this.tex);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      this.texWidth = 0;
      this.texHeight = 0;
    }
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    const uploads = atlas.takeUploads();
    const sizeChanged = atlas.width !== this.texWidth || atlas.height !== this.texHeight;
    if (this.texWidth === 0 || uploads.grew || sizeChanged) {
      // Full (re)allocation + upload.
      gl.texImage2D(
        gl.TEXTURE_2D, 0, gl.R8, atlas.width, atlas.height, 0, gl.RED, gl.UNSIGNED_BYTE, atlas.data,
      );
      this.texWidth = atlas.width;
      this.texHeight = atlas.height;
      return;
    }
    for (const r of uploads.rects) {
      const sub = subImage(atlas.data, atlas.width, r.x, r.y, r.w, r.h);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, r.x, r.y, r.w, r.h, gl.RED, gl.UNSIGNED_BYTE, sub);
    }
  }

  render(gl: WebGLRenderingContext | WebGL2RenderingContext, options: CustomRenderMethodInput): void {
    if (this.disabled || !(gl instanceof WebGL2RenderingContext)) return;
    const data = this.data;
    if (!data || !this.bundle || !this.gpu || !this.tex) return;
    const mainMatrix = options.defaultProjectionData.mainMatrix as ArrayLike<number> | undefined;
    if (!mainMatrix) return;
    const matrix = composeAnchorMatrix(mainMatrix, data.anchorX, data.anchorY);
    const zoom = this.map?.getZoom?.() ?? 0;
    const u = computeLabelUniforms(zoom, this.style);
    // `uloc`, not the sibling layers' `u`: that name already holds this frame's
    // computed uniform VALUES here.
    const { program, uniforms: uloc, attribs: a } = this.bundle;
    const g = this.gpu;

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.CULL_FACE);
    gl.useProgram(program);

    gl.uniformMatrix4fv(uloc.u_matrix, false, matrix);
    gl.uniform2f(uloc.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.uniform2f(uloc.u_atlas_size, this.texWidth, this.texHeight);
    gl.uniform1f(uloc.u_font_size, SDF_FONT_SIZE);
    gl.uniform1f(uloc.u_sdf_radius, SDF_RADIUS);
    gl.uniform1f(uloc.u_zoom, zoom);
    gl.uniform1fv(uloc.u_text_size_px, u.textSizePx);
    gl.uniform1fv(uloc.u_opacity, u.opacity);
    gl.uniform1fv(uloc.u_kind_visible, u.kindVisible);
    gl.uniform4fv(uloc.u_halo_color, u.haloColor);
    gl.uniform1fv(uloc.u_halo_width_px, u.haloWidthPx);
    gl.uniform1f(uloc.u_hide_reference, u.hideReference);
    gl.uniform1f(uloc.u_hide_jump, u.hideJump);

    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.uniform1i(uloc.u_atlas, 0);

    bindVec2(gl, a.a_anchor, g.anchor);
    bindVec2(gl, a.a_offset_em, g.offsetEm);
    bindVec2(gl, a.a_uv, g.uv);
    bindColor(gl, a.a_color, g.color);
    bindIntU8(gl, a.a_kind, g.kind);
    bindFloat(gl, a.a_reveal_zoom, g.revealZoom);
    bindIntU8(gl, a.a_flags, g.flags);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.index);
    gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, 0);

    this.checkFrameError(gl, data);
  }

  private checkFrameError(gl: WebGL2RenderingContext, data: LabelRenderData): void {
    if (this.checkedFrames >= MAX_CHECKED_FRAMES) return;
    const err = gl.getError();
    if (err !== gl.NO_ERROR) {
      const code = `0x${err.toString(16)}`;
      const reason = `gl-error-${code}`;
      this.disabled = true;
      this.disabledReason = reason;
      console.warn(`[customRender] label GL error ${code} after draw; self-disabling custom label layer`);
      this.onSelfDisable?.(reason);
      return;
    }
    this.checkedFrames += 1;
    if (!this.firstFrameLogged) {
      this.firstFrameLogged = true;
      console.info('[customRender] label first frame drawn OK', {
        glyphs: data.buffers.glyphCount,
        bytes: data.buffers.byteLength,
      });
    }
  }

  private freeGpu(): void {
    const gl = this.gl;
    if (!gl || !this.gpu) return;
    for (const key of Object.keys(this.gpu) as (keyof LabelGpu)[]) {
      const v = this.gpu[key];
      if (v instanceof WebGLBuffer) gl.deleteBuffer(v);
    }
    this.gpu = null;
  }

  onRemove(_map: MaplibreMap, _gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    const gl = this.gl;
    this.freeGpu();
    if (gl) {
      if (this.tex) gl.deleteTexture(this.tex);
      deleteProgramBundle(gl, this.bundle);
    }
    this.tex = null;
    this.texWidth = 0;
    this.texHeight = 0;
    this.bundle = null;
    this.gl = null;
    this.map = null;
  }
}

/** Copy an `w×h` sub-rect out of a row-major R8 grid into a tight buffer for
 *  `texSubImage2D` (rows in the source are `srcWidth` apart). */
function subImage(src: Uint8Array, srcWidth: number, x: number, y: number, w: number, h: number): Uint8Array {
  const out = new Uint8Array(w * h);
  for (let row = 0; row < h; row++) {
    const srcOff = (y + row) * srcWidth + x;
    out.set(src.subarray(srcOff, srcOff + w), row * w);
  }
  return out;
}
