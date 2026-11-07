/**
 * MapLibre `CustomLayerInterface` for the global view's bulk survey geometry
 * (#customRender). Sibling of `Map/surveyLayer/customRender/surveyCustomLayer.ts`
 * with the same GL discipline (binds its own program/buffers/attributes/uniforms
 * every frame, sets premultiplied-alpha blend, re-uploads in onAdd - which
 * GlobalSurveyLayer re-runs after a WebGL context restore, as MapLibre does not -
 * self-disables + notifies on a first-frames `gl.getError()` probe), tailored to
 * the global view: every system drawn at once in its own color, with per-system
 * hover (a uniform), selection / search-dim (per-vertex flags), and reference
 * handling baked in.
 *
 * WebGL2 only (GLSL ES 3.00 + UNSIGNED_INT indices). MapLibre 5.16 provides a
 * WebGL2 context; a WebGL1 context self-disables the layer with a warning so the
 * controller can keep the stock global layers visible.
 */
import type { CustomLayerInterface, CustomRenderMethodInput, Map as MaplibreMap } from 'maplibre-gl';
import type { GlobalLineBuffers, GlobalCircleBuffers } from './globalTessellate';
import { composeAnchorMatrix } from '../../Map/surveyLayer/customRender/mercator';
import {
  GLOBAL_LINE_VERTEX_SRC,
  GLOBAL_LINE_FRAGMENT_SRC,
  GLOBAL_CIRCLE_VERTEX_SRC,
  GLOBAL_CIRCLE_FRAGMENT_SRC,
} from './globalShaders';
import {
  type ProgramBundle,
  linkProgramBundle,
  deleteProgramBundle,
  bindVec2,
  bindFloat,
  bindColor,
  layerGlError,
} from '../../Map/surveyLayer/customRender/glProgram';

/** Notified when the layer disables itself mid-render; fired from inside
 *  MapLibre's render pass, so the controller must defer any style mutation. */
export type SelfDisableCallback = (reason: string) => void;

/** How many consecutive clean frames to validate with `gl.getError()` before
 *  trusting the layer and dropping the per-frame sync stall. */
const MAX_CHECKED_FRAMES = 5;

/** Normalized 0..1 RGBA (the station stroke color uniform). */
export type Rgba01 = readonly [number, number, number, number];

/** Everything the layer needs to draw one frame; swapped wholesale by update(). */
export interface GlobalCustomRenderData {
  anchorX: number;
  anchorY: number;
  line: GlobalLineBuffers;
  circle: GlobalCircleBuffers;
  /** DEVICE-pixel style sizes. */
  lineHalfWidthPx: number;
  circleRadiusPx: number;
  circleStrokePx: number;
  aaPx: number;
  /** Normalized station stroke color (theme strokeColor). */
  strokeColor: Rgba01;
}

interface LineGpu {
  posA: WebGLBuffer;
  posB: WebGLBuffer;
  endpoint: WebGLBuffer;
  side: WebGLBuffer;
  color: WebGLBuffer;
  reference: WebGLBuffer;
  dim: WebGLBuffer;
  system: WebGLBuffer;
  selected: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

interface CircleGpu {
  center: WebGLBuffer;
  corner: WebGLBuffer;
  fill: WebGLBuffer;
  allReference: WebGLBuffer;
  dim: WebGLBuffer;
  system: WebGLBuffer;
  selected: WebGLBuffer;
  index: WebGLBuffer;
  count: number;
}

/** Uniform / attribute names of the two programs, resolved once at link time
 *  ({@link ProgramBundle}). Each list must match the GLSL declarations in
 *  globalShaders.ts; `__tests__/globalProgramNames.test.ts` pins that. */
export const GLOBAL_LINE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_half_width',
  'u_aa',
  'u_hide_reference',
  'u_hover_system',
] as const;

export const GLOBAL_LINE_ATTRIBS = [
  'a_pos_a',
  'a_pos_b',
  'a_endpoint',
  'a_side',
  'a_color',
  'a_reference',
  'a_dim',
  'a_system',
  'a_selected',
] as const;

export const GLOBAL_CIRCLE_UNIFORMS = [
  'u_matrix',
  'u_viewport',
  'u_radius',
  'u_stroke_width',
  'u_aa',
  'u_hide_reference',
  'u_hover_system',
  'u_stroke_color',
] as const;

