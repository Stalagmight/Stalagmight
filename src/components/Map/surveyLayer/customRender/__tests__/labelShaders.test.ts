import { describe, expect, it } from 'vitest';
import { LABEL_FLAG, LABEL_VERTEX_SRC } from '../labelShaders';

/**
 * Label flag + shader contract (#customRender labels, reveal-zoom rework). The
 * old per-build boolean COLLIDE_HIDE is GONE - decluttering + decimation are now
 * a per-label `a_reveal_zoom` compared against the live `u_zoom` each frame,
 * which is monotone in zoom by construction. DRAG_HIDE stays a distinct flag so a
 * drag flip (flags-only upload) never disturbs the reveal buffer.
 */
describe('LABEL_FLAG bit contract', () => {
  it('every flag is a distinct single bit', () => {
    const values = Object.values(LABEL_FLAG);
    for (const v of values) expect(v & (v - 1)).toBe(0); // power of two
    let acc = 0;
    for (const v of values) {
      expect(acc & v).toBe(0); // no two share a bit
      acc |= v;
    }
    // FIRST_IN_LINE / REFERENCE / ALL_REFERENCE / DIM / DRAG_HIDE /
    // COMMENT_HIDE / JUMP / ALL_JUMP (no COLLIDE_HIDE).
    expect(Object.keys(LABEL_FLAG)).toHaveLength(8);
  });

  it('no longer defines a COLLIDE_HIDE bit (declutter moved to a_reveal_zoom)', () => {
    expect((LABEL_FLAG as Record<string, number>).COLLIDE_HIDE).toBeUndefined();
    expect(LABEL_VERTEX_SRC).not.toContain('COLLIDE_HIDE');
  });
});

describe('LABEL_VERTEX_SRC reveal-zoom + hide contract', () => {
  it('draws a label iff the live zoom has reached its reveal zoom (monotone)', () => {
    expect(LABEL_VERTEX_SRC).toContain('u_zoom < a_reveal_zoom');
    expect(LABEL_VERTEX_SRC).toContain('in float a_reveal_zoom;');
    expect(LABEL_VERTEX_SRC).toContain('uniform float u_zoom;');
  });

  it('collapses a drag-excluded label, and no longer does per-frame mod() decimation', () => {
    expect(LABEL_VERTEX_SRC).toContain('a_flags & FLAG_DRAG_HIDE');
    expect(LABEL_VERTEX_SRC).not.toContain('u_decim_mod');
    expect(LABEL_VERTEX_SRC).not.toContain('mod(a_ordinal');
  });

  it('collapses the picked-up comment (COMMENT_HIDE) parallel to DRAG_HIDE (C3)', () => {
    expect(LABEL_VERTEX_SRC).toContain('a_flags & FLAG_COMMENT_HIDE');
  });
});
