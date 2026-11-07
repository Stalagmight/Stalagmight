/**
 * [#303] Stale-reply coverage for `useUnwindTarget`.
 *
 * The hook has TWO entry points into the same `GET /revisions/`: `resolve` (the 300ms
 * hover, which must know whether there is anything to show before it opens the panel)
 * and `refresh` (the panel itself, re-asking so a push or a lock change while it sits
 * open is noticed). Crossing the toolbar and then opening therefore leaves both in
 * flight, and the reply that lands LAST used to win - `target` and the 5s memo alike.
 *
 * Two guards, because they fail differently: a monotonic request id (a newer ask for the
 * SAME project supersedes the older one) and a project-identity capture (the app moved on
 * while the server was answering). Both must cover the memo write, not just the setState:
 * a wrong value cached there outlives its own reply for the whole TTL.
 *
 * `.tsx` is load-bearing - `vite.config.ts` routes only `*.test.tsx` to jsdom, and
 * `renderHook` needs it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('mock error'));

import { useUnwindTarget, type ChainTarget } from '../useUnwindTarget';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import type { SpeleoUnwindTarget } from '../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';
import { deferred } from '../../test/factories/async';

const OLDER: SpeleoUnwindTarget = { hexsha: 'aaa111', is_current: false, discards_unpushed: false };
const NEWER: SpeleoUnwindTarget = { hexsha: 'bbb222', is_current: true, discards_unpushed: true };

const asBack = (raw: SpeleoUnwindTarget): ChainTarget => ({ direction: 'back', ...raw });

const project = (file_path: string) =>
  ({
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-01-01T00:00:00Z',
    file_path,
  }) as never;

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    project: project('/cave/a.stalx'),
    projectTreeGeneration: 1,
  });
  // Only a remote project has revisions to walk - otherwise `fetchTarget` never calls.
  useSpeleoStore.setState({
    projectLinkKind: 'remote',
    remoteProjectId: 'p1',
    sessionGeneration: 1,
    chainTargetGeneration: 1,
  });
});

/** `enabled: false` so the mount effect does not fire a fetch of its own: every request
 *  below is one the test issued, which is what makes the ordering assertions readable. */
const renderTarget = () => renderHook(() => useUnwindTarget(false));

