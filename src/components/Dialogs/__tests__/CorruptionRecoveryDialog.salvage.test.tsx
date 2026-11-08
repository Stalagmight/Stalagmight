/**
 * [#576 HFI-3] The recovery dialog's "Export Salvageable Data": the file is
 * written by the backend (`write_salvaged_data`, under the write-path policy).
 * It used to be written from the webview with the fs plugin's `writeTextFile`,
 * which has no scope for user paths, so in the real app every salvage export
 * ended in "Salvage Failed: forbidden path" with nothing written.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { SalvageWriteResult } from '../../../types/ipc/SalvageWriteResult';

vi.mock('../../../utils/ipc');

const saveDialogMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/fileDialogs')>()),
  saveDialog: saveDialogMock,
}));

const closeDialog = vi.fn();
vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({
    closeDialog,
    dialogData: { corruptPath: '/survey/CaveA.stalx', errorMessage: 'Database integrity check failed.' },
  }),
}));
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ loadProject: vi.fn() }),
}));
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryStubs({ setProjectActionSource: vi.fn() }),
);
vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

import '../../../i18n';
import CorruptionRecoveryDialog from '../CorruptionRecoveryDialog';
import { invokeIpcMock as invokeIpc } from '../../../test/factories/ipc';
import { useOverwritePromptStore } from '../../../utils/overwriteGuard';

/** What the write answers: the counts and warnings of the salvage it wrote. */
const WRITTEN: SalvageWriteResult = {
  sectionCount: 2,
  shotCount: 17,
  mergeCount: 1,
  errors: [],
  projectIncluded: true,
};

/** Script the backend: the open check passes, the write answers `write`. */
function backend(write: () => Promise<SalvageWriteResult>): void {
  invokeIpc.mockImplementation(async (command: string) => {
    if (command === 'check_salvage_source') return null;
    if (command === 'write_salvaged_data') return write();
    throw new Error(`unexpected ${command}`);
  });
}

async function exportSalvage(): Promise<void> {
  render(<CorruptionRecoveryDialog />);
  fireEvent.click(screen.getByTestId('recovery-export-salvage'));
}

describe('CorruptionRecoveryDialog salvage export (#576 HFI-3)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpc.mockReset();
    saveDialogMock.mockResolvedValue('/survey/rescued/salvaged.json');
  });

  it('has the backend write the salvage file to the chosen path and reports what was saved', async () => {
    backend(async () => WRITTEN);

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    expect(invokeIpc).toHaveBeenCalledWith(
      'write_salvaged_data',
      { path: '/survey/CaveA.stalx', destPath: '/survey/rescued/salvaged.json' },
      expect.anything(),
    );
    expect(screen.getByText(/\/survey\/rescued\/salvaged\.json/)).toBeInTheDocument();
  });

  it('shows the backend reason when the write is refused, and claims nothing was saved', async () => {
    backend(async () => {
      throw new Error('Refused to change salvaged.json: that is inside a system folder');
    });

    await exportSalvage();

    expect(await screen.findByText('Salvage Failed')).toBeInTheDocument();
    expect(screen.getByText(/inside a system folder/)).toBeInTheDocument();
    expect(screen.queryByText('Data Exported')).toBeNull();
  });

  it('writes nothing when the save dialog is cancelled', async () => {
    saveDialogMock.mockResolvedValue(null);
    backend(async () => WRITTEN);

    await exportSalvage();

    await waitFor(() => expect(saveDialogMock).toHaveBeenCalledTimes(1));
    expect(invokeIpc).toHaveBeenCalledWith('check_salvage_source', { path: '/survey/CaveA.stalx' }, expect.anything());
    expect(invokeIpc).not.toHaveBeenCalledWith('write_salvaged_data', expect.anything(), expect.anything());
  });

  // [#576 NF-W12FS-1] The cancel went to the result step with no result, which
  // renders nothing: the dialog was left with its title and an empty body.
  it('goes back to the recovery choices when the save dialog is cancelled', async () => {
    saveDialogMock.mockResolvedValue(null);
    backend(async () => WRITTEN);

    await exportSalvage();

    await waitFor(() => expect(saveDialogMock).toHaveBeenCalledTimes(1));
    expect(await screen.findByTestId('recovery-export-salvage')).toBeInTheDocument();
    expect(screen.getByTestId('recovery-attempt-repair')).toBeInTheDocument();
    expect(screen.getByText('Database integrity check failed.')).toBeInTheDocument();
    expect(screen.queryByText('Data Exported')).toBeNull();
    expect(screen.queryByText('Salvage Failed')).toBeNull();
  });
});

