/**
 * Closing a project drops the Export-mode option set.
 *
 * `uiStore.applyDisplaySettings` resets it on load/create, but a CLOSE never
 * reaches that funnel - `clearOpenProjectState` is the only thing that runs -
 * so without the reset there, the next project opened from a closed state
 * inherited the previous one's format/DPI/threshold. `exitGasPlan()`, which
 * `clearOpenProjectState` already called, only leaves the sub-mode; it does not
 * clear either store's VALUES.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => String(e),
}));

import { useProjectStore } from '../projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../speleoStore';
import { useExportStore, DEFAULT_IMAGE_OPTIONS } from '../exportStore';

describe('closeProject resets the Export-mode options', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useProjectStore.setState({ isClosing: false } as never);
    useExportStore.getState().reset();
  });

  it('restores every slice when the close succeeds', async () => {
    invokeIpcMock.mockResolvedValue({ success: true });
    const s = useExportStore.getState();
    s.setExportType('data');
    s.setDataFormat('tml');
    s.setImageOptions({ ...DEFAULT_IMAGE_OPTIONS, format: 'pdf', dpi: 600 });

    await useProjectStore.getState().closeProject();

    const after = useExportStore.getState();
    expect(after.exportType).toBe('image');
    expect(after.dataFormat).toBe('csv');
    expect(after.imageOptions).toEqual(DEFAULT_IMAGE_OPTIONS);
  });

  it('leaves them alone when the close is REFUSED - the project is still open', async () => {
    invokeIpcMock.mockResolvedValue({ success: false, error: 'unsaved changes' });
    useExportStore.getState().setDataFormat('tml');

    await useProjectStore.getState().closeProject();

    expect(useExportStore.getState().dataFormat).toBe('tml');
  });
  it.each([false, true])('pauses editing and refreshes status after a failed release (IPC rejection: %s)', async (reject) => {
    const previous = useSpeleoStore.getState();
    const refreshLinkStatus = vi.fn().mockResolvedValue(undefined);
    const link = { editState: 'editing' } as SpeleoLinkState;
    useSpeleoStore.setState({ remoteProjectId: 'synthetic-remote', link, refreshLinkStatus });
    if (reject) invokeIpcMock.mockRejectedValueOnce(new Error('Synthetic lost response'));
    else invokeIpcMock.mockResolvedValueOnce({ success: false, error: 'Release response lost' });
    try {
      const result = await useProjectStore.getState().closeProject();
      expect(result.success).toBe(false);
      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
      expect(refreshLinkStatus).toHaveBeenCalledOnce();
    } finally {
      useSpeleoStore.setState(previous);
    }
  });

  it('does not pause a successor account after a stale failed close response', async () => {
    const previous = useSpeleoStore.getState();
    const refreshLinkStatus = vi.fn().mockResolvedValue(undefined);
    const link = { editState: 'editing' } as SpeleoLinkState;
    useSpeleoStore.setState({ remoteProjectId: 'synthetic-remote', link, refreshLinkStatus });
    invokeIpcMock.mockImplementationOnce(async () => {
      useSpeleoStore.setState({ sessionGeneration: previous.sessionGeneration + 1 });
      return { success: false, error: 'Old release failed' };
    });
    try {
      await useProjectStore.getState().closeProject();
      expect(useSpeleoStore.getState().link).toBe(link);
      expect(refreshLinkStatus).not.toHaveBeenCalled();
    } finally {
      useSpeleoStore.setState(previous);
    }
  });

  it('preserves the open project and export state when close IPC rejects', async () => {
    const before = useProjectStore.getState().project;
    const project = { name: 'Synthetic remote', file_path: '/synthetic/remote.stalx' } as never;
    useProjectStore.setState({ project });
    invokeIpcMock.mockRejectedValueOnce(new Error('Synthetic connection failure'));
    useExportStore.getState().setDataFormat('tml');
    try {
      const result = await useProjectStore.getState().closeProject();
      expect(result.success).toBe(false);
      expect(result.error).toContain('Synthetic connection failure');
      expect(invokeIpcMock).toHaveBeenCalledWith('close_project', { releaseLock: true, discardLocal: false }, expect.anything());
      expect(useProjectStore.getState().project).toBe(project);
      expect(useProjectStore.getState().isClosing).toBe(false);
      expect(useExportStore.getState().dataFormat).toBe('tml');
    } finally {
      useProjectStore.setState({ project: before });
    }
  });

});
