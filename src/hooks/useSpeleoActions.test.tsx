/**
 * Unit test - the SpeleoDB push/unlock verbs flush the survey to disk so the
 * projectStore's `isDirty` flag clears immediately.
 *
 * Regression guard: a confirmed push/unlock already persists the survey to disk
 * (the backend's speleodb_upload runs save_project before the upload), but the FE
 * `isDirty` flag is a separate concept. Left alone it only cleared on the 30s
 * autosave debounce, so exiting in that window fired a false "Unsaved Changes"
 * prompt for edits that were already on disk (and pushed to SpeleoDB). The verbs
 * now re-flush from the FE (saveProject) on the confirmed-success paths only.
 *
 * `.tsx` so this runs in the jsdom tier (renderHook).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

vi.mock('../utils/ipc');
// The one-arg call (humanizeSpeleoError) must still yield a string.
stubGetErrorMessage(fallbackOr('mock error'));

import { useSpeleoActions } from './useSpeleoActions';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../test/factories/ipc';

let saveProjectMock: ReturnType<typeof vi.fn>;

/** [#576 NF-W8FE-2] Done and Push answer the wire envelope, never a bare outcome. */
function report<R>(result: R): { result: R; save_warnings: [] } {
  return { result, save_warnings: [] };
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  saveProjectMock = vi.fn(async () => ({ success: true }));
  // A georeferenced zero datum keeps noteNoAnchorIfNeeded quiet on the push paths.
  useProjectStore.setState({
    project: { zero_datums: [{}] } as never,
    saveProject: saveProjectMock,
  });
  useSpeleoStore.setState({
    busy: false,
    applyDone: vi.fn(),
    applyPush: vi.fn(),
    bumpPoll: vi.fn(),
    clearSession: vi.fn(),
    raiseNoAnchorBanner: vi.fn(),
  });
  useUIStore.setState({ showToast: vi.fn() });
});