/**
 * [#576 HFI2-F1] GTK's save dialog does not append the filter's extension, so a
 * name typed bare reached the backend as `salvaged` and was refused ("the file
 * must end in .json"). The answer now goes through `resolveSavePath`, as every
 * other save and export does: the extension is added, and only a REWRITTEN name
 * that already exists is asked about.
 */
describe('CorruptionRecoveryDialog salvage export, a name typed without .json (#576 HFI2-F1)', () => {
  /** The backend, with a file at the rewritten name or not. */
  function backendWith(rewrittenExists: boolean): void {
    invokeIpc.mockImplementation(async (command: string) => {
      if (command === 'check_salvage_source') return null;
      if (command === 'save_target_exists') return rewrittenExists;
      if (command === 'write_salvaged_data') return WRITTEN;
      throw new Error(`unexpected ${command}`);
    });
  }
  const written = (): unknown[] =>
    invokeIpc.mock.calls.filter(([command]) => command === 'write_salvaged_data').map(([, args]) => args);

  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpc.mockReset();
    useOverwritePromptStore.setState({ request: null });
    saveDialogMock.mockResolvedValue('/survey/rescued/salvaged');
  });

  it('adds .json and writes there, without asking when nothing is at that name', async () => {
    backendWith(false);

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    expect(written()).toEqual([{ path: '/survey/CaveA.stalx', destPath: '/survey/rescued/salvaged.json' }]);
    expect(invokeIpc).toHaveBeenCalledWith(
      'save_target_exists',
      { path: '/survey/rescued/salvaged.json' },
      expect.anything(),
    );
    expect(screen.getByText(/\/survey\/rescued\/salvaged\.json/)).toBeInTheDocument();
    expect(useOverwritePromptStore.getState().request).toBeNull();
  });

  it('asks before replacing a file at the rewritten name, and writes nothing when declined', async () => {
    backendWith(true);

    await exportSalvage();

    await waitFor(() => expect(useOverwritePromptStore.getState().request).not.toBeNull());
    expect(useOverwritePromptStore.getState().request?.path).toBe('/survey/rescued/salvaged.json');
    act(() => useOverwritePromptStore.getState().request?.resolve(false));

    // Declining is a cancel: back to the choices, nothing written, no result.
    expect(await screen.findByTestId('recovery-export-salvage')).toBeInTheDocument();
    expect(written()).toEqual([]);
    expect(screen.queryByText('Salvage Failed')).toBeNull();
  });

  it('does not probe a name the dialog returned with .json already (the dialog asked)', async () => {
    saveDialogMock.mockResolvedValue('/survey/rescued/salvaged.json');
    backendWith(true);

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    expect(written()).toEqual([{ path: '/survey/CaveA.stalx', destPath: '/survey/rescued/salvaged.json' }]);
    expect(invokeIpc).not.toHaveBeenCalledWith('save_target_exists', expect.anything(), expect.anything());
  });
});

/**
 * [#576 W30-5] The message reports what was WRITTEN. The dialog read the damaged
 * file once for its summary and the backend read it again to write the file; a
 * damaged file can read differently twice, so the counts shown were those of a
 * read that was not the one saved. The write answers its own counts now.
 */
