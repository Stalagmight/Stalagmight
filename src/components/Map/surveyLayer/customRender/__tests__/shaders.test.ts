import { describe, it, expect } from 'vitest';
import {
  highlightSizeMul,
  DEFAULT_SHOT_HOVER_WIDTH_MUL,
  DEFAULT_SHOT_SELECT_WIDTH_MUL,
  DEFAULT_STATION_HOVER_SIZE_MUL,
  DEFAULT_STATION_SELECT_SIZE_MUL,
} from '../shaders';

/**
 * Highlight size-multiplier resolution (C2): the custom survey shader bumps a
 * hovered / selected line's shot width + station radius by a ratio derived
 * from the configured absolute px, falling back to the historical baked ratios.
 */
describe('highlightSizeMul (configured shot/station highlight sizes)', () => {
  it('resolves the emphasized-over-base ratio from configured px', () => {
    expect(highlightSizeMul(2, 4, DEFAULT_SHOT_HOVER_WIDTH_MUL)).toBe(2); // 4/2
    expect(highlightSizeMul(2, 3, DEFAULT_SHOT_SELECT_WIDTH_MUL)).toBe(1.5); // 3/2
    expect(highlightSizeMul(4, 6, DEFAULT_STATION_HOVER_SIZE_MUL)).toBe(1.5); // 6/4
    expect(highlightSizeMul(4, 5, DEFAULT_STATION_SELECT_SIZE_MUL)).toBe(1.25); // 5/4
  });

  it('honors independent config that differs from the stock defaults', () => {
    // A user who widens hover to 8 over a base 2 gets 4x, not the baked 2x.
    expect(highlightSizeMul(2, 8, DEFAULT_SHOT_HOVER_WIDTH_MUL)).toBe(4);
  });

  it('falls back to the baked ratio when the emphasized px is unset', () => {
    expect(highlightSizeMul(2, undefined, DEFAULT_SHOT_HOVER_WIDTH_MUL)).toBe(
      DEFAULT_SHOT_HOVER_WIDTH_MUL,
    );
  });

  it('falls back when the base or emphasized px is non-positive (guards div-by-zero)', () => {
    expect(highlightSizeMul(0, 4, DEFAULT_SHOT_HOVER_WIDTH_MUL)).toBe(DEFAULT_SHOT_HOVER_WIDTH_MUL);
    expect(highlightSizeMul(2, 0, DEFAULT_SHOT_SELECT_WIDTH_MUL)).toBe(DEFAULT_SHOT_SELECT_WIDTH_MUL);
    expect(highlightSizeMul(-2, 4, DEFAULT_STATION_HOVER_SIZE_MUL)).toBe(
      DEFAULT_STATION_HOVER_SIZE_MUL,
    );
  });
});
