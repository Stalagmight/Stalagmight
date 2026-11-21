/**
 * The list a restore picks from (the recovery dialog's "Restore from Backup",
 * and Recent Projects' "Restore an earlier copy"): each copy in words, with a
 * local date and its size, newest first (the backend's order), the raw file
 * name in a tooltip. Picking a row only selects it; the sentence under the
 * list says what Restore will do, and the dialog's footer holds the Restore
 * button. Nothing is written until that button is pressed.
 */
import { formatDate } from '../../i18n/format';
import { extractFilename } from '../../utils/path';
import { formatBytes } from '../../utils/mnemo';
import { recoveryBackupItemTestId } from '../../utils/testId';
import type { BackupInfo } from '../../types/ipc/BackupInfo';
import type { BackupKind } from '../../types/ipc/BackupKind';

/** What each kind of copy is called in the list. */
export const BACKUP_KIND_LABELS: Record<BackupKind, string> = {
  pre_upgrade: 'Original before the upgrade to the new format',
  pre_restore: 'The file as it was before the last restore',
  backup: 'Backup',
  interrupted_save: 'Copy from a save that was interrupted',
};

/** `Nov 21, 2026, 02:50 AM` in the viewer's zone, or the raw text if it does not parse. */
export function formatBackupDate(modifiedAt: string): string {
  const date = new Date(modifiedAt);
  if (!modifiedAt || Number.isNaN(date.getTime())) return modifiedAt;
  return formatDate(date, undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/** The size as the list shows it (`sizeBytes` is a bigint in the type, a number over IPC). */
export function formatBackupSize(sizeBytes: BackupInfo['sizeBytes'] | number): string {
  return formatBytes(Number(sizeBytes));
}

/**
 * The name a restore keeps the replaced file under: the listed copy the last
 * restore kept, else `<project>` with its extension replaced by
 * `.stalx.pre-restore`, as the backend names it.
 */
export function preRestoreName(projectPath: string, backups: BackupInfo[]): string {
  const listed = backups.find((b) => b.kind === 'pre_restore');
  if (listed) return extractFilename(listed.path);
  const name = extractFilename(projectPath);
  const dot = name.lastIndexOf('.');
  return `${dot > 0 ? name.slice(0, dot) : name}.stalx.pre-restore`;
}

/**
 * What Restore will do with `selected`. `damaged`: the project is the damaged
 * file the recovery dialog opened for.
 */
export function restoreConfirmText(
  projectPath: string,
  backups: BackupInfo[],
  selected: BackupInfo,
  damaged: boolean,
): string {
  const project = extractFilename(projectPath);
  const kept = preRestoreName(projectPath, backups);
  if (selected.kind === 'pre_restore') {
    return (
      `Put ${project} back as it was before the last restore? The two files swap places: ` +
      `the current file is kept as ${kept}, so you can switch back.`
    );
  }
  return damaged
    ? `Replace the damaged ${project} with this copy? The damaged file is kept as ${kept} and is listed here.`
    : `Replace ${project} with this copy? The current file is kept as ${kept} and is listed here, so you can switch back.`;
}

/**
 * When restoring `selected` overwrites the copy the last restore kept (there is
 * one, and it is not the copy being restored, which swaps instead): what is
 * lost, said apart from the sentence above, in the warning colour.
 */
export function restoreOverwriteWarning(backups: BackupInfo[], selected: BackupInfo): string | null {
  const previous = backups.find((b) => b.kind === 'pre_restore');
  if (!previous || selected.kind === 'pre_restore') return null;
  return (
    `The copy kept by the last restore (${formatBackupDate(previous.modifiedAt)}) is overwritten ` +
    'and cannot be restored afterwards.'
  );
}

interface RestoreBackupPickerProps {
  projectPath: string;
  backups: BackupInfo[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
  /** The recovery dialog's damaged-file mode (vs Recent Projects' restore). */
  damaged: boolean;
}

export function RestoreBackupPicker({ projectPath, backups, selectedPath, onSelect, damaged }: RestoreBackupPickerProps) {
  const project = extractFilename(projectPath);
  const selected = backups.find((b) => b.path === selectedPath) ?? null;
  const warning = selected ? restoreOverwriteWarning(backups, selected) : null;
  return (
    <div className="space-y-3">
      <p className="text-text-secondary" id="restore-backup-intro">
        {damaged
          ? 'Choose the copy to put back in place of the damaged file.'
          : `Choose the copy to put back in place of ${project}.`}{' '}
        Nothing changes until you press Restore.
      </p>
      <div
        className="space-y-1 max-h-60 overflow-auto"
        role="radiogroup"
        aria-labelledby="restore-backup-intro"
        data-testid="recovery-backup-list"
      >
        {backups.map((backup, index) => {
          const isSelected = backup.path === selectedPath;
          const filename = extractFilename(backup.path);
          return (
            <label
              key={backup.path}
              title={filename}
              className={`flex items-start gap-2 w-full text-left p-2 rounded border cursor-pointer transition-colors ${
                isSelected ? 'bg-accent-20 border-accent' : 'border-border hover:bg-accent-10'
              }`}
              data-testid={`recovery-backup-row-${index}`}
            >
              <input
                type="radio"
                name="restore-backup"
                className="mt-1 accent-accent"
                checked={isSelected}
                onChange={() => onSelect(backup.path)}
                data-testid={recoveryBackupItemTestId(index)}
              />
              <span className="min-w-0">
                <span className="block text-sm font-medium">{BACKUP_KIND_LABELS[backup.kind] ?? 'Backup'}</span>
                <span className="block text-xs text-text-tertiary mt-0.5">
                  {formatBackupDate(backup.modifiedAt)} &middot; {formatBackupSize(backup.sizeBytes)}
                </span>
              </span>
            </label>
          );
        })}
      </div>
      {selected && (
        <p className="text-sm text-text-primary" data-testid="recovery-backup-confirm">
          {restoreConfirmText(projectPath, backups, selected, damaged)}
        </p>
      )}
      {warning && (
        <p className="text-sm text-warning-text" data-testid="recovery-backup-overwrite-warning">
          {warning}
        </p>
      )}
    </div>
  );
}
