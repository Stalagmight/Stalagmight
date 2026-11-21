/**
 * Demo finding I-13: an export that wrote its file says so, with the file's
 * name, in a success toast; a failed or cancelled one does not. Injection:
 * removing the `announceExported` calls from `handleExport` fails the first
 * two cases.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: vi.fn(async () => undefined),
}));
const saveDialogMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/fileDialogs')>()),
  saveDialog: saveDialogMock,
}));

import '../../../i18n';
import ExportControls from '../ExportControls';
import { DEFAULT_IMAGE_OPTIONS } from '../../../store/exportStore';
import type { DataFormat, ExportType } from '../../../store/exportStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { TOAST_KEYS } from '../../../utils/toastKeys';

function renderExport(exportType: ExportType, dataFormat: DataFormat) {
  render(
    <ExportControls
      variant="panel"
      exportType={exportType}
      onExportTypeChange={() => undefined}
      dataFormat={dataFormat}
      onDataFormatChange={() => undefined}
      imageOptions={DEFAULT_IMAGE_OPTIONS}
      onImageOptionsChange={() => undefined}
    />,
  );
}

const exportCsv = vi.fn(async () => undefined);
const exportImage = vi.fn(async () => undefined);

describe('ExportControls success toast', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useUIStore.getState().clearToast();
    useProjectStore.setState({
      project: { lines: [{ id: 1, name: 'Main', shots: [] }] },
      exportCsv,
      exportImage,
      getAnchoredLineIds: vi.fn(async () => [1]),
    } as never);
  });
  afterEach(() => {
    useUIStore.getState().clearToast();
  });

  const exported = () => useUIStore.getState().toasts.filter((t) => t.key === TOAST_KEYS.EXPORT_SUCCEEDED);

  it('names the data file it wrote', async () => {
    saveDialogMock.mockResolvedValue('/home/diver/Surveys/demo-cave.csv');
    renderExport('data', 'csv');
    await userEvent.click(screen.getByTestId('export-button'));
    await waitFor(() => expect(exportCsv).toHaveBeenCalledWith('/home/diver/Surveys/demo-cave.csv'));
    await waitFor(() => expect(exported()).toHaveLength(1));
    const toast = exported()[0]!;
    expect(toast.kind).toBe('success');
    expect(toast.message).toBe('Exported to demo-cave.csv');
    // The folder stays out of it (a Windows path is split the same way).
    expect(toast.message).not.toContain('/home');
  });

  it('names the image file it wrote', async () => {
    saveDialogMock.mockResolvedValue('C:\\Surveys\\map.png');
    renderExport('image', 'csv');
    await userEvent.click(screen.getByTestId('export-button'));
    await waitFor(() => expect(exportImage).toHaveBeenCalled());
    await waitFor(() => expect(exported().map((t) => t.message)).toEqual(['Exported to map.png']));
  });

  it('says nothing when the save picker is cancelled or the export fails', async () => {
    saveDialogMock.mockResolvedValue(null);
    renderExport('data', 'csv');
    await userEvent.click(screen.getByTestId('export-button'));
    await waitFor(() => expect(saveDialogMock).toHaveBeenCalled());
    expect(exportCsv).not.toHaveBeenCalled();
    expect(exported()).toHaveLength(0);

    saveDialogMock.mockResolvedValue('/tmp/x.csv');
    exportCsv.mockRejectedValueOnce(new Error('disk full'));
    await userEvent.click(screen.getByTestId('export-button'));
    await waitFor(() =>
      expect(useUIStore.getState().toasts.some((t) => t.key === TOAST_KEYS.EXPORT_FAILED)).toBe(true),
    );
    expect(exported()).toHaveLength(0);
  });
});
