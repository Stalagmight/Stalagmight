import { useCallback, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Trans, useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { isMac } from '../../utils/shortcuts';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

/** The Full Disk Access pane - step 1 of the written remediation below. The
 *  scheme is granted explicitly in `src-tauri/capabilities/default.json`
 *  (`opener:allow-open-url`); without that grant `openUrl` rejects, which the
 *  button surfaces rather than silently no-ops. */
const MACOS_PRIVACY_SETTINGS_URL =
  'x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles';

/**
 * Shown when the OS denies filesystem access to a project file - most often
 * because macOS reset previously-granted folder access after an update. Gives
 * actionable guidance instead of the misleading "corrupted database" / raw OS
 * error the user would otherwise see (#39).
 *
 * The fix lives outside the app (System Settings), so on macOS the dialog
 * carries a deep-link button - the power user has already left; this button
 * exists for the user who needs their hand held to the right pane.
 */
export default function SavePermissionDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  const mac = isMac();
  const context = dialogData.context === 'open' ? 'open' : 'save';

  const handleClose = useCallback(() => {
    trackDialogComplete('save-permission');
    closeDialog();
  }, [closeDialog]);

  const [settingsOpenFailed, setSettingsOpenFailed] = useState(false);
  const handleOpenSettings = useCallback(() => {
    // A user-initiated "take me there" that silently does nothing is worse
    // than no button (same rule as useSpeleoWebLink) - fall back to naming
    // the manual path inline; the numbered steps above it stay authoritative.
    void openUrl(MACOS_PRIVACY_SETTINGS_URL).catch(() => setSettingsOpenFailed(true));
  }, []);

  const action = context === 'open' ? t('dialogs.savePermission.actionOpen') : t('dialogs.savePermission.actionSave');

  return (
    <DialogWrapper title={t('dialogs.savePermission.title')} onClose={handleClose} size="md" data-testid="save-permission-dialog">
      <div className="space-y-4">
        <p className="text-text-secondary">
          {t('dialogs.savePermission.denied', { action })}
          {' '}
          {mac
            ? t('dialogs.savePermission.reasonMac')
            : t('dialogs.savePermission.reasonOther')}
          {' '}<span className="text-text-primary">{t('dialogs.savePermission.notCorrupted')}</span>
        </p>

        {mac ? (
          <div className="p-3 bg-surface-alt rounded text-sm text-text-secondary space-y-1">
            <p className="text-text-primary font-medium">{t('dialogs.savePermission.fixHeading')}</p>
            <ol className="list-decimal list-inside space-y-1">
              <li>
                <Trans
                  i18nKey="dialogs.savePermission.macStep1"
                  components={{ hl: <span className="text-text-primary" /> }}
                />
              </li>
              <li>
                <Trans
                  i18nKey="dialogs.savePermission.macStep2"
                  components={{ hl: <span className="text-text-primary" />, code: <code /> }}
                />
              </li>
              <li>{t('dialogs.savePermission.macStep3')}</li>
            </ol>
          </div>
        ) : (
          <div className="p-3 bg-surface-alt rounded text-sm text-text-secondary">
            <Trans
              i18nKey="dialogs.savePermission.otherSteps"
              components={{ code: <code /> }}
            />
          </div>
        )}

        {settingsOpenFailed && (
          <p className="text-sm text-warning" data-testid="save-permission-open-settings-failed">
            {t('dialogs.savePermission.openSettingsFailed')}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          {mac && (
            <Button onClick={handleOpenSettings} data-testid="save-permission-open-settings">
              {t('dialogs.savePermission.openSettings')}
            </Button>
          )}
          <Button variant="primary" onClick={handleClose} data-testid="save-permission-close">{t('dialogs.savePermission.ok')}</Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
