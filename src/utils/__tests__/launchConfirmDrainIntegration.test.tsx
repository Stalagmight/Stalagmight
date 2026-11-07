// @vitest-environment jsdom
/**
 * #445 review: does confirming a swap with ANOTHER file still `deferred` hold
 * the drain until the confirmed route finishes?
 *
 * Exercises the real `launchWithFile.ts` + `uiStore.ts` + `openOrImport.ts` +
 * `LaunchOpenConfirmDialog` (with real i18n) together - only the backend IPC,
 * the Tauri event listener, `loadProject`, and DialogWrapper/Button are
 * mocked - so the regression this pins (closing the dialog BEFORE awaiting
 * the confirmed route let the "resume the next deferred path" subscriber
 * race it, per LaunchOpenConfirmDialog.tsx's own comment on `handleConfirm`)
 * can't hide behind a mock standing in for any of the real pieces that
 * interact.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { renderHook } from '@testing-library/react';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn().mockResolvedValue(null) }));

vi.mock('../ipc');
stubGetErrorMessage(fallbackOr('error'));

const listenMock = vi.hoisted(() => vi.fn(async () => () => {}));
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

vi.mock('../../components/Dialogs/DialogWrapper', async () =>
  (await import('../../test/factories/components')).dialogWrapperStub(),
);

vi.mock('../../components/common/Button', async () => (await import('../../test/factories/components')).buttonStub());

import { useLaunchWithFile, resetLaunchFileQueueForTest } from '../launchWithFile';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import LaunchOpenConfirmDialog from '../../components/Dialogs/LaunchOpenConfirmDialog';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

let queued: string[] = [];
let loadProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  resetLaunchFileQueueForTest();
  queued = [];
  loadProject = vi.fn(async () => ({ success: true }));
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, screenOccupants: 0 });
  useProjectStore.setState({ project: { name: 'Open' } as never, isDirty: false, loadProject } as never);
  useSpeleoStore.setState({ link: null });
  listenMock.mockImplementation(async () => () => {});
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd !== 'take_pending_launch_files') throw new Error(`unexpected command ${cmd}`);
    const batch = queued;
    queued = [];
    return batch;
  });
});

describe('the swap-confirm holds the launch drain for its whole flow', () => {
  it('does not route the second queued .stalx until the confirmed first one finishes', async () => {
    queued = ['/dives/one.stalx', '/dives/two.stalx'];
    renderHook(() => useLaunchWithFile(true));

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm'));
    expect(useUIStore.getState().dialogData.path).toBe('/dives/one.stalx');
    expect(loadProject).not.toHaveBeenCalled();

    // loadProject hangs so the test can observe the window where the old,
    // buggy close-before-route ordering let the resume-drain jump ahead.
    let resolveLoad: () => void = () => {};
    loadProject.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveLoad = () => resolve({ success: true });
        }),
    );

    render(<LaunchOpenConfirmDialog />);
    fireEvent.click(screen.getByTestId('launch-open-confirm-open'));

    await waitFor(() => expect(loadProject).toHaveBeenCalledWith('/dives/one.stalx'));

    // The confirmed route is still in flight - the dialog slot (and so
    // `screenBusy()`) must still read busy, holding the second path. Checking
    // `dialogData.path` (not just that SOME dialog is up) is what actually
    // catches the regression: under the old close-before-route ordering, the
    // resume-drain fires while path 1's load is still pending, reaches path 2,
    // and raises a FRESH confirm for it - which still leaves `activeDialog`
    // at `launch-open-confirm`, just with `dialogData` silently clobbered to
    // describe path 2 while path 1 is still loading underneath it.
    expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm');
    expect(useUIStore.getState().dialogData.path).toBe('/dives/one.stalx');
    expect(loadProject).not.toHaveBeenCalledWith('/dives/two.stalx');

    resolveLoad();

    // Once the first flow settles, the resume-drain picks up the second path
    // - which raises its OWN swap-confirm (loadProject is a stub, so the
    // store's `project` never actually clears), proving it was queued and
    // reached, not lost.
    await waitFor(() => expect(useUIStore.getState().dialogData.path).toBe('/dives/two.stalx'));
    expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm');
  });
});
