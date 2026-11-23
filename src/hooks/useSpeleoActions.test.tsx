import { useSpeleoReadOnlyNotice } from '../utils/autoAcquireSpeleoLock';
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
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  // The one-arg call (humanizeSpeleoError) must still yield a string.
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'mock error',
}));

import { useSpeleoActions } from './useSpeleoActions';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import i18n from '../i18n';

let saveProjectMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  saveProjectMock = vi.fn(async () => ({ success: true }));
  // A georeferenced zero datum keeps noteNoAnchorIfNeeded quiet on the push paths.
  useProjectStore.setState({
    project: { file_path: '/remote.stalx', zero_datums: [{}] } as never,
    saveProject: saveProjectMock,
  });
  useSpeleoStore.setState({
    busy: false,
    applyDone: vi.fn(),
    applyPush: vi.fn(),
    clearSession: vi.fn(),
    raiseNoAnchorBanner: vi.fn(),
  });
  useUIStore.setState({ showToast: vi.fn() });
});

describe('useSpeleoActions - flush to disk after a confirmed push/unlock', () => {
  it('unlock (done → released) saves the project to disk (quiet autosave)', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(saveProjectMock).toHaveBeenCalledWith(undefined, true);
  });

  it('push (→ pushed) saves the project to disk (quiet autosave)', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'pushed' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push();
    });

    expect(saveProjectMock).toHaveBeenCalledWith(undefined, true);
  });

  it('does NOT flush on an unconfirmed push (no false-clean local state)', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'push_unconfirmed' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it('does NOT flush on a divergence result', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'divergence_needed' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push();
    });

    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it('a failed flush does not surface as a push failure (best-effort, stays quiet)', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
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
        },
    });
    invokeIpcMock.mockRejectedValue({
      message: 'Network error talking to SpeleoDB: Invalid argument',
      action: 'Retry',
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
    invokeIpcMock.mockResolvedValue({ kind: 'divergence_needed' });
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
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
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
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
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
    invokeIpcMock.mockResolvedValue({ kind: 'pushed' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push('m');
    });

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'speleodb-pushed', 'success');
  });

  it('a released lock asks for a success toast', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
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
    // The backend FF reload reopens the SAME path in place; without the explicit
    // refresh the old tree would stay on screen (Update appearing not to reset to
    // tip). Never through `loadProject`: that is a project switch, and its
    // `cancel_loading` ends the backend's edit session (sdbdemo2 issue 1).
    const loadProject = vi.fn(async () => ({ success: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ isDirty: false, loadProject, refreshProjectState });
    useSpeleoStore.setState({ link: { hasUnpushedEdits: false } as never, applyUpdate: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockImplementation(async (cmd) => cmd === 'speleodb_project_access_at' ? { release_context: {} } : { kind: 'fast_forwarded', local_path: '/x.stalx' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.update();
    });

    expect(loadProject).not.toHaveBeenCalled();
    expect(refreshProjectState).toHaveBeenCalled();
    expect(refreshLinkStatus).toHaveBeenCalled();
  });

  it('startEditing (→ fast_forwarded) refreshes geometry + link state', async () => {
    // The startEditing FF arm has the same in-place-reopen refresh as update; guard it
    // too (deleting its refresh calls would leave a stale tree after a Start-editing FF).
    const loadProject = vi.fn(async () => ({ success: true }));
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {
      useSpeleoStore.setState({ link: { editState: 'editing' } as never });
    });
    useProjectStore.setState({ loadProject, refreshProjectState });
    useSpeleoStore.setState({ applyStartEditing: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/y.stalx' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.startEditing();
    });

    expect(loadProject).not.toHaveBeenCalled();
    expect(refreshProjectState).toHaveBeenCalled();
    expect(refreshLinkStatus).toHaveBeenCalled();
  });

  /**
   * [#270] Both fast-forward arms ignored whether the reload landed and carried on.
   * A reply that arrives after the user opened another project (or signed in
   * again) describes a tree that is not on screen.
   *
   * `startEditing`'s own contract makes this load-bearing - `true` means "we hold
   * the lock", and `HeldLockResumeDialog` hands the shell over on it (discarding
   * every remaining lock), while `useRevertRevision` goes straight on to restore
   * + push. `refreshProjectState` has no staleness guard at all and overwrites
   * project/stations/surveyData wholesale, so it must not run either.
   */
  it('startEditing (→ fast_forwarded) refuses when the reload did not land', async () => {
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ refreshProjectState });
    useSpeleoStore.setState({ remoteProjectId: 'p1', applyStartEditing: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockImplementation(async () => {
      // Another project was opened while the download ran.
      useSpeleoStore.setState({ remoteProjectId: 'p2' });
      return { kind: 'fast_forwarded', local_path: '/y.stalx' };
    });
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
    const refreshProjectState = vi.fn(async () => {});
    const refreshLinkStatus = vi.fn(async () => {});
    useProjectStore.setState({ isDirty: false, refreshProjectState });
    useSpeleoStore.setState({ remoteProjectId: 'p1', link: { hasUnpushedEdits: false } as never, applyUpdate: vi.fn(), refreshLinkStatus });
    invokeIpcMock.mockImplementation(async (cmd) => {
      if (cmd === 'speleodb_project_access_at') return { release_context: {} };
      // Another project was opened while the download ran.
      useSpeleoStore.setState({ remoteProjectId: 'p2' });
      return { kind: 'fast_forwarded', local_path: '/x.stalx' };
    });
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
    expect(useSpeleoReadOnlyNotice.getState().notice?.message).toContain('other@example.com');
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
    // Reconciled in place, never through the project switch (sdbdemo2 issue 1).
    expect(loadProject).not.toHaveBeenCalled();

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
    let settlePush: (r: { kind: string }) => void = () => {};
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
      settlePush({ kind: 'pushed' });
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
    invokeIpcMock.mockResolvedValue({ kind: 'not_editing' });
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
    invokeIpcMock.mockResolvedValue({ kind: 'not_editing' });
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
    invokeIpcMock.mockResolvedValue({ kind: 'not_editing' });
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

  // The Start editing buttons (the map's read-only pill, the SpeleoDB tab, the held-lock
  // prompt) after an offline open (F1): the backend's DTO keeps the raw diagnostic in
  // `message` and the cause in `transport`. The notice is the worded line for the cause,
  // and no toast repeats it.
  it('a Start editing whose reply was lost is worded for the lost connection, not the raw error', async () => {
    useSpeleoReadOnlyNotice.setState({ notice: null });
    useSpeleoStore.setState({
      session: { loggedIn: true, userEmail: 'diver@example.com', endpoint: 'https://synthetic.example' },
    });
    invokeIpcMock.mockRejectedValue({
      message: 'Network error talking to SpeleoDB: error sending request', action: 'Retry', status: null,
      transport: 'Dropped',
    });
    const { result } = renderHook(() => useSpeleoActions());

    let held: boolean | undefined;
    await act(async () => {
      held = await result.current.startEditing();
    });

    expect(held).toBe(false);
    const message = useSpeleoReadOnlyNotice.getState().notice?.message ?? '';
    expect(message).toBe(
      'The connection to SpeleoDB at https://synthetic.example was lost before it finished answering. ' +
        'Check your network connection and try again.',
    );
    expect(message).not.toMatch(/network error|error sending request|could not connect/i);
    expect(useUIStore.getState().showToast).not.toHaveBeenCalled();
    expect(useSpeleoStore.getState().clearSession).not.toHaveBeenCalled();
  });

  // Demo items 3 and 7 (screenshots 04, 08): Resume editing on a project already
  // open. The notice is not "Project opened read-only" (nothing was opened), and
  // when the lock request may have landed and nothing could confirm it
  // (`lock_may_be_held`), it says the lock may be held and what happens to it, so
  // the "Couldn't hand back the lock" question at Close is not a surprise.
  it.each([
    [true, true],
    [undefined, false],
  ] as const)('a failed Resume editing is titled for what happened; lock_may_be_held %s says so: %s', async (flag, said) => {
    useSpeleoReadOnlyNotice.setState({ notice: null });
    useSpeleoStore.setState({
      session: { loggedIn: true, userEmail: 'diver@example.com', endpoint: 'https://synthetic.example' },
      // The last lock scan saw the lock free.
      remoteProjectId: 'p1', remoteLocks: { p1: null },
    });
    invokeIpcMock.mockRejectedValue({
      message: 'Network error talking to SpeleoDB: error sending request', action: 'Retry', status: null,
      transport: 'Dropped', ...(flag ? { lock_may_be_held: flag } : {}),
    });
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current.startEditing();
    });
    const notice = useSpeleoReadOnlyNotice.getState().notice;
    expect(notice?.kind).toBe('stillReadOnly');
    const sentence = 'The lock request may have reached SpeleoDB; if it did, Stalagmight hands the lock back '
      + 'when you close the project, or you can try again.';
    expect(notice?.message).toBe(
      'The connection to SpeleoDB at https://synthetic.example was lost before it finished answering. '
        + `Check your network connection and try again.${said ? ` ${sentence}` : ''}`,
    );
    // sdbdemo2 P3 (screenshot 07): a lock request that may have landed leaves no
    // "free when last checked" behind; one that surely did not keeps the scan.
    expect('p1' in useSpeleoStore.getState().remoteLocks).toBe(!said);
  });

  it('a Resume editing someone else is in the way of is titled for what happened too', async () => {
    useSpeleoReadOnlyNotice.setState({ notice: null });
    useSpeleoStore.setState({ applyStartEditing: vi.fn() });
    invokeIpcMock.mockResolvedValue({ kind: 'locked_by', email: 'other@example.com' });
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current.startEditing();
    });
    expect(useSpeleoReadOnlyNotice.getState().notice?.kind).toBe('stillReadOnly');
  });
});

