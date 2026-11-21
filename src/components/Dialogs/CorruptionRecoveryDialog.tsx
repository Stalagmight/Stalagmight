import { useCallback, useEffect, useRef, useState } from 'react';
import { saveDialog } from '../../utils/fileDialogs';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc } from '../../utils/ipc';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { RestoreBackupPicker } from './RestoreBackupPicker';
import type { RepairResult } from '../../types/ipc/RepairResult';
import type { BackupInfo } from '../../types/ipc/BackupInfo';
import type { SalvagedData } from '../../types/ipc/SalvagedData';
import { trackDialogComplete, setProjectActionSource } from '../../utils/telemetry';

type Step = 'choose' | 'repairing' | 'listing-backups' | 'pick-backup' | 'restoring' | 'salvaging' | 'result';

interface ResultInfo {
  success: boolean;
  title: string;
  message: string;
  /** If true, offer to re-open the file after dismissal */
  canReopen: boolean;
}

export default function CorruptionRecoveryDialog() {
  const { closeDialog, dialogData } = useUIStore();
  const { loadProject } = useProjectStore();

  const filePath = dialogData.corruptPath as string;
  const errorMessage = (dialogData.errorMessage as string) ?? 'The project file is damaged and cannot be opened normally.';
  // Opened from Recent Projects for a file that opens fine: only the restore,
  // straight to the list of backups.
  const restoreOnly = dialogData.mode === 'restore';

  const [step, setStep] = useState<Step>(restoreOnly ? 'listing-backups' : 'choose');
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  // The copy picked in the list. Picking only selects: Restore restores.
  const [selectedBackup, setSelectedBackup] = useState<string | null>(null);
  const [result, setResult] = useState<ResultInfo | null>(null);
  const [busy, setBusy] = useState(false);

  // ── Attempt Repair ──────────────────────────────────────────────
  const handleRepair = useCallback(async () => {
    setStep('repairing');
    setBusy(true);
    try {
      const res = await invokeIpc<RepairResult>('attempt_repair', { path: filePath }, {
        purpose: 'repair a damaged project file',
      });
      setResult({
        success: res.success,
        title: res.success ? 'Repair Succeeded' : 'Repair Incomplete',
        message: res.message,
        canReopen: res.success,
      });
    } catch (e) {
      setResult({
        success: false,
        title: 'Repair Failed',
        message: String(e),
        canReopen: false,
      });
    } finally {
      setBusy(false);
      setStep('result');
    }
  }, [filePath]);

  // ── List & Pick Backup ──────────────────────────────────────────
  const handleListBackups = useCallback(async () => {
    setStep('listing-backups');
    setBusy(true);
    try {
      const list = await invokeIpc<BackupInfo[]>('list_backups', { path: filePath }, {
        purpose: 'list backup files',
      });
      setBackups(list);
      setSelectedBackup(null);
      if (list.length === 0) {
        setResult({
          success: false,
          title: 'No Backups Found',
          // Beside the project, and the originals kept from before an upgrade.
          message: 'No backups of this project were found.',
          canReopen: false,
        });
        setStep('result');
      } else {
        setStep('pick-backup');
      }
    } catch (e) {
      setResult({
        success: false,
        title: 'Error',
        message: String(e),
        canReopen: false,
      });
      setStep('result');
    } finally {
      setBusy(false);
    }
  }, [filePath]);

  const handleRestoreBackup = useCallback(async (backupPath: string) => {
    setStep('restoring');
    setBusy(true);
    try {
      const msg = await invokeIpc<string>('restore_from_backup', {
        projectPath: filePath,
        backupPath,
      }, {
        purpose: 'restore from backup',
      });
      setResult({
        success: true,
        title: 'Restore Succeeded',
        message: msg,
        canReopen: true,
      });
    } catch (e) {
      setResult({
        success: false,
        title: 'Restore Failed',
        message: String(e),
        canReopen: false,
      });
    } finally {
      setBusy(false);
      setStep('result');
    }
  }, [filePath]);

  // Restore-only: list the backups once, on open.
  const listedOnOpen = useRef(false);
  useEffect(() => {
    if (!restoreOnly || listedOnOpen.current) return;
    listedOnOpen.current = true;
    void handleListBackups();
  }, [restoreOnly, handleListBackups]);

  // ── Export Salvageable Data ─────────────────────────────────────
  const handleSalvage = useCallback(async () => {
    setStep('salvaging');
    setBusy(true);
    try {
      const data = await invokeIpc<SalvagedData>('export_salvageable_data', { path: filePath }, {
        purpose: 'salvage data from a damaged project file',
      });

      // Prompt user for save location
      const destPath = await saveDialog({
        title: 'Save Salvaged Data',
        defaultPath: 'salvaged-data.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      }, 'recovery-export-salvage');

      if (!destPath) {
        // User cancelled save dialog
        setStep('choose');
        setBusy(false);
        return;
      }

      // Write the full project JSON or lines JSON to disk
      const content = data.fullProjectJson ?? data.linesJson ?? '{}';
      const { writeTextFile } = await import('@tauri-apps/plugin-fs');
      await writeTextFile(destPath, content);

      const parts: string[] = [];
      if (data.lineCount > 0) parts.push(`${data.lineCount} line(s)`);
      if (data.shotCount > 0) parts.push(`${data.shotCount} shot(s)`);
      if (data.mergeCount > 0) parts.push(`${data.mergeCount} merge(s)`);
      const summary = parts.length > 0 ? parts.join(', ') : 'no recoverable data';

      let message = `Saved to ${destPath}\n\nRecovered: ${summary}`;
      if (data.errors.length > 0) {
        message += `\n\nWarnings:\n${data.errors.map(e => `  - ${e}`).join('\n')}`;
      }

      setResult({
        success: true,
        title: 'Data Exported',
        message,
        canReopen: false,
      });
    } catch (e) {
      setResult({
        success: false,
        title: 'Salvage Failed',
        message: String(e),
        canReopen: false,
      });
    } finally {
      setBusy(false);
      setStep('result');
    }
  }, [filePath]);

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
    if (restoreOnly) {
      // Nothing else to choose for a file that opens fine.
      closeDialog();
      return;
    }
    setStep('choose');
  }, [restoreOnly, closeDialog]);

  // ── Render ──────────────────────────────────────────────────────
  const renderContent = () => {
    // Step: choosing an action
    if (step === 'choose') {
      return (
        <div className="space-y-4">
          <p className="text-text-secondary">{errorMessage}</p>
          <p className="text-sm text-text-tertiary">
            Choose a recovery option below. A backup of the corrupt file will be created before any repair attempt.
          </p>

          <div className="space-y-2 pt-2">
            <button
              type="button"
              onClick={handleRepair}
              className="w-full text-left p-3 rounded border border-border hover:bg-surface-alt transition-colors"
              data-testid="recovery-attempt-repair"
            >
              <div className="font-medium">Attempt Repair</div>
              <div className="text-sm text-text-tertiary mt-0.5">
                {/* Both file formats: a version 2 file is rebuilt from what decodes
                    (the damaged original kept as `.backup`); an old SQLite file
                    gets 0.6.0's backup, REINDEX and VACUUM. */}
                Keeps a copy of the damaged file, then rebuilds the project from what can still be read.
              </div>
            </button>

            <button
              type="button"
              onClick={handleListBackups}
              className="w-full text-left p-3 rounded border border-border hover:bg-surface-alt transition-colors"
              data-testid="recovery-restore-backup"
            >
              <div className="font-medium">Restore from Backup</div>
              <div className="text-sm text-text-tertiary mt-0.5">
                Replace the corrupt file with a previous backup.
              </div>
            </button>

            <button
              type="button"
              onClick={handleSalvage}
              className="w-full text-left p-3 rounded border border-border hover:bg-surface-alt transition-colors"
              data-testid="recovery-export-salvage"
            >
              <div className="font-medium">Export Salvageable Data</div>
              <div className="text-sm text-text-tertiary mt-0.5">
                Export whatever data can be read from the corrupt file as JSON.
              </div>
            </button>
          </div>

          <div className="flex justify-end pt-4 border-t border-border">
            <Button onClick={closeDialog} data-testid="recovery-cancel">Cancel</Button>
          </div>
        </div>
      );
    }

    // Step: busy (repairing / listing / restoring / salvaging)
    if (busy) {
      const labels: Record<string, string> = {
        repairing: 'Attempting repair...',
        'listing-backups': 'Scanning for backups...',
        restoring: 'Restoring from backup...',
        salvaging: 'Exporting salvageable data...',
      };
      return (
        <div className="flex items-center gap-3 py-4">
          <div className="w-5 h-5 border-2 border-accent border-t-transparent rounded-full animate-spin" />
          <span className="text-text-secondary">{labels[step] ?? 'Working...'}</span>
        </div>
      );
    }

    // Step: pick a backup. A row only selects it; the sentence under the list
    // says what Restore does, and nothing is written until Restore is pressed
    // (real-app demo I-2: one click on a row used to replace the project). The
    // damaged-file mode gets the same confirm: a misclick there swaps the
    // damaged file for the wrong copy just the same.
    if (step === 'pick-backup') {
      return (
        <div className="space-y-4">
          <RestoreBackupPicker
            projectPath={filePath}
            backups={backups}
            selectedPath={selectedBackup}
            onSelect={setSelectedBackup}
            damaged={!restoreOnly}
          />
          <div className="flex flex-wrap justify-end gap-2 pt-4 border-t border-border">
            {restoreOnly ? (
              <Button onClick={handleBackToChoose} data-testid="recovery-backup-cancel">Cancel</Button>
            ) : (
              <Button onClick={handleBackToChoose} data-testid="recovery-backup-back">Back</Button>
            )}
            <Button
              variant="primary"
              disabled={!selectedBackup}
              title={selectedBackup ? undefined : 'Choose a copy first'}
              onClick={() => { if (selectedBackup) void handleRestoreBackup(selectedBackup); }}
              data-testid="recovery-backup-restore"
            >
              Restore
            </Button>
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
            {!result.success && !restoreOnly && (
              <Button onClick={handleBackToChoose} data-testid="recovery-result-retry">Try Another Option</Button>
            )}
            <Button onClick={() => { trackDialogComplete('corruption-recovery'); closeDialog(); }} data-testid="recovery-result-close">Close</Button>
            {result.canReopen && (
              <Button variant="primary" onClick={handleReopen} data-testid="recovery-result-reopen">Open Project</Button>
            )}
          </div>
        </div>
      );
    }

    return null;
  };

  return (
    <DialogWrapper
      title={restoreOnly ? 'Restore an Earlier Copy' : 'File Corruption Detected'}
      onClose={closeDialog}
      size="lg"
      data-testid="corruption-recovery-dialog"
      closeOnBackdropClick={!busy}
    >
      {renderContent()}
    </DialogWrapper>
  );
}
