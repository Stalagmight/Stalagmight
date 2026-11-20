/**
 * The recovery dialog's restore-only mode (Recent Projects: "Restore an
 * earlier copy"): for a file that opens fine it lists the project's backups at
 * once, never offers the damaged-file options, and its Back closes it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));
const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import CorruptionRecoveryDialog from '../CorruptionRecoveryDialog';
import { useUIStore } from '../../../store/uiStore';

const closeDialog = vi.fn();
const PATH = '/caves/Main Sump.stalx';
const KEPT = '/data/backups/Main Sump.0123456789ab.20261120T101112.123456789Z.pre-v2.stalx';

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpc.mockImplementation(async (command: string) =>
    command === 'list_backups'
      ? [{ path: KEPT, modifiedAt: '2026-11-20T10:11:12Z', sizeBytes: 4096 }]
      : undefined,
  );
  useUIStore.setState({ closeDialog, dialogData: { corruptPath: PATH, mode: 'restore' } } as never);
});

describe('CorruptionRecoveryDialog restore-only', () => {
  it('lists the backups at once and offers no damaged-file option', async () => {
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByTestId('recovery-backup-item-0')).toBeInTheDocument());
    expect(invokeIpc).toHaveBeenCalledWith('list_backups', { path: PATH }, expect.anything());
    expect(invokeIpc).toHaveBeenCalledTimes(1);
    expect(screen.getByText('Restore an Earlier Copy')).toBeInTheDocument();
    expect(screen.queryByText('File Corruption Detected')).toBeNull();
    expect(screen.queryByText('Attempt Repair')).toBeNull();
    fireEvent.click(screen.getByTestId('recovery-backup-back'));
    expect(closeDialog).toHaveBeenCalledOnce();
  });

  it('says so when there is no backup, without offering another option', async () => {
    invokeIpc.mockResolvedValue([]);
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByText('No Backups Found')).toBeInTheDocument());
    expect(screen.queryByTestId('recovery-result-retry')).toBeNull();
    expect(screen.getByTestId('recovery-result-close')).toBeInTheDocument();
  });
});