export const GLOBAL_CIRCLE_ATTRIBS = [
  'a_center',
  'a_corner',
  'a_fill',
  'a_all_reference',
  'a_dim',
  'a_system',
  'a_selected',
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

export class GlobalCustomLayer implements CustomLayerInterface {
  readonly id: string;
  readonly type = 'custom' as const;
  readonly renderingMode = '2d' as const;

  private gl: WebGL2RenderingContext | null = null;
  private lineBundle: ProgramBundle<typeof GLOBAL_LINE_UNIFORMS, typeof GLOBAL_LINE_ATTRIBS> | null = null;
  private circleBundle: ProgramBundle<
    typeof GLOBAL_CIRCLE_UNIFORMS,
    typeof GLOBAL_CIRCLE_ATTRIBS
  > | null = null;
  private lineGpu: LineGpu | null = null;
  private circleGpu: CircleGpu | null = null;
  private disabled = false;
  private disabledReason: string | null = null;
  private readonly onSelfDisable: SelfDisableCallback | null;
  private checkedFrames = 0;
  private firstFrameLogged = false;

  private data: GlobalCustomRenderData | null = null;
  /** Whole-layer draw toggle (markers-only view hides the survey). */
  private visible = true;
  /** Hide reference shots + all-reference stations (shader uniform). */
  private hideReference = false;
  /** Hovered system index, or -1 (a cursor-follow uniform, no re-upload). */
  private hoverSystem = -1;

  constructor(
    id: string,
    initial: GlobalCustomRenderData | null,
    onSelfDisable: SelfDisableCallback | null = null,
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

  /** Show/hide the whole custom survey (markers-only view). */
  setVisible(visible: boolean): void {
    this.visible = visible;
  }

  /** Hide/show reference shots + all-reference stations. */
  setReferenceShotsVisible(visible: boolean): void {
    this.hideReference = !visible;
  }

  /** Set the hovered system's index (-1 = none). Uniform-only, no re-upload. */
  setHoverSystem(index: number): void {
    this.hoverSystem = index;
  }

  onAdd(_map: MaplibreMap, gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    if (typeof WebGL2RenderingContext === 'undefined' || !(gl instanceof WebGL2RenderingContext)) {
      console.warn('[globalCustomRender] WebGL2 unavailable; disabling custom layer');
      this.disabled = true;
      this.disabledReason = 'webgl2-unavailable';
      return;
    }
    this.gl = gl;
    this.disabled = false;
    this.disabledReason = null;
    // linkProgramBundle() already logs getShaderInfoLog / getProgramInfoLog ungated.
    this.lineBundle = linkProgramBundle(
      gl, GLOBAL_LINE_VERTEX_SRC, GLOBAL_LINE_FRAGMENT_SRC,
      GLOBAL_LINE_UNIFORMS, GLOBAL_LINE_ATTRIBS, '[globalCustomRender]',
    );
    this.circleBundle = linkProgramBundle(
      gl, GLOBAL_CIRCLE_VERTEX_SRC, GLOBAL_CIRCLE_FRAGMENT_SRC,
      GLOBAL_CIRCLE_UNIFORMS, GLOBAL_CIRCLE_ATTRIBS, '[globalCustomRender]',
    );
    if (!this.lineBundle || !this.circleBundle) {
      console.warn('[globalCustomRender] shader program link failed; disabling custom layer');
      this.disabled = true;
      this.disabledReason = 'program-link-failed';
      return;
    }
    if (this.data) this.uploadBuffers(this.data);
  }

  /** Replace geometry/style; re-uploads GPU buffers if the context is live. */
  update(data: GlobalCustomRenderData): void {
    this.data = data;
    if (this.gl && !this.disabled) this.uploadBuffers(data);
  }

  private uploadBuffers(data: GlobalCustomRenderData): void {
    const gl = this.gl;
    if (!gl) return;
    this.freeGpu();
    const l = data.line;
    this.lineGpu = {
      posA: makeBuffer(gl, gl.ARRAY_BUFFER, l.posA),
      posB: makeBuffer(gl, gl.ARRAY_BUFFER, l.posB),
      endpoint: makeBuffer(gl, gl.ARRAY_BUFFER, l.endpoint),
      side: makeBuffer(gl, gl.ARRAY_BUFFER, l.side),
      color: makeBuffer(gl, gl.ARRAY_BUFFER, l.color),
      reference: makeBuffer(gl, gl.ARRAY_BUFFER, l.reference),
      dim: makeBuffer(gl, gl.ARRAY_BUFFER, l.dim),
      system: makeBuffer(gl, gl.ARRAY_BUFFER, l.system),
      selected: makeBuffer(gl, gl.ARRAY_BUFFER, l.selected),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, l.indices),
      count: l.indices.length,
    };
    const c = data.circle;
    this.circleGpu = {
      center: makeBuffer(gl, gl.ARRAY_BUFFER, c.center),
      corner: makeBuffer(gl, gl.ARRAY_BUFFER, c.corner),
      fill: makeBuffer(gl, gl.ARRAY_BUFFER, c.fill),
      allReference: makeBuffer(gl, gl.ARRAY_BUFFER, c.allReference),
      dim: makeBuffer(gl, gl.ARRAY_BUFFER, c.dim),
      system: makeBuffer(gl, gl.ARRAY_BUFFER, c.system),
      selected: makeBuffer(gl, gl.ARRAY_BUFFER, c.selected),
      index: makeBuffer(gl, gl.ELEMENT_ARRAY_BUFFER, c.indices),
      count: c.indices.length,
    };
  }

  /** Re-upload ONLY the per-vertex dim buffers (search-filter change). Array
   *  lengths must match the uploaded geometry's vertex counts. */
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

  /** Re-upload ONLY the per-vertex selected buffers (selection change). */
  updateSelected(lineSel: Float32Array, circleSel: Float32Array): void {
    const gl = this.gl;
    if (!gl || this.disabled) return;
    if (this.lineGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGpu.selected);
      gl.bufferData(gl.ARRAY_BUFFER, lineSel, gl.STATIC_DRAW);
    }
    if (this.circleGpu) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.circleGpu.selected);
      gl.bufferData(gl.ARRAY_BUFFER, circleSel, gl.STATIC_DRAW);
    }
  }

  render(gl: WebGLRenderingContext | WebGL2RenderingContext, options: CustomRenderMethodInput): void {
    if (this.disabled || !this.visible || !(gl instanceof WebGL2RenderingContext)) return;
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
    this.checkFrameError(gl, data);
  }

  private checkFrameError(gl: WebGL2RenderingContext, data: GlobalCustomRenderData): void {
    if (this.checkedFrames >= MAX_CHECKED_FRAMES) return;
    // A frame drawn as the context goes reports CONTEXT_LOST_WEBGL: not a fault
    // of this layer, which comes back with the context (#576 D38).
    const err = layerGlError(gl);
    if (err !== gl.NO_ERROR) {
      const code = `0x${err.toString(16)}`;
      const reason = `gl-error-${code}`;
      this.disabled = true;
      this.disabledReason = reason;
      console.warn(`[globalCustomRender] GL error ${code} after draw; self-disabling custom layer`);
      this.onSelfDisable?.(reason);
      return;
    }
    this.checkedFrames += 1;
    if (!this.firstFrameLogged) {
      this.firstFrameLogged = true;
      console.info('[globalCustomRender] first frame drawn OK', {
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
    data: GlobalCustomRenderData,
  ): void {
    const { program, uniforms: u, attribs: a } = this.lineBundle!;
    const g = this.lineGpu!;
    gl.useProgram(program);
    gl.uniformMatrix4fv(u.u_matrix, false, matrix);
    gl.uniform2f(u.u_viewport, viewport[0], viewport[1]);
    gl.uniform1f(u.u_half_width, data.lineHalfWidthPx);
    gl.uniform1f(u.u_aa, data.aaPx);
    gl.uniform1f(u.u_hide_reference, this.hideReference ? 1 : 0);
    gl.uniform1f(u.u_hover_system, this.hoverSystem);
    bindVec2(gl, a.a_pos_a, g.posA);
    bindVec2(gl, a.a_pos_b, g.posB);
    bindFloat(gl, a.a_endpoint, g.endpoint);
    bindFloat(gl, a.a_side, g.side);
    bindColor(gl, a.a_color, g.color);
    bindFloat(gl, a.a_reference, g.reference);
    bindFloat(gl, a.a_dim, g.dim);
    bindFloat(gl, a.a_system, g.system);
    bindFloat(gl, a.a_selected, g.selected);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.index);
    gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, 0);
  }

  private drawCircles(
    gl: WebGL2RenderingContext,
    matrix: Float32Array,
    viewport: [number, number],
    data: GlobalCustomRenderData,
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
    gl.uniform1f(u.u_hover_system, this.hoverSystem);
    const sc = data.strokeColor;
    gl.uniform4f(u.u_stroke_color, sc[0], sc[1], sc[2], sc[3]);
    bindVec2(gl, a.a_center, g.center);
    bindVec2(gl, a.a_corner, g.corner);
    bindColor(gl, a.a_fill, g.fill);
    bindFloat(gl, a.a_all_reference, g.allReference);
    bindFloat(gl, a.a_dim, g.dim);
    bindFloat(gl, a.a_system, g.system);
    bindFloat(gl, a.a_selected, g.selected);
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
