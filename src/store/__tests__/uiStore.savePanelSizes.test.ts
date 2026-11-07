import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useUIStore } from '../uiStore';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

describe('uiStore.savePanelSizes', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue(undefined);
  });

  // Regression: panel sizes persist as a Rust u32. A fractional value (sub-pixel
  // sizes from getBoundingClientRect under the webview UI zoom / HiDPI) fails serde
  // deserialization, the IPC throws, and savePanelSizes swallows it - so the resize
  // silently never saved and the panel reverted to its last whole-pixel value (e.g.
  // the data table stuck at its 500px max). Rounding before the IPC fixes it.
  it('rounds a fractional size to a whole pixel before the persist IPC', async () => {
    await useUIStore.getState().savePanelSizes('sections', 250.5);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { sectionsPanelWidth: 251 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );

    invokeIpcMock.mockClear();
    await useUIStore.getState().savePanelSizes('data-table', 347.9);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { dataTableHeight: 348 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );
  });

  it('passes a whole-pixel size through unchanged, keyed by the panel field name', async () => {
    await useUIStore.getState().savePanelSizes('data-table', 200);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { dataTableHeight: 200 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );
  });

  it('swallows a thrown IPC so the resize keeps working', async () => {
    invokeIpcMock.mockRejectedValueOnce(new Error('serde u32 deserialization failed'));

    await expect(useUIStore.getState().savePanelSizes('detail', 300)).resolves.toBeUndefined();
  });

  // Regression: a degenerate 0 (or any sub-minimum value) must not persist - there
  // is no "unset" sentinel, so a stored 0 reloads the panel collapsed. Clamp to the
  // panel's minimum at the store choke-point, not only in the resize-handle caller.
  it('clamps a below-minimum size up to the panel minimum before persisting', async () => {
    await useUIStore.getState().savePanelSizes('sections', 0);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { sectionsPanelWidth: 150 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );
  });

  it('clamps an above-maximum size down to the panel maximum', async () => {
    await useUIStore.getState().savePanelSizes('detail', 99999);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { detailPanelWidth: 500 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );
  });

  // #404: the import wizards' map column persists a width but has no open/hover
  // state, so it is a SizedPanelId and not a PanelId. It reaches the same IPC
  // through its own config field and its own (wider) bounds.
  it('persists the wizard map column under its own field and bounds', async () => {
    await useUIStore.getState().savePanelSizes('wizard-map', 512);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { wizardMapPanelWidth: 512 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );

    // 600 is above the detail panel's 500 ceiling but well inside this one's.
    invokeIpcMock.mockClear();
    await useUIStore.getState().savePanelSizes('wizard-map', 600);
    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { wizardMapPanelWidth: 600 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );

    invokeIpcMock.mockClear();
    await useUIStore.getState().savePanelSizes('wizard-map', 10);
    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { wizardMapPanelWidth: 280 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );
  });

  it('folds a non-finite size to the panel minimum instead of persisting NaN', async () => {
    await useUIStore.getState().savePanelSizes('data-table', Number.NaN);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'update_panel_sizes',
      { dataTableHeight: 100 },
      expect.objectContaining({ purpose: expect.any(String) }),
    );
  });
});
