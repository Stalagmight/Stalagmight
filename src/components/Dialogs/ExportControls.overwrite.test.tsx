/**
 * [#576 D35] Export writes the NORMALISED name (`map.svg` typed for a PNG export is
 * written as `map.png`), a file the OS overwrite prompt never asked about. With
 * the real naming rule and only the picker and the backend's existence probe stubbed, an
 * export onto an existing rewritten name must ask first, and Cancel must write
 * nothing.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import '../../i18n';

vi.mock('../../utils/ipc');
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('../../utils/fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/fileDialogs')>()),
  saveDialog: vi.fn(),
}));

import ExportControls from './ExportControls';
import RewrittenPathOverwritePrompt from './RewrittenPathOverwritePrompt';
import { useProjectStore } from '../../store/projectStore';
import { saveDialog } from '../../utils/fileDialogs';
import { useOverwritePromptStore } from '../../utils/overwriteGuard';
import { DEFAULT_IMAGE_OPTIONS } from '../../store/exportStore';
import { invokeIpcMock } from '../../test/factories/ipc';

/** [#576 HFI-2] The backend's answer to the existence probe (`save_target_exists`). */
function fileThere(present: boolean): void {
  invokeIpcMock.mockImplementation(async (cmd: string) => (cmd === 'save_target_exists' ? present : undefined));
}

/** The paths the existence probe was asked about. */
function probedPaths(): unknown[] {
  return invokeIpcMock.mock.calls
    .filter(([cmd]) => cmd === 'save_target_exists')
    .map(([, args]) => (args as { path: string }).path);
}

let exportImage: ReturnType<typeof vi.fn>;
let exportCsv: ReturnType<typeof vi.fn>;
const onComplete = vi.fn();

function renderExport(exportType: 'image' | 'data') {
  return render(
    <>
      <ExportControls
        variant="dialog"
        exportType={exportType}
        onExportTypeChange={vi.fn()}
        dataFormat="csv"
        onDataFormatChange={vi.fn()}
        imageOptions={{ ...DEFAULT_IMAGE_OPTIONS, format: 'png' }}
        onImageOptionsChange={vi.fn()}
        onComplete={onComplete}
      />
      <RewrittenPathOverwritePrompt />
    </>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockReset();
  useOverwritePromptStore.setState({ request: null });
  exportImage = vi.fn(async () => undefined);
  exportCsv = vi.fn(async () => undefined);
  useProjectStore.setState({
    project: { sections: [], zero_datums: [] } as never,
    exportImage,
    exportCsv,
  } as never);
});

describe('export onto a rewritten file name', () => {
  it('asks before replacing map.png when map.svg was typed; Cancel writes nothing', async () => {
    vi.mocked(saveDialog).mockResolvedValue('/mock/map.svg');
    fileThere(true);
    renderExport('image');

    await userEvent.click(screen.getByTestId('export-button'));

    const prompt = await screen.findByTestId('overwrite-rewritten-prompt');
    expect(prompt).toHaveTextContent('map.png');
    expect(prompt).toHaveTextContent('already exists');
    expect(probedPaths()).toEqual(['/mock/map.png']);
    expect(exportImage).not.toHaveBeenCalled();

    await userEvent.click(screen.getByTestId('overwrite-rewritten-cancel'));

    await waitFor(() => expect(screen.queryByTestId('overwrite-rewritten-prompt')).toBeNull());
    expect(exportImage).not.toHaveBeenCalled();
    // A declined replace leaves the dialog open, like a cancelled picker.
    expect(onComplete).not.toHaveBeenCalled();
  });

  it('writes the rewritten name once the user chooses Replace', async () => {
    vi.mocked(saveDialog).mockResolvedValue('/mock/map.svg');
    fileThere(true);
    renderExport('image');

    await userEvent.click(screen.getByTestId('export-button'));
    await userEvent.click(await screen.findByTestId('overwrite-rewritten-replace'));

    await waitFor(() => expect(exportImage).toHaveBeenCalledTimes(1));
    expect(exportImage.mock.calls[0]![0]).toBe('/mock/map.png');
  });

  it('keeps the tail of a dotted name and writes it without asking when nothing is there', async () => {
    vi.mocked(saveDialog).mockResolvedValue('/mock/CaveA 1.5');
    fileThere(false);
    renderExport('data');

    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(exportCsv).toHaveBeenCalledWith('/mock/CaveA 1.5.csv'));
    expect(screen.queryByTestId('overwrite-rewritten-prompt')).toBeNull();
  });

  it('does not ask again for the name the OS dialog already confirmed', async () => {
    vi.mocked(saveDialog).mockResolvedValue('/mock/map.png');
    fileThere(true);
    renderExport('image');

    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(exportImage).toHaveBeenCalledTimes(1));
    expect(probedPaths()).toEqual([]);
    expect(screen.queryByTestId('overwrite-rewritten-prompt')).toBeNull();
  });
});
