/**
 * Import from MNemo (#725): what the dialog preselects, how it reports each
 * outcome, and that a finished pull continues into the normal import route.
 */
import { StrictMode } from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { MnemoPort } from '../../types/ipc/MnemoPort';
import type { MnemoPullResult } from '../../types/ipc/MnemoPullResult';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/ipc')>()),
  invokeIpc: invokeIpcMock,
}));

const progress = vi.hoisted(() => ({ handler: null as null | ((e: { payload: { received: number } }) => void) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_event: string, handler: (e: { payload: { received: number } }) => void) => {
    progress.handler = handler;
    // Only its own listener: StrictMode's first, discarded listener unlistens
    // after the second has registered.
    return () => {
      if (progress.handler === handler) progress.handler = null;
    };
  }),
}));

const revealMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/plugin-opener', () => ({ revealItemInDir: revealMock }));

const routeMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('../../utils/openOrImport', () => ({ routeOpenedFile: routeMock }));

import MnemoImportDialog from './MnemoImportDialog';
import { useUIStore } from '../../store/uiStore';

const MNEMO: MnemoPort = {
  name: '/dev/cu.usbmodem101',
  vid: 0x2e8a,
  pid: 0x000a,
  manufacturer: 'Raspberry Pi',
  product: 'MNemo V2',
  likely_mnemo: true,
};
const OTHER: MnemoPort = {
  name: '/dev/cu.usbserial-1',
  vid: 0x0403,
  pid: 0x6001,
  manufacturer: 'FTDI',
  product: 'FT232R USB UART',
  likely_mnemo: false,
};
const RESULT: MnemoPullResult = {
  path: '/data/MNemo downloads/MNemo-2026-11-13_143005.dmp',
  file_name: 'MNemo-2026-11-13_143005.dmp',
  firmware: 'v2.4',
  bytes: 4096,
  sections: 3,
  shots: 41,
  skipped_sections: 0,
  unread_trailing_bytes: 0,
  unreadable_reason: null,
};

type Replies = {
  ports?: MnemoPort[];
  pull?: () => Promise<MnemoPullResult>;
};

function backend({ ports = [MNEMO], pull = async () => RESULT }: Replies = {}) {
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_mnemo_ports') return ports;
    if (cmd === 'pull_mnemo') return pull();
    if (cmd === 'cancel_mnemo_pull') return null;
    throw new Error(`unexpected command ${cmd}`);
  });
}

const closeDialog = vi.fn();

beforeEach(() => {
  invokeIpcMock.mockReset();
  revealMock.mockClear();
  routeMock.mockClear();
  closeDialog.mockReset();
  progress.handler = null;
  useUIStore.setState({ activeDialog: 'mnemo-import', closeDialog });
});

async function renderReady() {
  // StrictMode as in the app: its mount, cleanup, mount once left the dialog
  // ignoring every reply, and only the e2e run (which uses it) caught that.
  render(
    <StrictMode>
      <MnemoImportDialog />
    </StrictMode>
  );
  await waitFor(() => expect(screen.getByTestId('mnemo-refresh')).not.toBeDisabled());
}

function select(): HTMLSelectElement {
  return screen.getByTestId('mnemo-port-select') as HTMLSelectElement;
}

