import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged } from './types';
import { defaultConfig } from '../../../types/config';
import type { ThemeMode, ColorTheme } from '../../../types/config';
import { ThemePreviewCard, ColorThemeCircle } from './ThemePreviewCard';
import { SyntacticHighlightingSettings } from './SyntacticHighlightingSettings';
import { Select } from '../../common/Select';
import type { SelectOption } from '../../common/Select';

const THEME_MODE_OPTIONS: SelectOption[] = [
  { value: 'dark', label: '' },
  { value: 'light', label: '' },
  { value: 'auto', label: '' },
];

const COLOR_THEMES: ColorTheme[] = ['dark', 'cave', 'dracula', 'one_dark', 'nord', 'catppuccin_mocha', 'light', 'solarized_light', 'catppuccin_latte', 'kaleido_eink'];

/**
 * Appearance settings category content.
 * Maps to config.appearance: theme, day_color_theme, night_color_theme.
 */
export function AppearanceSettings({
  config,
  onChange,
  changedFields,
  onValidationError: _onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();

  const currentMode = config.appearance.theme;
  const dayColorTheme = config.appearance.day_color_theme;
  const nightColorTheme = config.appearance.night_color_theme;

  // Build translated options (labels depend on t() which can't be called at module level)
  const translatedOptions: SelectOption[] = THEME_MODE_OPTIONS.map((opt) => ({
    ...opt,
    label: t(`settings.appearance.theme.options.${opt.value}.label`),
  }));

  const handleThemeChange = (value: ThemeMode) => {
    onChange({
      appearance: {
        ...config.appearance,
        theme: value,
      },
    });
  };

  const handleDayColorThemeChange = (value: ColorTheme) => {
    onChange({
      appearance: {
        ...config.appearance,
        day_color_theme: value,
      },
    });
  };

  const handleNightColorThemeChange = (value: ColorTheme) => {
    onChange({
      appearance: {
        ...config.appearance,
        night_color_theme: value,
      },
    });
  };

  // Determine which mode is currently active (resolved from mode + OS)
  const resolvedMode: 'dark' | 'light' =
    currentMode === 'auto'
      ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
      : currentMode === 'light'
        ? 'light'
        : 'dark';

  // Description text for each section - only shown in auto mode
  const dayDescription =
    currentMode === 'auto'
      ? t('settings.appearance.theme.lightCard.description')
      : undefined;
  const nightDescription =
    currentMode === 'auto'
      ? t('settings.appearance.theme.darkCard.description')
      : undefined;

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'appearance')}
      aria-labelledby={getTabId(panelIdBase, 'appearance')}
      data-testid="settings-panel-appearance"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.appearance.title')}</h2>

      <SettingsSection title={t('settings.appearance.theme.title')} data-testid="section-theme">
        <SettingRow
          label={t('settings.appearance.theme.mode')}
          helpText={t('settings.appearance.theme.help')}
          isChanged={isFieldChanged(changedFields, 'appearance.theme')}
          onReset={() => onChange({ appearance: { ...config.appearance, theme: defaultConfig.appearance.theme } })}
          isDefault={config.appearance.theme === defaultConfig.appearance.theme}
          data-testid="setting-theme"
        >
          <Select
            value={currentMode}
            onChange={(value) => handleThemeChange(value as ThemeMode)}
            options={translatedOptions}
            aria-label={t('settings.appearance.theme.aria')}
            data-testid="theme-mode-select"
          />
        </SettingRow>

        {/* Side-by-side theme cards */}
        <div className="flex gap-3 mt-3" data-testid="theme-preview-cards">
          {/* Day theme section */}
          <div className="flex-1 min-w-0" data-testid="theme-section-day">
            <ThemePreviewCard
              mode="day"
              colorTheme={dayColorTheme}
              isActive={resolvedMode === 'light'}
              description={dayDescription}
            />
            {/* Color theme circles */}
            <div className="flex flex-wrap gap-2 mt-2">
              {COLOR_THEMES.map((ct) => (
                <ColorThemeCircle
                  key={ct}
                  colorTheme={ct}
                  isSelected={dayColorTheme === ct}
                  onClick={() => handleDayColorThemeChange(ct)}
                />
              ))}
            </div>
          </div>

          {/* Night theme section */}
          <div className="flex-1 min-w-0" data-testid="theme-section-night">
            <ThemePreviewCard
              mode="night"
              colorTheme={nightColorTheme}
              isActive={resolvedMode === 'dark'}
              description={nightDescription}
            />
            {/* Color theme circles */}
            <div className="flex flex-wrap gap-2 mt-2">
              {COLOR_THEMES.map((ct) => (
                <ColorThemeCircle
                  key={ct}
                  colorTheme={ct}
                  isSelected={nightColorTheme === ct}
                  onClick={() => handleNightColorThemeChange(ct)}
                />
              ))}
            </div>
          </div>
        </div>
      </SettingsSection>

      <SyntacticHighlightingSettings
        config={config}
        onChange={onChange}
        changedFields={changedFields}
      />
    </div>
  );
}
