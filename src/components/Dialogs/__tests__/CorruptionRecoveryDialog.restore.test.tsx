/**
 * The recovery dialog's restore step, in its restore-only mode (Recent
 * Projects: "Restore an earlier copy") and its damaged-file mode: each copy is
 * listed in words with a local date and size, the raw file name in a tooltip;
 * a click on a row only selects it, the sentence under the list says what
 * Restore does, and only Restore restores (real-app demo I-2: one click on a
 * row used to replace the project).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));
const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import CorruptionRecoveryDialog from '../CorruptionRecoveryDialog';
import { preRestoreName, restoreConfirmText, restoreOverwriteWarning } from '../RestoreBackupPicker';
import { useUIStore } from '../../../store/uiStore';
import type { BackupInfo } from '../../../types/ipc/BackupInfo';

const closeDialog = vi.fn();
const PATH = '/caves/Main Sump.stalx';
const KEPT_NAME = 'Main Sump.0123456789ab.20261120T101112.123456789Z.pre-v2.stalx';
const KEPT = `/data/backups/${KEPT_NAME}`;
const SAFETY = '/caves/Main Sump.stalx.pre-restore';
const REPAIR = '/caves/Main Sump.stalx.backup';

/** Midday UTC: the same calendar day in every zone the tests may run in. */
const original: BackupInfo = { path: KEPT, modifiedAt: '2026-11-20T12:11:12.000Z', sizeBytes: 180_000n, kind: 'pre_upgrade' };
const safety: BackupInfo = { path: SAFETY, modifiedAt: '2026-11-21T12:30:00.000Z', sizeBytes: 4_500n, kind: 'pre_restore' };
const repair: BackupInfo = { path: REPAIR, modifiedAt: '2026-11-20T13:00:00.000Z', sizeBytes: 4_400n, kind: 'backup' };

/** What `list_backups` returns, in the backend's (newest first) order. */
let listed: BackupInfo[] = [original];

beforeEach(() => {
  vi.clearAllMocks();
  listed = [original];
  invokeIpc.mockImplementation(async (command: string) => {
    if (command === 'list_backups') return listed;
    if (command === 'restore_from_backup') return 'Restored the original from before the upgrade to the new format.';
    return undefined;
  });
  useUIStore.setState({ closeDialog, dialogData: { corruptPath: PATH, mode: 'restore' } } as never);
});

const restoreCalls = () => invokeIpc.mock.calls.filter(([command]) => command === 'restore_from_backup');

