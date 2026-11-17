/**
 * MapLibre expression builder utilities for priority-based styling.
 *
 * These pure functions build MapLibre GL JS expressions as nested arrays.
 * They can be easily unit tested since expressions are just data structures.
 *
 * @see https://maplibre.org/maplibre-style-spec/expressions/
 */

import type { ExpressionSpecification } from 'maplibre-gl';

/**
 * A priority level for building case expressions.
 * Higher priority conditions (earlier in array) override lower ones.
 */
export interface PriorityLevel {
  /** MapLibre boolean expression that evaluates the condition */
  condition: ExpressionSpecification;
  /** Value to return when condition is true */
  value: number | string;
}

/**
 * Builds a MapLibre case expression with priority-ordered conditions.
 * Higher priority conditions (earlier in array) override lower ones.
 *
 * @example
 * const strokeWidth = buildPriorityExpression([
 *   { condition: compositeIdEquals(['lineId', 'stationIndex'], '1:5'), value: 4 },
 *   { condition: inLine('1'), value: 3 },
 * ], 1);
 * // Returns: ['case', ['==', ...], 4, ['==', ...], 3, 1]
 */
export function buildPriorityExpression(
  priorities: PriorityLevel[],
  fallback: number | string
): ExpressionSpecification {
  if (priorities.length === 0) {
    // Return literal expression for empty priorities
    return ['literal', fallback] as unknown as ExpressionSpecification;
  }

  const cases: unknown[] = ['case'];
  for (const { condition, value } of priorities) {
    cases.push(condition, value);
  }
  cases.push(fallback);

  return cases as unknown as ExpressionSpecification;
}

/**
 * Creates an equality condition for composite ID matching.
 * Used for station/shot IDs in format "lineId:index".
 *
 * CRITICAL: Feature properties store lineId as NUMBER.
 * This function converts both parts to strings before concatenating
 * to ensure reliable comparison.
 *
 * @param parts - Tuple of [lineIdProp, indexProp] property names
 * @param value - The composite ID to match (e.g., "1:5") or null for "no match"
 *
 * @example
 * compositeIdEquals(['lineId', 'stationIndex'], '1:5')
 * // Returns: ['==', ['concat', ['to-string', ['get', 'lineId']], ':', ['to-string', ['get', 'stationIndex']]], '1:5']
 *
 * compositeIdEquals(['lineId', 'stationIndex'], null)
 * // Returns: false (constant false expression)
 */
export function compositeIdEquals(
  parts: [string, string],
  value: string | null
): ExpressionSpecification {
  // Return constant false when value is null to guarantee "no match"
  if (value === null) {
    return false as unknown as ExpressionSpecification;
  }

  const idExpr = [
    'concat',
    ['to-string', ['get', parts[0]]],
    ':',
    ['to-string', ['get', parts[1]]],
  ];
  return ['==', idExpr, value] as unknown as ExpressionSpecification;
}

/**
 * Creates a line membership condition.
 *
 * CRITICAL: lineId is stored as NUMBER in feature properties.
 * This function converts it to string for comparison to avoid
 * type mismatch issues in MapLibre expressions.
 *
 * @param lineId - Line ID as string (will be compared to stringified feature property), or null for "no match"
 *
 * @example
 * inLine('1')
 * // Returns: ['==', ['to-string', ['get', 'lineId']], '1']
 *
 * inLine(null)
 * // Returns: false (constant false expression)
 */
export function inLine(lineId: string | null): ExpressionSpecification {
  // Return constant false when lineId is null to guarantee "no match"
  if (lineId === null) {
    return false as unknown as ExpressionSpecification;
  }

  // Features store lineId as number - convert to string for comparison
  return ['==', ['to-string', ['get', 'lineId']], lineId] as unknown as ExpressionSpecification;
}
