import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createNumericHandler } from './types';
import { defaultConfig, type Config } from '../../../types/config';
import { validateValidationConfig } from '../../../utils/validation';

/**
 * Validation settings category content.
 * Maps to config.validation: warn_shot_length_min/max, warn_depth_change_per_shot_max,
 * azimuth_diff_threshold, pitch_diff_threshold.
 */
export function ValidationSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
  scope = 'global',
  // Baseline for "reset to default" / is-default. The global dialog uses the
  // built-in defaults; the project dialog passes the user's GLOBAL config (the
  // "defaults for new projects", #57) so reset matches that mental model.
  defaults = defaultConfig.validation,
}: SettingsCategoryProps & { scope?: 'global' | 'project'; defaults?: Config['validation'] }) {
  const { t } = useTranslation();
  // Validation state
  const hasErrorRef = useRef(false);

  // Validate on mount and when config changes (shared rules, see
  // utils/validation). Per-field messages were never rendered in this panel;
  // only the boolean is reported via onValidationError.
  useEffect(() => {
    const errs = validateValidationConfig(config.validation);
    const hasError = Object.values(errs).some((e) => e !== null);
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('validation', hasError);
    }
  }, [config.validation, onValidationError]);

  const handleShotLengthMinChange = createNumericHandler(onChange,
    (n) => ({ validation: { ...config.validation, warn_shot_length_min: n } }));
  const handleShotLengthMaxChange = createNumericHandler(onChange,
    (n) => ({ validation: { ...config.validation, warn_shot_length_max: n } }));
  const handleDepthChangeMaxChange = createNumericHandler(onChange,
    (n) => ({ validation: { ...config.validation, warn_depth_change_per_shot_max: n } }));
  const handleAzimuthDiffChange = createNumericHandler(onChange,
    (n) => ({ validation: { ...config.validation, azimuth_diff_threshold: n } }));
  const handlePitchDiffChange = createNumericHandler(onChange,
    (n) => ({ validation: { ...config.validation, pitch_diff_threshold: n } }));
  const handleInclinationDepthDiffChange = createNumericHandler(onChange,
    (n) => ({ validation: { ...config.validation, inclination_depth_diff_threshold: n } }));

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'validation')}
      aria-labelledby={getTabId(panelIdBase, 'validation')}
      data-testid="settings-panel-validation"
    >
      <h2 className="text-lg font-medium mb-1">{t('settings.validation.title')}</h2>
      {scope === 'global' && (
        <p className="text-xs text-text-secondary mb-4">{t('settings.validation.defaultsNote')}</p>
      )}

      <SettingsSection title={t('settings.validation.sections.shotLength')} data-testid="section-shot-length">
        <SettingRow
          label={t('settings.validation.shotLength.min.label')}
          helpText={t('settings.validation.shotLength.min.help')}
          isChanged={isFieldChanged(changedFields, 'validation.warn_shot_length_min')}
          onReset={() => onChange({ validation: { ...config.validation, warn_shot_length_min: defaults.warn_shot_length_min } })}
          isDefault={config.validation.warn_shot_length_min === defaults.warn_shot_length_min}
          data-testid="setting-shot-length-min"
        >
          <input
            type="number" inputMode="decimal"
            step={0.1}
            min={0}
            value={config.validation.warn_shot_length_min}
            onChange={(e) => handleShotLengthMinChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.validation.shotLength.min.aria')}
            data-testid="input-shot-length-min"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.validation.shotLength.max.label')}
          helpText={t('settings.validation.shotLength.max.help')}
          isChanged={isFieldChanged(changedFields, 'validation.warn_shot_length_max')}
          onReset={() => onChange({ validation: { ...config.validation, warn_shot_length_max: defaults.warn_shot_length_max } })}
          isDefault={config.validation.warn_shot_length_max === defaults.warn_shot_length_max}
          data-testid="setting-shot-length-max"
        >
          <input
            type="number" inputMode="decimal"
            step={0.1}
            min={config.validation.warn_shot_length_min + 0.1}
            value={config.validation.warn_shot_length_max}
            onChange={(e) => handleShotLengthMaxChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.validation.shotLength.max.aria')}
            data-testid="input-shot-length-max"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.validation.sections.depth')} data-testid="section-depth-warnings">
        <SettingRow
          label={t('settings.validation.depth.max.label')}
          helpText={t('settings.validation.depth.max.help')}
          isChanged={isFieldChanged(changedFields, 'validation.warn_depth_change_per_shot_max')}
          onReset={() => onChange({ validation: { ...config.validation, warn_depth_change_per_shot_max: defaults.warn_depth_change_per_shot_max } })}
          isDefault={config.validation.warn_depth_change_per_shot_max === defaults.warn_depth_change_per_shot_max}
          data-testid="setting-depth-change-max"
        >
          <input
            type="number" inputMode="decimal"
            step={0.1}
            min={0.1}
            value={config.validation.warn_depth_change_per_shot_max}
            onChange={(e) => handleDepthChangeMaxChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.validation.depth.max.aria')}
            data-testid="input-depth-change-max"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.validation.sections.diffThresholds')} data-testid="section-diff-thresholds">
        <SettingRow
          label={t('settings.validation.azimuthDiff.label')}
          helpText={t('settings.validation.azimuthDiff.help')}
          isChanged={isFieldChanged(changedFields, 'validation.azimuth_diff_threshold')}
          onReset={() => onChange({ validation: { ...config.validation, azimuth_diff_threshold: defaults.azimuth_diff_threshold } })}
          isDefault={config.validation.azimuth_diff_threshold === defaults.azimuth_diff_threshold}
          data-testid="setting-azimuth-diff"
        >
          <input
            type="number" inputMode="decimal"
            step={1}
            min={0}
            max={180}
            value={config.validation.azimuth_diff_threshold}
            onChange={(e) => handleAzimuthDiffChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.validation.azimuthDiff.aria')}
            data-testid="input-azimuth-diff"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.validation.pitchDiff.label')}
          helpText={t('settings.validation.pitchDiff.help')}
          isChanged={isFieldChanged(changedFields, 'validation.pitch_diff_threshold')}
          onReset={() => onChange({ validation: { ...config.validation, pitch_diff_threshold: defaults.pitch_diff_threshold } })}
          isDefault={config.validation.pitch_diff_threshold === defaults.pitch_diff_threshold}
          data-testid="setting-pitch-diff"
        >
          <input
            type="number" inputMode="decimal"
            step={1}
            min={0}
            max={90}
            value={config.validation.pitch_diff_threshold}
            onChange={(e) => handlePitchDiffChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.validation.pitchDiff.aria')}
            data-testid="input-pitch-diff"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.validation.inclinationDepthDiff.label')}
          helpText={t('settings.validation.inclinationDepthDiff.help')}
          isChanged={isFieldChanged(changedFields, 'validation.inclination_depth_diff_threshold')}
          onReset={() => onChange({ validation: { ...config.validation, inclination_depth_diff_threshold: defaults.inclination_depth_diff_threshold } })}
          isDefault={config.validation.inclination_depth_diff_threshold === defaults.inclination_depth_diff_threshold}
          data-testid="setting-inclination-depth-diff"
        >
          <input
            type="number" inputMode="decimal"
            step={1}
            min={0}
            max={90}
            value={config.validation.inclination_depth_diff_threshold}
            onChange={(e) => handleInclinationDepthDiffChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.validation.inclinationDepthDiff.aria')}
            data-testid="input-inclination-depth-diff"
          />
        </SettingRow>
      </SettingsSection>
    </div>
  );
}
