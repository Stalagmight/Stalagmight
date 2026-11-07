/**
 * MapLibre `CustomLayerInterface` for the drag-GHOST (#mem drag latency). A small
 * sibling of `surveyCustomLayer.ts` that draws the moving-geometry preview of a
 * move drag directly from typed-array buffers, with the live drag delta carried
 * as ONE screen-pixel uniform (`u_offset_px`) applied in the vertex shader. A
 * pointermove updates that single uniform + `triggerRepaint` - no worker, no
 * `setData`, no per-frame rebuild - which removes the async re-bake overshoot the
 * stock GeoJSON ghost + `line-translate` state machine suffered.
 *
 * Same GL discipline as the bulk layer (WebGL2 / GLSL ES 3.00, premultiplied
 * blend, first-frames `getError()` self-disable). It fails on its own: a
 * compile/link failure at add, or a GL error mid-drag, self-disables JUST the
 * ghost via `onSelfDisable`, and the controller falls back to the stock GeoJSON
 * ghost so the user is never left without a preview.
 */
import type { CustomLayerInterface, CustomRenderMethodInput, Map as MaplibreMap } from 'maplibre-gl';
import type { GhostLineBuffers, GhostCircleBuffers } from './ghostTessellate';
import { composeAnchorMatrix } from './mercator';
import {
  GHOST_LINE_VERTEX_SRC,
  GHOST_LINE_FRAGMENT_SRC,
  GHOST_CIRCLE_VERTEX_SRC,
  GHOST_CIRCLE_FRAGMENT_SRC,
} from './ghostShaders';
import {
  type ProgramBundle,
  linkProgramBundle,
  deleteProgramBundle,
  layerGlError,
  bindVec2,
  bindFloat,
  bindColor,
} from './glProgram';

/** See {@link import('./surveyCustomLayer').SelfDisableCallback}. */
export type GhostSelfDisableCallback = (reason: string) => void;

/** How many clean frames to validate with `gl.getError()` before trusting the
 *  layer (a `getError()` is a pipeline stall). */
const MAX_CHECKED_FRAMES = 5;

/** Everything the ghost layer draws for a drag; set once at begin. */
export interface GhostRenderData {
  anchorX: number;
  anchorY: number;
  line: GhostLineBuffers;
  circle: GhostCircleBuffers;
  /** DEVICE-pixel style sizes (css * dpr). */
  lineHalfWidthPx: number;
  circleRadiusPx: number;
  circleStrokePx: number;
  aaPx: number;
}

interface LineGpu {
  posA: WebGLBuffer;
  posB: WebGLBuffer;
  endpoint: WebGLBuffer;
  side: WebGLBuffer;
  moveA: WebGLBuffer;
  moveB: WebGLBuffer;
  color: WebGLBuffer;
  widthMul: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

interface CircleGpu {
  center: WebGLBuffer;
  corner: WebGLBuffer;
  move: WebGLBuffer;
  fill: WebGLBuffer;
  stroke: WebGLBuffer;
  sizeMul: WebGLBuffer;
  strokeMul: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

/** Uniform / attribute names of the two programs, resolved once at link time
 *  ({@link ProgramBundle}). Each list must match the GLSL declarations in
 *  ghostShaders.ts; `__tests__/programNames.test.ts` pins that. */
export const GHOST_LINE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_half_width',
  'u_aa',
  'u_offset_px',
] as const;

export const GHOST_LINE_ATTRIBS = [
  'a_pos_a',
  'a_pos_b',
  'a_endpoint',
  'a_side',
  'a_move_a',
  'a_move_b',
  'a_color',
  'a_width_mul',
] as const;

export const GHOST_CIRCLE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_radius',
  'u_stroke_width',
  'u_aa',
  'u_offset_px',
] as const;

export const GHOST_CIRCLE_ATTRIBS = [
  'a_center',
  'a_corner',
  'a_move',
  'a_fill',
  'a_stroke',
  'a_size_mul',
  'a_stroke_mul',
] as const;

