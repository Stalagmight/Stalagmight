import { useCallback, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Toggle } from '../../common/Toggle';
import { Select } from '../../common/Select';
import { Button } from '../../common/Button';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createNumericHandler, createToggleHandler, createSelectHandler, parseIntRadix10 } from './types';
import { defaultConfig } from '../../../types/config';
import type { ShortcutFeedback } from '../../../types/config';
import type { UpdateTrack } from '../../../types/ipc/UpdateTrack';
import { buildDefaultUpdateTrack } from '../../../utils/updateTrack';
import { isTelemetryAvailable } from '../../../utils/telemetry';
import { restartOnboardingProgress } from '../../../hooks/useOnboardingRestart';

/**
 * General settings category content.
 * Maps to config.general: auto_save, auto_save_interval_seconds, recent_projects_max,
 * undo_history_limit, undo_persist_limit, shortcut_feedback.
 */
export function GeneralSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  // Validation state
  const hasErrorRef = useRef(false);

  // Validate on mount and when config changes
  useEffect(() => {
    const errors: string[] = [];

    // Validate auto_save_interval_seconds
    if (
      config.general.auto_save_interval_seconds < 10 ||
      config.general.auto_save_interval_seconds > 3600
    ) {
      errors.push(t('settings.general.validation.autoSaveInterval'));
    }

    // Validate recent_projects_max
    if (config.general.recent_projects_max < 1 || config.general.recent_projects_max > 50) {
      errors.push(t('settings.general.validation.recentProjects'));
    }

    // Validate undo_history_limit
    if (config.general.undo_history_limit < 10 || config.general.undo_history_limit > 1000) {
      errors.push(t('settings.general.validation.undoHistory'));
    }

    // Validate undo_persist_limit
    if (
      config.general.undo_persist_limit < 0 ||
      config.general.undo_persist_limit > config.general.undo_history_limit
    ) {
      errors.push(t('settings.general.validation.undoPersist'));
    }

    const hasError = errors.length > 0;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('general', hasError);
    }
  }, [config.general, onValidationError, t]);

  // --- Saving settings ---
  const handleAutoSaveToggle = createToggleHandler(onChange,
    (v) => ({ general: { ...config.general, auto_save: v } }));
  const handleAutoSaveIntervalChange = createNumericHandler(onChange,
    (n) => ({ general: { ...config.general, auto_save_interval_seconds: n } }), parseIntRadix10);

  // --- Recent projects ---
  const handleRecentProjectsChange = createNumericHandler(onChange,
    (n) => ({ general: { ...config.general, recent_projects_max: n } }), parseIntRadix10);

  // --- Undo history ---
  const handleUndoHistoryChange = createNumericHandler(onChange,
    (n) => ({ general: { ...config.general, undo_history_limit: n } }), parseIntRadix10);
  const handleUndoPersistChange = createNumericHandler(onChange,
    (n) => ({ general: { ...config.general, undo_persist_limit: n } }), parseIntRadix10);

  // --- Startup ---
  const handleSplashScreenToggle = createToggleHandler(onChange,
    (v) => ({ general: { ...config.general, splash_screen: v } }));
  const handleOnboardingToggle = createToggleHandler(onChange,
    (v) => ({ general: { ...config.general, onboarding_enabled: v } }));
  const handleRestartOnboarding = useCallback(() => {
    // Clears `useOnboarding`'s live remembered-step map immediately, at
    // click time - not gated on this staged change reaching Save (#368).
    // See useOnboardingRestart.ts for why the config write below cannot be
    // relied on to signal this on its own.
    restartOnboardingProgress();
    onChange({
      general: {
        ...config.general,
        onboarding_enabled: true,
        onboarding_completed_flows: [],
        // Re-arms the guides for this session; App re-sets the flag once they arm.
        onboarding_first_launch_done: false,
      },
    });
  }, [config.general, onChange]);

  // --- Feedback ---
  const handleShortcutFeedbackChange = createSelectHandler(onChange,
    (v) => ({ general: { ...config.general, shortcut_feedback: v as ShortcutFeedback } }));

  // --- Language / format ---
  const handleLocaleChange = createSelectHandler(onChange,
    (v) => ({ general: { ...config.general, locale: v } }));
  const handleNumberFormatChange = createSelectHandler(onChange,
    (v) => ({ general: { ...config.general, number_format: v } }));
  const handleDateFormatChange = createSelectHandler(onChange,
    (v) => ({ general: { ...config.general, date_format: v } }));

  // --- Telemetry ---
  const handleTelemetryToggle = createToggleHandler(onChange,
    (v) => ({ general: { ...config.general, telemetry_enabled: v } }));

  // --- Updates (#579) ---
  const handleUpdateCheckToggle = createToggleHandler(onChange,
    (v) => ({ general: { ...config.general, update_check_enabled: v } }));
  const handleUpdateTrackChange = createSelectHandler(onChange,
    (v) => ({ general: { ...config.general, update_track: v as UpdateTrack } }));

  // Not `defaultConfig.general.update_track` - that snapshot always says
  // "stable" while the Rust default is build-dependent. See
  // `buildDefaultUpdateTrack`, which the category-wide reset shares.
  const defaultUpdateTrack: UpdateTrack = buildDefaultUpdateTrack();

  // The first-launch flag alone is enough to offer a restart: guides can arm and
  // then never fire (no project opened that session), leaving nothing completed
  // but no way for them to run again.
  const canRestartOnboarding =
    (config.general.onboarding_completed_flows?.length ?? 0) > 0 ||
    config.general.onboarding_first_launch_done ||
    !config.general.onboarding_enabled;

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'general')}
      aria-labelledby={getTabId(panelIdBase, 'general')}
      data-testid="settings-panel-general"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.general.title')}</h2>

      <SettingsSection title={t('settings.general.sections.language')} data-testid="section-language">
        <SettingRow
          label={t('settings.general.language.label')}
          helpText={t('settings.general.language.help')}
          isChanged={isFieldChanged(changedFields, 'general.locale')}
          onReset={() => onChange({ general: { ...config.general, locale: defaultConfig.general.locale } })}
          isDefault={config.general.locale === defaultConfig.general.locale}
          data-testid="setting-language"
        >
          <Select
            value={config.general.locale || 'system'}
            onChange={handleLocaleChange}
            options={[
              { value: 'system', label: t('settings.general.language.options.system') },
              { value: 'en-US', label: t('settings.general.language.options.enUS') },
            ]}
            className="w-48"
            aria-label={t('settings.general.language.aria')}
            data-testid="select-language"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.numberFormat.label')}
          helpText={t('settings.general.numberFormat.help')}
          isChanged={isFieldChanged(changedFields, 'general.number_format')}
          onReset={() => onChange({ general: { ...config.general, number_format: defaultConfig.general.number_format } })}
          isDefault={config.general.number_format === defaultConfig.general.number_format}
          data-testid="setting-number-format"
        >
          <Select
            value={config.general.number_format || 'locale'}
            onChange={handleNumberFormatChange}
            options={[
              { value: 'locale', label: t('settings.general.numberFormat.options.locale') },
              { value: 'en-US', label: t('settings.general.numberFormat.options.period') },
              { value: 'de-DE', label: t('settings.general.numberFormat.options.comma') },
              { value: 'fr-FR', label: t('settings.general.numberFormat.options.space') },
            ]}
            className="w-48"
            aria-label={t('settings.general.numberFormat.aria')}
            data-testid="select-number-format"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.dateFormat.label')}
          helpText={t('settings.general.dateFormat.help')}
          isChanged={isFieldChanged(changedFields, 'general.date_format')}
          onReset={() => onChange({ general: { ...config.general, date_format: defaultConfig.general.date_format } })}
          isDefault={config.general.date_format === defaultConfig.general.date_format}
          data-testid="setting-date-format"
        >
          <Select
            value={config.general.date_format || 'locale'}
            onChange={handleDateFormatChange}
            options={[
              { value: 'locale', label: t('settings.general.dateFormat.options.locale') },
              { value: 'en-US', label: t('settings.general.dateFormat.options.mdy') },
              { value: 'en-GB', label: t('settings.general.dateFormat.options.dmy') },
              { value: 'en-CA', label: t('settings.general.dateFormat.options.ymd') },
            ]}
            className="w-48"
            aria-label={t('settings.general.dateFormat.aria')}
            data-testid="select-date-format"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.general.sections.saving')} data-testid="section-saving">
        <SettingRow
          label={t('settings.general.autoSave.label')}
          helpText={t('settings.general.autoSave.help')}
          isChanged={isFieldChanged(changedFields, 'general.auto_save')}
          onReset={() => onChange({ general: { ...config.general, auto_save: defaultConfig.general.auto_save } })}
          isDefault={config.general.auto_save === defaultConfig.general.auto_save}
          data-testid="setting-auto-save"
        >
          <Toggle
            checked={config.general.auto_save}
            onChange={handleAutoSaveToggle}
            aria-label={t('settings.general.autoSave.aria')}
            data-testid="toggle-auto-save"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.autoSaveInterval.label')}
          helpText={t('settings.general.autoSaveInterval.help')}
          isChanged={isFieldChanged(changedFields, 'general.auto_save_interval_seconds')}
          onReset={() => onChange({ general: { ...config.general, auto_save_interval_seconds: defaultConfig.general.auto_save_interval_seconds } })}
          isDefault={config.general.auto_save_interval_seconds === defaultConfig.general.auto_save_interval_seconds}
          data-testid="setting-auto-save-interval"
        >
          <input
            type="number" inputMode="decimal"
            min={10}
            max={3600}
            value={config.general.auto_save_interval_seconds}
            onChange={(e) => handleAutoSaveIntervalChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.general.autoSaveInterval.aria')}
            data-testid="input-auto-save-interval"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.general.sections.recentProjects')} data-testid="section-recent">
        <SettingRow
          label={t('settings.general.recentProjectsMax.label')}
          helpText={t('settings.general.recentProjectsMax.help')}
          isChanged={isFieldChanged(changedFields, 'general.recent_projects_max')}
          onReset={() => onChange({ general: { ...config.general, recent_projects_max: defaultConfig.general.recent_projects_max } })}
          isDefault={config.general.recent_projects_max === defaultConfig.general.recent_projects_max}
          data-testid="setting-recent-max"
        >
          <input
            type="number" inputMode="decimal"
            min={1}
            max={50}
            value={config.general.recent_projects_max}
            onChange={(e) => handleRecentProjectsChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.general.recentProjectsMax.aria')}
            data-testid="input-recent-max"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.general.sections.undoHistory')} data-testid="section-undo">
        <SettingRow
          label={t('settings.general.undoHistoryLimit.label')}
          helpText={t('settings.general.undoHistoryLimit.help')}
          isChanged={isFieldChanged(changedFields, 'general.undo_history_limit')}
          onReset={() => onChange({ general: { ...config.general, undo_history_limit: defaultConfig.general.undo_history_limit } })}
          isDefault={config.general.undo_history_limit === defaultConfig.general.undo_history_limit}
          data-testid="setting-undo-limit"
        >
          <input
            type="number" inputMode="decimal"
            min={10}
            max={1000}
            value={config.general.undo_history_limit}
            onChange={(e) => handleUndoHistoryChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.general.undoHistoryLimit.aria')}
            data-testid="input-undo-limit"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.undoPersistLimit.label')}
          helpText={t('settings.general.undoPersistLimit.help')}
          isChanged={isFieldChanged(changedFields, 'general.undo_persist_limit')}
          onReset={() => onChange({ general: { ...config.general, undo_persist_limit: defaultConfig.general.undo_persist_limit } })}
          isDefault={config.general.undo_persist_limit === defaultConfig.general.undo_persist_limit}
          data-testid="setting-undo-persist"
        >
          <input
            type="number" inputMode="decimal"
            min={0}
            max={config.general.undo_history_limit}
            value={config.general.undo_persist_limit}
            onChange={(e) => handleUndoPersistChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.general.undoPersistLimit.aria')}
            data-testid="input-undo-persist"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.general.sections.feedback')} data-testid="section-feedback">
        <SettingRow
          label={t('settings.general.shortcutFeedback.label')}
          helpText={t('settings.general.shortcutFeedback.help')}
          isChanged={isFieldChanged(changedFields, 'general.shortcut_feedback')}
          onReset={() => onChange({ general: { ...config.general, shortcut_feedback: defaultConfig.general.shortcut_feedback } })}
          isDefault={config.general.shortcut_feedback === defaultConfig.general.shortcut_feedback}
          data-testid="setting-shortcut-feedback"
        >
          <Select
            value={config.general.shortcut_feedback}
            onChange={handleShortcutFeedbackChange}
            options={[
              { value: 'none', label: t('settings.general.shortcutFeedback.options.none') },
              { value: 'non_obvious', label: t('settings.general.shortcutFeedback.options.nonObvious') },
              { value: 'all', label: t('settings.general.shortcutFeedback.options.all') },
            ]}
            className="w-40"
            aria-label={t('settings.general.shortcutFeedback.aria')}
            data-testid="select-shortcut-feedback"
          />
        </SettingRow>
      </SettingsSection>


      <SettingsSection title={t('settings.general.sections.startup')} data-testid="section-startup">
        <SettingRow
          label={t('settings.general.splashScreen.label')}
          helpText={t('settings.general.splashScreen.help')}
          isChanged={isFieldChanged(changedFields, 'general.splash_screen')}
          onReset={() => onChange({ general: { ...config.general, splash_screen: defaultConfig.general.splash_screen } })}
          isDefault={config.general.splash_screen === defaultConfig.general.splash_screen}
          data-testid="setting-splash-screen"
        >
          <Toggle
            checked={config.general.splash_screen}
            onChange={handleSplashScreenToggle}
            aria-label={t('settings.general.splashScreen.aria')}
            data-testid="toggle-splash-screen"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.onboarding.enabled.label')}
          helpText={t('settings.general.onboarding.enabled.help')}
          isChanged={isFieldChanged(changedFields, 'general.onboarding_enabled')}
          onReset={() => onChange({ general: { ...config.general, onboarding_enabled: defaultConfig.general.onboarding_enabled } })}
          isDefault={config.general.onboarding_enabled === defaultConfig.general.onboarding_enabled}
          data-testid="setting-onboarding-enabled"
        >
          <Toggle
            checked={config.general.onboarding_enabled}
            onChange={handleOnboardingToggle}
            aria-label={t('settings.general.onboarding.enabled.aria')}
            data-testid="toggle-onboarding-enabled"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.onboarding.restart.label')}
          helpText={t('settings.general.onboarding.restart.help')}
          isChanged={isFieldChanged(changedFields, 'general.onboarding_completed_flows', 'general.onboarding_enabled', 'general.onboarding_first_launch_done')}
          onReset={handleRestartOnboarding}
          isDefault={!canRestartOnboarding}
          data-testid="setting-onboarding-restart"
        >
          <Button
            variant="secondary"
            onClick={handleRestartOnboarding}
            disabled={!canRestartOnboarding}
            data-testid="restart-onboarding-button"
          >
            {t('settings.general.onboarding.restart.action')}
          </Button>
        </SettingRow>
      </SettingsSection>

      {isTelemetryAvailable() && (
        <SettingsSection title={t('settings.general.sections.privacy', 'Privacy')} data-testid="section-privacy">
          <SettingRow
            label={t('settings.general.telemetry.label', 'Share anonymous usage statistics')}
            helpText={t('settings.general.telemetry.help', 'Help improve Stalagmight by sharing which features you use. No personal data or survey content is collected.')}
            isChanged={isFieldChanged(changedFields, 'general.telemetry_enabled')}
            onReset={() => onChange({ general: { ...config.general, telemetry_enabled: defaultConfig.general.telemetry_enabled } })}
            isDefault={config.general.telemetry_enabled === defaultConfig.general.telemetry_enabled}
            data-testid="setting-telemetry"
          >
            <Toggle
              checked={config.general.telemetry_enabled}
              onChange={handleTelemetryToggle}
              aria-label={t('settings.general.telemetry.aria', 'Toggle anonymous usage statistics')}
              data-testid="toggle-telemetry"
            />
          </SettingRow>
        </SettingsSection>
      )}

      <SettingsSection title={t('settings.general.sections.updates')} data-testid="section-updates">
        <SettingRow
          label={t('settings.general.updateCheck.label')}
          helpText={t('settings.general.updateCheck.help')}
          isChanged={isFieldChanged(changedFields, 'general.update_check_enabled')}
          onReset={() => onChange({ general: { ...config.general, update_check_enabled: defaultConfig.general.update_check_enabled } })}
          isDefault={config.general.update_check_enabled === defaultConfig.general.update_check_enabled}
          data-testid="setting-update-check"
        >
          <Toggle
            checked={config.general.update_check_enabled}
            onChange={handleUpdateCheckToggle}
            aria-label={t('settings.general.updateCheck.aria')}
            data-testid="toggle-update-check"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.general.updateTrack.label')}
          helpText={t('settings.general.updateTrack.help')}
          isChanged={isFieldChanged(changedFields, 'general.update_track')}
          onReset={() => onChange({ general: { ...config.general, update_track: defaultUpdateTrack } })}
          isDefault={config.general.update_track === defaultUpdateTrack}
          data-testid="setting-update-track"
        >
          <Select
            value={config.general.update_track}
            onChange={handleUpdateTrackChange}
            options={[
              { value: 'stable', label: t('settings.general.updateTrack.options.stable') },
              { value: 'weekly', label: t('settings.general.updateTrack.options.weekly') },
            ]}
            className="w-40"
            aria-label={t('settings.general.updateTrack.aria')}
            data-testid="select-update-track"
          />
        </SettingRow>
      </SettingsSection>
    </div>
  );
}