describe('CorruptionRecoveryDialog salvage export, the counts it reports (#576 W30-5)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpc.mockReset();
    saveDialogMock.mockResolvedValue('/survey/rescued/salvaged.json');
  });

  it('shows the counts and warnings of the salvage that was written', async () => {
    backend(async () => ({
      sectionCount: 5,
      shotCount: 40,
      mergeCount: 3,
      errors: ['Failed to read station merges: database disk image is malformed'],
      projectIncluded: true,
    }));

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    const message = screen.getByText(/Recovered:/);
    expect(message).toHaveTextContent('Recovered: 5 section(s), 40 shot(s), 3 merge(s)');
    expect(message).toHaveTextContent('Failed to read station merges: database disk image is malformed');
    expect(message).not.toHaveTextContent('17 shot(s)');
  });
});

/**
 * [#576 Q-W37-1-D1] The damaged file is read in full ONCE, by the write (HFI-3).
 * The dialog used to run a full salvage read first (`export_salvageable_data`)
 * only to fail early on a file that cannot be opened; it now asks the backend
 * to open the file and nothing more (`check_salvage_source`).
 */
describe('CorruptionRecoveryDialog salvage export, one read of the damaged file (#576 Q-W37-1-D1)', () => {
  const commands = (): string[] => invokeIpc.mock.calls.map(([command]) => command as string);

  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpc.mockReset();
    saveDialogMock.mockResolvedValue('/survey/rescued/salvaged.json');
  });

  it('only opens the file before asking for a destination, then the write salvages it', async () => {
    backend(async () => WRITTEN);

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    expect(commands()).toEqual(['check_salvage_source', 'write_salvaged_data']);
    expect(invokeIpc).toHaveBeenCalledWith('check_salvage_source', { path: '/survey/CaveA.stalx' }, expect.anything());
  });

  it('fails before asking for a destination when the file cannot be opened', async () => {
    invokeIpc.mockImplementation(async (command: string) => {
      if (command === 'check_salvage_source') throw new Error('Salvage failed: File does not exist');
      throw new Error(`unexpected ${command}`);
    });

    await exportSalvage();

    expect(await screen.findByText('Salvage Failed')).toBeInTheDocument();
    expect(screen.getByText(/File does not exist/)).toBeInTheDocument();
    expect(saveDialogMock).not.toHaveBeenCalled();
    expect(commands()).toEqual(['check_salvage_source']);
  });
});

/**
 * [#576 Q-W30-2-D1] When the project itself cannot be read the file holds only
 * its sections, yet the message listed the shots and merges the backend had
 * counted as recovered. It now leaves them out and says plainly that they are
 * not included.
 */
describe('CorruptionRecoveryDialog salvage export, a project that could not be read (#576 Q-W30-2-D1)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpc.mockReset();
    saveDialogMock.mockResolvedValue('/survey/rescued/salvaged.json');
  });

  it('says plainly that shots and merges are not included, and does not list them as recovered', async () => {
    backend(async () => ({
      sectionCount: 2,
      shotCount: 17,
      mergeCount: 1,
      errors: ['Failed to load full project: no such table: shots'],
      projectIncluded: false,
    }));

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    const message = screen.getByText(/Recovered:/);
    expect(message).toHaveTextContent('Recovered: 2 section(s)');
    expect(message).not.toHaveTextContent('17 shot(s)');
    expect(message).not.toHaveTextContent('1 merge(s)');
    expect(message).toHaveTextContent(
      'Shots and merges are not included: the project itself could not be read, so only what could be read of its sections was saved.',
    );
    expect(message).toHaveTextContent('Failed to load full project: no such table: shots');
  });

  it('says nothing of the kind when the whole project was saved', async () => {
    backend(async () => WRITTEN);

    await exportSalvage();

    expect(await screen.findByText('Data Exported')).toBeInTheDocument();
    const message = screen.getByText(/Recovered:/);
    expect(message).toHaveTextContent('Recovered: 2 section(s), 17 shot(s), 1 merge(s)');
    expect(message).not.toHaveTextContent('not included');
  });
});