describe('useSpeleoActions - flush to disk after a confirmed push/unlock', () => {
  it('unlock (done → released) saves the project to disk (quiet autosave)', async () => {
    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(saveProjectMock).toHaveBeenCalledWith(undefined, true);
  });

  it('push (→ pushed) saves the project to disk (quiet autosave)', async () => {
    invokeIpcMock.mockResolvedValue(report({ kind: 'pushed' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push();
    });

    expect(saveProjectMock).toHaveBeenCalledWith(undefined, true);
  });

  it('does NOT flush on an unconfirmed push (no false-clean local state)', async () => {
    invokeIpcMock.mockResolvedValue(report({ kind: 'push_unconfirmed' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it('does NOT flush on a divergence result', async () => {
    invokeIpcMock.mockResolvedValue(report({ kind: 'divergence_needed' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push();
    });

    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it('a failed flush does not surface as a push failure (best-effort, stays quiet)', async () => {
    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    saveProjectMock.mockRejectedValue(new Error('disk full'));
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    const { result } = renderHook(() => useSpeleoActions());

    let outcome: unknown;
    await act(async () => {
      outcome = await result.current.done();
    });

    // The unlock still reports success and no error toast is raised - the flush
    // is best-effort and isDirty simply stays set (safe: the push already
    // persisted the same edits).
    expect(outcome).toEqual({ kind: 'released' });
    expect(showToast).not.toHaveBeenCalledWith(expect.anything(), 'speleodb-action-error', 'error');
  });
});

/**
 * #260 - a failed push/unlock keeps BOTH surfaces: the immediate error toast
 * (which auto-clears) and the durable `push_failed` banner carrying the same
 * humanized line plus the verb/message its Retry replays.
 */
describe('useSpeleoActions - a failed push/unlock raises the persistent banner (#260)', () => {
  beforeEach(() => {
    // Use the REAL banner reducers (the outer beforeEach only stubs the apply* verbs).
    useSpeleoStore.setState({ banner: null });
  });

  it('push failure raises push_failed with the push origin + message, and does NOT also toast', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push('my message');
    });

    expect(useSpeleoStore.getState().banner).toEqual({
      kind: 'push_failed',
      message: expect.any(String),
      origin: 'push',
      pushMessage: 'my message',
    });
    // One event, one surface (docs/notifications.md): the banner is the durable
    // surface (plus the dialogs' inline notice); the old extra toast is gone.
    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), 'speleodb-action-error', 'error');
  });

  it('done failure raises push_failed with the done origin', async () => {
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done(null);
    });

    expect(useSpeleoStore.getState().banner).toMatchObject({
      kind: 'push_failed',
      origin: 'done',
      pushMessage: null,
    });
  });

  it('a transport failure reaches the banner humanized, not as a raw errno (#270)', async () => {
    // What the user actually hit: the host blackholed at the routing level, so the
    // backend surfaced EINVAL. The DTO's typed `transport` cause is what the
    // humanizer keys off; the raw text must not reach the user.
    useSpeleoStore.setState({
      session: {
        loggedIn: true,
        userEmail: 'me@example.com',
        endpoint: 'https://cave.example.internal:8443/',
        keychainBacked: true,
        storage: 'keychain',
        keychain: 'available',
      },
    });
    invokeIpcMock.mockRejectedValue({
      message: 'Network error talking to SpeleoDB: Invalid argument',
      code: 'SPELEODB_RETRY',
      status: null,
      transport: 'Unknown',
    });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done('wrap up');
    });

    const banner = useSpeleoStore.getState().banner as { kind: string; message: string };
    expect(banner.kind).toBe('push_failed');
    expect(banner.message).not.toContain('Invalid argument');
    expect(banner.message).toContain('https://cave.example.internal:8443/');
  });

  it('a retry clears the stale failure banner before the new attempt lands', async () => {
    useSpeleoStore.setState({
      banner: { kind: 'push_failed', message: 'old', origin: 'push', pushMessage: null },
    });
    invokeIpcMock.mockResolvedValue(report({ kind: 'divergence_needed' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push();
    });

    // Even on a non-success outcome that leaves the banner slot alone, the previous
    // failure must not linger (it described an attempt that has been superseded).
    expect(useSpeleoStore.getState().banner).toBeNull();
  });
});

/**
 * #260 - "Changes uploaded and the write lock released." used to fire even when
 * nothing was uploaded. The remote dirty bit (`link.hasUnpushedEdits`) captured
 * BEFORE the call picks the wording.
 */
describe('useSpeleoActions - the release toast reflects whether anything was uploaded (#260)', () => {
  const seedLink = (hasUnpushedEdits: boolean) =>
    useSpeleoStore.setState({
      link: {
        permission: 'READ_AND_WRITE',
        editState: 'editing',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits,
        unwound: false,
        forceEditing: false,
        forceEditAllowed: false,
        undoMode: 'full',
        remoteState: 'up_to_date',
        noAccess: false,
        serverError: null,
      },
    });

  it('says the changes were uploaded when there were unpushed edits', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    seedLink(true);
    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(showToast).toHaveBeenCalledWith(
      'Changes uploaded and the write lock released.',
      'speleodb-released',
      'success',
    );
  });

  it('says nothing was uploaded when the session had no unpushed edits', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    seedLink(false);
    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(showToast).toHaveBeenCalledWith(
      'Write lock released. Nothing to upload.',
      'speleodb-released',
      'success',
    );
  });
});

/**
 * The toast KIND is the whole point of the accent rework: before it, a
 * successful push and a failed one rendered identically, so a landed upload gave
 * no perceptible confirmation. These pin the semantics at the source.
 */
describe('useSpeleoActions - toast kinds', () => {
  it('a landed push asks for a success toast', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue(report({ kind: 'pushed' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push('m');
    });

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'speleodb-pushed', 'success');
  });

  it('a released lock asks for a success toast', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'speleodb-released', 'success');
  });

  it('a failed unlock raises the #260 banner without an extra toast', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), 'speleodb-action-error', 'error');
    expect(useSpeleoStore.getState().banner).toMatchObject({ kind: 'push_failed' });
  });
});