describe('useUnwindTarget - overlapping lookups (#303)', () => {
  it('reuses the just-resolved target when the panel opens', async () => {
    invokeIpcMock.mockResolvedValue(OLDER);
    const { result, rerender } = renderHook(
      ({ enabled }) => useUnwindTarget(enabled),
      { initialProps: { enabled: false } },
    );

    await act(async () => {
      await result.current.resolve();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(1);

    rerender({ enabled: true });
    await act(async () => {
      await Promise.resolve();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(1);
    expect(result.current.target).toEqual(asBack(OLDER));
  });

  it('keeps the newest answer when an older request replies last', async () => {
    const first = deferred<SpeleoUnwindTarget | null>();
    const second = deferred<SpeleoUnwindTarget | null>();
    invokeIpcMock.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { result } = renderTarget();

    // The hover starts one, opening the panel starts another - both in flight.
    let resolved: Promise<ChainTarget | null>;
    act(() => {
      resolved = result.current.resolve();
      void result.current.refresh();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);

    // The NEWER request answers first, the older one straggles in behind it.
    await act(async () => {
      second.resolve(NEWER);
      first.resolve(OLDER);
      await Promise.resolve();
    });

    expect(result.current.target).toEqual(asBack(NEWER));
    // The straggler must not be handed back to its own caller either - it would open the
    // panel on a step that is no longer the one being offered.
    await expect(resolved!).resolves.toBeNull();

    // ...and the memo agrees: inside the TTL this answers without a third fetch, so a
    // guard that covered only `setTarget` would surface the older answer here.
    let memo: ChainTarget | null = null;
    await act(async () => {
      memo = await result.current.resolve();
    });
    expect(memo).toEqual(asBack(NEWER));
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
  });

  it('drops a reply for a project the app has since left', async () => {
    const pending = deferred<SpeleoUnwindTarget | null>();
    invokeIpcMock.mockReturnValueOnce(pending.promise);
    const { result } = renderTarget();

    let resolved: Promise<ChainTarget | null>;
    act(() => {
      resolved = result.current.resolve();
    });

    // The user closes this project and opens another while the server is answering.
    act(() => {
      useProjectStore.setState({ project: project('/cave/b.stalx') });
    });
    await act(async () => {
      pending.resolve(OLDER);
      await Promise.resolve();
    });

    expect(result.current.target).toBeNull();
    await expect(resolved!).resolves.toBeNull();

    // Nothing was memoised, so the next ask is a real fetch rather than five seconds of
    // the previous project's answer.
    invokeIpcMock.mockResolvedValueOnce(NEWER);
    let next: ChainTarget | null = null;
    await act(async () => {
      next = await result.current.resolve();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
    expect(next).toEqual(asBack(NEWER));
  });

  it('drops a reply when the same-path project tree changes in flight', async () => {
    const pending = deferred<SpeleoUnwindTarget | null>();
    invokeIpcMock.mockReturnValueOnce(pending.promise);
    const { result } = renderTarget();

    let resolved: Promise<ChainTarget | null>;
    act(() => {
      resolved = result.current.resolve();
      useProjectStore.setState({ projectTreeGeneration: 2 });
    });
    await act(async () => {
      pending.resolve(OLDER);
      await Promise.resolve();
    });

    expect(result.current.target).toBeNull();
    await expect(resolved!).resolves.toBeNull();
  });

  it('stores and memoises a reply that nothing superseded (control)', async () => {
    invokeIpcMock.mockResolvedValue(OLDER);
    const { result } = renderTarget();

    let first: ChainTarget | null = null;
    await act(async () => {
      first = await result.current.resolve();
    });
    expect(first).toEqual(asBack(OLDER));
    expect(result.current.target).toEqual(asBack(OLDER));

    // The TTL memo is intact: a second hover inside the window costs no request.
    await act(async () => {
      await result.current.resolve();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(1);
  });
});

/**
 * [#303] The memo must not outlive the project state it describes.
 *
 * The request-id and identity guards sit on the fetch, but `resolve` returns from the
 * memo BEFORE reaching them - so with a key of direction + remote-ness alone, anything
 * that moved the copy (a chain step, a push, a revision swap - none of which unmount the
 * dropdown, because the project stays open) left a hover answering with the pre-swap
 * hexsha for the whole TTL, and that value goes straight into the confirmation dialog.
 */
describe('useUnwindTarget - the memo is bound to the project', () => {
  it('re-fetches inside the TTL once the project has moved', async () => {
    invokeIpcMock.mockResolvedValueOnce({ hexsha: 'before', is_current: false, discards_unpushed: false });
    const { result } = renderHook(() => useUnwindTarget(false, 'back'));

    await act(async () => {
      await result.current.resolve();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(1);

    // Same key inputs, so without the identity term this second call is a memo hit.
    await act(async () => {
      await result.current.resolve();
    });
    expect(invokeIpcMock).toHaveBeenCalledTimes(1);

    // A swap moves the copy without unmounting anything.
    invokeIpcMock.mockResolvedValueOnce({ hexsha: 'after', is_current: false, discards_unpushed: false });
    act(() => {
      const current = useProjectStore.getState().project as Record<string, unknown> | null;
      useProjectStore.setState({
        project: { ...(current ?? {}), modified_at: 'moved' } as never,
      });
    });

    const fresh = await act(async () => result.current.resolve());
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
    expect(fresh?.hexsha).toBe('after');
  });

  it('re-fetches after an in-place edit with unchanged project metadata', async () => {
    invokeIpcMock
      .mockResolvedValueOnce(OLDER)
      .mockResolvedValueOnce(NEWER);
    const { result } = renderTarget();

    await act(async () => {
      await result.current.resolve();
    });
    act(() => {
      useProjectStore.setState({ projectTreeGeneration: 2 });
    });

    let next: ChainTarget | null = null;
    await act(async () => {
      next = await result.current.resolve();
    });

    expect(next).toEqual(asBack(NEWER));
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
  });

  it('does not reuse a target resolved under an earlier login generation', async () => {
    invokeIpcMock
      .mockResolvedValueOnce(OLDER)
      .mockResolvedValueOnce(NEWER);
    const { result } = renderTarget();

    await act(async () => {
      await result.current.resolve();
    });
    act(() => {
      useSpeleoStore.setState({ sessionGeneration: 2 });
    });

    let next: ChainTarget | null = null;
    await act(async () => {
      next = await result.current.resolve();
    });

    expect(next).toEqual(asBack(NEWER));
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
  });

  it('re-fetches after a link-only chain change', async () => {
    invokeIpcMock.mockResolvedValueOnce(OLDER).mockResolvedValueOnce(NEWER);
    const { result } = renderTarget();

    await act(async () => {
      await result.current.resolve();
    });
    act(() => {
      useSpeleoStore.getState().invalidateChainTargets();
    });

    let next: ChainTarget | null = null;
    await act(async () => {
      next = await result.current.resolve();
    });

    expect(next).toEqual(asBack(NEWER));
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
  });
});
