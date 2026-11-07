import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { isTelemetryAvailable, setTelemetryEnabled } from '../../utils/telemetry';
import { invokeIpc } from '../../utils/ipc';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import type { Config } from '../../types/config';

interface TelemetryOptInDialogProps {
  config: Config;
  onConfigChanged: (config: Config) => void;
}

export default function TelemetryOptInDialog({
  config,
  onConfigChanged,
}: TelemetryOptInDialogProps) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(true);

  const shouldShow =
    visible &&
    isTelemetryAvailable() &&
    !config.general.telemetry_opt_in_shown;

  // Independent top-layer overlay (rendered outside `activeDialog`) - register
  // occupancy so a cold-launch drain doesn't raise a second modal over it (#445
  // review).
  useScreenOccupant(shouldShow);

  const handleChoice = useCallback(
    async (enabled: boolean) => {
      const updatedConfig: Config = {
        ...config,
        general: {
          ...config.general,
          telemetry_enabled: enabled,
          telemetry_opt_in_shown: true,
        },
      };

      try {
        // [#576 D27-32(c)] The two settings this prompt owns, applied onto the
        // backend's live config - not the whole config it was handed at mount.
        await invokeIpc(
          'patch_config',
          {
            patches: [
              { path: ['general', 'telemetry_enabled'], value: enabled },
              { path: ['general', 'telemetry_opt_in_shown'], value: true },
            ],
          },
          { purpose: 'save telemetry opt-in choice' },
        );
        onConfigChanged(updatedConfig);

        // Enable/disable JS-side collection + notify Rust collector
        setTelemetryEnabled(enabled);
        try {
          await invokeIpc('set_telemetry_enabled', { enabled });
        } catch {
          // Non-fatal - setting is saved in config
        }
      } catch {
        // Config save failed - still dismiss the dialog
      }

      setVisible(false);
    },
    [config, onConfigChanged],
  );

  if (!shouldShow) return null;

  return (
    <DialogWrapper
      title={t('telemetry.optIn.title', 'Help Improve Stalagmight')}
      onClose={() => void handleChoice(false)}
      size="sm"
      data-testid="telemetry-opt-in-dialog"
      closeOnBackdropClick={false}
    >
      <div className="space-y-4">
        <p className="text-text-secondary">
          {t(
            'telemetry.optIn.description',
            'Help improve Stalagmight by sharing anonymous usage statistics. We only collect which features are used and how often - no personal data, no survey data, no file contents.',
          )}
        </p>

        <details className="text-sm text-text-tertiary">
          <summary className="cursor-pointer hover:text-text-secondary" data-testid="telemetry-opt-in-details-toggle">
            {t('telemetry.optIn.whatWeCollect', 'What we collect')}
          </summary>
          <ul className="mt-2 ml-4 list-disc space-y-1">
            <li>{t('telemetry.optIn.collect.commands', 'Which commands are used (e.g., import, export, undo)')}</li>
            <li>{t('telemetry.optIn.collect.tools', 'Which map tools are activated (e.g., measure, draw)')}</li>
            <li>{t('telemetry.optIn.collect.session', 'Session duration and OS type')}</li>
            <li>{t('telemetry.optIn.collect.noPII', 'No names, no file paths, no survey data')}</li>
          </ul>
        </details>

        <div className="flex justify-end gap-3 pt-4 border-t border-border">
          <Button
            variant="secondary"
            onClick={() => void handleChoice(false)}
            data-testid="telemetry-opt-in-no"
          >
            {t('telemetry.optIn.no', 'No thanks')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void handleChoice(true)}
            data-testid="telemetry-opt-in-yes"
          >
            {t('telemetry.optIn.yes', 'Yes, share data')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
