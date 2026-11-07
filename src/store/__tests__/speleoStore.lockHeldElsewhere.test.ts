/**
 * [#576 RV2-R-13] A Done that finished here but left the account's lock with the
 * session that took it.
 *
 * The backend now answers `lock_held_elsewhere` when this app session did not take
 * the write lock it was editing under (a second window or computer on the same
 * account, or this app after a restart): the edits are uploaded and editing ends
 * here, but the lock is not released. The frontend must treat the work as landed -
 * like `released` - without claiming the lock was handed back.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import { doneFinished, useSpeleoStore } from '../speleoStore';
import type { DoneResult } from '../../types/ipc.generated';

const HELD_ELSEWHERE = { kind: 'lock_held_elsewhere' } as DoneResult;

beforeEach(() => {
  useSpeleoStore.getState().setLinkOnOpen({ permission: 'ADMIN' });
  const link = useSpeleoStore.getState().link;
  if (!link) throw new Error('fixture: no link');
  useSpeleoStore.setState({
    link: {
      ...link,
      editState: 'editing',
      hasUnpushedEdits: true,
      lockHolder: 'me@x',
      lockedByOther: false,
    },
    banner: { kind: 'push_unconfirmed' },
  });
});

describe('Done that left the lock with another session (#576 RV2-R-13)', () => {
  it('marks the work landed and editing over, but keeps the holder', () => {
    useSpeleoStore.getState().applyDone(HELD_ELSEWHERE);

    const { link, banner } = useSpeleoStore.getState();
    expect(link?.hasUnpushedEdits).toBe(false);
    expect(link?.editState).toBe('read-only');
    expect(link?.lockHolder).toBe('me@x');
    expect(link?.lockedByOther).toBe(false);
    expect(banner).toBeNull();
  });

  it('counts as a finished Done for the exit prompts and the Done dialog', () => {
    expect(doneFinished('lock_held_elsewhere')).toBe(true);
    expect(doneFinished('released')).toBe(true);
    for (const kind of ['not_editing', 'divergence_needed', 'push_unconfirmed'] as const) {
      expect(doneFinished(kind)).toBe(false);
    }
    expect(doneFinished(null)).toBe(false);
  });
});