describe('useSpeleoActions - fast-forward resets the on-screen tree (#218)', () => {
  it('does not route a start-editing reply after the open project changes', async () => {
    let finishRequest!: (value: { kind: 'editing' }) => void;
    invokeIpcMock.mockReturnValue(
      new Promise<{ kind: 'editing' }>((resolve) => {
        finishRequest = resolve;
      }),
    );
    const applyStartEditing = vi.fn();
    useSpeleoStore.setState({ remoteProjectId: 'p1', applyStartEditing });
    const { result } = renderHook(() => useSpeleoActions());
    let pending!: Promise<boolean>;
    act(() => {
      pending = result.current.startEditing();
    });
    await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalled());

    useSpeleoStore.setState({ remoteProjectId: 'p2' });
    finishRequest({ kind: 'editing' });

    let held: boolean | undefined;
    await act(async () => {
      held = await pending;
    });
    expect(held).toBe(false);
    expect(applyStartEditing).not.toHaveBeenCalled();
  });

  it('update (→ fast_forwarded) refreshes geometry + link state', async () => {
    // The backend FF reload reopens the SAME path, so loadProject short-circuits on
    // the "already open" guard; without the explicit refresh the old tree would stay
    // on screen (Update appearing not to reset to tip).
    const loadProject = vi.fn(async () => ({ success: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ loadProject, refreshProjectState, checkProjectIntegrity: vi.fn(async () => []) });
    useSpeleoStore.setState({ applyUpdate: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/x.stalx' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.update();
    });

    expect(loadProject).toHaveBeenCalledWith('/x.stalx');
    expect(refreshProjectState).toHaveBeenCalled();
    expect(refreshLinkStatus).toHaveBeenCalled();
  });

  it('startEditing (→ fast_forwarded) refreshes geometry + link state', async () => {
    // The startEditing FF arm has the same in-place-reopen refresh as update; guard it
    // too (deleting its refresh calls would leave a stale tree after a Start-editing FF).
    const loadProject = vi.fn(async () => ({ success: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ loadProject, refreshProjectState, checkProjectIntegrity: vi.fn(async () => []) });
    useSpeleoStore.setState({ applyStartEditing: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/y.stalx' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.startEditing();
    });

    expect(loadProject).toHaveBeenCalledWith('/y.stalx');
    expect(refreshProjectState).toHaveBeenCalled();
    expect(refreshLinkStatus).toHaveBeenCalled();
  });

  /**
   * [#270] Both fast-forward arms ignored `loadProject`'s answer and carried on.
   * `takeRemote` has read it since it was written, for the reason that applies
   * here too: `loadProject` reports `success: false` when a NEWER load superseded
   * it, so the swapped tree is not what is on screen.
   *
   * `startEditing`'s own contract makes this load-bearing - `true` means "we hold
   * the lock", and `HeldLockResumeDialog` hands the shell over on it (discarding
   * every remaining lock), while `useRevertRevision` goes straight on to restore
   * + push. `refreshProjectState` has no staleness guard at all and overwrites
   * project/stations/surveyData wholesale, so it must not run either.
   */
  it('startEditing (→ fast_forwarded) refuses when the reload did not land', async () => {
    const loadProject = vi.fn(async () => ({ success: false, superseded: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ loadProject, refreshProjectState, checkProjectIntegrity: vi.fn(async () => []) });
    useSpeleoStore.setState({ applyStartEditing: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/y.stalx' });
    const { result } = renderHook(() => useSpeleoActions());

    let held: boolean | undefined;
    await act(async () => {
      held = await result.current.startEditing();
    });

    expect(held).toBe(false);
    expect(refreshProjectState).not.toHaveBeenCalled();
    expect(refreshLinkStatus).not.toHaveBeenCalled();
  });

  it('update (→ fast_forwarded) stops when the reload did not land', async () => {
    const loadProject = vi.fn(async () => ({ success: false, superseded: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ loadProject, refreshProjectState, checkProjectIntegrity: vi.fn(async () => []) });
    useSpeleoStore.setState({ applyUpdate: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/x.stalx' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.update();
    });

    expect(refreshProjectState).not.toHaveBeenCalled();
    expect(refreshLinkStatus).not.toHaveBeenCalled();
  });
});

describe('useSpeleoActions - discard routes every StartEditingResult (#303)', () => {
  const setup = () => {
    const loadProject = vi.fn(async () => ({ success: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    const applyStartEditing = vi.fn();
    const checkHistoryDiscarded = vi.fn(async () => {});
    useProjectStore.setState({
      loadProject,
      refreshProjectState,
      checkProjectIntegrity: vi.fn(async () => []),
      checkHistoryDiscarded,
    });
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      applyStartEditing,
      refreshLinkStatus,
    });
    return {
      loadProject,
      refreshProjectState,
      refreshLinkStatus,
      applyStartEditing,
      checkHistoryDiscarded,
    };
  };

  /**
   * [#473] A released discard still REPLACED the tree, so its history must be re-read.
   *
   * `speleodb_discard_changes` fast-forwards to the server tip - "throwing away whatever
   * it held" - and only THEN decides whether to hand the lock back
   * (`speleodb_push.rs::speleodb_discard_changes`). So a `null` result means "released
   * AND swapped", and an earlier version of this call site read it as "nothing happened"
   * and skipped the re-read on the one discard flow that actually changes the bytes:
   * no banner when the tip's history is unreadable, and a stale banner from the discarded
   * copy when it is fine.
   */
  it('re-reads the discarded-history flag on a released discard', async () => {
    const { checkHistoryDiscarded } = setup();
    invokeIpcMock.mockResolvedValue(null);
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.discardChanges(false);
    });

    expect(checkHistoryDiscarded).toHaveBeenCalled();
  });

  it('distinguishes a released discard from a kept edit session', async () => {
    const { applyStartEditing } = setup();
    invokeIpcMock.mockResolvedValue(null);
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: Awaited<ReturnType<typeof result.current.discardChanges>> | undefined;

    await act(async () => {
      outcome = await result.current.discardChanges(false);
    });

    expect(outcome).toBe('released');
    expect(applyStartEditing).not.toHaveBeenCalled();
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_discard_changes', {
      keepLock: false,
      acknowledged: true,
    });
  });

  it('applies the ordinary editing result after keeping the lock', async () => {
    const { applyStartEditing, refreshProjectState } = setup();
    const editing = { kind: 'editing' } as const;
    invokeIpcMock.mockResolvedValue(editing);
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: Awaited<ReturnType<typeof result.current.discardChanges>> | undefined;

    await act(async () => {
      outcome = await result.current.discardChanges(true);
    });

    expect(outcome).toBe('editing');
    expect(applyStartEditing).toHaveBeenCalledWith(editing);
    expect(refreshProjectState).toHaveBeenCalled();
  });

  it('applies lock contention instead of pretending the lock was kept', async () => {
    const { applyStartEditing } = setup();
    const locked = { kind: 'locked_by', email: 'other@example.com' } as const;
    invokeIpcMock.mockResolvedValue(locked);
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: Awaited<ReturnType<typeof result.current.discardChanges>> | undefined;

    await act(async () => {
      outcome = await result.current.discardChanges(true);
    });

    expect(outcome).toBe('discarded_without_lock');
    expect(applyStartEditing).toHaveBeenCalledWith(locked);
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(
      expect.any(String),
      'speleodb-locked-by',
      'warning',
    );
  });

  it('applies divergence and reloads a fast-forwarded result', async () => {
    const { applyStartEditing, loadProject } = setup();
    const fastForwarded = { kind: 'fast_forwarded', local_path: '/tip.stalx' } as const;
    invokeIpcMock.mockResolvedValue(fastForwarded);
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: Awaited<ReturnType<typeof result.current.discardChanges>> | undefined;

    await act(async () => {
      outcome = await result.current.discardChanges(true);
    });

    expect(outcome).toBe('editing');
    expect(applyStartEditing).toHaveBeenCalledWith(fastForwarded);
    expect(loadProject).toHaveBeenCalledWith('/tip.stalx');

    const divergence = { kind: 'divergence_needed' } as const;
    invokeIpcMock.mockResolvedValueOnce(divergence);
    await act(async () => {
      outcome = await result.current.discardChanges(true);
    });
    expect(outcome).toBe('discarded_without_lock');
    expect(applyStartEditing).toHaveBeenCalledWith(divergence);
  });

  it('does not route a discard reply after the open project changes', async () => {
    const { applyStartEditing, refreshProjectState } = setup();
    let finishRequest!: (value: { kind: 'editing' }) => void;
    invokeIpcMock.mockReturnValue(
      new Promise<{ kind: 'editing' }>((resolve) => {
        finishRequest = resolve;
      }),
    );
    const { result } = renderHook(() => useSpeleoActions());
    let pending!: ReturnType<typeof result.current.discardChanges>;
    act(() => {
      pending = result.current.discardChanges(true);
    });
    await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalled());

    useSpeleoStore.setState({ remoteProjectId: 'p2' });
    finishRequest({ kind: 'editing' });

    let outcome: Awaited<typeof pending> | undefined;
    await act(async () => {
      outcome = await pending;
    });
    expect(outcome).toBe('stale');
    expect(applyStartEditing).not.toHaveBeenCalled();
    expect(refreshProjectState).not.toHaveBeenCalled();
  });
});

/**
 * [#270] `busy` is ONE shared boolean, and `clearLink` (project close/switch) and
 * `restoreLink` (a rolled-back remote open) both reset it out of band. A verb
 * whose call was still in flight then cleared the flag for a NEWER verb on a
 * NEWER project. That is a data-safety seam, not cosmetics: `SpeleoQuitDialog` /
 * `SpeleoCloseDialog` refuse to close while busy because dismissing mid-push lets
 * the in-flight release resolve and call `performQuit()`, whose one-shot flag
 * makes the resulting close skip `guardQuit` entirely.
 */
describe('useSpeleoActions - the shared busy flag is ownership-checked (#270)', () => {
  it("a stale verb finishing does not clear a newer verb's busy flag", async () => {
    useSpeleoStore.setState({ applyPush: vi.fn(), applyStartEditing: vi.fn() });
    // Both calls hang until we settle them by hand, so the two verbs genuinely
    // overlap the way they do when a close lands mid-push.
    let settlePush: (r: { result: { kind: string }; save_warnings: [] }) => void = () => {};
    let settleStart: (r: { kind: string }) => void = () => {};
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_push') return new Promise((res) => { settlePush = res; });
      return new Promise((res) => { settleStart = res; });
    });
    const { result } = renderHook(() => useSpeleoActions());

    let pushDone: Promise<unknown> = Promise.resolve();
    let startDone: Promise<unknown> = Promise.resolve();
    await act(async () => {
      pushDone = result.current.push('m');
    });
    expect(useSpeleoStore.getState().busy).toBe(true);

    // The project closes under it (close/switch resets the link AND the flag) ...
    act(() => {
      useSpeleoStore.getState().clearLink();
    });
    expect(useSpeleoStore.getState().busy).toBe(false);

    // ... and a verb for the NEXT project claims the flag.
    await act(async () => {
      startDone = result.current.startEditing();
    });
    expect(useSpeleoStore.getState().busy).toBe(true);

    // Now the abandoned push lands. Its `finally` must NOT clear a flag it no
    // longer owns - doing so re-opened the mid-push dismiss route above.
    await act(async () => {
      settlePush(report({ kind: 'pushed' }));
      await pushDone;
    });
    expect(useSpeleoStore.getState().busy).toBe(true);

    // The owner's own release still works.
    await act(async () => {
      settleStart({ kind: 'editing' });
      await startDone;
    });
    expect(useSpeleoStore.getState().busy).toBe(false);
  });
});

