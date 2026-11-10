import { useEffect, useRef, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Select } from '../../common/Select';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createNumericHandler, createSelectHandler, parseIntRadix10 } from './types';
import { getThemedDisplayDefaults, getThemedSectionColorDefaults } from '../../../types/config';
import { useUIStore } from '../../../store/uiStore';
import {
  getUiScale,
  setUiScale,
  clampUiScale,
  UI_SCALE_MIN,
  UI_SCALE_MAX,
  UI_SCALE_STEP,
  UI_SCALE_DEFAULT,
  UI_SCALE_EVENT,
} from '../../../utils/uiScale';

/**
 * Display settings category content.
 * Maps to config.display + config.section_colors: grid settings, font
 * settings, measure-path colour, palette. The depth / distance colour palette
 * is not here: it lives on the map legend's cog (`[map_palette]`).
 */
export function DisplaySettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  const displayDefaults = getThemedDisplayDefaults(resolvedTheme);
  const sectionColorDefaults = getThemedSectionColorDefaults(resolvedTheme);

  // UI scale (whole-interface zoom). Lives in localStorage / webview zoom rather
  // than config; applies immediately like the Ctrl+= shortcut. Sync to external
  // changes (keyboard shortcut) while the dialog is open.
  const [uiScale, setUiScaleState] = useState<number>(() => getUiScale());
  useEffect(() => {
    const onScaleChange = (e: Event) => {
      const detail = (e as CustomEvent<number>).detail;
      if (typeof detail === 'number') setUiScaleState(detail);
    };
    window.addEventListener(UI_SCALE_EVENT, onScaleChange as EventListener);
    return () => window.removeEventListener(UI_SCALE_EVENT, onScaleChange as EventListener);
  }, []);
  const handleUiScaleChange = (value: number) => {
    setUiScaleState(clampUiScale(value));
    setUiScale(value);
  };

  // Validation state
  const hasErrorRef = useRef(false);

  // Validate on mount and when config changes
  useEffect(() => {
    const errors: string[] = [];

    // Validate font_size
    if (config.display.font_size < 8 || config.display.font_size > 32) {
      errors.push(t('settings.display.validation.fontSize'));
    }

    const hasError = errors.length > 0;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('display', hasError);
    }
  }, [config.display, onValidationError, t]);

  const gridSpacingOptions = useMemo(() => [
    { value: '1', label: t('settings.display.grid.spacingOptionOne', { value: 1 }) },
    { value: '5', label: t('settings.display.grid.spacingOptionOther', { value: 5 }) },
    { value: '10', label: t('settings.display.grid.spacingOptionOther', { value: 10 }) },
    { value: '25', label: t('settings.display.grid.spacingOptionOther', { value: 25 }) },
    { value: '50', label: t('settings.display.grid.spacingOptionOther', { value: 50 }) },
  ], [t]);

  // The "Background" colour picker that used to live here drove
  // `display.default_background`, which no renderer ever read (#357 item 10). The
  // field is gone; the map canvas comes from the theme's `--cc-map-background`.

  // --- Grid ---
  const handleGridColorChange = createSelectHandler(onChange,
    (v) => ({ display: { ...config.display, grid_color: v } }));
  const handleGridSpacingChange = createNumericHandler(onChange,
    (n) => ({ display: { ...config.display, grid_spacing: n } }), parseIntRadix10);

  // --- Font ---
  const handleFontSizeChange = createNumericHandler(onChange,
    (n) => ({ display: { ...config.display, font_size: n } }), parseIntRadix10);
  const handleFontColorChange = createSelectHandler(onChange,
    (v) => ({ display: { ...config.display, font_color: v } }));

  // --- Measure tool (#26) ---
  const handleMeasurePathColorChange = createSelectHandler(onChange,
    (v) => ({ display: { ...config.display, measure_path_color: v } }));

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'display')}
      aria-labelledby={getTabId(panelIdBase, 'display')}
      data-testid="settings-panel-display"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.display.title')}</h2>

      <SettingsSection title={t('settings.display.sections.interface')} data-testid="section-interface">
        <SettingRow
          label={t('settings.display.uiScale.label')}
          helpText={t('settings.display.uiScale.help')}
          isChanged={uiScale !== UI_SCALE_DEFAULT}
          data-testid="setting-ui-scale"
          onReset={() => handleUiScaleChange(UI_SCALE_DEFAULT)}
          isDefault={uiScale === UI_SCALE_DEFAULT}
        >
          <div className="flex items-center gap-3">
            <input
              type="range"
              min={UI_SCALE_MIN}
              max={UI_SCALE_MAX}
              step={UI_SCALE_STEP}
              value={uiScale}
              onChange={(e) => handleUiScaleChange(Number(e.target.value))}
              className="w-40 cursor-pointer"
              aria-label={t('settings.display.uiScale.aria')}
              data-testid="input-ui-scale"
            />
            <span
              className="w-12 text-sm text-right tabular-nums text-text-secondary"
              data-testid="ui-scale-value"
            >
              {Math.round(uiScale * 100)}%
            </span>
          </div>
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.display.sections.grid')} data-testid="section-grid">
        <SettingRow
          label={t('settings.display.grid.color.label')}
          helpText={t('settings.display.grid.color.help')}
          isChanged={isFieldChanged(changedFields, 'display.grid_color')}
          data-testid="setting-grid-color"
          onReset={() => onChange({ display: { ...config.display, grid_color: displayDefaults.grid_color } })}
          isDefault={config.display.grid_color === displayDefaults.grid_color}
        >
          <input
            type="color"
            value={config.display.grid_color}
            onChange={(e) => handleGridColorChange(e.target.value)}
            className="w-12 h-8 rounded border border-border cursor-pointer"
            aria-label={t('settings.display.grid.color.aria')}
            data-testid="input-grid-color"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.display.grid.spacing.label')}
          helpText={t('settings.display.grid.spacing.help')}
          isChanged={isFieldChanged(changedFields, 'display.grid_spacing')}
          data-testid="setting-grid-spacing"
          onReset={() => onChange({ display: { ...config.display, grid_spacing: displayDefaults.grid_spacing } })}
          isDefault={config.display.grid_spacing === displayDefaults.grid_spacing}
        >
          <Select
            value={String(config.display.grid_spacing)}
            onChange={handleGridSpacingChange}
            options={gridSpacingOptions}
            className="w-32"
            aria-label={t('settings.display.grid.spacing.aria')}
            data-testid="select-grid-spacing"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.display.sections.font')} data-testid="section-font">
        <SettingRow
          label={t('settings.display.font.size.label')}
          helpText={t('settings.display.font.size.help')}
          isChanged={isFieldChanged(changedFields, 'display.font_size')}
          data-testid="setting-font-size"
          onReset={() => onChange({ display: { ...config.display, font_size: displayDefaults.font_size } })}
          isDefault={config.display.font_size === displayDefaults.font_size}
        >
          <input
            type="number" inputMode="decimal"
            min={8}
            max={32}
            value={config.display.font_size}
            onChange={(e) => handleFontSizeChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.display.font.size.aria')}
            data-testid="input-font-size"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.display.font.color.label')}
          helpText={t('settings.display.font.color.help')}
          isChanged={isFieldChanged(changedFields, 'display.font_color')}
          data-testid="setting-font-color"
          onReset={() => onChange({ display: { ...config.display, font_color: displayDefaults.font_color } })}
          isDefault={config.display.font_color === displayDefaults.font_color}
        >
          <input
            type="color"
            value={config.display.font_color}
            onChange={(e) => handleFontColorChange(e.target.value)}
            className="w-12 h-8 rounded border border-border cursor-pointer"
            aria-label={t('settings.display.font.color.aria')}
            data-testid="input-font-color"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.display.sections.measure')} data-testid="section-measure">
        <SettingRow
          label={t('settings.display.measurePathColor.label')}
          helpText={t('settings.display.measurePathColor.help')}
          isChanged={isFieldChanged(changedFields, 'display.measure_path_color')}
          data-testid="setting-measure-path-color"
          onReset={() => onChange({ display: { ...config.display, measure_path_color: displayDefaults.measure_path_color } })}
          isDefault={config.display.measure_path_color === displayDefaults.measure_path_color}
        >
          <input
            type="color"
            value={config.display.measure_path_color}
            onChange={(e) => handleMeasurePathColorChange(e.target.value)}
            className="w-12 h-8 rounded border border-border cursor-pointer"
            aria-label={t('settings.display.measurePathColor.aria')}
            data-testid="input-measure-path-color"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.display.sections.sectionColors')} data-testid="section-colors-palette">
        <SettingRow
          label={t('settings.display.sectionColors.label')}
          helpText={t('settings.display.sectionColors.help')}
          isChanged={isFieldChanged(changedFields, 'section_colors.palette')}
          data-testid="setting-color-palette"
          onReset={() => onChange({ section_colors: { palette: [...sectionColorDefaults.palette] } })}
          isDefault={JSON.stringify(config.section_colors.palette) === JSON.stringify(sectionColorDefaults.palette)}
        >
          <div className="flex gap-1">
            {config.section_colors.palette.map((color, index) => (
              <input
                key={index}
                type="color"
                value={color}
                onChange={(e) => {
                  const newPalette = [...config.section_colors.palette];
                  newPalette[index] = e.target.value;
                  onChange({
                    section_colors: {
                      palette: newPalette,
                    },
                  });
                }}
                className="w-6 h-6 rounded border border-border cursor-pointer"
                aria-label={t('settings.display.sectionColors.aria', { index: index + 1 })}
                data-testid={`input-palette-color-${index}`}
              />
            ))}
          </div>
        </SettingRow>
      </SettingsSection>
    </div>
  );
}
