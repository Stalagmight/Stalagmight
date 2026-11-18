/**
 * Unit tests for DetailLink component helper functions.
 *
 * Tests focus on the exported utility functions for test ID and aria-label generation.
 * Component integration is tested via Playwright.
 */

import { describe, it, expect } from 'vitest';
import {
  generateTestId,
  generateAriaLabel,
  hasPosition,
  type DetailLinkType,
  type StationOrShotTarget,
  type LineTarget,
} from '../DetailLink';

describe('generateTestId', () => {
  describe('station links', () => {
    it('generates correct test ID for station', () => {
      const target: StationOrShotTarget = { lineId: 2, position: 5 };
      expect(generateTestId('station', target)).toBe('station-2-5-link');
    });

    it('generates correct test ID for first station in first line (0-indexed)', () => {
      const target: StationOrShotTarget = { lineId: 1, position: 0 };
      expect(generateTestId('station', target)).toBe('station-1-0-link');
    });

    it('handles large numbers', () => {
      const target: StationOrShotTarget = { lineId: 123, position: 456 };
      expect(generateTestId('station', target)).toBe('station-123-456-link');
    });
  });

  describe('shot links', () => {
    it('generates correct test ID for shot', () => {
      const target: StationOrShotTarget = { lineId: 2, position: 3 };
      expect(generateTestId('shot', target)).toBe('shot-2-3-link');
    });

    it('generates correct test ID for first shot', () => {
      const target: StationOrShotTarget = { lineId: 1, position: 1 };
      expect(generateTestId('shot', target)).toBe('shot-1-1-link');
    });

    it('handles large numbers', () => {
      const target: StationOrShotTarget = { lineId: 99, position: 88 };
      expect(generateTestId('shot', target)).toBe('shot-99-88-link');
    });
  });

  describe('line links', () => {
    it('generates correct test ID for line', () => {
      const target: LineTarget = { lineId: 2 };
      expect(generateTestId('line', target)).toBe('line-2-link');
    });

    it('ignores position for line links', () => {
      const target: StationOrShotTarget = { lineId: 5, position: 99 };
      expect(generateTestId('line', target)).toBe('line-5-link');
    });

    it('generates correct test ID for first line', () => {
      const target: LineTarget = { lineId: 1 };
      expect(generateTestId('line', target)).toBe('line-1-link');
    });
  });

  describe('error handling', () => {
    it('throws error for station without position', () => {
      const target: LineTarget = { lineId: 1 };
      expect(() => generateTestId('station', target)).toThrow('station links require a position');
    });

    it('throws error for shot without position', () => {
      const target: LineTarget = { lineId: 1 };
      expect(() => generateTestId('shot', target)).toThrow('shot links require a position');
    });

    it('handles zero values (edge case)', () => {
      const target: StationOrShotTarget = { lineId: 0, position: 0 };
      expect(generateTestId('shot', target)).toBe('shot-0-0-link');
    });
  });
});

describe('generateAriaLabel', () => {
  describe('station labels', () => {
    it('generates correct aria-label for station', () => {
      expect(generateAriaLabel('station', '2:5')).toBe('Select station 2:5');
    });

    it('handles station with name', () => {
      expect(generateAriaLabel('station', '1:1 (Entrance)')).toBe('Select station 1:1 (Entrance)');
    });
  });

  describe('shot labels', () => {
    it('generates correct aria-label for shot', () => {
      expect(generateAriaLabel('shot', '2:3')).toBe('Select shot 2:3');
    });

    it('handles shot with additional info', () => {
      expect(generateAriaLabel('shot', '5:10')).toBe('Select shot 5:10');
    });
  });

  describe('line labels', () => {
    it('generates correct aria-label for line', () => {
      expect(generateAriaLabel('line', '2: Main Passage')).toBe('Select line 2: Main Passage');
    });

    it('handles unnamed line', () => {
      expect(generateAriaLabel('line', '3: Unnamed')).toBe('Select line 3: Unnamed');
    });

    it('handles line with just ID', () => {
      expect(generateAriaLabel('line', '1')).toBe('Select line 1');
    });
  });

  describe('display text variations', () => {
    it('handles empty display text without trailing space', () => {
      expect(generateAriaLabel('station', '')).toBe('Select station');
    });

    it('handles whitespace-only display text without trailing space', () => {
      expect(generateAriaLabel('station', '   ')).toBe('Select station');
    });

    it('trims leading/trailing whitespace from display text', () => {
      expect(generateAriaLabel('station', '  2:5  ')).toBe('Select station 2:5');
    });

    it('handles special characters in display text', () => {
      expect(generateAriaLabel('line', '1: Cave "A"')).toBe('Select line 1: Cave "A"');
    });

    it('handles numbers only', () => {
      expect(generateAriaLabel('shot', '123')).toBe('Select shot 123');
    });
  });
});

