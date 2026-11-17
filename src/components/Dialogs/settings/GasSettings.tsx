import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged } from './types';
import { defaultConfig } from '../../../types/config';
import { PositiveNumberField } from '../../common/PositiveNumberField';
import { Toggle } from '../../common/Toggle';

/**
 * Gas & route-planning settings category (#38). Persisted personal defaults a plan is
 * seeded from:
 *  - swim speed - the exit-leg (and symmetric survey-in) speed, default 10 m/min;
 *  - jump RMV multiplier - gas bump on reference/jump legs (tying in a jump is extra
 *    work), >= 1.0, default 1.0; 1.5 is a useful value.
 * The DPV rig is planned in Plan mode's DPV tab (#317), not here.
 * Swim speed / jump multiplier are clamped server-side in GasConfig::normalize.
 * Client-side, PositiveNumberField rejects a non-positive entry at the field itself (it reverts
 * to the last valid value on blur, no error message); a sub-minimum positive jump multiplier
 * (0 < jump < 1) still commits and is caught by the validation effect below, which blocks Save
 * via the error indicator instead.
 */
export function GasSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const hasErrorRef = useRef(false);

  useEffect(() => {
    const swim = config.gas.swim_speed_m_per_min;
    const jump = config.gas.jump_rmv_multiplier;
    const hasError = !Number.isFinite(swim) || swim <= 0 || !Number.isFinite(jump) || jump < 1;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('gas', hasError);
    }
  }, [config.gas, onValidationError]);

  const setGas = (patch: Partial<typeof config.gas>) =>
    onChange({ gas: { ...config.gas, ...patch } });

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'gas')}
      aria-labelledby={getTabId(panelIdBase, 'gas')}
      data-testid="settings-panel-gas"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.gas.title')}</h2>

      <SettingsSection title={t('settings.gas.sections.planning')} data-testid="section-gas-planning">
        <SettingRow
          label={t('settings.gas.swimSpeed.label')}
          helpText={t('settings.gas.swimSpeed.help')}
          isChanged={isFieldChanged(changedFields, 'gas.swim_speed_m_per_min')}
          onReset={() => setGas({ swim_speed_m_per_min: defaultConfig.gas.swim_speed_m_per_min })}
          isDefault={config.gas.swim_speed_m_per_min === defaultConfig.gas.swim_speed_m_per_min}
          data-testid="setting-gas-swim-speed"
        >
          <PositiveNumberField
            value={config.gas.swim_speed_m_per_min}
            onCommit={(n) => setGas({ swim_speed_m_per_min: n })}
            step={1}
            className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            ariaLabel={t('settings.gas.swimSpeed.aria')}
            testId="input-gas-swim-speed"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.gas.jumpMultiplier.label')}
          helpText={t('settings.gas.jumpMultiplier.help')}
          isChanged={isFieldChanged(changedFields, 'gas.jump_rmv_multiplier')}
          onReset={() => setGas({ jump_rmv_multiplier: defaultConfig.gas.jump_rmv_multiplier })}
          isDefault={config.gas.jump_rmv_multiplier === defaultConfig.gas.jump_rmv_multiplier}
          data-testid="setting-gas-jump-multiplier"
        >
          <PositiveNumberField
            value={config.gas.jump_rmv_multiplier}
            onCommit={(n) => setGas({ jump_rmv_multiplier: n })}
            step={0.1}
            className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            ariaLabel={t('settings.gas.jumpMultiplier.aria')}
            testId="input-gas-jump-multiplier"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.gas.includeReferenceLegs.label')}
          helpText={t('settings.gas.includeReferenceLegs.help')}
          isChanged={isFieldChanged(changedFields, 'gas.include_reference_legs')}
          onReset={() => setGas({ include_reference_legs: defaultConfig.gas.include_reference_legs })}
          isDefault={config.gas.include_reference_legs === defaultConfig.gas.include_reference_legs}
          data-testid="setting-gas-include-reference-legs"
        >
          <Toggle
            checked={config.gas.include_reference_legs}
            onChange={(v) => setGas({ include_reference_legs: v })}
            aria-label={t('settings.gas.includeReferenceLegs.aria')}
            data-testid="toggle-gas-include-reference-legs"
          />
        </SettingRow>
      </SettingsSection>
    </div>
  );
}
