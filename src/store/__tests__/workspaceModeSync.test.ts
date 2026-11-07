import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useUIStore } from '../uiStore';
import { sendWorkspaceMode, startWorkspaceModeSync } from '../workspaceModeSync';
import { invokeIpcMock } from '../../test/factories/ipc';
import { nextTimer } from '../../test/factories/async';

vi.mock('../../utils/ipc');

/**
 * #576 C5: the backend refuses survey edits while the workspace is Plan or Export,
 * which only works if it is told the mode. Every `appMode` change is sent, in order,
 * whichever path made it - the mode switcher, and the load reset in
 * `applyDisplaySettings` that puts a newly opened project back in Survey.
 */
const sentModes = () =>
  invokeIpcMock.mock.calls
    .filter(([command]) => command === 'set_workspace_mode')
    .map(([, args]) => (args as { mode: string }).mode);

/** Let the chained sends drain. */
const settle = () => sendWorkspaceMode(useUIStore.getState().appMode);

describe('workspace mode sync', () => {
  let stop: () => void;

  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue(null);
    useUIStore.setState({ appMode: 'survey' });
  });

  afterEach(() => {
    stop?.();
  });

  it('sends the current mode at start, then every change, in order', async () => {
    // The first send stays in flight until released, so the order is not just
    // the order of the calls: a later send must WAIT for it (#576 RV2-FE-G2 -
    // with an instantly resolving invoke, unchained sends passed this test too).
    let releaseFirst: () => void = () => {};
    invokeIpcMock.mockImplementationOnce(
      () => new Promise<null>((resolve) => { releaseFirst = () => resolve(null); }),
    );
    stop = startWorkspaceModeSync();
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('export');
    useUIStore.getState().setAppMode('survey');
    await nextTimer();

    expect(sentModes(), 'a later mode was sent before the first send settled').toEqual(['survey']);
    releaseFirst();
    await settle();

    // The trailing 'survey' is `settle`'s own send.
    expect(sentModes()).toEqual(['survey', 'plan', 'export', 'survey', 'survey']);
  });

  // RV2-FE-G2: the test above resolves every send at once, so it passed with the
  // ordering removed. Here the first send is still on the wire when the mode changes
  // twice: neither later send may leave before it lands.
  it('holds each send until the one before it has landed', async () => {
    let landFirst: () => void = () => {};
    invokeIpcMock.mockImplementationOnce(
      () =>
        new Promise<null>((resolve) => {
          landFirst = () => resolve(null);
        })
    );
    stop = startWorkspaceModeSync();
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('export');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(sentModes(), 'a later mode overtook the one still on the wire').toEqual(['survey']);

    landFirst();
    await settle();
    expect(sentModes()).toEqual(['survey', 'plan', 'export', 'export']);
  });

  it('sends Survey when a project load resets the mode', async () => {
    useUIStore.setState({ appMode: 'plan' });
    stop = startWorkspaceModeSync();
    // A load hands over the project's saved settings; none matter here.
    useUIStore.getState().applyDisplaySettings({} as never);
    await settle();

    expect(useUIStore.getState().appMode).toBe('survey');
    expect(sentModes().slice(0, 2)).toEqual(['plan', 'survey']);
  });

  it('a failed send does not wedge the next one', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    invokeIpcMock.mockRejectedValueOnce('ipc down');
    stop = startWorkspaceModeSync();
    useUIStore.getState().setAppMode('plan');
    await settle();

    expect(sentModes()).toEqual(['survey', 'plan', 'plan']);
    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });

  it('stops sending once unsubscribed', async () => {
    stop = startWorkspaceModeSync();
    stop();
    useUIStore.getState().setAppMode('plan');
    await sendWorkspaceMode('survey');

    expect(sentModes()).toEqual(['survey', 'survey']);
  });
});
