/**
 * Component test - [#576 SDB-2MACHINE-D1] which answer of the sign-out prompt a stray
 * Enter lands on.
 *
 * DialogWrapper focuses the [data-primary-action] button, so Enter answers with it.
 * When every lock the prompt names is in use elsewhere (adopted from the account,
 * no edit made here: another computer is editing under it), the default is "Keep":
 * a stray Enter must not release the other computer's lock. Any lock taken or edited
 * here in the list keeps "Release and sign out" as the default, as before.
 *
 * Driven through the real store: the staged logout asks the (mocked) backend which
 * locks it holds, raises the prompt, and signs out with the answer. jsdom lays nothing
 * out, so every element fails the heuristic's `getClientRects()` visibility check and
 * the focus would fall through to the close X; the check is stubbed to "visible" here
 * so the focus, and the Enter, land where they do in the app.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');

import SpeleoSignOutDialog from './SpeleoSignOutDialog';
import { useSpeleoStore } from '../../store/speleoStore';
import { invokeIpcMock } from '../../test/factories/ipc';
import type { SpeleoLockLeftHeld, SpeleoSignOutLocks } from '../../types/ipc.generated';

const CAVE_A: SpeleoLockLeftHeld = { project_id: 'a', project_name: 'CaveA' };
const CAVE_B: SpeleoLockLeftHeld = { project_id: 'b', project_name: 'CaveB' };
const CAVE_C: SpeleoLockLeftHeld = { project_id: 'c', project_name: 'CaveC' };

/** Every IPC a release could go out through. */
const RELEASE_COMMANDS = ['speleodb_release_current_lock', 'speleodb_release_lock', 'speleodb_release_all_locks'];

/** The calls made to `cmd`, as their argument objects. */
const callsTo = (cmd: string) =>
  invokeIpcMock.mock.calls.filter(([name]) => name === cmd).map(([, args]) => args as Record<string, unknown>);

/** Stage a logout over a session holding `locks`, and wait for its prompt. The
 *  pending sign-out comes back wrapped: an async function returning the bare promise
 *  would make its caller wait for the answer the test has not given yet. */
async function promptFor(locks: SpeleoSignOutLocks): Promise<{ applied: Promise<{ ok: boolean }> }> {
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_sign_out_locks') return locks;
    if (cmd === 'speleodb_logout') return { locks_left_held: [] };
    return [];
  });
  useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
  const applied = useSpeleoStore.getState().applyPendingAuth('https://x/');
  await vi.waitFor(() => expect(useSpeleoStore.getState().signOutPrompt).not.toBeNull());
  render(<SpeleoSignOutDialog />);
  return { applied };
}

let laidOut: { mockRestore: () => void } | null = null;

beforeEach(() => {
  laidOut = vi
    .spyOn(HTMLElement.prototype, 'getClientRects')
    .mockReturnValue([new DOMRect(0, 0, 10, 10)] as unknown as DOMRectList);
  useSpeleoStore.setState({ signOutPrompt: null });
  useSpeleoStore.getState().setSessionFromLogin({
    user_email: 'old@x',
    endpoint: 'https://x/',
    keychain_backed: true,
    storage: 'keychain',
    keychain: 'available',
    needs_storage_choice: false,
    replaced_session: false,
    locks_left_held: [],
  });
});

afterEach(() => {
  laidOut?.mockRestore();
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({ session: null, pendingAuth: null, signOutPrompt: null });
});

describe('SpeleoSignOutDialog - the default answer (#576 SDB-2MACHINE-D1)', () => {
  it('only locks in use elsewhere: Keep is the default, and Enter keeps them', async () => {
    const { applied } = await promptFor({ releasable: [], unpushed: [], in_use_elsewhere: [CAVE_B, CAVE_C] });

    const keep = screen.getByTestId('speleodb-sign-out-keep');
    const release = screen.getByTestId('speleodb-sign-out-release');
    // Both answers stay on offer: Release still hands them back when clicked.
    expect(release).toHaveTextContent('Release and sign out');
    // Where the focus heuristic settles, and which answer is marked the default -
    // read before the Enter, which closes the prompt.
    await waitFor(() => expect(document.activeElement).toHaveAttribute('data-primary-action'));
    const focused = document.activeElement?.getAttribute('data-testid');
    const defaults = [keep, release].filter((b) => b.hasAttribute('data-primary-action')).map((b) => b.dataset.testid);

    await userEvent.keyboard('{Enter}');

    expect((await applied).ok).toBe(true);
    expect(callsTo('speleodb_logout')).toEqual([{ releaseLocks: false }]);
    expect(invokeIpcMock.mock.calls.map(([name]) => name).filter((n) => RELEASE_COMMANDS.includes(n))).toEqual(
      [],
    );
    expect({ focused, defaults }).toEqual({
      focused: 'speleodb-sign-out-keep',
      defaults: ['speleodb-sign-out-keep'],
    });
    expect(useSpeleoStore.getState().session).toBeNull();
  });

  it.each([
    ['a lock taken here beside one in use elsewhere', { releasable: [CAVE_A], unpushed: [], in_use_elsewhere: [CAVE_B] }],
    ['a lock over un-pushed edits beside one in use elsewhere', { releasable: [], unpushed: [CAVE_A], in_use_elsewhere: [CAVE_B] }],
    ['only locks taken here', { releasable: [CAVE_A], unpushed: [], in_use_elsewhere: [] }],
  ])('%s: Release stays the default, and Enter releases', async (_label, locks: SpeleoSignOutLocks) => {
    const { applied } = await promptFor(locks);

    const keep = screen.getByTestId('speleodb-sign-out-keep');
    const release = screen.getByTestId('speleodb-sign-out-release');
    expect(release).toHaveAttribute('data-primary-action');
    expect(keep).not.toHaveAttribute('data-primary-action');
    await waitFor(() => expect(release).toHaveFocus());

    await userEvent.keyboard('{Enter}');

    expect((await applied).ok).toBe(true);
    expect(callsTo('speleodb_logout')).toEqual([{ releaseLocks: true }]);
  });
});
