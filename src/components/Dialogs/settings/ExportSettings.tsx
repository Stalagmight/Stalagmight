import { useEffect, useRef, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Toggle } from '../../common/Toggle';
import { Select } from '../../common/Select';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createNumericHandler, createToggleHandler, createSelectHandler, parseIntRadix10 } from './types';
import { defaultConfig } from '../../../types/config';

/**
 * Export settings category content.
 * Maps to config.export: default_image_format, default_dpi,
 * default_include_satellite/grid/labels.
 */
export function ExportSettings({
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

    // Validate DPI
    if (config.export.default_dpi < 72 || config.export.default_dpi > 1200) {
      errors.push(t('settings.export.validation.dpi'));
    }

    const hasError = errors.length > 0;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('export', hasError);
    }
  }, [config.export, onValidationError, t]);

  const imageFormatOptions = useMemo(() => [
    { value: 'png', label: t('settings.export.format.options.png') },
    { value: 'svg', label: t('settings.export.format.options.svg') },
    { value: 'pdf', label: t('settings.export.format.options.pdf') },
  ], [t]);

  const dpiOptions = useMemo(() => [
    { value: '72', label: t('settings.export.dpi.options.screen') },
    { value: '150', label: t('settings.export.dpi.options.lowPrint') },
    { value: '300', label: t('settings.export.dpi.options.standardPrint') },
    { value: '600', label: t('settings.export.dpi.options.highQuality') },
  ], [t]);

  const handleFormatChange = createSelectHandler(onChange,
    (v) => ({ export: { ...config.export, default_image_format: v } }));
  const handleDpiChange = createNumericHandler(onChange,
    (n) => ({ export: { ...config.export, default_dpi: n } }), parseIntRadix10);
  const handleIncludeSatelliteChange = createToggleHandler(onChange,
    (v) => ({ export: { ...config.export, default_include_satellite: v } }));
  const handleIncludeGridChange = createToggleHandler(onChange,
    (v) => ({ export: { ...config.export, default_include_grid: v } }));
  const handleIncludeLabelsChange = createToggleHandler(onChange,
    (v) => ({ export: { ...config.export, default_include_labels: v } }));

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'export')}
      aria-labelledby={getTabId(panelIdBase, 'export')}
      data-testid="settings-panel-export"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.export.title')}</h2>

      <SettingsSection title={t('settings.export.sections.imageFormat')} data-testid="section-image-format">
        <SettingRow
          label={t('settings.export.format.label')}
          helpText={t('settings.export.format.help')}
          isChanged={isFieldChanged(changedFields, 'export.default_image_format')}
          onReset={() => onChange({ export: { ...config.export, default_image_format: defaultConfig.export.default_image_format } })}
          isDefault={config.export.default_image_format === defaultConfig.export.default_image_format}
          data-testid="setting-image-format"
        >
          <Select
            value={config.export.default_image_format}
            onChange={handleFormatChange}
            options={imageFormatOptions}
            className="w-32"
            aria-label={t('settings.export.format.aria')}
            data-testid="select-image-format"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.export.dpi.label')}
          helpText={t('settings.export.dpi.help')}
          isChanged={isFieldChanged(changedFields, 'export.default_dpi')}
          onReset={() => onChange({ export: { ...config.export, default_dpi: defaultConfig.export.default_dpi } })}
          isDefault={config.export.default_dpi === defaultConfig.export.default_dpi}
          data-testid="setting-dpi"
        >
          <Select
            value={String(config.export.default_dpi)}
            onChange={handleDpiChange}
            options={dpiOptions}
            className="w-48"
            aria-label={t('settings.export.dpi.aria')}
            data-testid="select-dpi"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.export.sections.layers')} data-testid="section-default-layers">
        <SettingRow
          label={t('settings.export.layers.satellite.label')}
          helpText={t('settings.export.layers.satellite.help')}
          isChanged={isFieldChanged(changedFields, 'export.default_include_satellite')}
          onReset={() => onChange({ export: { ...config.export, default_include_satellite: defaultConfig.export.default_include_satellite } })}
          isDefault={config.export.default_include_satellite === defaultConfig.export.default_include_satellite}
          data-testid="setting-include-satellite"
        >
          <Toggle
            checked={config.export.default_include_satellite}
            onChange={handleIncludeSatelliteChange}
            aria-label={t('settings.export.layers.satellite.aria')}
            data-testid="toggle-include-satellite"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.export.layers.grid.label')}
          helpText={t('settings.export.layers.grid.help')}
          isChanged={isFieldChanged(changedFields, 'export.default_include_grid')}
          onReset={() => onChange({ export: { ...config.export, default_include_grid: defaultConfig.export.default_include_grid } })}
          isDefault={config.export.default_include_grid === defaultConfig.export.default_include_grid}
          data-testid="setting-include-grid"
        >
          <Toggle
            checked={config.export.default_include_grid}
            onChange={handleIncludeGridChange}
            aria-label={t('settings.export.layers.grid.aria')}
            data-testid="toggle-include-grid"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.export.layers.labels.label')}
          helpText={t('settings.export.layers.labels.help')}
          isChanged={isFieldChanged(changedFields, 'export.default_include_labels')}
          onReset={() => onChange({ export: { ...config.export, default_include_labels: defaultConfig.export.default_include_labels } })}
          isDefault={config.export.default_include_labels === defaultConfig.export.default_include_labels}
          data-testid="setting-include-labels"
        >
          <Toggle
            checked={config.export.default_include_labels}
            onChange={handleIncludeLabelsChange}
            aria-label={t('settings.export.layers.labels.aria')}
            data-testid="toggle-include-labels"
          />
        </SettingRow>
      </SettingsSection>
    </div>
  );
}
