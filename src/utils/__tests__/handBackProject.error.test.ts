/**
 * frontend#8: the close prompt's hand-back runs the destination's SpeleoDB access
 * check before closing. That IPC rejects with a structured `SpeleoErrorDto`, and the
 * error dialog used to render it with `String(error)` - "[object Object]".
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('../ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc')>()),
  invokeIpc: invokeIpcMock,
}));

import { handBackProject } from '../quit';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

const openDialog = vi.fn();
const closeProject = vi.fn(async () => ({ success: true }));

beforeEach(() => {
  invokeIpcMock.mockReset();
  openDialog.mockReset();
  closeProject.mockClear();
  useProjectStore.setState({ project: { name: 'Open project' } as never, closeProject: closeProject as never });
  useUIStore.setState({ openDialog: openDialog as never, activeDialog: null, dialogData: {} });
});

describe('handBackProject', () => {
  it('shows the message of a structured SpeleoDB error, not [object Object]', async () => {
    invokeIpcMock.mockRejectedValue({
      message: 'invalid SpeleoDB project id',
      action: 'Fatal',
      status: null,
      transport: null,
      unpushed_local: null,
      newer_schema: null,
    });

    await handBackProject({ pendingOpenPath: '/synthetic/next.stalx' });

    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_project_access_at', { path: '/synthetic/next.stalx' });
    expect(openDialog).toHaveBeenCalledWith('error', {
      title: 'Open Failed',
      message: 'invalid SpeleoDB project id',
    });
    // A failed access check replays nothing: the open project stays open.
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('still shows a plain Error message', async () => {
    invokeIpcMock.mockRejectedValue(new Error('disk unreadable'));

    await handBackProject({ pendingOpenPath: '/synthetic/next.stalx' });

    expect(openDialog).toHaveBeenCalledWith('error', { title: 'Open Failed', message: 'disk unreadable' });
  });
});
