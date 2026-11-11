/**
 * MapLibre `CustomLayerInterface` for the bulk survey geometry prototype
 * (#customRender). Draws shots (screen-space AA lines) and stations (fragment
 * circles) directly from typed-array vertex buffers - no vector tiles, no
 * publication machinery. An edit becomes a buffer re-upload + `triggerRepaint`.
 *
 * GL discipline (per the CustomLayerInterface contract):
 *  - onAdd(map, gl): compile programs, create + upload buffers. Called again by
 *    `styleRestore.ts` after a context restore / worker restart, so it fully
 *    rebuilds.
 *  - render(gl, opts): binds its OWN program / buffers / attributes / uniforms
 *    every frame, assumes nothing about incoming GL state, and sets the
 *    premultiplied-alpha blend MapLibre expects (ONE, ONE_MINUS_SRC_ALPHA).
 *  - onRemove(map, gl): frees all GL resources.
 *  - render() early-returns if buffers/programs are gone (context loss guard).
 *
 * WebGL2 only (GLSL ES 3.00 + UNSIGNED_INT indices). MapLibre 5.16 provides a
 * WebGL2 context; if a WebGL1 context is ever passed the programs fail to
 * compile and the layer disables itself with a console warning (prototype).
 */
import type { CustomLayerInterface, CustomRenderMethodInput, Map as MaplibreMap } from 'maplibre-gl';
import type { LineBuffers, CircleBuffers } from './tessellate';
import { composeAnchorMatrix } from './mercator';
import {
  LINE_VERTEX_SRC,
  LINE_FRAGMENT_SRC,
  CIRCLE_VERTEX_SRC,
  CIRCLE_FRAGMENT_SRC,
} from './shaders';
import {
  type ProgramBundle,
  linkProgramBundle,
  deleteProgramBundle,
  bindVec2,
  bindFloat,
  bindColor,
} from './glProgram';

/**
 * Notified when the layer disables itself mid-render (a GL error surfaced by
 * the first-frames `gl.getError()` probe). The controller removes the dead
 * layer and latches fatal - there is no stock fallback to restore. Fired from
 * inside MapLibre's render pass, so the controller must defer any style
 * mutation.
 */
export type SelfDisableCallback = (reason: string) => void;

/** How many consecutive clean frames to validate with `gl.getError()` before
 *  trusting the layer and dropping the per-frame sync stall that getError() costs. */
const MAX_CHECKED_FRAMES = 5;

/** Everything the layer needs to draw one frame; swapped wholesale by update(). */
export interface CustomRenderData {
  anchorX: number;
  anchorY: number;
  line: LineBuffers;
  circle: CircleBuffers;
  /** DEVICE-pixel style sizes. */
  lineHalfWidthPx: number;
  circleRadiusPx: number;
  circleStrokePx: number;
  aaPx: number;
  /** Configured highlight size multipliers (C2): shot line width bump for a
   *  hovered / selected section and station radius bump likewise, resolved from
   *  the independent `map.shot.hover_width` / `selection_width` +
   *  `map.station.hover_radius` / `selection_radius` config (or the historical
   *  2x/1.5x, 1.5x/1.25x defaults). Fed to the shaders' `u_*_mul` uniforms. */
  lineHoverWidthMul: number;
  lineSelectWidthMul: number;
  circleHoverSizeMul: number;
  circleSelectSizeMul: number;
}

interface LineGpu {
  posA: WebGLBuffer;
  posB: WebGLBuffer;
  endpoint: WebGLBuffer;
  side: WebGLBuffer;
  colorFrom: WebGLBuffer;
  colorTo: WebGLBuffer;
  reference: WebGLBuffer;
  dim: WebGLBuffer;
  hide: WebGLBuffer;
  highlight: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

interface CircleGpu {
  center: WebGLBuffer;
  corner: WebGLBuffer;
  fill: WebGLBuffer;
  stroke: WebGLBuffer;
  sizeMul: WebGLBuffer;
  dim: WebGLBuffer;
  hide: WebGLBuffer;
  highlight: WebGLBuffer;
  allReference: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

/** Uniform / attribute names of the two programs, resolved once at link time
 *  ({@link ProgramBundle}). Each list must match the GLSL declarations in
 *  shaders.ts; `__tests__/programNames.test.ts` pins that. */
export const LINE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_half_width',
  'u_aa',
  'u_hide_reference',
  'u_hide_jump',
  'u_hover_width_mul',
  'u_select_width_mul',
] as const;

export const LINE_ATTRIBS = [
  'a_pos_a',
  'a_pos_b',
  'a_endpoint',
  'a_side',
  'a_color_from',
  'a_color_to',
  'a_reference',
  'a_dim',
  'a_hide',
  'a_highlight',
] as const;

export const CIRCLE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_radius',
  'u_stroke_width',
  'u_aa',
  'u_hide_reference',
  'u_hide_jump',
  'u_hover_size_mul',
  'u_select_size_mul',
] as const;

