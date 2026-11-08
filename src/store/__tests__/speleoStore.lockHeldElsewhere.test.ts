/**
 * [#576 SDB-2MACHINE-D1] A Done that finished here but left the account's lock with
 * the session using it.
 *
 * Two computers on one account: A edits under the lock, and B's status poll adopted
 * it. B made no edit under it, so B's Done answers `lock_held_elsewhere`: nothing to
 * upload, editing ends here, and the lock is not released. The frontend must treat
 * the edit session as finished - like `released` - without claiming the lock was
 * handed back.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import { doneFinished, useSpeleoStore } from '../speleoStore';
import type { DoneResult } from '../../types/ipc.generated';

const HELD_ELSEWHERE: DoneResult = { kind: 'lock_held_elsewhere' };

beforeEach(() => {
  useSpeleoStore.getState().setLinkOnOpen({ permission: 'ADMIN' });
  const link = useSpeleoStore.getState().link;
  if (!link) throw new Error('fixture: no link');
  useSpeleoStore.setState({
    link: {
      ...link,
      editState: 'editing',
      hasUnpushedEdits: false,
      lockHolder: 'me@x',
      lockedByOther: false,
    },
    banner: { kind: 'push_unconfirmed' },
  });
});

describe('Done that left an adopted lock with the session using it (#576 SDB-2MACHINE-D1)', () => {
  it('ends editing here, but keeps the holder', () => {
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