// #270: `speleodb_push_only` / `speleodb_done` answer `not_editing` when the edit
// gate was already closed - nothing was uploaded and nothing was released. Reported
// as Pushed/Released, that produced a green "Changes pushed to SpeleoDB." toast, a
// disk flush and a cleared dirty bit for work that never left this computer. It is
// reachable in normal use: a 401 auto-drop closes the gate but keeps the dirty bit,
// and the push-failure banner's Retry re-issues the verb from exactly that state.
describe('useSpeleoActions - a no-op push/release is reported honestly (#270)', () => {
  it('push (→ not_editing) warns instead of claiming success, and does not flush', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue(report({ kind: 'not_editing' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push('m');
    });

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'speleodb-not-editing', 'warning');
    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), 'speleodb-pushed', 'success');
    // The flush exists to clear `isDirty` after a CONFIRMED upload; nothing was
    // uploaded, so a false-clean local state must not be manufactured.
    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it('done (→ not_editing) warns, and the caller sees a kind that is not "released"', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue(report({ kind: 'not_editing' }));
    const { result } = renderHook(() => useSpeleoActions());

    let resolved: { kind: string } | null = null;
    await act(async () => {
      resolved = (await result.current.done()) as { kind: string } | null;
    });

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'speleodb-not-editing', 'warning');
    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), 'speleodb-released', 'success');
    // The close/quit dialogs only leave on `released`, so they correctly stay open.
    expect(resolved).toEqual({ kind: 'not_editing' });
    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  /**
   * [#270] Every attempt clears the durable failure banner on its way in, because
   * a fresh attempt supersedes the old error. `not_editing` neither lands nor
   * throws, so nothing re-raised it and the only surviving trace was a 3 s toast -
   * and inside `SpeleoDoneDialog` (which stays open on any kind but `released`)
   * the inline notice went blank too, leaving the same button with no error text.
   * The way in is exactly the documented one: a 401 auto-drop closes the edit gate
   * and the banner's own Retry re-issues the verb from there.
   */
  it.each([
    ['push', 'push' as const],
    ['done', 'done' as const],
  ])('%s (→ not_editing) keeps a durable surface, with the message it was given', async (verb, origin) => {
    useSpeleoStore.setState({
      banner: { kind: 'push_failed', message: 'the old error', origin, pushMessage: 'my notes' },
    });
    invokeIpcMock.mockResolvedValue(report({ kind: 'not_editing' }));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      if (verb === 'push') await result.current.push('my notes');
      else await result.current.done('my notes');
    });

    expect(useSpeleoStore.getState().banner).toEqual({
      kind: 'push_failed',
      // Re-raised with what actually happened, not the stale line from before.
      message: expect.stringContaining('Nothing was uploaded'),
      origin,
      // Retry replays the commit message the user typed.
      pushMessage: 'my notes',
    });
  });
});

