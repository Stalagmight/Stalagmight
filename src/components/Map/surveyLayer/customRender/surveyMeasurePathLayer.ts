/**
 * MapLibre `CustomLayerInterface` for the dedicated MEASURE-PATH tint (#26,
 * #customRender stock deletion). A small sibling of `surveyCustomLayer.ts` /
 * `surveyGhostLayer.ts` that draws the measured along-cave-line path directly
 * from typed-array buffers: the measured shots as round-cap tint lines, the
 * measured stations as filled tint discs, both in ONE tint carried as a
 * `u_color` uniform.
 *
 * Same GL discipline as its siblings (WebGL2 / GLSL ES 3.00, premultiplied
 * blend, first-frames `getError()` self-disable). It fails on its own: a
 * compile/link failure at add, or a GL error mid-render, self-disables JUST this
 * layer via `onSelfDisable`. There is no stock `survey-measure-path` fallback
 * anymore, so the controller drops the dead layer and surfaces the failure
 * (`onFatal`) rather than degrading silently.
 */
import type { CustomLayerInterface, CustomRenderMethodInput, Map as MaplibreMap } from 'maplibre-gl';
import type { MeasureLineBuffers, MeasureCircleBuffers } from './measurePathTessellate';
import { composeAnchorMatrix } from './mercator';
import {
  MEASURE_LINE_VERTEX_SRC,
  MEASURE_LINE_FRAGMENT_SRC,
  MEASURE_CIRCLE_VERTEX_SRC,
  MEASURE_CIRCLE_FRAGMENT_SRC,
} from './measurePathShaders';
import {
  type ProgramBundle,
  linkProgramBundle,
  deleteProgramBundle,
  layerGlError,
  bindVec2,
  bindFloat,
} from './glProgram';

/** See {@link import('./surveyCustomLayer').SelfDisableCallback}. */
export type MeasureSelfDisableCallback = (reason: string) => void;

/** How many clean frames to validate with `gl.getError()` before trusting the
 *  layer (a `getError()` is a pipeline stall). */
const MAX_CHECKED_FRAMES = 5;

/** Everything the measure-path layer draws; swapped wholesale by `setData`. */
export interface MeasurePathRenderData {
  anchorX: number;
  anchorY: number;
  line: MeasureLineBuffers;
  circle: MeasureCircleBuffers;
  /** DEVICE-pixel style sizes (css * dpr). */
  lineHalfWidthPx: number;
  circleRadiusPx: number;
  aaPx: number;
  /** Tint RGBA, each channel 0..1 (straight alpha; premultiplied in the frag). */
  color: readonly [number, number, number, number];
}

interface LineGpu {
  posA: WebGLBuffer;
  posB: WebGLBuffer;
  endpoint: WebGLBuffer;
  side: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

interface CircleGpu {
  center: WebGLBuffer;
  corner: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

/** Uniform / attribute names of the two programs, resolved once at link time
 *  ({@link ProgramBundle}). Each list must match the GLSL declarations in
 *  measurePathShaders.ts; `__tests__/programNames.test.ts` pins that. */
export const MEASURE_LINE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_half_width',
  'u_aa',
  'u_color',
] as const;

export const MEASURE_LINE_ATTRIBS = ['a_pos_a', 'a_pos_b', 'a_endpoint', 'a_side'] as const;

export const MEASURE_CIRCLE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_radius',
  'u_aa',
  'u_color',
] as const;

export const MEASURE_CIRCLE_ATTRIBS = ['a_center', 'a_corner'] as const;

function makeBuffer(gl: WebGL2RenderingContext, target: number, data: ArrayBufferView): WebGLBuffer {
  const buf = gl.createBuffer()!;
  gl.bindBuffer(target, buf);
  gl.bufferData(target, data, gl.STATIC_DRAW);
  return buf;
}

export class SurveyMeasurePathLayer implements CustomLayerInterface {
  readonly id: string;
  readonly type = 'custom' as const;
  readonly renderingMode = '2d' as const;