/**
 * [#303] The guard that makes every push route safe at once.
 *
 * Placed in the hook rather than in each caller because there are three ways to reach a
 * push - the upload dialog (from the SpeleoDB tab, Save or a banner), the close prompt
 * and the quit prompt - and the trap was that one of them made it the primary action.
 * A guard per caller would have fixed that one and left the others.
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
    // Nothing is mid-flow on the SpeleoDB tab route: raising a return hop would put a
    // dialog the user had already left back on top of them.
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

it('does not apply or flush a done result after its close operation loses ownership', async () => {
  let resolve!: (value: unknown) => void;
  invokeIpcMock.mockReturnValue(new Promise((yes) => { resolve = yes; }));
  const { result } = renderHook(() => useSpeleoActions());
  let current = true;
  let pending!: Promise<unknown>;
  act(() => { pending = result.current.done('Synthetic update', { isCurrent: () => current }); });
  current = false;
  await act(async () => { resolve({ kind: 'released' }); await pending; });
  expect(useSpeleoStore.getState().applyDone).not.toHaveBeenCalled();
  expect(saveProjectMock).not.toHaveBeenCalled();
  expect(useSpeleoStore.getState().busy).toBe(false);
});

it('keeps edits made during an upload unpushed even if its reply says pushed', async () => {
  useSpeleoStore.setState({ link: null, session: null, remoteProjectId: 'synthetic' });
  let finish!: (value: { kind: 'pushed' }) => void;
  invokeIpcMock.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const { result } = renderHook(() => useSpeleoActions());
  let operation!: ReturnType<typeof result.current.push>;
  act(() => { operation = result.current.push('Synthetic snapshot'); });
  useProjectStore.setState((state) => ({ projectTreeGeneration: state.projectTreeGeneration + 1 }));
  let outcome;
  await act(async () => { finish({ kind: 'pushed' }); outcome = await operation; });
  expect(outcome).toEqual({ kind: 'push_unconfirmed' });
  expect(saveProjectMock).not.toHaveBeenCalled();
});
it('ignores a successful upload response after the project changes', async () => {
  useSpeleoStore.setState({ link: null, session: null, remoteProjectId: 'synthetic' });
  let finish!: (value: { kind: 'pushed' }) => void;
  invokeIpcMock.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const { result } = renderHook(() => useSpeleoActions());
  let operation!: ReturnType<typeof result.current.push>;
  act(() => { operation = result.current.push('Synthetic snapshot'); });
  useSpeleoStore.setState({ remoteProjectId: 'replacement' });
  let outcome;
  await act(async () => { finish({ kind: 'pushed' }); outcome = await operation; });
  expect(outcome).toBeNull();
  expect(saveProjectMock).not.toHaveBeenCalled();
});
it('manual refresh waits for fresh lock and permission status even when the revision is current', async () => {
  let finishStatus!: () => void;
  const refreshLinkStatus = vi.fn(() => new Promise<void>((resolve) => { finishStatus = resolve; }));
  useProjectStore.setState({ isDirty: false });
  useSpeleoStore.setState({ link: { hasUnpushedEdits: false } as never, applyUpdate: vi.fn(), refreshLinkStatus });
  invokeIpcMock.mockImplementation(async (command) => command === 'speleodb_project_access_at' ? { release_context: {} } : { kind: 'up_to_date' });
  const { result } = renderHook(() => useSpeleoActions());
  let request!: Promise<void>;
  await act(async () => { request = result.current.update(); });
  expect(refreshLinkStatus).toHaveBeenCalledOnce();
  expect(useSpeleoStore.getState().busy).toBe(true);
  await act(async () => {
    finishStatus();
    await request;
  });
  expect(useSpeleoStore.getState().busy).toBe(false);
});

// The backend hands no lock back on Take remote (demo item 2): an edit session
// carries over the reload, a read-only one stays read-only. The frontend reopens
// without acquiring (data#3) and takes the edit state from the re-read status.
//
// sdbdemo2 issues 1 and 2: the real app stayed read-only after Take the SpeleoDB
// version and after a Resume editing that downloaded a newer revision first. The
// frontend reconciled the in-place reload through `loadProject`, a project
// switch whose `cancel_loading` moves the backend's session on - which ends the
// edit session the reload had just carried or started. The model below is that
// backend: a project switch ends the bound edit session, and the link status
// reports the session as it stands.
describe('useSpeleoActions - an in-place reload keeps the edit session (sdbdemo2 issues 1, 2)', () => {
  const backend = (editing: boolean) => {
    const model = { bound: editing };
    // A project switch: `loadProject` runs `cancel_loading`, which ends the
    // backend's edit session (the lock itself stays held on SpeleoDB).
    const loadProject = vi.fn(async () => {
      model.bound = false;
      return { success: true };
    });
    const refreshLinkStatus = vi.fn(async () => {
      useSpeleoStore.setState({
        link: { editState: model.bound ? 'editing' : 'read-only', hasUnpushedEdits: false } as never,
      });
    });
    useProjectStore.setState({ isDirty: false, loadProject, refreshProjectState: vi.fn(async () => {}) });
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      link: { editState: editing ? 'editing' : 'read-only', hasUnpushedEdits: true } as never,
      refreshLinkStatus,
      applyStartEditing: vi.fn((result: { kind: string }) => {
        if (result.kind === 'editing' || result.kind === 'fast_forwarded') {
          useSpeleoStore.setState({ link: { editState: 'editing', hasUnpushedEdits: false } as never });
        }
      }),
    });
    useSpeleoReadOnlyNotice.setState({ notice: null });
    return { model, loadProject, refreshLinkStatus };
  };

  it('Take remote while editing goes on editing the SpeleoDB version', async () => {
    const { loadProject, refreshLinkStatus } = backend(true);
    invokeIpcMock.mockImplementation(async (cmd) => (cmd === 'speleodb_take_remote' ? '/remote.stalx' : null));
    const { result } = renderHook(() => useSpeleoActions());

    let took: boolean | undefined;
    await act(async () => {
      took = await result.current.takeRemote();
    });

    expect(took).toBe(true);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_take_remote');
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_start_editing');
    expect(loadProject).not.toHaveBeenCalled();
    expect(refreshLinkStatus).toHaveBeenCalled();
    expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    expect(useSpeleoReadOnlyNotice.getState().notice).toBeNull();
  });

  it('Take remote from a read-only project stays read-only, with no notice', async () => {
    backend(false);
    invokeIpcMock.mockImplementation(async (cmd) => (cmd === 'speleodb_take_remote' ? '/remote.stalx' : null));
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.takeRemote();
    });

    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_start_editing');
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(useSpeleoReadOnlyNotice.getState().notice).toBeNull();
  });

  it('Take remote that comes back read-only from an edit session says so', async () => {
    const { model } = backend(true);
    invokeIpcMock.mockImplementation(async (cmd) => {
      // The session ended meanwhile (a lost-lock signal): the reload carries nothing.
      model.bound = false;
      return cmd === 'speleodb_take_remote' ? '/remote.stalx' : null;
    });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.takeRemote();
    });

    const notice = useSpeleoReadOnlyNotice.getState().notice;
    expect(notice?.kind).toBe('stillReadOnly');
    expect(notice?.message).toBe(i18n.t('speleodb.readOnlyNotice.reloadedNotEditing'));
  });

  it('one Resume editing click on a copy behind downloads, takes the lock and edits', async () => {
    const { model, loadProject } = backend(false);
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockImplementation(async (cmd) => {
      if (cmd !== 'speleodb_start_editing') return null;
      // The backend downloaded the newer revision, reopened it in place and
      // bound the edit session to it.
      model.bound = true;
      return { kind: 'fast_forwarded', local_path: '/remote.stalx' };
    });
    const { result } = renderHook(() => useSpeleoActions());

    let held: boolean | undefined;
    await act(async () => {
      held = await result.current.startEditing();
    });

    expect(held).toBe(true);
    expect(loadProject).not.toHaveBeenCalled();
    expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    expect(useSpeleoReadOnlyNotice.getState().notice).toBeNull();
    expect(showToast).toHaveBeenCalledWith(i18n.t('speleodb.editBar.toasts.editing'), 'speleodb-editing', 'success');
  });

  it('a Resume editing whose reload ends read-only says so instead of nothing', async () => {
    backend(false);
    invokeIpcMock.mockImplementation(async (cmd) => (
      // The backend reports the reload, but the re-read status says read-only.
      cmd === 'speleodb_start_editing' ? { kind: 'fast_forwarded', local_path: '/remote.stalx' } : null
    ));
    const { result } = renderHook(() => useSpeleoActions());

    let held: boolean | undefined;
    await act(async () => {
      held = await result.current.startEditing();
    });

    expect(held).toBe(false);
    const notice = useSpeleoReadOnlyNotice.getState().notice;
    expect(notice?.kind).toBe('stillReadOnly');
    expect(notice?.message).toBe(i18n.t('speleodb.readOnlyNotice.reloadedNotEditing'));
  });
});

/**
 * R3 (same-account overwrite): SpeleoDB's lock is the account's, so another
 * session of the same account holds it too. The backend refuses an upload when
 * SpeleoDB's latest revision is not the copy's base and not an upload of this
 * session (`divergence_needed`, nothing uploaded); the frontend must route that
 * to the divergence prompt - never a success. A race lost after that check is
 * reported by the landed result (`replaced_revisions`), on the banner alone.
 */