export const CIRCLE_ATTRIBS = [
  'a_center',
  'a_corner',
  'a_fill',
  'a_stroke',
  'a_size_mul',
  'a_all_reference',
  'a_dim',
  'a_hide',
  'a_highlight',
] as const;

function makeBuffer(
  gl: WebGL2RenderingContext,
  target: number,
  data: ArrayBufferView<ArrayBufferLike>,
): WebGLBuffer {
  const buf = gl.createBuffer()!;
  gl.bindBuffer(target, buf);
  gl.bufferData(target, data, gl.STATIC_DRAW);
  return buf;
}

export class SurveyCustomLayer implements CustomLayerInterface {
  readonly id: string;
  readonly type = 'custom' as const;
  readonly renderingMode = '2d' as const;

  private gl: WebGL2RenderingContext | null = null;
  private lineBundle: ProgramBundle<typeof LINE_UNIFORMS, typeof LINE_ATTRIBS> | null = null;
  private circleBundle: ProgramBundle<typeof CIRCLE_UNIFORMS, typeof CIRCLE_ATTRIBS> | null = null;
  private lineGpu: LineGpu | null = null;
  private circleGpu: CircleGpu | null = null;
  private disabled = false;
  /** Machine-readable reason the layer is disabled ('webgl2-unavailable',
   *  'program-link-failed', 'gl-error-0x…'); null while healthy. */
  private disabledReason: string | null = null;
  private readonly onSelfDisable: SelfDisableCallback | null;
  /** Count of clean frames validated so far; caps at MAX_CHECKED_FRAMES. */
  private checkedFrames = 0;
  private firstFrameLogged = false;

  private data: CustomRenderData | null = null;
  /** Draw toggles mirroring the stock shots / stations layers' `visibility`
   *  (setShowShots / setShowStations). Skipping a draw is the custom-layer
   *  equivalent of hiding the stock layer. */
  private drawShots = true;
  private drawStations = true;
  /** Hide reference shots (setShowReferences false). Fed to the shaders'
   *  `u_hide_reference` uniform. Separate from jumps since #27. */
  private hideReference = false;
  /** Hide jump shots (setShowJumps false). Fed to `u_hide_jump`. */
  private hideJump = false;

  /** Show/hide custom-rendered shots (mirrors setShowShots). */
  setShotsVisible(visible: boolean): void {
    this.drawShots = visible;
  }

  /** Show/hide custom-rendered stations (mirrors setShowStations). */
  setStationsVisible(visible: boolean): void {
    this.drawStations = visible;
  }

  /** Hide/show reference shots (mirrors setShowReferences). A station touched
   *  only by virtual shots hides only when BOTH kinds are hidden. */
  setReferenceShotsVisible(visible: boolean): void {
    this.hideReference = !visible;
  }

  /** Hide/show jump shots (mirrors setShowJumps, #27). */
  setJumpShotsVisible(visible: boolean): void {
    this.hideJump = !visible;
  }

  /** Set before addLayer so onAdd uploads immediately. `onSelfDisable` lets the
   *  controller restore stock layers if a GL error kills the layer mid-render. */
  constructor(
    id: string,
    initial: CustomRenderData | null,
    onSelfDisable: SelfDisableCallback | null = null,
  ) {
    this.id = id;
    this.data = initial;
    this.onSelfDisable = onSelfDisable;
  }

  /** True once onAdd bailed or a GL error self-disabled the layer. */
  isDisabled(): boolean {
    return this.disabled;
  }

