import { describe, it, expect } from 'vitest';
import {
  applySegmentEdits,
  clampSegmentSpeed,
  parseGridBoolean,
  SEG_COL,
  type SegmentEditContext,
} from '../segmentsGridEdits';
import { speedToDisplay } from '../../../utils/formatGas';
import type { DpvRig, PlanSegment } from '../../../types/ipc.generated';

const seg = (over: Partial<PlanSegment> = {}): PlanSegment => ({
  speed_m_per_min: null,
  on_scooter: false,
  ...over,
});

const RIG: DpvRig = {
  primary: { speed_m_per_min: 45, burn_minutes: 90 },
  backup: { speed_m_per_min: 40, burn_minutes: 60 },
  range_reserve_fraction: 0.5,
};

const metric = (dpv: DpvRig | null = null): SegmentEditContext => ({ units: 'metric', dpv });
const imperial = (dpv: DpvRig | null = null): SegmentEditContext => ({ units: 'imperial', dpv });

describe('clampSegmentSpeed', () => {
  it('clamps to the engine range the segment will actually be flown at', () => {
    expect(clampSegmentSpeed(0)).toBe(0.1);
    expect(clampSegmentSpeed(-5)).toBe(0.1);
    expect(clampSegmentSpeed(18)).toBe(18);
    expect(clampSegmentSpeed(5000)).toBe(1000);
  });
});

describe('parseGridBoolean', () => {
  it('accepts the truthy token table', () => {
    for (const token of ['1', 'true', 'T', 'yes', 'Y', 'on', 'x', '✓', ' TRUE ']) {
      expect(parseGridBoolean(token, false)).toBe(true);
    }
  });

  it('accepts the falsy token table', () => {
    for (const token of ['0', 'false', 'F', 'no', 'N', 'off', ' OFF ']) {
      expect(parseGridBoolean(token, true)).toBe(false);
    }
  });

  it('treats an empty cell as the current value, so a clear never unequips', () => {
    expect(parseGridBoolean('', true)).toBe(true);
    expect(parseGridBoolean('   ', false)).toBe(false);
  });

  it('skips anything it does not recognise rather than guessing', () => {
    expect(parseGridBoolean('maybe', false)).toBeUndefined();
    expect(parseGridBoolean('2', false)).toBeUndefined();
    expect(parseGridBoolean('-', true)).toBeUndefined();
  });
});

describe('applySegmentEdits - speed', () => {
  it('converts a metric display value straight to canonical m/min', () => {
    const rows = [seg()];
    const next = applySegmentEdits(rows, [{ row: 0, colKey: SEG_COL.speed, value: 24 }], metric());
    expect(next[0]?.speed_m_per_min).toBe(24);
  });

  it('round-trips an imperial display value back to the canonical speed', () => {
    const rows = [seg()];
    const display = speedToDisplay(18, 'imperial');
    const next = applySegmentEdits(
      rows,
      [{ row: 0, colKey: SEG_COL.speed, value: display }],
      imperial()
    );
    expect(next[0]?.speed_m_per_min).toBeCloseTo(18, 10);
  });

  it('clamps AFTER converting, so an imperial edit is judged in m/min', () => {
    // 0.2 ft/min is 0.06 m/min - below the engine minimum even though the typed
    // number is above it. Clamping in display space would have kept 0.06.
    const slow = applySegmentEdits(
      [seg()],
      [{ row: 0, colKey: SEG_COL.speed, value: 0.2 }],
      imperial()
    );
    expect(slow[0]?.speed_m_per_min).toBe(0.1);
    // 3281 ft/min is just over 1000 m/min; clamping in display space would have
    // produced 1000 ft/min = 304.8 m/min.
    const fast = applySegmentEdits(
      [seg()],
      [{ row: 0, colKey: SEG_COL.speed, value: 3281 }],
      imperial()
    );
    expect(fast[0]?.speed_m_per_min).toBe(1000);
  });

  it('clears the speed back to inherited on a null (cleared cell)', () => {
    const rows = [seg({ speed_m_per_min: 30 })];
    const next = applySegmentEdits(rows, [{ row: 0, colKey: SEG_COL.speed, value: null }], metric());
    expect(next[0]?.speed_m_per_min).toBeNull();
    expect(next[0]?.on_scooter).toBe(false);
  });

  it('skips a non-finite value instead of writing NaN', () => {
    const rows = [seg({ speed_m_per_min: 30 })];
    const next = applySegmentEdits(
      rows,
      [{ row: 0, colKey: SEG_COL.speed, value: Number.NaN }],
      metric()
    );
    expect(next).toBe(rows);
  });
});

