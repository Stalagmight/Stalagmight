/**
 * The import fuzzer's per-step oracle for the orphan dialog's repeated-shot
 * table (#681), `orphanRepeatBreach`: the twin of the DMP wizard's
 * `overlap-mode-select` checks, which the orphan dialog no longer renders.
 *
 * INJECTION (tried): dropping the `keep_existing` clause reds "a refused Keep
 * existing on an actionable row"; dropping the undo clause reds both "moved the
 * undo stack" cases; `isOrphanRepeatControl` matching only the per-row prefix
 * reds "recognises the set-all buttons".
 */
import { describe, expect, it } from 'vitest';

import {
  isOrphanRepeatControl,
  orphanRepeatBreach,
  type OrphanRepeatStep,
} from '../scenarios/importDialogModel';

const step = (over: Partial<OrphanRepeatStep> = {}): OrphanRepeatStep => ({
  testid: 'overlap-choice-3',
  value: 'use_new',
  performed: true,
  skip: null,
  liveAfter: 2,
  undoBefore: 4,
  undoAfter: 4,
  ...over,
});

describe('isOrphanRepeatControl', () => {
  it('recognises the per-row choices and the set-all buttons, nothing else', () => {
    expect(isOrphanRepeatControl('overlap-choice-0')).toBe(true);
    expect(isOrphanRepeatControl('overlap-set-all-keep-existing')).toBe(true);
    expect(isOrphanRepeatControl('overlap-set-all-average')).toBe(true);
    expect(isOrphanRepeatControl('overlap-mode-select')).toBe(false);
    expect(isOrphanRepeatControl('merge-station-select')).toBe(false);
  });
});

describe('orphanRepeatBreach', () => {
  it('a choice that left the undo stack alone holds', () => {
    expect(orphanRepeatBreach(step())).toBeNull();
  });

  it('a refused Keep existing on an actionable row fails', () => {
    const breach = orphanRepeatBreach(
      step({ value: 'keep_existing', performed: false, skip: 'option-absent' }),
    );
    expect(breach?.headline).toMatch(/Keep existing was not offered/);
  });

  it('a refused Use new is the doubly-anchored guard, not a failure', () => {
    expect(orphanRepeatBreach(step({ performed: false, skip: 'option-absent' }))).toBeNull();
  });

  it('a refused Keep existing on a row that is gone or disabled is not judged', () => {
    const refused = { value: 'keep_existing', performed: false, skip: 'option-absent' } as const;
    expect(orphanRepeatBreach(step({ ...refused, liveAfter: undefined }))).toBeNull();
    expect(orphanRepeatBreach(step({ ...refused, liveAfter: 1 }))).toBeNull();
  });

  it('a per-row choice that moved the undo stack fails', () => {
    expect(orphanRepeatBreach(step({ undoAfter: 5 }))?.headline).toMatch(/moved the undo stack/);
  });

  it('a set-all click that moved the undo stack fails', () => {
    const breach = orphanRepeatBreach(step({ testid: 'overlap-set-all-average', value: null, undoAfter: 3 }));
    expect(breach?.headline).toMatch(/moved the undo stack/);
  });

  it('an unreadable undo depth is not judged', () => {
    expect(orphanRepeatBreach(step({ undoBefore: -1, undoAfter: 5 }))).toBeNull();
  });
});
