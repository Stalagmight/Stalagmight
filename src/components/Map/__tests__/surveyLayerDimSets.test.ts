/**
 * Dim-set pushes skip no-op changes (#576 D8). Outside the move tools MapCanvas
 * handed SurveyLayer FRESH empty anchored / immovable sets on every edit, and each
 * push re-ran the custom renderer's dim pass: two O(shots) per-vertex
 * recomputes, four GPU buffer uploads and a label-flag upload per edit, for a
 * dimming that was "nothing" before and after. An unchanged set - or empty to
 * empty - must not reach the renderer; a real change, including back to empty,
 * still must.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';
import { makeFakeSurveyMap } from './surveyLayerFakeMap';
import { FEATURE_ID_STRIDE } from '../../../utils/ids';

interface DimTargets {
  customRender: { setDimming: (s: unknown) => void };
  customLabels: { setDimming: (s: unknown) => void };
}

let layer: SurveyLayer;
let geometryDim: ReturnType<typeof vi.fn>;
let labelDim: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  layer = new SurveyLayer(makeFakeSurveyMap().map);
  const targets = layer as unknown as DimTargets;
  geometryDim = vi.spyOn(targets.customRender, 'setDimming') as unknown as ReturnType<typeof vi.fn>;
  labelDim = vi.spyOn(targets.customLabels, 'setDimming') as unknown as ReturnType<typeof vi.fn>;
});

afterEach(() => {
  layer.release();
  vi.restoreAllMocks();
});

describe('SurveyLayer dim-set pushes', () => {
  it('fresh empty sets on every edit do not re-run the dim pass', () => {
    for (let edit = 0; edit < 3; edit++) {
      layer.setAnchoredSections(new Set());
      layer.setImmovableFeatures(new Set(), new Set());
    }
    expect(geometryDim).not.toHaveBeenCalled();
    expect(labelDim).not.toHaveBeenCalled();
  });

  it('immovable: a change dims, the same set again does not, back to empty clears', () => {
    const stations = new Set([100001]);
    const shots = new Set<number>();
    layer.setImmovableFeatures(stations, shots);
    expect(geometryDim).toHaveBeenCalledTimes(1);
    layer.setImmovableFeatures(stations, shots);
    expect(geometryDim).toHaveBeenCalledTimes(1);
    layer.setImmovableFeatures(new Set(), new Set());
    expect(geometryDim).toHaveBeenCalledTimes(2);
    expect(geometryDim.mock.calls[1]![0]).toMatchObject({ immovableStationFeatureIds: new Set() });
    expect(labelDim).toHaveBeenCalledTimes(2);
  });

  it('anchored: a change dims, the same set again does not, back to empty clears', () => {
    const anchored = new Set([3]);
    layer.setAnchoredSections(anchored);
    layer.setAnchoredSections(anchored);
    expect(geometryDim).toHaveBeenCalledTimes(1);
    layer.setAnchoredSections(new Set());
    expect(geometryDim).toHaveBeenCalledTimes(2);
    expect(geometryDim.mock.calls[1]![0]).toMatchObject({ anchoredSectionIds: new Set() });
  });
});

/**
 * A tiny evaluator for the expression subset the closure dimming uses (case / any /
 * all / in / get / literal), so the test asserts what the paint DOES to a closure
 * rather than how the expression is spelled.
 */
type Expr = unknown;
function evaluate(expr: Expr, props: Record<string, unknown>): unknown {
  if (!Array.isArray(expr)) return expr;
  const [op, ...args] = expr as [string, ...Expr[]];
  switch (op) {
    case 'literal':
      return args[0];
    case 'get':
      return props[args[0] as string];
    case 'in':
      return (evaluate(args[1], props) as unknown[]).includes(evaluate(args[0], props));
    case 'all':
      return args.every((a) => evaluate(a, props) === true);
    case 'any':
      return args.some((a) => evaluate(a, props) === true);
    case 'case': {
      for (let i = 0; i + 1 < args.length; i += 2) {
        if (evaluate(args[i], props) === true) return evaluate(args[i + 1], props);
      }
      return evaluate(args[args.length - 1], props);
    }
    default:
      throw new Error(`unsupported expression op ${op}`);
  }
}

describe('SurveyLayer loop-closure dimming (#576 NF-MAP-4)', () => {
  const CLOSURES = 'survey-loop-closures';
  // Closure from section 1 station 0 (immovable) to section 2 station 3.
  const pinned = { sectionIdA: 1, sectionIdB: 2, stationA: '1:0', stationB: '2:3' };
  // Closure between two anchored sections (3 and 4).
  const anchoredBoth = { sectionIdA: 3, sectionIdB: 4, stationA: '3:1', stationB: '4:2' };
  const free = { sectionIdA: 5, sectionIdB: 6, stationA: '5:1', stationB: '6:2' };

  let fake: ReturnType<typeof makeFakeSurveyMap>;
  let dimLayer: SurveyLayer;
  beforeEach(() => {
    fake = makeFakeSurveyMap();
    dimLayer = new SurveyLayer(fake.map);
  });
  afterEach(() => dimLayer.release());

  const opacityOf = (closure: Record<string, unknown>) => evaluate(fake.paint(CLOSURES, 'line-opacity'), closure);

  it('clearing the anchored set keeps the immovable closures dimmed', () => {
    // Move Segment -> Move Station: the immovable set lands first, the anchored set
    // is cleared a render later. The anchored rule's 0.8 used to erase the
    // immovable rule's dimming, since both wrote the same paint property.
    dimLayer.setAnchoredSections(new Set([3, 4]));
    dimLayer.setImmovableFeatures(new Set([1 * FEATURE_ID_STRIDE + 0]), new Set());
    dimLayer.setAnchoredSections(new Set());
    expect(opacityOf(pinned)).toBe(0.25);
    expect(opacityOf(anchoredBoth)).toBe(0.8);
    expect(opacityOf(free)).toBe(0.8);
  });

  it('both rules apply at once, each to its own closures', () => {
    dimLayer.setImmovableFeatures(new Set([1 * FEATURE_ID_STRIDE + 0]), new Set());
    dimLayer.setAnchoredSections(new Set([3, 4]));
    expect(opacityOf(pinned)).toBe(0.25);
    expect(opacityOf(anchoredBoth)).toBe(0.25);
    expect(opacityOf(free)).toBe(0.8);
    dimLayer.setImmovableFeatures(new Set(), new Set());
    expect(opacityOf(pinned)).toBe(0.8);
    expect(opacityOf(anchoredBoth)).toBe(0.25);
  });

  it('nothing dimmed is the plain 0.8', () => {
    dimLayer.setAnchoredSections(new Set([3]));
    dimLayer.setAnchoredSections(new Set());
    expect(fake.paint(CLOSURES, 'line-opacity')).toBe(0.8);
  });
});