describe('applySegmentEdits - scooter coupling', () => {
  it('auto-fills the rig primary speed when ticking a span with no speed of its own', () => {
    const rows = [seg()];
    const next = applySegmentEdits(
      rows,
      [{ row: 0, colKey: SEG_COL.scooter, value: true }],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: true, speed_m_per_min: 45 });
  });

  it('leaves an explicit speed alone when ticking', () => {
    const rows = [seg({ speed_m_per_min: 22 })];
    const next = applySegmentEdits(
      rows,
      [{ row: 0, colKey: SEG_COL.scooter, value: true }],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: true, speed_m_per_min: 22 });
  });

  it('ticks without a speed when there is no rig to take one from', () => {
    const rows = [seg()];
    const next = applySegmentEdits(
      rows,
      [{ row: 0, colKey: SEG_COL.scooter, value: true }],
      metric(null)
    );
    expect(next[0]).toEqual({ on_scooter: true, speed_m_per_min: null });
  });

  it('unticking always returns the span to inheriting, auto-filled or not', () => {
    const autoFilled = applySegmentEdits(
      [seg({ on_scooter: true, speed_m_per_min: 45 })],
      [{ row: 0, colKey: SEG_COL.scooter, value: false }],
      metric(RIG)
    );
    expect(autoFilled[0]).toEqual({ on_scooter: false, speed_m_per_min: null });
    const handTyped = applySegmentEdits(
      [seg({ on_scooter: true, speed_m_per_min: 22 })],
      [{ row: 0, colKey: SEG_COL.scooter, value: false }],
      metric(RIG)
    );
    expect(handTyped[0]).toEqual({ on_scooter: false, speed_m_per_min: null });
  });

  it('does not touch the speed when the flag does not move (the change guard)', () => {
    // A paste stamping `false` over an already-unticked row, and a re-tick of an
    // already-ticked one: neither may rewrite the speed column.
    const unticked = [seg({ speed_m_per_min: 22 })];
    expect(
      applySegmentEdits(unticked, [{ row: 0, colKey: SEG_COL.scooter, value: false }], metric(RIG))
    ).toBe(unticked);
    const ticked = [seg({ on_scooter: true, speed_m_per_min: 22 })];
    expect(
      applySegmentEdits(ticked, [{ row: 0, colKey: SEG_COL.scooter, value: true }], metric(RIG))
    ).toBe(ticked);
  });

  it('parses a pasted boolean string, and skips an unrecognised one', () => {
    const rows = [seg()];
    expect(
      applySegmentEdits(rows, [{ row: 0, colKey: SEG_COL.scooter, value: 'yes' }], metric(RIG))[0]
    ).toEqual({ on_scooter: true, speed_m_per_min: 45 });
    expect(
      applySegmentEdits(rows, [{ row: 0, colKey: SEG_COL.scooter, value: 'perhaps' }], metric(RIG))
    ).toBe(rows);
  });
});

describe('applySegmentEdits - batches', () => {
  it('a batch-authored speed survives the same batch untick (full-row paste onto a scootered span)', () => {
    // Codex P2 on #590: pasting a swum row with its own pace onto a scootered
    // row emits speed before scooter; the untick reset must keep the pasted
    // pace, not return the span to inheriting.
    const rows = [seg({ on_scooter: true, speed_m_per_min: 40 })];
    const next = applySegmentEdits(
      rows,
      [
        { row: 0, colKey: SEG_COL.speed, value: 15 },
        { row: 0, colKey: SEG_COL.scooter, value: false },
      ],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: false, speed_m_per_min: 15 });
  });

  it('a no-op speed edit still counts as batch-authored for the untick', () => {
    // The pasted speed equals the current one, so the speed edit itself changes
    // nothing - but it is still the paste supplying that pace, and the untick
    // beside it must not wipe it.
    const rows = [seg({ on_scooter: true, speed_m_per_min: 15 })];
    const next = applySegmentEdits(
      rows,
      [
        { row: 0, colKey: SEG_COL.speed, value: 15 },
        { row: 0, colKey: SEG_COL.scooter, value: false },
      ],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: false, speed_m_per_min: 15 });
  });

  it('an untick with no speed beside it in the batch still resets to inherited', () => {
    const rows = [seg({ on_scooter: true, speed_m_per_min: 40 })];
    const next = applySegmentEdits(
      rows,
      [{ row: 0, colKey: SEG_COL.scooter, value: false }],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: false, speed_m_per_min: null });
  });

  it('an untick before the speed edit lands on the pasted pace regardless of order', () => {
    const rows = [seg({ on_scooter: true, speed_m_per_min: 40 })];
    const next = applySegmentEdits(
      rows,
      [
        { row: 0, colKey: SEG_COL.scooter, value: false },
        { row: 0, colKey: SEG_COL.speed, value: 15 },
      ],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: false, speed_m_per_min: 15 });
  });

  it('applies edits in order, so a speed written first survives the tick beside it', () => {
    const rows = [seg(), seg()];
    const next = applySegmentEdits(
      rows,
      [
        { row: 0, colKey: SEG_COL.speed, value: 30 },
        { row: 0, colKey: SEG_COL.scooter, value: true },
      ],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: true, speed_m_per_min: 30 });
    expect(next[1]).toBe(rows[1]);
  });

  it('auto-fills when the tick comes first and the speed lands after it', () => {
    const next = applySegmentEdits(
      [seg()],
      [
        { row: 0, colKey: SEG_COL.scooter, value: true },
        { row: 0, colKey: SEG_COL.speed, value: 30 },
      ],
      metric(RIG)
    );
    expect(next[0]).toEqual({ on_scooter: true, speed_m_per_min: 30 });
  });

  it('returns the identical array when no edit changed anything', () => {
    const rows = [seg({ speed_m_per_min: 24 }), seg({ on_scooter: true, speed_m_per_min: 45 })];
    const next = applySegmentEdits(
      rows,
      [
        { row: 0, colKey: SEG_COL.speed, value: 24 },
        { row: 1, colKey: SEG_COL.scooter, value: true },
        { row: 9, colKey: SEG_COL.speed, value: 12 },
        { row: 0, colKey: 'from', value: 'nonsense' },
      ],
      metric(RIG)
    );
    expect(next).toBe(rows);
  });
});
