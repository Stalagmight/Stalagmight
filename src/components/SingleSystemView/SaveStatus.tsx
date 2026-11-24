import { useTranslation } from 'react-i18next';
import type { AutoSaveStatus } from '../../hooks/useAutoSave';
import { selectFileReadOnly } from '../../hooks/useEditingBlocked';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';

/**
 * The status bar's save state (bottom right of the project view): Saving,
 * Saved, the autosave countdown, Unsaved changes, or a failed autosave (which
 * doubles as a retry). It names the file's own state where "Saved" would
 * mislead: "Read-only" for a file made with a newer Stalagmight, which this
 * version never saves (demo issue N-7), and "Old format" for a 0.6.0 file no
 * save has converted yet (N-6).
 */
export function SaveStatus({
  status,
  countdown,
  isDirty,
  onSave,
}: {
  status: AutoSaveStatus;
  countdown: number;
  isDirty: boolean;
  onSave: () => void;
}) {
  const { t } = useTranslation();
  const linked = useSpeleoStore((s) => s.remoteProjectId !== null);
  const fileReadOnly = useProjectStore(selectFileReadOnly);
  const legacyFileOnDisk = useProjectStore((s) => s.legacyFileOnDisk);
  // "Saved" only when the project's own file holds it in this version's
  // format: a 0.6.0 file stays one until the first save converts it (demo
  // issue N-6), and a file made with a newer Stalagmight is never saved here
  // (N-7).
  const savedText = legacyFileOnDisk
    ? t('singleView.status.oldFormat')
    : linked ? 'Saved locally' : t('singleView.status.saved');

  let text: string;
  let className = '';

  if (fileReadOnly) {
    return (
      <span data-testid="save-status" data-save-state="read-only">
        {t('singleView.status.readOnly')}
      </span>
    );
  }

  switch (status) {
    case 'saving':
      text = t('singleView.status.saving');
      break;
    case 'saved':
      text = savedText;
      break;
    case 'pending':
      text = t('singleView.status.autoSaveCountdown', { seconds: countdown });
      break;
    case 'error':
      text = t('singleView.status.saveError');
      className = 'text-red-400';
      break;
    case 'idle':
    default:
      text = isDirty ? t('singleView.status.unsavedChanges') : savedText;
      if (isDirty) className = 'text-yellow-400';
      break;
  }

  // On a failed save, the status doubles as a retry - clicking it runs the same
  // save as the burger-menu Save button.
  if (status === 'error') {
    return (
      <button
        type="button"
        data-testid="save-status"
        onClick={() => onSave()}
        title={t('common.actions.save')}
        className={`${className} cursor-pointer hover:underline`}
      >
        {text}
      </button>
    );
  }

  return (
    <span data-testid="save-status" className={className}>
      {text}
    </span>
  );
}