describe('MnemoImportDialog', () => {
  it('preselects the one MNemo, reads it, and continues into the normal import route', async () => {
    backend();
    await renderReady();
    expect(select().value).toBe(MNEMO.name);

    await userEvent.click(screen.getByTestId('mnemo-download'));

    expect(invokeIpcMock).toHaveBeenCalledWith('pull_mnemo', { port: MNEMO.name }, expect.anything());
    expect(await screen.findByTestId('mnemo-result-summary')).toHaveTextContent(
      'Saved MNemo-2026-11-13_143005.dmp: 3 sections, 41 shots.'
    );

    await userEvent.click(screen.getByTestId('mnemo-continue'));
    expect(closeDialog).toHaveBeenCalled();
    expect(routeMock).toHaveBeenCalledWith(RESULT.path, 'Import Failed');
  });

  it('never preselects a device it does not recognise', async () => {
    backend({ ports: [OTHER] });
    await renderReady();
    expect(select().value).toBe('');
    expect(screen.getByTestId('mnemo-download')).toBeDisabled();
    expect(screen.getByText(/not recognised as an MNemo/)).toBeInTheDocument();

    await userEvent.selectOptions(select(), OTHER.name);
    expect(screen.getByTestId('mnemo-download')).not.toBeDisabled();
  });

  it('does not guess between two MNemos', async () => {
    backend({ ports: [MNEMO, { ...MNEMO, name: '/dev/cu.usbmodem102' }] });
    await renderReady();
    expect(select().value).toBe('');
  });

  it('says what to do when no device is connected', async () => {
    backend({ ports: [] });
    await renderReady();
    expect(screen.getByTestId('mnemo-no-ports')).toBeInTheDocument();
    expect(screen.getByTestId('mnemo-download')).toBeDisabled();
  });

  it('turns a typed failure into guidance and lets the user try again', async () => {
    backend({ pull: () => Promise.reject({ kind: 'port_busy' }) });
    await renderReady();
    await userEvent.click(screen.getByTestId('mnemo-download'));

    expect(await screen.findByTestId('mnemo-error')).toHaveTextContent('Another program is using the MNemo');
    expect(screen.getByTestId('mnemo-download')).not.toBeDisabled();
    expect(routeMock).not.toHaveBeenCalled();
  });

  it('shows progress, cancels on request, and treats the cancel as no error', async () => {
    let reject: (e: unknown) => void = () => {};
    backend({ pull: () => new Promise((_, r) => (reject = r)) });
    await renderReady();
    await userEvent.click(screen.getByTestId('mnemo-download'));

    act(() => progress.handler?.({ payload: { received: 2048 } }));
    expect(screen.getByTestId('mnemo-progress')).toHaveTextContent('2.0 kB received');

    await userEvent.click(screen.getByTestId('mnemo-cancel-pull'));
    expect(invokeIpcMock).toHaveBeenCalledWith('cancel_mnemo_pull', undefined, expect.anything());

    await act(async () => reject({ kind: 'cancelled' }));
    await waitFor(() => expect(screen.getByTestId('mnemo-download')).toBeInTheDocument());
    expect(screen.queryByTestId('mnemo-error')).toBeNull();
  });

  it('closing mid-transfer cancels the pull instead of closing', async () => {
    backend({ pull: () => new Promise(() => {}) });
    await renderReady();
    await userEvent.click(screen.getByTestId('mnemo-download'));
    await userEvent.keyboard('{Escape}');

    expect(invokeIpcMock).toHaveBeenCalledWith('cancel_mnemo_pull', undefined, expect.anything());
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('reports sections it could not read', async () => {
    backend({ pull: async () => ({ ...RESULT, skipped_sections: 2, unread_trailing_bytes: 70 }) });
    await renderReady();
    await userEvent.click(screen.getByTestId('mnemo-download'));

    const losses = await screen.findByTestId('mnemo-result-losses');
    expect(losses).toHaveTextContent('2 sections could not be read');
    expect(losses).toHaveTextContent('70 bytes after the last complete section were not read');
    expect(screen.getByTestId('mnemo-continue')).not.toBeDisabled();
  });

  it('keeps an unreadable dump but will not import it, and can show the file', async () => {
    backend({ pull: async () => ({ ...RESULT, sections: 0, shots: 0, unreadable_reason: 'No sections found' }) });
    await renderReady();
    await userEvent.click(screen.getByTestId('mnemo-download'));

    expect(await screen.findByTestId('mnemo-result-unreadable')).toHaveTextContent('No sections found');
    expect(screen.getByTestId('mnemo-continue')).toBeDisabled();

    await userEvent.click(screen.getByTestId('mnemo-show-file'));
    expect(revealMock).toHaveBeenCalledWith(RESULT.path);
  });
});
