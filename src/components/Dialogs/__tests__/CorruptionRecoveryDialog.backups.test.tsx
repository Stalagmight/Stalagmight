/**
 * [#576 C15] The recovery dialog's Restore list: every candidate the backend
 * offers is labelled by what it is (Backup / Interrupted save / Your file before
 * restore N), and one that failed the backend's checks is listed but greyed out,
 * with the reason, and cannot be picked.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { BackupInfo } from '../../../types/ipc/BackupInfo';

vi.mock('../../../utils/ipc');

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

function candidate(
  file: string,
  kind: BackupInfo['kind'],
  extra: Partial<BackupInfo> = {},
): BackupInfo {
  return {
    path: `/survey/${file}`,
    sizeBytes: 2048,
    modifiedAt: '2026-11-01T10:00:00+00:00',
    kind,
    restoreNumber: null,
    usable: true,
    problem: null,
    ...extra,
  };
}

const LIST: BackupInfo[] = [
  candidate('CaveA.stalx.tmp', 'interrupted_save'),
  candidate('CaveA.stalx.backup', 'backup'),
  candidate('CaveA.stalx.pre-restore.2', 'pre_restore', { restoreNumber: 2 }),
  candidate('CaveA.stalx.backup.1', 'backup', {
    usable: false,
    problem: 'it is not a readable project file',
  }),
];

async function openRestoreList() {
  invokeIpc.mockImplementation(async (command: string) => {
    if (command === 'list_backups') return LIST;
    if (command === 'restore_from_backup') return 'Restored.';
    throw new Error(`unexpected ${command}`);
  });
  render(<CorruptionRecoveryDialog />);
  fireEvent.click(screen.getByTestId('recovery-restore-backup'));
  await screen.findByTestId('recovery-backup-item-0');
}

describe('CorruptionRecoveryDialog restore list (#576 C15)', () => {
  beforeEach(() => vi.clearAllMocks());

  it('labels each candidate by what it is', async () => {
    await openRestoreList();
    expect(screen.getByTestId('recovery-backup-item-0')).toHaveTextContent('Interrupted save');
    expect(screen.getByTestId('recovery-backup-item-1')).toHaveTextContent('Backup');
    expect(screen.getByTestId('recovery-backup-item-2')).toHaveTextContent(
      'Your file before restore 2',
    );
  });

  it('greys out a candidate that failed the checks, says why, and will not restore it', async () => {
    await openRestoreList();
    const broken = screen.getByTestId('recovery-backup-item-3');
    expect(broken).toBeDisabled();
    expect(broken).toHaveTextContent('Cannot be restored: it is not a readable project file.');
    fireEvent.click(broken);
    expect(invokeIpc).not.toHaveBeenCalledWith('restore_from_backup', expect.anything(), expect.anything());

    const usable = screen.getByTestId('recovery-backup-item-1');
    expect(usable).toBeEnabled();
    fireEvent.click(usable);
    expect(invokeIpc).toHaveBeenCalledWith(
      'restore_from_backup',
      { projectPath: '/survey/CaveA.stalx', backupPath: '/survey/CaveA.stalx.backup' },
      expect.anything(),
    );
  });
});