  /** Why the layer is disabled, or null while healthy. */
  getDisabledReason(): string | null {
    return this.disabledReason;
  }

  onAdd(_map: MaplibreMap, gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    if (typeof WebGL2RenderingContext === 'undefined' || !(gl instanceof WebGL2RenderingContext)) {
      // Ungated: the controller reads this state and must keep stock layers up.
      console.warn('[customRender] WebGL2 unavailable; disabling custom layer');
      this.disabled = true;
      this.disabledReason = 'webgl2-unavailable';
      return;
    }
    this.gl = gl;
    this.disabled = false;
    this.disabledReason = null;
    // linkProgramBundle() already logs getShaderInfoLog / getProgramInfoLog ungated.
    this.lineBundle = linkProgramBundle(
      gl, LINE_VERTEX_SRC, LINE_FRAGMENT_SRC, LINE_UNIFORMS, LINE_ATTRIBS, '[customRender]',
    );
    this.circleBundle = linkProgramBundle(
      gl, CIRCLE_VERTEX_SRC, CIRCLE_FRAGMENT_SRC, CIRCLE_UNIFORMS, CIRCLE_ATTRIBS, '[customRender]',
    );
    if (!this.lineBundle || !this.circleBundle) {
      console.warn('[customRender] shader program link failed; disabling custom layer');
      this.disabled = true;
      this.disabledReason = 'program-link-failed';
      return;
    }
    if (this.data) this.uploadBuffers(this.data);
  }

  /** Replace the geometry/style; re-uploads GPU buffers if the context is live. */
  update(data: CustomRenderData): void {
    this.data = data;
    if (this.gl && !this.disabled) this.uploadBuffers(data);
  }

