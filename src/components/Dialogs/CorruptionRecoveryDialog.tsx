import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { saveDialog } from '../../utils/fileDialogs';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc } from '../../utils/ipc';
import { resolveSavePath } from '../../utils/overwriteGuard';
import { extractFilename } from '../../utils/path';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type { BackupInfo } from '../../types/ipc/BackupInfo';
import { trackDialogComplete, setProjectActionSource } from '../../utils/telemetry';
import { recoveryBackupItemTestId } from '../../utils/testId';
import { useShallow } from 'zustand/react/shallow';

type Step = 'choose' | 'repairing' | 'listing-backups' | 'pick-backup' | 'restoring' | 'salvaging' | 'result';

/**
 * [#576 C15] What a restore candidate is, in the user's words. The backend says
 * what each file is (`kind`); the label is the dialog's.
 */
function backupLabel(backup: BackupInfo, t: TFunction): string {
  switch (backup.kind) {
    case 'interrupted_save':
      return t('dialogs.corruptionRecovery.backupKind.interruptedSave');
    case 'pre_restore':
      return backup.restoreNumber !== null
        ? t('dialogs.corruptionRecovery.backupKind.preRestoreNumbered', { number: backup.restoreNumber })
        : t('dialogs.corruptionRecovery.backupKind.preRestore');
    case 'backup':
    default:
      return t('dialogs.corruptionRecovery.backupKind.backup');
  }
}

interface ResultInfo {
  success: boolean;
  title: string;
  message: string;
  /** If true, offer to re-open the file after dismissal */
  canReopen: boolean;
}

