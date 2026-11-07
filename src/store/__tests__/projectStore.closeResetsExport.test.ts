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

vi.mock('../../utils/ipc');
stubGetErrorMessage(stringOfError);

import { useProjectStore } from '../projectStore';
import { useExportStore, DEFAULT_IMAGE_OPTIONS } from '../exportStore';
import { invokeIpcMock, stubGetErrorMessage, stringOfError } from '../../test/factories/ipc';

describe('closeProject resets the Export-mode options', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useProjectStore.setState({ isClosing: false } as never);
    useExportStore.getState().reset();
  });

  it('restores every slice when the close succeeds', async () => {
    invokeIpcMock.mockResolvedValue({ warnings: [] });
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
    invokeIpcMock.mockRejectedValue({ code: 'FAILED', message: 'unsaved changes' });
    useExportStore.getState().setDataFormat('tml');

    await useProjectStore.getState().closeProject();

    expect(useExportStore.getState().dataFormat).toBe('tml');
  });
});