describe('hasPosition', () => {
  it('returns true for target with position', () => {
    const target: StationOrShotTarget = { lineId: 1, position: 5 };
    expect(hasPosition(target)).toBe(true);
  });

  it('returns false for target without position', () => {
    const target: LineTarget = { lineId: 1 };
    expect(hasPosition(target)).toBe(false);
  });

  it('returns true for zero position (valid)', () => {
    const target: StationOrShotTarget = { lineId: 1, position: 0 };
    expect(hasPosition(target)).toBe(true);
  });
});

describe('type safety', () => {
  it('accepts all valid link types with proper targets', () => {
    const stationTarget: StationOrShotTarget = { lineId: 1, position: 1 };
    const lineTarget: LineTarget = { lineId: 1 };

    // Station and shot require position
    expect(generateTestId('station', stationTarget)).toContain('-link');
    expect(generateTestId('shot', stationTarget)).toContain('-link');

    // Line works without position
    expect(generateTestId('line', lineTarget)).toContain('-link');

    // All types work with ariaLabel
    const types: DetailLinkType[] = ['station', 'shot', 'line'];
    for (const type of types) {
      const ariaLabel = generateAriaLabel(type, '1:1');
      expect(ariaLabel).toContain('Select');
    }
  });

  it('LineTarget only requires lineId', () => {
    const validTarget: LineTarget = { lineId: 1 };
    expect(validTarget.lineId).toBe(1);
    expect(hasPosition(validTarget)).toBe(false);
  });

  it('StationOrShotTarget requires both lineId and position', () => {
    const validTarget: StationOrShotTarget = { lineId: 1, position: 5 };
    expect(validTarget.lineId).toBe(1);
    expect(validTarget.position).toBe(5);
    expect(hasPosition(validTarget)).toBe(true);
  });
});

describe('test ID format consistency', () => {
  it('all test IDs end with -link suffix', () => {
    const targets = [
      { type: 'station' as const, target: { lineId: 1, position: 1 } as StationOrShotTarget },
      { type: 'shot' as const, target: { lineId: 2, position: 3 } as StationOrShotTarget },
      { type: 'line' as const, target: { lineId: 5 } as LineTarget },
    ];

    for (const { type, target } of targets) {
      const testId = generateTestId(type, target);
      expect(testId).toMatch(/-link$/);
    }
  });

  it('test IDs contain the type', () => {
    const types: DetailLinkType[] = ['station', 'shot', 'line'];
    const target: StationOrShotTarget = { lineId: 1, position: 1 };

    for (const type of types) {
      const testId = generateTestId(type, target);
      expect(testId).toContain(type);
    }
  });

  it('test IDs use hyphen separators', () => {
    const target: StationOrShotTarget = { lineId: 2, position: 5 };
    const testId = generateTestId('station', target);

    // Should not contain any other separators
    expect(testId).not.toContain(':');
    expect(testId).not.toContain('_');
    expect(testId).not.toContain(' ');
  });
});

describe('aria-label format consistency', () => {
  it('all aria-labels start with "Select"', () => {
    const types: DetailLinkType[] = ['station', 'shot', 'line'];

    for (const type of types) {
      const ariaLabel = generateAriaLabel(type, 'test');
      expect(ariaLabel).toMatch(/^Select /);
    }
  });

  it('aria-labels include the element type', () => {
    const types: DetailLinkType[] = ['station', 'shot', 'line'];

    for (const type of types) {
      const ariaLabel = generateAriaLabel(type, 'test');
      expect(ariaLabel).toContain(type);
    }
  });

  it('aria-labels include the display text', () => {
    const displayTexts = ['1:1', '5:10', 'Main Passage', '99'];

    for (const displayText of displayTexts) {
      const ariaLabel = generateAriaLabel('station', displayText);
      expect(ariaLabel).toContain(displayText);
    }
  });
});