// #270: `startEditing(): Promise<boolean>` had all four known variants handled but no
// exhaustiveness guard, so a fifth backend variant fell through to `return true` -
// "we hold the lock". `HeldLockResumeDialog` acts on that by calling
// `clearHeldLocks()`, discarding every remaining lock with no way to re-raise the
// prompt until the next login scan.
describe('useSpeleoActions - startEditing falls to the REFUSING answer (#270)', () => {
  it('answers false for a variant it does not recognise', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    useSpeleoStore.setState({ applyStartEditing: vi.fn() });
    invokeIpcMock.mockResolvedValue({ kind: 'something_new' });
    const { result } = renderHook(() => useSpeleoActions());

    let held: boolean | undefined;
    await act(async () => {
      held = await result.current.startEditing();
    });

    expect(held).toBe(false);
    warn.mockRestore();
  });

  it('still answers true when we really do hold the lock', async () => {
    useSpeleoStore.setState({ applyStartEditing: vi.fn() });
    invokeIpcMock.mockResolvedValue({ kind: 'editing' });
    const { result } = renderHook(() => useSpeleoActions());

    let held: boolean | undefined;
    await act(async () => {
      held = await result.current.startEditing();
    });

    expect(held).toBe(true);
  });
});

/**
 * [#303] The guard that makes every push route safe at once.
 *
 * Placed in the hook rather than in each caller because there are four ways to reach a
 * push - the status button, the SpeleoDB menu, the close prompt and the quit prompt - and
 * the trap was that one of them made it the primary action. A guard per caller would have
 * fixed that one and left the others.
 */
