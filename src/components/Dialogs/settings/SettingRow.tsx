import { useId, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ResetButton } from '../../common/ResetButton';

interface SettingRowProps {
  /** Label text for the setting */
  label: string;
  /** Optional help text shown below the label */
  helpText?: string;
  /** Whether this setting requires app restart to take effect */
  requiresRestart?: boolean;
  /** Whether the setting value has been changed from saved state */
  isChanged?: boolean;
  /** The control element (Toggle, Select, input, etc.) */
  children: ReactNode;
  /** Test ID for E2E tests */
  'data-testid'?: string;
  /** Callback to reset this setting to its default value */
  onReset?: () => void;
  /** Whether the setting is currently at its default value (disables reset button) */
  isDefault?: boolean;
}

/**
 * Consistent layout for a single setting row.
 * Provides label, help text, restart indicator, and control placement.
 */
export function SettingRow({
  label,
  helpText,
  requiresRestart = false,
  isChanged = false,
  children,
  'data-testid': dataTestId,
  onReset,
  isDefault = true,
}: SettingRowProps) {
  const { t } = useTranslation();
  const labelId = useId();

  return (
    <div
      className={`py-3 ${isChanged ? 'bg-accent-5 -mx-2 px-2 rounded' : ''}`}
      data-testid={dataTestId}
    >
      <div className="flex items-start justify-between gap-4">
        {/* Label and help text */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span id={labelId} className="text-sm font-medium text-text-primary">
              {label}
            </span>
            {requiresRestart && (
              <span
                className="text-xs px-1.5 py-0.5 bg-warning-20 text-warning rounded"
                title={t('settings.common.restartTitle')}
              >
                {t('settings.common.restartBadge')}
              </span>
            )}
          </div>
          {helpText && (
            <p className="text-xs text-text-secondary mt-0.5">{helpText}</p>
          )}
        </div>

        {/* Control + Reset button */}
        <div className="flex items-center gap-2 flex-shrink-0" aria-labelledby={labelId}>
          {children}
          {onReset && (
            <ResetButton
              onReset={onReset}
              isDefault={isDefault}
              data-testid={dataTestId ? `${dataTestId}-reset` : undefined}
            />
          )}
        </div>
      </div>
    </div>
  );
}