export default function CorruptionRecoveryDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  const loadProject = useProjectStore((s) => s.loadProject);

  const filePath = dialogData.corruptPath as string;
  const errorMessage = (dialogData.errorMessage as string) ?? t('dialogs.corruptionRecovery.defaultError');

  const [step, setStep] = useState<Step>('choose');
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [result, setResult] = useState<ResultInfo | null>(null);
  const [busy, setBusy] = useState(false);

  // ── Attempt Repair ──────────────────────────────────────────────
  const handleRepair = useCallback(async () => {
    setStep('repairing');
    setBusy(true);
    try {
      const res = await invokeIpc('attempt_repair', { path: filePath }, {
        purpose: 'repair corrupt database',
      });
      setResult({
        success: res.success,
        title: res.success ? t('dialogs.corruptionRecovery.results.repairSucceeded') : t('dialogs.corruptionRecovery.results.repairIncomplete'),
        message: res.message,
        canReopen: res.success,
      });
    } catch (e) {
      setResult({
        success: false,
        title: t('dialogs.corruptionRecovery.results.repairFailed'),
        message: String(e),
        canReopen: false,
      });
    } finally {
      setBusy(false);
      setStep('result');
    }
  }, [filePath, t]);

  // ── List & Pick Backup ──────────────────────────────────────────
  const handleListBackups = useCallback(async () => {
    setStep('listing-backups');
    setBusy(true);
    try {
      const list = await invokeIpc('list_backups', { path: filePath }, {
        purpose: 'list backup files',
      });
      setBackups(list);
      if (list.length === 0) {
        setResult({
          success: false,
          title: t('dialogs.corruptionRecovery.results.noBackups'),
          message: t('dialogs.corruptionRecovery.results.noBackupsMessage'),
          canReopen: false,
        });
        setStep('result');
      } else {
        setStep('pick-backup');
      }
    } catch (e) {
      setResult({
        success: false,
        title: t('dialogs.corruptionRecovery.results.error'),
        message: String(e),
        canReopen: false,
      });
      setStep('result');
    } finally {
      setBusy(false);
    }
  }, [filePath, t]);

  const handleRestoreBackup = useCallback(async (backupPath: string) => {
    setStep('restoring');
    setBusy(true);
    try {
      const msg = await invokeIpc('restore_from_backup', {
        projectPath: filePath,
        backupPath,
      }, {
        purpose: 'restore from backup',
      });
      setResult({
        success: true,
        title: t('dialogs.corruptionRecovery.results.restoreSucceeded'),
        message: msg,
        canReopen: true,
      });
    } catch (e) {
      setResult({
        success: false,
        title: t('dialogs.corruptionRecovery.results.restoreFailed'),
        message: String(e),
        canReopen: false,
      });
    } finally {
      setBusy(false);
      setStep('result');
    }
  }, [filePath, t]);

  // ── Export Salvageable Data ─────────────────────────────────────
  const handleSalvage = useCallback(async () => {
    setStep('salvaging');
    setBusy(true);
    // A cancelled save dialog goes back to the choices, not to the result step
    // with no result, which renders an empty body (#576 NF-W12FS-1).
    let cancelled = false;
    try {
      // [#576 W30-5] This read no longer feeds the message (the write below
      // answers the counts of what it wrote). It stays so that a file the
      // backend cannot open at all fails here, before a destination is asked.
      await invokeIpc('export_salvageable_data', { path: filePath }, {
        purpose: 'salvage data from corrupt database',
      });

      // Prompt user for save location
      const answer = await saveDialog({
        title: t('dialogs.corruptionRecovery.salvage.saveTitle'),
        defaultPath: 'salvaged-data.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      }, 'recovery-export-salvage');

      // [#576 HFI2-F1] As every other save: GTK does not append the filter's
      // extension, so `.json` is added here, and a rewritten name that is
      // already taken is asked about (declining is a cancel).
      const destPath = answer ? await resolveSavePath(answer, 'json') : null;
      if (!destPath) {
        cancelled = true;
        return;
      }

      // [#576 HFI-3] The backend writes the file (the full project JSON, else
      // the sections JSON), behind the write-path policy: the webview's fs
      // plugin has no scope for user paths, so writing it here always failed.
      // [#576 W30-5] The counts and warnings of the salvage that was written.
      const written = await invokeIpc('write_salvaged_data', { path: filePath, destPath }, {
        purpose: 'write salvaged data from corrupt database',
      });

      const parts: string[] = [];
      if (written.sectionCount > 0) parts.push(t('dialogs.corruptionRecovery.salvage.sections', { count: written.sectionCount }));
      if (written.shotCount > 0) parts.push(t('dialogs.corruptionRecovery.salvage.shots', { count: written.shotCount }));
      if (written.mergeCount > 0) parts.push(t('dialogs.corruptionRecovery.salvage.merges', { count: written.mergeCount }));
      const summary = parts.length > 0 ? parts.join(', ') : t('dialogs.corruptionRecovery.salvage.noData');

      let message = t('dialogs.corruptionRecovery.salvage.savedTo', { path: destPath, summary });
      if (written.errors.length > 0) {
        message += `\n\n${t('dialogs.corruptionRecovery.salvage.warnings', { list: written.errors.map(e => `  - ${e}`).join('\n') })}`;
      }

      setResult({
        success: true,
        title: t('dialogs.corruptionRecovery.results.dataExported'),
        message,
        canReopen: false,
      });
    } catch (e) {
      setResult({
        success: false,
        title: t('dialogs.corruptionRecovery.results.salvageFailed'),
        message: String(e),
        canReopen: false,
      });
    } finally {
      setBusy(false);
      setStep(cancelled ? 'choose' : 'result');
    }
  }, [filePath, t]);

  // ── Re-open after successful repair/restore ─────────────────────
  const handleReopen = useCallback(async () => {
    trackDialogComplete('corruption-recovery');
    closeDialog();
    setProjectActionSource('dialog');
    await loadProject(filePath);
  }, [closeDialog, filePath, loadProject]);

  // ── Result: go back to choose ───────────────────────────────────
  const handleBackToChoose = useCallback(() => {
    setResult(null);
    setStep('choose');
  }, []);

  // ── Render ──────────────────────────────────────────────────────
  const renderContent = () => {
    // Step: choosing an action
    if (step === 'choose') {
      return (
        <div className="space-y-4">
          <p className="text-text-secondary">{errorMessage}</p>
          <p className="text-sm text-text-tertiary">
            {t('dialogs.corruptionRecovery.choose.intro')}
          </p>

          <div className="space-y-2 pt-2">
            <button
              type="button"
              onClick={handleRepair}
              className="w-full text-left p-3 rounded border border-border hover:bg-surface-alt transition-colors"
              data-testid="recovery-attempt-repair"
            >
              <div className="font-medium">{t('dialogs.corruptionRecovery.choose.repair')}</div>
              <div className="text-sm text-text-tertiary mt-0.5">
                {t('dialogs.corruptionRecovery.choose.repairHint')}
              </div>
            </button>

            <button
              type="button"
              onClick={handleListBackups}
              className="w-full text-left p-3 rounded border border-border hover:bg-surface-alt transition-colors"
              data-testid="recovery-restore-backup"
            >
              <div className="font-medium">{t('dialogs.corruptionRecovery.choose.restore')}</div>
              <div className="text-sm text-text-tertiary mt-0.5">
                {t('dialogs.corruptionRecovery.choose.restoreHint')}
              </div>
            </button>

            <button
              type="button"
              onClick={handleSalvage}
              className="w-full text-left p-3 rounded border border-border hover:bg-surface-alt transition-colors"
              data-testid="recovery-export-salvage"
            >
              <div className="font-medium">{t('dialogs.corruptionRecovery.choose.salvage')}</div>
              <div className="text-sm text-text-tertiary mt-0.5">
                {t('dialogs.corruptionRecovery.choose.salvageHint')}
              </div>
            </button>
          </div>

          <div className="flex justify-end pt-4 border-t border-border">
            <Button onClick={closeDialog} data-testid="recovery-cancel">{t('common.actions.cancel')}</Button>
          </div>
        </div>
      );
    }

    // Step: busy (repairing / listing / restoring / salvaging)
    if (busy) {
      const labels: Record<string, string> = {
        repairing: t('dialogs.corruptionRecovery.busy.repairing'),
        'listing-backups': t('dialogs.corruptionRecovery.busy.listingBackups'),
        restoring: t('dialogs.corruptionRecovery.busy.restoring'),
        salvaging: t('dialogs.corruptionRecovery.busy.salvaging'),
      };
      return (
        <div className="flex items-center gap-3 py-4">
          <div className="w-5 h-5 border-2 border-accent border-t-transparent rounded-full animate-spin" />
          <span className="text-text-secondary">{labels[step] ?? t('dialogs.corruptionRecovery.busy.working')}</span>
        </div>
      );
    }

    // Step: pick a backup
    if (step === 'pick-backup') {
      return (
        <div className="space-y-4">
          <p className="text-text-secondary">{t('dialogs.corruptionRecovery.pick.selectFile')}</p>
          <p className="text-sm text-text-tertiary">
            {t('dialogs.corruptionRecovery.pick.checkedHint')}
          </p>
          <div className="space-y-1 max-h-60 overflow-auto">
            {backups.map((backup, index) => {
              const sizeKb = (backup.sizeBytes / 1024).toFixed(1);
              const filename = extractFilename(backup.path);
              // [#576 C15] An unusable candidate is listed (so the user sees it
              // exists, and why it is not offered) but cannot be picked.
              return (
                <button
                  key={backup.path}
                  type="button"
                  onClick={() => handleRestoreBackup(backup.path)}
                  disabled={!backup.usable}
                  aria-disabled={!backup.usable}
                  className="w-full text-left p-2 rounded border border-border hover:bg-surface-alt transition-colors disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                  data-testid={recoveryBackupItemTestId(index)}
                >
                  <div className="text-sm font-medium">{backupLabel(backup, t)}</div>
                  <div className="font-mono text-xs text-text-secondary mt-0.5">{filename}</div>
                  <div className="text-xs text-text-tertiary mt-0.5">
                    {t('dialogs.corruptionRecovery.pick.size', { sizeKb, modifiedAt: backup.modifiedAt })}
                  </div>
                  {!backup.usable && (
                    <div className="text-xs text-error mt-0.5">
                      {t('dialogs.corruptionRecovery.pick.cannotRestore', { problem: backup.problem ?? t('dialogs.corruptionRecovery.pick.notPassed') })}
                    </div>
                  )}
                </button>
              );
            })}
          </div>
          <div className="flex justify-end pt-4 border-t border-border">
            <Button onClick={handleBackToChoose} data-testid="recovery-backup-back">{t('common.actions.back')}</Button>
          </div>
        </div>
      );
    }

    // Step: result
    if (step === 'result' && result) {
      return (
        <div className="space-y-4">
          <h3 className={`text-lg font-medium ${result.success ? 'text-success' : 'text-error'}`}>
            {result.title}
          </h3>
          <p className="text-text-secondary whitespace-pre-wrap">{result.message}</p>
          <div className="flex justify-end gap-2 pt-4 border-t border-border">
            {!result.success && (
              <Button onClick={handleBackToChoose} data-testid="recovery-result-retry">{t('dialogs.corruptionRecovery.tryAnother')}</Button>
            )}
            {result.canReopen ? (
              <Button variant="primary" onClick={handleReopen} data-testid="recovery-result-reopen">{t('dialogs.corruptionRecovery.openProject')}</Button>
            ) : (
              <Button onClick={() => { trackDialogComplete('corruption-recovery'); closeDialog(); }} data-testid="recovery-result-close">{t('common.actions.close')}</Button>
            )}
          </div>
        </div>
      );
    }

    return null;
  };

  return (
    <DialogWrapper
      title={t('dialogs.corruptionRecovery.title')}
      onClose={closeDialog}
      size="lg"
      data-testid="corruption-recovery-dialog"
      closeOnBackdropClick={!busy}
    >
      {renderContent()}
    </DialogWrapper>
  );
}
