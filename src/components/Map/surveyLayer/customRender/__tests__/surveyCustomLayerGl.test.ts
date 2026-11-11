import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CustomRenderMethodInput, Map as MaplibreMap } from 'maplibre-gl';
import { SurveyCustomLayer, type CustomRenderData } from '../surveyCustomLayer';
import { buildLineBuffers, buildCircleBuffers, buildStationLookup, projectStationsRel } from '../tessellate';
import { emptySurveyData, type SurveyData } from '../../../../../types/survey-data';
import type { Rgba8 } from '../colorPack';
import { FakeGl, FakeWebGLBuffer, asGl } from './fakeGl';

/** One section, 3 stations, 2 valid shots -> 2 line segments + 3 circles. */
function makeSurvey(): SurveyData {
  const d = emptySurveyData();
  d.sections = [
    {
      id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000',
      direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2,
    },
  ];
  d.shotCount = 2;
  d.shotIds = new Uint32Array([0, 1]);
  d.shotSectionIds = new Uint32Array([1, 1]);
  d.shotFlags = new Uint8Array([0, 0]);
  d.stationCount = 3;
  d.stationSectionIds = new Uint32Array([1, 1, 1]);
  d.stationIndices = new Uint32Array([0, 1, 2]);
  d.stationCoords = new Float64Array([0, 0, 0, 10, 0, -1, 10, 10, -2]);
  d.stationFlags = new Uint8Array([0, 0, 0]);
  return d;
}

/** Real tessellated buffers + the style sizes the controller derives. */
function makeData(): CustomRenderData {
  const d = makeSurvey();
  const lookup = buildStationLookup(d);
  const rel = projectStationsRel(d, null, 0, 0);
  const color = (): Rgba8 => [1, 2, 3, 255];
  return {
    anchorX: 0,
    anchorY: 0,
    line: buildLineBuffers(d, rel, lookup, color),
    circle: buildCircleBuffers(d, rel, color, color),
    lineHalfWidthPx: 1,
    circleRadiusPx: 4,
    circleStrokePx: 1,
    aaPx: 1,
    lineHoverWidthMul: 2,
    lineSelectWidthMul: 1.5,
    circleHoverSizeMul: 1.5,
    circleSelectSizeMul: 1.25,
  };
}

const fakeMap = {} as unknown as MaplibreMap;

/** render() reads only defaultProjectionData.mainMatrix (mercator -> clip). */
const renderOptions = {
  defaultProjectionData: {
    mainMatrix: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1],
  },
} as unknown as CustomRenderMethodInput;

describe('SurveyCustomLayer resolves GL locations once at link time (#339)', () => {
  beforeEach(() => {
    // The layer instanceof-guards both globals; neither exists in node.
    vi.stubGlobal('WebGL2RenderingContext', FakeGl);
    vi.stubGlobal('WebGLBuffer', FakeWebGLBuffer);
    vi.spyOn(console, 'info').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('queries every location in onAdd and none while drawing', () => {
    const gl = new FakeGl();
    const data = makeData();
    // The controller's real sequence: construct with the initial data, addLayer
    // runs onAdd (link + upload), a later sync() calls update().
    const layer = new SurveyCustomLayer('custom-survey', data);
    layer.onAdd(fakeMap, asGl(gl));
    expect(layer.isDisabled()).toBe(false);
    // 8 line + 9 circle uniforms, 10 line + 9 circle attributes.
    expect(gl.count('getUniformLocation')).toBe(17);
    expect(gl.count('getAttribLocation')).toBe(19);

    // A data update re-uploads buffers; the cache belongs to the program, so it
    // must NOT be rebuilt here.
    layer.update(data);
    expect(gl.count('getUniformLocation')).toBe(17);
    expect(gl.count('getAttribLocation')).toBe(19);

    for (let i = 0; i < 3; i++) layer.render(asGl(gl), renderOptions);
    expect(gl.count('getUniformLocation')).toBe(17);
    expect(gl.count('getAttribLocation')).toBe(19);
    // Each frame still draws both programs and binds every attribute.
    expect(gl.count('drawElements')).toBe(6);
    expect(gl.count('useProgram')).toBe(6);
    expect(gl.count('vertexAttribPointer')).toBe(3 * 19);
  });

  // The edit path re-uploads the circle draw order outside render(). The
  // ELEMENT_ARRAY_BUFFER binding is VAO state, so that write must go through
  // COPY_WRITE_BUFFER, or it swaps the index buffer of whatever VAO MapLibre
  // left bound.
  it('re-uploads the circle draw order without touching the element-array binding', () => {
    const gl = new FakeGl();
    const data = makeData();
    const layer = new SurveyCustomLayer('custom-survey', data);
    layer.onAdd(fakeMap, asGl(gl));
    const bindsBefore = gl.bufferBinds.length;
    data.circle.indices.reverse();
    layer.uploadCircleIndices();
    const binds = gl.bufferBinds.slice(bindsBefore);
    expect(binds.some((b) => b.target === gl.ELEMENT_ARRAY_BUFFER)).toBe(false);
    expect(binds.map((b) => b.target)).toEqual([gl.COPY_WRITE_BUFFER, gl.COPY_WRITE_BUFFER]);
    expect(binds[0]!.buffer).toBeInstanceOf(FakeWebGLBuffer);
    expect(binds[1]!.buffer).toBeNull(); // unbound after the write
    expect(gl.subDataCalls).toEqual([{ target: gl.COPY_WRITE_BUFFER, offset: 0, data: data.circle.indices }]);
  });

  it('frees both programs on remove, after which render draws nothing', () => {
    const gl = new FakeGl();
    const layer = new SurveyCustomLayer('custom-survey', makeData());
    layer.onAdd(fakeMap, asGl(gl));
    layer.render(asGl(gl), renderOptions);
    expect(gl.count('drawElements')).toBe(2);

    layer.onRemove(fakeMap, asGl(gl));
    expect(gl.count('deleteProgram')).toBe(2);

    layer.render(asGl(gl), renderOptions);
    expect(gl.count('drawElements')).toBe(2);
  });
});