function makeBuffer(gl: WebGL2RenderingContext, target: number, data: ArrayBufferView): WebGLBuffer {
  const buf = gl.createBuffer()!;
  gl.bindBuffer(target, buf);
  gl.bufferData(target, data, gl.STATIC_DRAW);
  return buf;
}

export class SurveyGhostLayer implements CustomLayerInterface {
  readonly id: string;
  readonly type = 'custom' as const;
  readonly renderingMode = '2d' as const;

  private gl: WebGL2RenderingContext | null = null;
  private lineBundle: ProgramBundle<typeof GHOST_LINE_UNIFORMS, typeof GHOST_LINE_ATTRIBS> | null = null;
  private circleBundle: ProgramBundle<typeof GHOST_CIRCLE_UNIFORMS, typeof GHOST_CIRCLE_ATTRIBS> | null = null;
  private lineGpu: LineGpu | null = null;
  private circleGpu: CircleGpu | null = null;
  private disabled = false;
  private disabledReason: string | null = null;
  private readonly onSelfDisable: GhostSelfDisableCallback | null;
  private checkedFrames = 0;

  private data: GhostRenderData | null = null;
  /** Live drag delta in DEVICE px (css delta * dpr), screen space (+y down). */
  private offsetX = 0;
  private offsetY = 0;