describe('useSpeleoActions - pushing an unwound copy is confirmed first (#303)', () => {
  const seedUnwound = (unwoundNotEdited: boolean) =>
    useSpeleoStore.setState({
      link: {
        permission: 'READ_AND_WRITE',
        editState: 'editing',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits: true,
        unwound: unwoundNotEdited,
        forceEditing: false,
        forceEditAllowed: false,
        undoMode: 'full',
        remoteState: 'up_to_date',
        noAccess: false,
        serverError: null,
      },
    });

  it('raises the confirmation instead of uploading', async () => {
    seedUnwound(true);
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog });
    const { result } = renderHook(() => useSpeleoActions());

    let outcome: unknown;
    await act(async () => {
      outcome = await result.current.done('msg');
    });

    expect(openDialog).toHaveBeenCalledWith('speleodb-unwound-push', { message: 'msg' });
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_done', expect.anything());
    // Null, not a result: the close prompt only leaves once a push RELEASES, so a
    // fabricated outcome here would let it walk away mid-confirmation.
    expect(outcome).toBeNull();
  });

  it('uploads once the confirmation reports the user chose it', async () => {
    seedUnwound(true);
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done('msg', { acceptOlderVersion: true });
    });

    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_done', {
      message: 'msg',
      acceptOlderVersion: true,
    });
  });

  it('leaves an ordinary dirty copy alone - the guard is not a nag', async () => {
    seedUnwound(false);
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done('msg');
    });

    expect(openDialog).not.toHaveBeenCalled();
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_done', {
      message: 'msg',
      acceptOlderVersion: false,
    });
  });

  it('guards push-without-release too', async () => {
    seedUnwound(true);
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      expect(await result.current.push('msg')).toBeNull();
    });

    expect(openDialog).toHaveBeenCalledWith('speleodb-unwound-push', {
      message: 'msg',
      mode: 'push',
    });
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_push', expect.anything());
  });

  /**
   * [#303] Answering `null` is only safe for a caller that stays put. The exit prompts do
   * not: they are already evicted from uiStore's single `activeDialog` slot by the time
   * they read it, so the quit or close the user asked for went with them - the push then
   * happened from inside the confirmation and the app carried on running regardless.
   * `pendingExit` is the intent riding along so the confirmation can finish it.
   */
  it('carries an exit caller intent, and the return hop back to its prompt', async () => {
    seedUnwound(true);
    const openDialog = vi.fn();
    useUIStore.setState({
      openDialog,
      activeDialog: 'speleodb-quit',
      dialogData: { cancelReturnsTo: 'dmp-selection' },
    });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done(null, { pendingExit: { kind: 'quit' } });
    });

    expect(openDialog).toHaveBeenCalledWith('speleodb-unwound-push', {
      message: null,
      pendingExit: { kind: 'quit' },
      // The displaced prompt AND its own data, so cancelling twice walks back to the
      // import wizard the quit guard displaced rather than stopping one hop short.
      cancelReturnsTo: 'speleodb-quit',
      cancelReturnsData: { cancelReturnsTo: 'dmp-selection' },
    });
  });

  it('records no return hop for the routes with no exit waiting', async () => {
    // The status button and the lock menu are pressable with another dialog on screen,
    // and nothing is mid-flow there: raising a return hop would put a dialog the user
    // had already left back on top of them.
    seedUnwound(true);
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog, activeDialog: 'speleodb-done', dialogData: {} });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done(null);
    });

    expect(openDialog).toHaveBeenCalledWith('speleodb-unwound-push', { message: null });
  });
});