describe('CorruptionRecoveryDialog restore-only', () => {
  it('lists the backups at once and offers no damaged-file option', async () => {
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByTestId('recovery-backup-item-0')).toBeInTheDocument());
    expect(invokeIpc).toHaveBeenCalledWith('list_backups', { path: PATH }, expect.anything());
    expect(invokeIpc).toHaveBeenCalledTimes(1);
    expect(screen.getByText('Restore an Earlier Copy')).toBeInTheDocument();
    expect(screen.queryByText('File Corruption Detected')).toBeNull();
    expect(screen.queryByText('Attempt Repair')).toBeNull();
    fireEvent.click(screen.getByTestId('recovery-backup-cancel'));
    expect(closeDialog).toHaveBeenCalledOnce();
    expect(restoreCalls()).toHaveLength(0);
  });

  it('says what each copy is, with a local date and size, newest first, the file name in a tooltip', async () => {
    listed = [safety, repair, original];
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByTestId('recovery-backup-row-2')).toBeInTheDocument());
    const rows = [0, 1, 2].map((i) => screen.getByTestId(`recovery-backup-row-${i}`));
    expect(rows.map((row) => row.querySelector('.font-medium')?.textContent)).toEqual([
      'The file as it was before the last restore',
      'Backup',
      'Original before the upgrade to the new format',
    ]);
    expect(rows.map((row) => row.getAttribute('title'))).toEqual([
      'Main Sump.stalx.pre-restore',
      'Main Sump.stalx.backup',
      KEPT_NAME,
    ]);
    expect(rows[2]).toHaveTextContent('Nov 20, 2026');
    expect(rows[2]).toHaveTextContent('180.0 kB');
    // Never the raw name, hash or nanosecond stamp in the row itself.
    expect(rows[2]).not.toHaveTextContent('pre-v2');
    expect(rows[2]).not.toHaveTextContent('12:11:12.000Z');
  });

  it('a click on a row only selects it; Restore restores the selected copy', async () => {
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByTestId('recovery-backup-item-0')).toBeInTheDocument());
    const restore = screen.getByTestId('recovery-backup-restore');
    expect(restore).toBeDisabled();
    expect(restore).toHaveAttribute('title', 'Choose a copy first');
    expect(screen.queryByTestId('recovery-backup-confirm')).toBeNull();

    fireEvent.click(screen.getByTestId('recovery-backup-row-0'));

    expect(screen.getByTestId('recovery-backup-item-0')).toBeChecked();
    expect(restoreCalls()).toHaveLength(0);
    expect(screen.getByTestId('recovery-backup-confirm')).toHaveTextContent(
      'Replace Main Sump.stalx with this copy? The current file is kept as Main Sump.stalx.pre-restore ' +
        'and is listed here, so you can switch back.',
    );
    expect(restore).toBeEnabled();

    fireEvent.click(restore);

    await waitFor(() => expect(screen.getByText('Restore Succeeded')).toBeInTheDocument());
    expect(restoreCalls()).toEqual([
      ['restore_from_backup', { projectPath: PATH, backupPath: KEPT }, expect.anything()],
    ]);
    // The result can be left without opening the project.
    expect(screen.getByTestId('recovery-result-reopen')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('recovery-result-close'));
    expect(closeDialog).toHaveBeenCalledOnce();
  });

  it('says that restoring the copy the last restore kept swaps the two files back', async () => {
    listed = [safety, original];
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByTestId('recovery-backup-item-0')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('recovery-backup-item-0'));
    expect(screen.getByTestId('recovery-backup-confirm')).toHaveTextContent(
      'Put Main Sump.stalx back as it was before the last restore? The two files swap places: the current ' +
        'file is kept as Main Sump.stalx.pre-restore, so you can switch back.',
    );
    fireEvent.click(screen.getByTestId('recovery-backup-restore'));
    await waitFor(() => expect(restoreCalls()).toHaveLength(1));
    expect(restoreCalls()[0]?.[1]).toEqual({ projectPath: PATH, backupPath: SAFETY });
  });

  it('warns that another restore overwrites the copy the last restore kept', async () => {
    listed = [safety, original];
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByTestId('recovery-backup-item-1')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('recovery-backup-item-1'));
    expect(screen.getByTestId('recovery-backup-confirm')).toHaveTextContent('Replace Main Sump.stalx with this copy?');
    const warning = screen.getByTestId('recovery-backup-overwrite-warning');
    expect(warning).toHaveTextContent(
      /^The copy kept by the last restore \(Nov 21, 2026, .+\) is overwritten and cannot be restored afterwards\.$/,
    );
    expect(warning).toHaveClass('text-warning-text');
    // Restoring that copy itself swaps, and loses nothing.
    fireEvent.click(screen.getByTestId('recovery-backup-item-0'));
    expect(screen.queryByTestId('recovery-backup-overwrite-warning')).toBeNull();
  });

  it('says so when there is no backup, without offering another option', async () => {
    listed = [];
    render(<CorruptionRecoveryDialog />);
    await waitFor(() => expect(screen.getByText('No Backups Found')).toBeInTheDocument());
    expect(screen.queryByTestId('recovery-result-retry')).toBeNull();
    expect(screen.getByTestId('recovery-result-close')).toBeInTheDocument();
  });
});

describe('CorruptionRecoveryDialog damaged-file restore', () => {
  beforeEach(() => {
    listed = [repair];
    useUIStore.setState({
      closeDialog,
      dialogData: { corruptPath: PATH, errorMessage: 'The project breaks an invariant.' },
    } as never);
  });

  it('asks before replacing the damaged file, and Back returns to the options', async () => {
    render(<CorruptionRecoveryDialog />);
    fireEvent.click(screen.getByTestId('recovery-restore-backup'));
    await waitFor(() => expect(screen.getByTestId('recovery-backup-item-0')).toBeInTheDocument());
    const dialog = screen.getByTestId('corruption-recovery-dialog');
    expect(within(dialog).getByText(/Choose the copy to put back in place of the damaged file/)).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('recovery-backup-item-0'));

    expect(restoreCalls()).toHaveLength(0);
    expect(screen.getByTestId('recovery-backup-confirm')).toHaveTextContent(
      'Replace the damaged Main Sump.stalx with this copy? The damaged file is kept as Main Sump.stalx.pre-restore ' +
        'and is listed here.',
    );
    fireEvent.click(screen.getByTestId('recovery-backup-back'));
    expect(screen.getByTestId('recovery-attempt-repair')).toBeInTheDocument();
    expect(restoreCalls()).toHaveLength(0);
  });
});

describe('restore wording', () => {
  it('names the copy a restore keeps as the backend does', () => {
    expect(preRestoreName('/caves/cave.stalx', [])).toBe('cave.stalx.pre-restore');
    expect(preRestoreName('/caves/cave.STALX', [])).toBe('cave.stalx.pre-restore');
    expect(preRestoreName('/caves/cave', [])).toBe('cave.stalx.pre-restore');
    expect(preRestoreName('C:\\caves\\cave.stalx', [safety])).toBe('Main Sump.stalx.pre-restore');
  });

  it('warns of nothing when no restore copy would be overwritten', () => {
    expect(restoreConfirmText(PATH, [original], original, false)).not.toContain('last restore');
    expect(restoreOverwriteWarning([original], original)).toBeNull();
    expect(restoreOverwriteWarning([safety, original], safety)).toBeNull();
  });
});
