import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { invokeIpc } from '../../utils/ipc';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';

/**
 * The footer Cancel of a survey-file import dialog WHILE its import runs
 * (RV4-FE-MISC1, #576 NF-FIX3A-2).
 *
 * The backend registers a file import as the active load and checks its token
 * between phases (`run_file_import`), so `cancel_loading` stops it before anything
 * is applied and the command fails `IMPORT_CANCELLED: ...`, which
 * `surfaceImportFailure` ends with a quiet note. So pressing this asks for that
 * instead of closing: the dialog stays open until the command settles, because a
 * cancel that arrives after the last cancellable point does not stop the import,
 * and the dialog's own result handling still has to run when it lands.
 *
 * It invokes `cancel_loading` itself rather than `projectStore.cancelLoading()`,
 * which also resets the loading state: that would hide the dialog's progress line
 * (and stop listening for its progress) while the import is still running.
 * [#576 W44-NF3] It names the import that state is for (the name the store sent
 * with the import), so a cancel that runs after the import ended stops nothing else.
 *
 * Render it in place of the dialog's idle Cancel only while the import runs; the
 * pressed state then goes with the run, so a later import starts from Cancel.
 */
export function ImportCancelButton({
  children,
  'data-testid': testId,
}: {
  children: ReactNode;
  'data-testid': string;
}) {
  const { t } = useTranslation();
  const [requested, setRequested] = useState(false);

  const requestCancel = () => {
    if (requested) return;
    setRequested(true);
    // Idempotent and answers nothing; if the request itself fails, the import
    // simply runs to its end and the dialog handles that result as usual.
    const loadToken = useProjectStore.getState().loading.loadToken ?? null;
    invokeIpc('cancel_loading', { loadToken }, { purpose: 'cancel the running file import' }).catch(() => {});
  };

  return (
    <Button onClick={requestCancel} disabled={requested} data-testid={testId}>
      {requested ? t('dialogs.importFailure.cancelling') : children}
    </Button>
  );
}