/**
 * #576 RV4-FE-SP4 / RV4-R-NITS S-8, consistent with RV2-R-15: Discard changes and Take
 * remote replace the survey like a Restore does, so the read-only Plan and Export
 * workspaces refuse them before any IPC, with the notice a refused edit gets, instead of
 * reaching the backend (and, for Take remote, re-pulling) first.
 */
describe('useSpeleoActions - survey swaps refused in the Plan/Export workspace (#576 RV4-FE-SP4)', () => {
  afterEach(() => {
    useUIStore.setState({ appMode: 'survey' });
  });
  const notice = 'Plan and Export are read-only - switch to Survey to edit the survey.';

  it('Discard changes in Plan refuses before the IPC', async () => {
    useUIStore.setState({ appMode: 'plan' });
    invokeIpcMock.mockResolvedValue(null);
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: Awaited<ReturnType<typeof result.current.discardChanges>> | undefined;

    await act(async () => {
      outcome = await result.current.discardChanges(true);
    });

    const commands = invokeIpcMock.mock.calls.map((c) => c[0]);
    console.log(`[SP4] discard in plan: outcome=${outcome} ipc=${JSON.stringify(commands)}`);
    expect(outcome).toBe('workspace_read_only');
    expect(commands).toEqual([]);
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(notice, 'edit-disabled', 'warning');
  });

  it('Take remote in Export refuses before the IPC', async () => {
    useUIStore.setState({ appMode: 'export' });
    invokeIpcMock.mockResolvedValue('/cache/p1.stalx');
    const { result } = renderHook(() => useSpeleoActions());
    let took: Awaited<ReturnType<typeof result.current.takeRemote>> | undefined;

    await act(async () => {
      took = await result.current.takeRemote();
    });

    const commands = invokeIpcMock.mock.calls.map((c) => c[0]);
    console.log(`[SP4] take remote in export: took=${took} ipc=${JSON.stringify(commands)}`);
    // [#576 NF-FIX4FE-1] Reported as the refusal it is, so the dialog can close.
    expect(took).toBe('workspace_read_only');
    expect(commands).toEqual([]);
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(notice, 'edit-disabled', 'warning');
  });
});