  private uploadBuffers(data: CustomRenderData): void {
    const gl = this.gl;
    if (!gl) return;
    this.freeGpu();
    const l = data.line;
    this.lineGpu = {
      posA: makeBuffer(gl, gl.ARRAY_BUFFER, l.posA),
      posB: makeBuffer(gl, gl.ARRAY_BUFFER, l.posB),
      endpoint: makeBuffer(gl, gl.ARRAY_BUFFER, l.endpoint),
      side: makeBuffer(gl, gl.ARRAY_BUFFER, l.side),
      colorFrom: makeBuffer(gl, gl.ARRAY_BUFFER, l.colorFrom),
      colorTo: makeBuffer(gl, gl.ARRAY_BUFFER, l.colorTo),
      reference: makeBuffer(gl, gl.ARRAY_BUFFER, l.reference),
      dim: makeBuffer(gl, gl.ARRAY_BUFFER, l.dim),
      hide: makeBuffer(gl, gl.ARRAY_BUFFER, l.hide),
      highlight: makeBuffer(gl, gl.ARRAY_BUFFER, l.highlight),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, l.indices),
      count: l.indices.length,
    };
    const c = data.circle;
    this.circleGpu = {
      center: makeBuffer(gl, gl.ARRAY_BUFFER, c.center),
      corner: makeBuffer(gl, gl.ARRAY_BUFFER, c.corner),
      fill: makeBuffer(gl, gl.ARRAY_BUFFER, c.fill),
      stroke: makeBuffer(gl, gl.ARRAY_BUFFER, c.stroke),
      sizeMul: makeBuffer(gl, gl.ARRAY_BUFFER, c.sizeMul),
      dim: makeBuffer(gl, gl.ARRAY_BUFFER, c.dim),
      hide: makeBuffer(gl, gl.ARRAY_BUFFER, c.hide),
      highlight: makeBuffer(gl, gl.ARRAY_BUFFER, c.highlight),
      allReference: makeBuffer(gl, gl.ARRAY_BUFFER, c.allReference),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, c.indices),
      count: c.indices.length,
    };
  }

  /**
   * Re-upload ONLY the per-vertex dim buffers (line + circle) after an
   * anchored / immovable set change, without rebuilding positions, colors, or
   * indices. The arrays must match the currently-uploaded geometry's vertex
   * counts (the controller recomputes them from the same SoA + exclusion), so
   * this is the cheap path for move-tool activation vs a full geometry rebuild.
   */
  updateDim(lineDim: Float32Array, circleDim: Float32Array): void {
    const gl = this.gl;
    if (!gl || this.disabled) return;
    if (this.lineGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGpu.dim);
      gl.bufferData(gl.ARRAY_BUFFER, lineDim, gl.STATIC_DRAW);
    }
    if (this.circleGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.circleGpu.dim);
      gl.bufferData(gl.ARRAY_BUFFER, circleDim, gl.STATIC_DRAW);
    }
  }

  /**
   * Re-upload ONLY the per-vertex hide buffers (line + circle) after a drag
   * begin / end. The cheap counterpart to a full rebuild-with-exclusion: the
   * moving features stay in the buffers and the shader collapses the marked
   * verts. Array lengths must match the uploaded geometry's vertex counts.
   */
  updateHide(lineHide: Float32Array, circleHide: Float32Array): void {
    const gl = this.gl;
    if (!gl || this.disabled) return;
    if (this.lineGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGpu.hide);
      gl.bufferData(gl.ARRAY_BUFFER, lineHide, gl.STATIC_DRAW);
    }
    if (this.circleGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.circleGpu.hide);
      gl.bufferData(gl.ARRAY_BUFFER, circleHide, gl.STATIC_DRAW);
    }
  }

  /**
   * Re-upload ONLY the per-vertex highlight buffers (line + circle) after a
   * Move-Segment hover-set change. Sibling of {@link updateDim}: the buffers keep
   * every feature, so this recomputes just the flag arrays (controller) and
   * re-uploads them, no geometry / color rebuild. Array lengths must match the
   * uploaded geometry's vertex counts.
   */
  updateHighlight(lineHighlight: Float32Array, circleHighlight: Float32Array): void {
    const gl = this.gl;
    if (!gl || this.disabled) return;
    if (this.lineGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGpu.highlight);
      gl.bufferData(gl.ARRAY_BUFFER, lineHighlight, gl.STATIC_DRAW);
    }
    if (this.circleGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.circleGpu.highlight);
      gl.bufferData(gl.ARRAY_BUFFER, circleHighlight, gl.STATIC_DRAW);
    }
  }

  /**
   * Re-upload ONLY the per-vertex color buffers (line colorFrom / colorTo,
   * circle fill / stroke) after a GLOBAL recolor that left geometry unchanged
   * (overlay enter/exit, theme change, depth / severity toggle). The cheap
   * color-only fast path: positions, sizes, reference / dim / hide / highlight,
   * and indices stay as built, so this swaps just the four color attributes.
   * Array lengths must match the uploaded geometry's vertex counts (the
   * controller recomputes them in place from the same SoA).
   */
  updateColors(
    lineColorFrom: Uint8Array,
    lineColorTo: Uint8Array,
    circleFill: Uint8Array,
    circleStroke: Uint8Array,
  ): void {
    const gl = this.gl;
    if (!gl || this.disabled) return;
    if (this.lineGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGpu.colorFrom);
      gl.bufferData(gl.ARRAY_BUFFER, lineColorFrom, gl.STATIC_DRAW);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGpu.colorTo);
      gl.bufferData(gl.ARRAY_BUFFER, lineColorTo, gl.STATIC_DRAW);
    }
    if (this.circleGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.circleGpu.fill);
      gl.bufferData(gl.ARRAY_BUFFER, circleFill, gl.STATIC_DRAW);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.circleGpu.stroke);
      gl.bufferData(gl.ARRAY_BUFFER, circleStroke, gl.STATIC_DRAW);
    }
  }

  /** `gl.bufferSubData` a `[vStart, vStart+vCount)` vertex slice of one attribute
   *  from `src` (interleave `elemPerVertex`), reusing the WebGL2 srcOffset/length
   *  overload so no temporary slice is allocated. */
  private subUpload(
    buf: WebGLBuffer,
    src: Float32Array | Uint8Array,
    elemPerVertex: number,
    vStart: number,
    vCount: number,
  ): void {
    const gl = this.gl!;
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferSubData(
      gl.ARRAY_BUFFER,
      vStart * elemPerVertex * src.BYTES_PER_ELEMENT,
      src,
      vStart * elemPerVertex,
      vCount * elemPerVertex,
    );
  }

  /**
   * Re-upload the edit-mutable line attributes (positions + colors + reference)
   * for a `[vStart, vStart+vCount)` vertex range, after the incremental edit path
   * rewrote that range in `this.data.line`. Constant attributes (endpoint/side),
   * the index buffer, and the separately-managed dim/hide buffers are untouched.
   */
  uploadLineVertexRange(vStart: number, vCount: number): void {
    const gl = this.gl;
    if (!gl || this.disabled || !this.lineGpu || !this.data || vCount <= 0) return;
    const l = this.data.line;
    this.subUpload(this.lineGpu.posA, l.posA, 2, vStart, vCount);
    this.subUpload(this.lineGpu.posB, l.posB, 2, vStart, vCount);
    this.subUpload(this.lineGpu.colorFrom, l.colorFrom, 4, vStart, vCount);
    this.subUpload(this.lineGpu.colorTo, l.colorTo, 4, vStart, vCount);
    this.subUpload(this.lineGpu.reference, l.reference, 1, vStart, vCount);
  }

  /** Circle counterpart of {@link uploadLineVertexRange}: re-upload center, fill,
   *  stroke, size multiplier, and all-reference for a vertex range. */
  uploadCircleVertexRange(vStart: number, vCount: number): void {
    const gl = this.gl;
    if (!gl || this.disabled || !this.circleGpu || !this.data || vCount <= 0) return;
    const c = this.data.circle;
    this.subUpload(this.circleGpu.center, c.center, 2, vStart, vCount);
    this.subUpload(this.circleGpu.fill, c.fill, 4, vStart, vCount);
    this.subUpload(this.circleGpu.stroke, c.stroke, 4, vStart, vCount);
    this.subUpload(this.circleGpu.sizeMul, c.sizeMul, 1, vStart, vCount);
    this.subUpload(this.circleGpu.allReference, c.allReference, 1, vStart, vCount);
  }

  /**
   * Re-upload the circle index buffer (same length) after the incremental edit
   * path rewrote its draw order in `this.data.circle.indices` - a kind edit moved
   * a station between ranks (`writeCircleDrawOrder`). Vertex buffers untouched.
   */
  uploadCircleIndices(): void {
    const gl = this.gl;
    if (!gl || this.disabled || !this.circleGpu || !this.data) return;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.circleGpu.index);
    gl.bufferSubData(gl.ELEMENT_ARRAY_BUFFER, 0, this.data.circle.indices);
  }

  render(gl: WebGLRenderingContext | WebGL2RenderingContext, options: CustomRenderMethodInput): void {
    if (this.disabled || !(gl instanceof WebGL2RenderingContext)) return;
    const data = this.data;
    if (!data || !this.lineBundle || !this.circleBundle || !this.lineGpu || !this.circleGpu) return;

    // MapLibre 5.16's options.modelViewProjectionMatrix maps WORLD-PIXEL space
    // (0..worldSize) to clip space; the mercator [0,1] -> clip matrix that a
    // custom layer needs is options.defaultProjectionData.mainMatrix. Feeding
    // the world-pixel matrix here projects the survey millions of pixels
    // off-screen (the observed blank layer, with clean GL state and no errors).
    const mainMatrix = options.defaultProjectionData.mainMatrix as
      | ArrayLike<number>
      | undefined;
    // Guard exotic projections that might omit mainMatrix: skip the frame
    // rather than compose against garbage.
    if (!mainMatrix) return;
    const matrix = composeAnchorMatrix(mainMatrix, data.anchorX, data.anchorY);
    const viewport: [number, number] = [gl.drawingBufferWidth, gl.drawingBufferHeight];

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.CULL_FACE);

    if (this.drawShots) this.drawLines(gl, matrix, viewport, data);
    if (this.drawStations) this.drawCircles(gl, matrix, viewport, data);
    this.checkFrameError(gl, data);
  }

  /**
   * On the first few frames, verify our draws produced no GL error (WebKitGTK
   * can drop a whole layer silently with no console output). Counts SUCCESSFUL
   * frames; after MAX_CHECKED_FRAMES clean frames it stops calling getError()
   * (a synchronous GPU pipeline stall) and trusts the layer. The first clean
   * frame logs ungated; any error self-disables the layer and notifies the
   * controller, which tears it down for good - there is no stock fallback.
   */
  private checkFrameError(gl: WebGL2RenderingContext, data: CustomRenderData): void {
    if (this.checkedFrames >= MAX_CHECKED_FRAMES) return;
    const err = gl.getError();
    if (err !== gl.NO_ERROR) {
      const code = `0x${err.toString(16)}`;
      const reason = `gl-error-${code}`;
      this.disabled = true;
      this.disabledReason = reason;
      // Ungated: this is the exact failure the prototype was blind to before.
      console.warn(`[customRender] GL error ${code} after draw; self-disabling custom layer`);
      this.onSelfDisable?.(reason);
      return;
    }
    this.checkedFrames += 1;
    if (!this.firstFrameLogged) {
      this.firstFrameLogged = true;
      // Ungated one-shot: proof the custom layer actually put pixels on screen.
      console.info('[customRender] first frame drawn OK', {
        shotSegments: data.line.segmentCount,
        stations: data.circle.stationCount,
        bytes: data.line.byteLength + data.circle.byteLength,
      });
    }
  }

  private drawLines(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: CustomRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.lineBundle!;
    const g = this.lineGpu!;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_half_width, data.lineHalfWidthPx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform1f(u.u_hide_reference, this.hideReference ? 1 : 0);
    gl.uniform1f(u.u_hide_jump, this.hideJump ? 1 : 0);
    gl.uniform1f(u.u_hover_width_mul, data.lineHoverWidthMul);
    gl.uniform1f(u.u_select_width_mul, data.lineSelectWidthMul);
    bindVec2(gl, a.a_pos_a, g.posA);
    bindVec2(gl, a.a_pos_b, g.posB);
    bindFloat(gl, a.a_endpoint, g.endpoint);
    bindFloat(gl, a.a_side, g.side);
    bindColor(gl, a.a_color_from, g.colorFrom);
    bindColor(gl, a.a_color_to, g.colorTo);
    bindFloat(gl, a.a_reference, g.reference);
    bindFloat(gl, a.a_dim, g.dim);
    bindFloat(gl, a.a_hide, g.hide);
    bindFloat(gl, a.a_highlight, g.highlight);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.index);
    gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, 0);
  }

  private drawCircles(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: CustomRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.circleBundle!;
    const g = this.circleGpu!;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_radius, data.circleRadiusPx);
    gl.uniform1f(u.u_stroke_width, data.circleStrokePx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform1f(u.u_hide_reference, this.hideReference ? 1 : 0);
    gl.uniform1f(u.u_hide_jump, this.hideJump ? 1 : 0);
    gl.uniform1f(u.u_hover_size_mul, data.circleHoverSizeMul);
    gl.uniform1f(u.u_select_size_mul, data.circleSelectSizeMul);
    bindVec2(gl, a.a_center, g.center);
    bindVec2(gl, a.a_corner, g.corner);
    bindColor(gl, a.a_fill, g.fill);
    bindColor(gl, a.a_stroke, g.stroke);
    bindFloat(gl, a.a_size_mul, g.sizeMul);
    bindFloat(gl, a.a_all_reference, g.allReference);
    bindFloat(gl, a.a_dim, g.dim);
    bindFloat(gl, a.a_hide, g.hide);
    bindFloat(gl, a.a_highlight, g.highlight);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.index);
    gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, 0);
  }

  private freeGpu(): void {
    const gl = this.gl;
    if (!gl) return;
    for (const g of [this.lineGpu, this.circleGpu]) {
      if (!g) continue;
      for (const key of Object.keys(g) as (keyof typeof g)[]) {
        const v = g[key];
        if (v instanceof WebGLBuffer) gl.deleteBuffer(v);
      }
    }
    this.lineGpu = null;
    this.circleGpu = null;
  }

  onRemove(_map: MaplibreMap, _gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    const gl = this.gl;
    this.freeGpu();
    if (gl) {
      deleteProgramBundle(gl, this.lineBundle);
      deleteProgramBundle(gl, this.circleBundle);
    }
    this.lineBundle = null;
    this.circleBundle = null;
    this.gl = null;
  }
}
