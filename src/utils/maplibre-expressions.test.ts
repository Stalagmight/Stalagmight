/**
 * Unit tests for MapLibre expression builder utilities.
 * Tests verify plain array structure - expressions are just nested arrays,
 * no MapLibre runtime needed.
 */

import { describe, it, expect, vi } from 'vitest';
import type { ExpressionSpecification } from 'maplibre-gl';
import { buildPriorityExpression, compositeIdEquals, inLine } from './maplibre-expressions';

describe('buildPriorityExpression', () => {
  it('returns literal expression when priorities array is empty', () => {
    const result = buildPriorityExpression([], 1);
    expect(result).toEqual(['literal', 1]);
  });

  it('returns literal expression with string fallback when empty', () => {
    const result = buildPriorityExpression([], '#000000');
    expect(result).toEqual(['literal', '#000000']);
  });

  it('builds correct case expression with single priority', () => {
    const result = buildPriorityExpression(
      [{ condition: ['==', ['get', 'id'], 'abc'] as ExpressionSpecification, value: 3 }],
      1
    );

    expect(result).toEqual(['case', ['==', ['get', 'id'], 'abc'], 3, 1]);
  });

  it('maintains priority order (first condition wins)', () => {
    const result = buildPriorityExpression(
      [
        { condition: ['==', ['get', 'id'], 'a'] as ExpressionSpecification, value: 4 },
        { condition: ['==', ['get', 'id'], 'b'] as ExpressionSpecification, value: 3 },
        { condition: ['==', ['get', 'id'], 'c'] as ExpressionSpecification, value: 2 },
      ],
      1
    );

    // Verify array structure matches MapLibre case syntax
    expect(result[0]).toBe('case');
    // First condition and value (highest priority)
    expect(result[1]).toEqual(['==', ['get', 'id'], 'a']);
    expect(result[2]).toBe(4);
    // Second condition and value
    expect(result[3]).toEqual(['==', ['get', 'id'], 'b']);
    expect(result[4]).toBe(3);
    // Third condition and value
    expect(result[5]).toEqual(['==', ['get', 'id'], 'c']);
    expect(result[6]).toBe(2);
    // Fallback at the end
    expect(result[7]).toBe(1);
  });

  it('handles string fallback values', () => {
    const result = buildPriorityExpression(
      [{ condition: ['boolean', true] as unknown as ExpressionSpecification, value: '#ff0000' }],
      '#000000'
    );

    // Fallback should be last element
    expect(result[result.length - 1]).toBe('#000000');
  });

  it('handles string values in priorities', () => {
    const result = buildPriorityExpression(
      [{ condition: ['==', ['get', 'id'], 'a'] as ExpressionSpecification, value: '#ff0000' }],
      '#000000'
    );

    expect(result).toEqual(['case', ['==', ['get', 'id'], 'a'], '#ff0000', '#000000']);
  });
});

describe('compositeIdEquals', () => {
  it('creates concat + equality expression', () => {
    const result = compositeIdEquals(['lineId', 'stationIndex'], '123:0');

    expect(result).toEqual([
      '==',
      [
        'concat',
        ['to-string', ['get', 'lineId']],
        ':',
        ['to-string', ['get', 'stationIndex']],
      ],
      '123:0',
    ]);
  });

  it('handles different property names', () => {
    const result = compositeIdEquals(['line', 'index'], '42:7');

    expect(result).toEqual([
      '==',
      ['concat', ['to-string', ['get', 'line']], ':', ['to-string', ['get', 'index']]],
      '42:7',
    ]);
  });

  it('handles null value by returning false constant', () => {
    const result = compositeIdEquals(['lineId', 'stationIndex'], null);

    expect(result).toBe(false);
  });
});

describe('inLine', () => {
  it('creates to-string + equality expression', () => {
    const result = inLine('123');

    expect(result).toEqual(['==', ['to-string', ['get', 'lineId']], '123']);
  });

  it('handles different line ID values', () => {
    const result = inLine('sec-abc-456');

    expect(result).toEqual(['==', ['to-string', ['get', 'lineId']], 'sec-abc-456']);
  });

  it('handles null lineId by returning false constant', () => {
    const result = inLine(null);

    expect(result).toBe(false);
  });
});

describe('expression builder with mocked map', () => {
  it('produces valid expressions for setPaintProperty', () => {
    const mockMap = {
      setPaintProperty: vi.fn(),
    };

    const expr = buildPriorityExpression([{ condition: inLine('sec-1'), value: 3 }], 1);

    mockMap.setPaintProperty('layer-id', 'circle-stroke-width', expr);

    expect(mockMap.setPaintProperty).toHaveBeenCalledWith('layer-id', 'circle-stroke-width', [
      'case',
      ['==', ['to-string', ['get', 'lineId']], 'sec-1'],
      3,
      1,
    ]);
  });

  it('produces valid expressions with compositeIdEquals', () => {
    const mockMap = {
      setPaintProperty: vi.fn(),
    };

    const expr = buildPriorityExpression(
      [{ condition: compositeIdEquals(['lineId', 'stationIndex'], '1:5'), value: 4 }],
      1
    );

    mockMap.setPaintProperty('layer-id', 'circle-stroke-width', expr);

    expect(mockMap.setPaintProperty).toHaveBeenCalledWith('layer-id', 'circle-stroke-width', [
      'case',
      [
        '==',
        ['concat', ['to-string', ['get', 'lineId']], ':', ['to-string', ['get', 'stationIndex']]],
        '1:5',
      ],
      4,
      1,
    ]);
  });

  it('handles priority combinations (station > line > default)', () => {
    const mockMap = {
      setPaintProperty: vi.fn(),
    };

    // Build expression with station ID having highest priority
    const expr = buildPriorityExpression(
      [
        { condition: compositeIdEquals(['lineId', 'stationIndex'], '1:5'), value: 4 },
        { condition: inLine('1'), value: 3 },
      ],
      1
    );

    mockMap.setPaintProperty('layer-id', 'circle-stroke-width', expr);

    expect(mockMap.setPaintProperty).toHaveBeenCalledWith('layer-id', 'circle-stroke-width', [
      'case',
      // First condition: exact station match (highest priority)
      [
        '==',
        ['concat', ['to-string', ['get', 'lineId']], ':', ['to-string', ['get', 'stationIndex']]],
        '1:5',
      ],
      4,
      // Second condition: line match
      ['==', ['to-string', ['get', 'lineId']], '1'],
      3,
      // Fallback
      1,
    ]);
  });

  it('handles null values in priority conditions', () => {
    const mockMap = {
      setPaintProperty: vi.fn(),
    };

    // When no selection, conditions should be false constants
    const expr = buildPriorityExpression(
      [
        { condition: compositeIdEquals(['lineId', 'stationIndex'], null), value: 4 },
        { condition: inLine(null), value: 3 },
      ],
      1
    );

    mockMap.setPaintProperty('layer-id', 'circle-stroke-width', expr);

    // Both conditions are false constants, so they'll never match
    expect(mockMap.setPaintProperty).toHaveBeenCalledWith('layer-id', 'circle-stroke-width', [
      'case',
      false,
      4,
      false,
      3,
      1,
    ]);
  });
});