  private gl: WebGL2RenderingContext | null = null;
  private lineBundle: ProgramBundle<typeof MEASURE_LINE_UNIFORMS, typeof MEASURE_LINE_ATTRIBS> | null = null;
  private circleBundle: ProgramBundle<
    typeof MEASURE_CIRCLE_UNIFORMS,
    typeof MEASURE_CIRCLE_ATTRIBS
  > | null = null;
  private lineGpu: LineGpu | null = null;
  private circleGpu: CircleGpu | null = null;
  private disabled = false;
  private disabledReason: string | null = null;
  private readonly onSelfDisable: MeasureSelfDisableCallback | null;
  private checkedFrames = 0;

  private data: MeasurePathRenderData | null = null;

  constructor(
    id: string,
    initial: MeasurePathRenderData | null,
    onSelfDisable: MeasureSelfDisableCallback | null = null,
  ) {
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

  /** Swap the drawn path (re-upload buffers). The caller triggers the repaint. */
  setData(data: MeasurePathRenderData): void {
    this.data = data;
    if (this.gl && !this.disabled) this.uploadBuffers(data);
  }

  onAdd(_map: MaplibreMap, gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    if (typeof WebGL2RenderingContext === 'undefined' || !(gl instanceof WebGL2RenderingContext)) {
      console.warn('[measurePath] WebGL2 unavailable; disabling measure-path layer');
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
      gl, MEASURE_LINE_VERTEX_SRC, MEASURE_LINE_FRAGMENT_SRC,
      MEASURE_LINE_UNIFORMS, MEASURE_LINE_ATTRIBS, '[measurePath]',
    );
    this.circleBundle = linkProgramBundle(
      gl, MEASURE_CIRCLE_VERTEX_SRC, MEASURE_CIRCLE_FRAGMENT_SRC,
      MEASURE_CIRCLE_UNIFORMS, MEASURE_CIRCLE_ATTRIBS, '[measurePath]',
    );
    if (!this.lineBundle || !this.circleBundle) {
      console.warn('[measurePath] shader program link failed; disabling measure-path layer');
      this.disabled = true;
      this.disabledReason = 'program-link-failed';
      return;
    }
    if (this.data) this.uploadBuffers(this.data);
  }

  private uploadBuffers(data: MeasurePathRenderData): void {
    const gl = this.gl;
    if (!gl) return;
    this.freeGpu();
    const l = data.line;
    this.lineGpu = {
      posA: makeBuffer(gl, gl.ARRAY_BUFFER, l.posA),
      posB: makeBuffer(gl, gl.ARRAY_BUFFER, l.posB),
      endpoint: makeBuffer(gl, gl.ARRAY_BUFFER, l.endpoint),
      side: makeBuffer(gl, gl.ARRAY_BUFFER, l.side),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, l.indices),
      count: l.indices.length,
    };
    const c = data.circle;
    this.circleGpu = {
      center: makeBuffer(gl, gl.ARRAY_BUFFER, c.center),
      corner: makeBuffer(gl, gl.ARRAY_BUFFER, c.corner),
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

    // Shots (capsules) first, stations (discs) on top - the discs cover the path
    // junctions so a measured joint reads as a clean dot, mirroring the bulk
    // station-over-shot stacking.
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
      console.warn(`[measurePath] GL error 0x${err.toString(16)} after draw; self-disabling measure-path layer`);
      this.onSelfDisable?.(reason);
      return;
    }
    this.checkedFrames += 1;
  }

  private drawLines(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: MeasurePathRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.lineBundle!;
    const g = this.lineGpu!;
    if (g.count === 0) return;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_half_width, data.lineHalfWidthPx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform4f(u.u_color, data.color[0], data.color[1], data.color[2], data.color[3]);
    bindVec2(gl, a.a_pos_a, g.posA);
    bindVec2(gl, a.a_pos_b, g.posB);
    bindFloat(gl, a.a_endpoint, g.endpoint);
    bindFloat(gl, a.a_side, g.side);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.index);
    gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, 0);
  }

  private drawCircles(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: MeasurePathRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.circleBundle!;
    const g = this.circleGpu!;
    if (g.count === 0) return;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_radius, data.circleRadiusPx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform4f(u.u_color, data.color[0], data.color[1], data.color[2], data.color[3]);
    bindVec2(gl, a.a_center, g.center);
    bindVec2(gl, a.a_corner, g.corner);
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