describe('useSpeleoActions - same-account sessions (R3)', () => {
  const REPLACED = {
    id: '0123456789abcdef0123456789abcdef01234567',
    parent_ids: [],
    author_email: 'diver@example.com',
    authored_date: null,
    message: 'Surveyed the sump',
  };

  beforeEach(() => {
    // The REAL apply reducers: what the prompt and the banner read is the point.
    const real = useSpeleoStore.getInitialState();
    useSpeleoStore.setState({
      applyDone: real.applyDone,
      applyPush: real.applyPush,
      banner: null,
      replacedRevisions: null,
      divergence: null,
      link: {
        permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: 'diver@example.com', lockedByOther: false,
        hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date', noAccess: false,
        serverError: null,
      },
    });
  });

  it.each([
    ['Save Changes', 'push', 'speleodb_push'],
    ['Done (the close, quit and sign-out uploads)', 'done', 'speleodb_done'],
  ] as const)('%s refused over another session\'s revision opens the divergence prompt', async (_label, verb, command) => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue({ kind: 'divergence_needed' });
    const { result } = renderHook(() => useSpeleoActions());

    let outcome: unknown;
    await act(async () => {
      outcome = await result.current[verb]('From B');
    });

    expect(invokeIpcMock).toHaveBeenCalledWith(command, expect.objectContaining({ message: 'From B' }));
    expect(outcome).toEqual({ kind: 'divergence_needed' });
    const s = useSpeleoStore.getState();
    // The App-level effect opens `speleodb-divergence` on exactly this.
    expect(s.divergence).toBe(verb);
    // The work is still marked un-uploaded and the lock still held: nothing landed.
    expect(s.link?.hasUnpushedEdits).toBe(true);
    expect(s.link?.editState).toBe('editing');
    expect(showToast).not.toHaveBeenCalledWith(expect.anything(), expect.anything(), 'success');
    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it.each([
    ['push', { kind: 'pushed', replaced_revisions: [REPLACED] }],
    ['done', { kind: 'released', replaced_revisions: [REPLACED] }],
  ] as const)('a %s that replaced a revision pushed meanwhile says so on the banner only', async (verb, reply) => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue(reply);
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current[verb]('From B');
    });

    expect(useSpeleoStore.getState().replacedRevisions).toEqual([REPLACED]);
    // One event, one surface: no "Changes uploaded" toast beside it.
    expect(showToast).not.toHaveBeenCalled();
    // The upload landed, so the disk is flushed as after any landed upload.
    expect(saveProjectMock).toHaveBeenCalledWith(undefined, true);
  });

  // F4R3-locks-push-2: an edit lands after the backend took its snapshot and
  // before this reply: the push is shown as unconfirmed, but the upload did
  // land, and the backend gave its only copy of the report with it.
  // Injection: the downgrade replacing the whole result drops the report.
  it('a push downgraded for an edit during the reply keeps what it replaced', async () => {
    useProjectStore.setState({ projectTreeGeneration: 1 });
    invokeIpcMock.mockImplementation(async () => {
      useProjectStore.setState({ projectTreeGeneration: 2 });
      return { kind: 'pushed', replaced_revisions: [REPLACED] };
    });
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: unknown;
    await act(async () => {
      outcome = await result.current.push('From B');
    });
    expect(outcome).toEqual({ kind: 'push_unconfirmed', replaced_revisions: [REPLACED] });
    expect(useSpeleoStore.getState().banner).toEqual({ kind: 'push_unconfirmed' });
    expect(useSpeleoStore.getState().replacedRevisions).toEqual([REPLACED]);
  });

  // The Left item of the plan: a release with newer edits behind it raises its
  // push-failure line, and the replaced report stays beside it.
  it('a release with newer edits behind it keeps the replaced report beside its failure line', async () => {
    useProjectStore.setState({ projectTreeGeneration: 1 });
    invokeIpcMock.mockImplementation(async () => {
      useProjectStore.setState({ projectTreeGeneration: 2 });
      return { kind: 'released', replaced_revisions: [REPLACED] };
    });
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current.done('From B');
    });
    expect(useSpeleoStore.getState().banner?.kind).toBe('push_failed');
    expect(useSpeleoStore.getState().replacedRevisions).toEqual([REPLACED]);
  });

  // F4R3-locks-push-3: the upload landed and replaced a revision, then the
  // release (or the push's bookkeeping) failed. The failure carries the report.
  // Injection: the failure arm not raising `replacedRevisionsOf(e)` loses it.
  it.each(['push', 'done'] as const)('a %s failure after a landing names what it replaced', async (verb) => {
    invokeIpcMock.mockRejectedValue({
      message: 'boom', action: 'StayDirty', status: 500, transport: null, unpushed_local: null,
      newer_schema: null, replaced_revisions: [REPLACED],
    });
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current[verb]('From B');
    });
    expect(useSpeleoStore.getState().banner?.kind).toBe('push_failed');
    expect(useSpeleoStore.getState().replacedRevisions).toEqual([REPLACED]);
  });

  // F4R3-locks-push-8: the upload also left its undo history out to fit the
  // size limit, a fact of its own: said beside the replaced report.
  // Injection: the caveat toast inside `if (!replaced)` again is never shown.
  it.each([
    ['push', { kind: 'pushed', undo_history_omitted: true, replaced_revisions: [REPLACED] }, 'pushedWithoutHistory'],
    ['done', { kind: 'released', undo_history_omitted: true, replaced_revisions: [REPLACED] }, 'releasedWithoutHistory'],
  ] as const)('a %s that replaced a revision and left its undo history out says both', async (verb, reply, key) => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockResolvedValue(reply);
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current[verb]('From B');
    });
    expect(useSpeleoStore.getState().replacedRevisions).toEqual([REPLACED]);
    expect(showToast).toHaveBeenCalledWith(i18n.t(`speleodb.editBar.toasts.${key}`), expect.anything(), 'warning');
    expect(showToast).not.toHaveBeenCalledWith(expect.anything(), expect.anything(), 'success');
  });
});
