import { describe, expect, it } from 'vitest';
import type { ShotKind } from '../../types/ipc/ShotKind';
import {
  SHOT_KIND_CYCLE,
  countsTowardLength,
  isVirtualShotKind,
  nextShotKind,
  shotKindGlyph,
  surveyedLength,
} from '../shotKind';
import { shotKindFromFlags, shotKindToFlags } from '../../types/survey-data';

describe('shot kind cycle (#27)', () => {
  it('cycles blank -> J -> R -> blank', () => {
    expect(nextShotKind('Normal')).toBe('Jump');
    expect(nextShotKind('Jump')).toBe('Reference');
    expect(nextShotKind('Reference')).toBe('Normal');
  });

  it('returns to the starting kind after a full lap', () => {
    for (const start of SHOT_KIND_CYCLE) {
      let kind: ShotKind = start;
      for (let i = 0; i < SHOT_KIND_CYCLE.length; i++) kind = nextShotKind(kind);
      expect(kind).toBe(start);
    }
  });

  it('walks backwards on shift-click, wrapping past zero', () => {
    expect(nextShotKind('Normal', -1)).toBe('Reference');
    expect(nextShotKind('Reference', -1)).toBe('Jump');
    expect(nextShotKind('Jump', -1)).toBe('Normal');
  });

  it('forward then backward is a no-op from every state', () => {
    for (const kind of SHOT_KIND_CYCLE) {
      expect(nextShotKind(nextShotKind(kind), -1)).toBe(kind);
    }
  });

  it('renders a permanent shot as blank so virtual shots stand out', () => {
    expect(shotKindGlyph('Normal')).toBe('');
    expect(shotKindGlyph('Jump')).toBe('J');
    expect(shotKindGlyph('Reference')).toBe('R');
  });

  it('treats both virtual kinds as virtual', () => {
    expect(isVirtualShotKind('Normal')).toBe(false);
    expect(isVirtualShotKind('Jump')).toBe(true);
    expect(isVirtualShotKind('Reference')).toBe(true);
  });
});

describe('shot kind binary flag encoding (#27)', () => {
  it('round-trips every kind through the flags byte', () => {
    for (const kind of SHOT_KIND_CYCLE) {
      expect(shotKindFromFlags(shotKindToFlags(kind))).toBe(kind);
    }
  });

  it('matches the Rust discriminants, which are on the wire', () => {
    expect(shotKindToFlags('Normal')).toBe(0);
    expect(shotKindToFlags('Jump')).toBe(1);
    expect(shotKindToFlags('Reference')).toBe(2);
  });

  it('ignores the reserved high bits', () => {
    // Bits 2-7 are reserved for future use (e.g. a has_comment cache bit), so
    // setting them must not disturb the kind.
    expect(shotKindFromFlags(0b1111_1101)).toBe('Jump');
    expect(shotKindFromFlags(0b1010_1010)).toBe('Reference');
  });

  it('degrades an unknown discriminant to Reference, not Normal', () => {
    // Mirrors Rust's ShotKind::from_u8 on BOTH counts: a corrupt byte must not
    // break a load, and it must not produce a swimmable leg either. `Normal`
    // would enter gas, penetration and the ruler at normal cost; `Reference`
    // can only shrink a route, which is the safe direction (#27).
    expect(shotKindFromFlags(0b11)).toBe('Reference');
  });
});

describe('survey length leaves out Reference shots (#689)', () => {
  it('counts Normal and Jump, never Reference', () => {
    expect(countsTowardLength('Normal')).toBe(true);
    expect(countsTowardLength('Jump')).toBe(true);
    expect(countsTowardLength('Reference')).toBe(false);
  });

  it('sums and counts only the shots that count toward length', () => {
    const shots: { kind: ShotKind; length: number }[] = [
      { kind: 'Normal', length: 10 },
      { kind: 'Reference', length: 4 },
      { kind: 'Jump', length: 3 },
      { kind: 'Reference', length: 6 },
    ];
    expect(surveyedLength(shots)).toEqual({ length: 13, shotCount: 2 });
  });

  it('is zero for a run made only of Reference shots', () => {
    expect(surveyedLength([{ kind: 'Reference', length: 9 }])).toEqual({
      length: 0,
      shotCount: 0,
    });
  });
});
