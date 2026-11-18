import type { ExpressionSpecification } from 'maplibre-gl';
import { describe, expect, it } from 'vitest';
import { dimmedZoomOpacity } from '../opacityExpressions';

/**
 * Walk an expression and flag any `["zoom"]` reference that is NOT the direct
 * input of a top-level `interpolate`/`step` camera expression. MapLibre rejects
 * such expressions with:
 *   "zoom" expression may only be used as input to a top-level "step" or
 *   "interpolate" expression.
 * so the walker mirrors the exact constraint the runtime enforces.
 */
function hasIllegalZoom(expr: unknown, isTopLevel = true, isCameraInput = false): boolean {
  if (!Array.isArray(expr)) return false;
  const [op, ...rest] = expr;
  if (op === 'zoom') {
    // Legal only in the input slot of a top-level interpolate/step.
    return !isCameraInput;
  }
  if (op === 'interpolate' || op === 'interpolateHCL' || op === 'interpolateLab' || op === 'step') {
    // rest layout after stripping op:
    //   interpolate: [interpolation, input, ...stops]  → input is rest[1]
    //   step:        [input, ...stops]                 → input is rest[0]
    // The input child is a legal camera-input slot ONLY when this
    // interpolate/step is itself top-level (not nested under another
    // expression). Everything below is no longer top-level.
    return rest.some((child, idx) => {
      const inputSlot = op === 'step' ? idx === 0 : idx === 1;
      return hasIllegalZoom(child, false, inputSlot && isTopLevel);
    });
  }
  return rest.some((child) => hasIllegalZoom(child, false, false));
}

describe('dimmedZoomOpacity', () => {
  it('keeps zoom in the top-level MapLibre camera expression', () => {
    const check: ExpressionSpecification = ['==', ['get', 'lineId'], 4];

    expect(dimmedZoomOpacity(check)).toEqual([
      'interpolate', ['linear'], ['zoom'],
      14, ['case', check, 0.25, 0.7],
      18, ['case', check, 0.25, 1],
    ]);
  });

  it('never nests ["zoom"] below a non-camera expression', () => {
    const check: ExpressionSpecification = [
      'in', ['get', 'lineId'], ['literal', [1, 2, 3]],
    ];

    expect(hasIllegalZoom(dimmedZoomOpacity(check))).toBe(false);
  });

  it('detects the illegal shape it guards against (walker self-check)', () => {
    // The pre-fix expression: ['case', check, 0.25, <zoom interpolate>] nests
    // zoom below a `case`, which is exactly what MapLibre rejects.
    const illegal = [
      'case', ['get', 'x'], 0.25,
      ['interpolate', ['linear'], ['zoom'], 14, 0.7, 18, 1],
    ];

    expect(hasIllegalZoom(illegal)).toBe(true);
  });
});
