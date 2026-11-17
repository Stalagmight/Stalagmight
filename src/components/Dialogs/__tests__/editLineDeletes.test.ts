import { describe, it, expect } from 'vitest';
import { classifyDeleteRun, isEndRun } from '../editLineDeletes';

const ORDER = ['a', 'b', 'c', 'd', 'e'];
const of = (...keys: string[]) => new Set(keys);

describe('classifyDeleteRun', () => {
  it('calls a single first or last shot an end run', () => {
    expect(classifyDeleteRun(ORDER, of('a'))).toBe('prefix');
    expect(classifyDeleteRun(ORDER, of('e'))).toBe('suffix');
  });

  it('calls a contiguous run at either end an end run', () => {
    expect(classifyDeleteRun(ORDER, of('a', 'b', 'c'))).toBe('prefix');
    expect(classifyDeleteRun(ORDER, of('c', 'd', 'e'))).toBe('suffix');
  });

  it('does not care what order the targets are given in', () => {
    expect(classifyDeleteRun(ORDER, of('e', 'c', 'd'))).toBe('suffix');
  });

  it('calls a run in the middle mid, however long', () => {
    expect(classifyDeleteRun(ORDER, of('c'))).toBe('mid');
    expect(classifyDeleteRun(ORDER, of('b', 'c', 'd'))).toBe('mid');
  });

  it('calls a gapped selection mid even when it touches an end', () => {
    // Deleting the first and the last leaves the middle stranded either way.
    expect(classifyDeleteRun(ORDER, of('a', 'c'))).toBe('mid');
    expect(classifyDeleteRun(ORDER, of('a', 'e'))).toBe('mid');
    expect(classifyDeleteRun(ORDER, of('a', 'b', 'd', 'e'))).toBe('mid');
  });

  it('calls the whole line all', () => {
    expect(classifyDeleteRun(ORDER, of(...ORDER))).toBe('all');
  });

  it('reports none when nothing in the selection is in the line', () => {
    expect(classifyDeleteRun(ORDER, of('zz'))).toBe('none');
    expect(classifyDeleteRun([], of('a'))).toBe('none');
  });

  it('treats a two-shot line as ends only - there is no middle', () => {
    expect(classifyDeleteRun(['a', 'b'], of('a'))).toBe('prefix');
    expect(classifyDeleteRun(['a', 'b'], of('b'))).toBe('suffix');
  });

  it('ignores targets that are not in the line', () => {
    expect(classifyDeleteRun(ORDER, of('d', 'e', 'ghost'))).toBe('suffix');
  });
});

describe('isEndRun', () => {
  it('lets everything but a mid delete through', () => {
    expect(isEndRun('prefix')).toBe(true);
    expect(isEndRun('suffix')).toBe(true);
    expect(isEndRun('all')).toBe(true);
    expect(isEndRun('none')).toBe(true);
    expect(isEndRun('mid')).toBe(false);
  });
});

/**
 * Two arrangements the rule has to be right about and nothing exercised.
 *
 * `order` is the SAVED shots only, because that is the topology the backend's
 * split/shift preview answers for - a row added in the dialog is not in the
 * line yet. These pin what that means at the boundaries.
 */
describe('classifyDeleteRun with rows added in the dialog', () => {
  // Saved a,b,c,d with a new row between b and c. The caller passes only the
  // saved keys, so the new row is invisible here by construction.
  const SAVED = ['a', 'b', 'c', 'd'];

  it('calls a trailing run of saved shots a suffix even with a new row before it', () => {
    // c,d are last among the SAVED shots; the unsaved row sits earlier and will
    // simply be appended on save, so this stays a truncation.
    expect(classifyDeleteRun(SAVED, of('c', 'd'))).toBe('suffix');
  });

  it('still calls a saved run with saved shots on both sides mid', () => {
    expect(classifyDeleteRun(SAVED, of('b', 'c'))).toBe('mid');
  });

  it('calls every saved shot all, however many unsaved rows exist', () => {
    // The dialog can hold added rows the caller never passes; emptying the saved
    // line is still `all`, which isEndRun lets through without a prompt.
    expect(classifyDeleteRun(SAVED, of(...SAVED))).toBe('all');
    expect(isEndRun('all')).toBe(true);
  });
});

/**
 * A delete range that mixes an unsaved row with a saved mid-line shot.
 *
 * `classifyDeleteRun` sees only the SAVED keys, so the run reads as `mid` and
 * the dialog defers to the split/shift confirm. That deferral returned early,
 * before the new-row targets were processed - so confirming deleted the saved
 * shot and left the new row in the dialog. The fix carries the new-row keys into
 * the pending state, making the gesture atomic; these pin the classification the
 * fix rests on.
 */
describe('classifyDeleteRun with a mixed new-plus-saved range', () => {
  const SAVED = ['a', 'b', 'c', 'd'];

  it('still calls it mid when the saved half is mid-line', () => {
    // The new row is invisible here by construction, so 'b' alone decides.
    expect(classifyDeleteRun(SAVED, of('b'))).toBe('mid');
    expect(isEndRun('mid')).toBe(false);
  });

  it('calls it an end run when the saved half is at an end', () => {
    // No confirm, so the mixed range deletes outright and nothing is deferred.
    expect(isEndRun(classifyDeleteRun(SAVED, of('d')))).toBe(true);
    expect(isEndRun(classifyDeleteRun(SAVED, of('a')))).toBe(true);
  });

  it('calls a range of only new rows none, which never defers', () => {
    // Nothing saved is being deleted, so there is nothing to confirm.
    expect(classifyDeleteRun(SAVED, of())).toBe('none');
    expect(isEndRun('none')).toBe(true);
  });
});