  constructor(id: string, initial: GhostRenderData | null, onSelfDisable: GhostSelfDisableCallback | null = null) {
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

  /** Set the live drag delta (DEVICE px). Cheap: one uniform each frame, no
   *  buffer touch. The caller triggers the repaint. */
  setOffset(dxDevicePx: number, dyDevicePx: number): void {
    this.offsetX = dxDevicePx;
    this.offsetY = dyDevicePx;
  }

  onAdd(_map: MaplibreMap, gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    if (typeof WebGL2RenderingContext === 'undefined' || !(gl instanceof WebGL2RenderingContext)) {
      console.warn('[dragGhost] WebGL2 unavailable; disabling ghost layer');
      this.disabled = true;
      this.disabledReason = 'webgl2-unavailable';
      return;
    }
    this.gl = gl;
    this.disabled = false;
    this.disabledReason = null;
    // A re-add after a WebGL context restore runs here on the NEW context: re-arm
    // the first-frames GL error check, whose budget the lost context spent (#576 D38).
    this.checkedFrames = 0;
    // linkProgramBundle() already logs getShaderInfoLog / getProgramInfoLog ungated.
    this.lineBundle = linkProgramBundle(
      gl, GHOST_LINE_VERTEX_SRC, GHOST_LINE_FRAGMENT_SRC, GHOST_LINE_UNIFORMS, GHOST_LINE_ATTRIBS, '[dragGhost]',
    );
    this.circleBundle = linkProgramBundle(
      gl, GHOST_CIRCLE_VERTEX_SRC, GHOST_CIRCLE_FRAGMENT_SRC, GHOST_CIRCLE_UNIFORMS, GHOST_CIRCLE_ATTRIBS, '[dragGhost]',
    );
    if (!this.lineBundle || !this.circleBundle) {
      console.warn('[dragGhost] shader program link failed; disabling ghost layer');
      this.disabled = true;
      this.disabledReason = 'program-link-failed';
      return;
    }
    if (this.data) this.uploadBuffers(this.data);
  }

  private uploadBuffers(data: GhostRenderData): void {
    const gl = this.gl;
    if (!gl) return;
    this.freeGpu();
    const l = data.line;
    this.lineGpu = {
      posA: makeBuffer(gl, gl.ARRAY_BUFFER, l.posA),
      posB: makeBuffer(gl, gl.ARRAY_BUFFER, l.posB),
      endpoint: makeBuffer(gl, gl.ARRAY_BUFFER, l.endpoint),
      side: makeBuffer(gl, gl.ARRAY_BUFFER, l.side),
      moveA: makeBuffer(gl, gl.ARRAY_BUFFER, l.moveA),
      moveB: makeBuffer(gl, gl.ARRAY_BUFFER, l.moveB),
      color: makeBuffer(gl, gl.ARRAY_BUFFER, l.color),
      widthMul: makeBuffer(gl, gl.ARRAY_BUFFER, l.widthMul),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, l.indices),
      count: l.indices.length,
    };
    const c = data.circle;
    this.circleGpu = {
      center: makeBuffer(gl, gl.ARRAY_BUFFER, c.center),
      corner: makeBuffer(gl, gl.ARRAY_BUFFER, c.corner),
      move: makeBuffer(gl, gl.ARRAY_BUFFER, c.move),
      fill: makeBuffer(gl, gl.ARRAY_BUFFER, c.fill),
      stroke: makeBuffer(gl, gl.ARRAY_BUFFER, c.stroke),
      sizeMul: makeBuffer(gl, gl.ARRAY_BUFFER, c.sizeMul),
      strokeMul: makeBuffer(gl, gl.ARRAY_BUFFER, c.strokeMul),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, c.indices),
      count: c.indices.length,
    };
  }

  render(gl: WebGLRenderingContext | WebGL2RenderingContext, options: CustomRenderMethodInput): void {
    if (this.disabled || !(gl instanceof WebGL2RenderingContext)) return;
    const data = this.data;
    if (!data || !this.lineBundle || !this.circleBundle || !this.lineGpu || !this.circleGpu) return;
    const mainMatrix = options.defaultProjectionData.mainMatrix as ArrayLike<number> | undefined;
    if (!mainMatrix) return;
    const matrix = composeAnchorMatrix(mainMatrix, data.anchorX, data.anchorY);
    const viewport: [number, number] = [gl.drawingBufferWidth, gl.drawingBufferHeight];

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.CULL_FACE);

    this.drawLines(gl, matrix, viewport, data);
    this.drawCircles(gl, matrix, viewport, data);
    this.checkFrameError(gl);
  }

  private checkFrameError(gl: WebGL2RenderingContext): void {
    if (this.checkedFrames >= MAX_CHECKED_FRAMES) return;
    const err = layerGlError(gl);
    if (err !== gl.NO_ERROR) {
      const reason = `gl-error-0x${err.toString(16)}`;
      this.disabled = true;
      this.disabledReason = reason;
      console.warn(`[dragGhost] GL error 0x${err.toString(16)} after draw; self-disabling ghost layer`);
      this.onSelfDisable?.(reason);
      return;
    }
    this.checkedFrames += 1;
  }

  private drawLines(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: GhostRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.lineBundle!;
    const g = this.lineGpu!;
    if (g.count === 0) return;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_half_width, data.lineHalfWidthPx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform2f(u.u_offset_px, this.offsetX, this.offsetY);
    bindVec2(gl, a.a_pos_a, g.posA);
    bindVec2(gl, a.a_pos_b, g.posB);
    bindFloat(gl, a.a_endpoint, g.endpoint);
    bindFloat(gl, a.a_side, g.side);
    bindFloat(gl, a.a_move_a, g.moveA);
    bindFloat(gl, a.a_move_b, g.moveB);
    bindColor(gl, a.a_color, g.color);
    bindFloat(gl, a.a_width_mul, g.widthMul);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.index);
    gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, 0);
  }

  private drawCircles(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: GhostRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.circleBundle!;
    const g = this.circleGpu!;
    if (g.count === 0) return;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_radius, data.circleRadiusPx);
    gl.uniform1f(u.u_stroke_width, data.circleStrokePx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform2f(u.u_offset_px, this.offsetX, this.offsetY);
    bindVec2(gl, a.a_center, g.center);
    bindVec2(gl, a.a_corner, g.corner);
    bindFloat(gl, a.a_move, g.move);
    bindColor(gl, a.a_fill, g.fill);
    bindColor(gl, a.a_stroke, g.stroke);
    bindFloat(gl, a.a_size_mul, g.sizeMul);
    bindFloat(gl, a.a_stroke_mul, g.strokeMul);
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
