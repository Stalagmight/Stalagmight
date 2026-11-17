import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Select } from '../../common/Select';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createSelectHandler, parseIntRadix10 } from './types';
import { defaultConfig, type LengthUnit, type AngleUnit, type DecimalPrecision } from '../../../types/config';

const precisionFields: Array<{ key: keyof DecimalPrecision; i18nKey: string }> = [
  { key: 'length', i18nKey: 'length' },
  { key: 'coordinates', i18nKey: 'coordinates' },
  { key: 'depth', i18nKey: 'depth' },
  { key: 'azimuth', i18nKey: 'azimuth' },
  { key: 'inclination', i18nKey: 'inclination' },
  { key: 'gps', i18nKey: 'gps' },
];

/**
 * Units settings category content.
 * Maps to config.units: length, depth, angle, decimal_precision (per-category).
 */
export function UnitsSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const hasErrorRef = useRef(false);

  // Validate on mount and when config changes
  useEffect(() => {
    const dp = config.units.decimal_precision;
    const hasError = precisionFields.some(
      ({ key }) => dp[key] < 0 || dp[key] > 6,
    );

    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('units', hasError);
    }
  }, [config.units, onValidationError]);

  const handleLengthChange = createSelectHandler(onChange,
    (v) => ({ units: { ...config.units, length: v as LengthUnit } }));
  const handleDepthChange = createSelectHandler(onChange,
    (v) => ({ units: { ...config.units, depth: v as LengthUnit } }));
  const handleAngleChange = createSelectHandler(onChange,
    (v) => ({ units: { ...config.units, angle: v as AngleUnit } }));

  const handlePrecisionChange = (field: keyof DecimalPrecision, value: string) => {
    const num = parseIntRadix10(value);
    if (!isNaN(num)) {
      onChange({
        units: {
          ...config.units,
          decimal_precision: {
            ...config.units.decimal_precision,
            [field]: num,
          },
        },
      });
    }
  };

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'units')}
      aria-labelledby={getTabId(panelIdBase, 'units')}
      data-testid="settings-panel-units"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.units.title')}</h2>

      <SettingsSection title={t('settings.units.sections.measurement')} data-testid="section-measurement">
        <SettingRow
          label={t('settings.units.length.label')}
          helpText={t('settings.units.length.help')}
          isChanged={isFieldChanged(changedFields, 'units.length')}
          onReset={() => onChange({ units: { ...config.units, length: defaultConfig.units.length } })}
          isDefault={config.units.length === defaultConfig.units.length}
          data-testid="setting-length-unit"
        >
          <Select
            value={config.units.length}
            onChange={handleLengthChange}
            options={[
              { value: 'meters', label: t('settings.units.length.options.meters') },
              { value: 'feet', label: t('settings.units.length.options.feet') },
            ]}
            className="w-32"
            aria-label={t('settings.units.length.aria')}
            data-testid="select-length-unit"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.units.depth.label')}
          helpText={t('settings.units.depth.help')}
          isChanged={isFieldChanged(changedFields, 'units.depth')}
          onReset={() => onChange({ units: { ...config.units, depth: defaultConfig.units.depth } })}
          isDefault={config.units.depth === defaultConfig.units.depth}
          data-testid="setting-depth-unit"
        >
          <Select
            value={config.units.depth}
            onChange={handleDepthChange}
            options={[
              { value: 'meters', label: t('settings.units.depth.options.meters') },
              { value: 'feet', label: t('settings.units.depth.options.feet') },
            ]}
            className="w-32"
            aria-label={t('settings.units.depth.aria')}
            data-testid="select-depth-unit"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.units.angle.label')}
          helpText={t('settings.units.angle.help')}
          isChanged={isFieldChanged(changedFields, 'units.angle')}
          onReset={() => onChange({ units: { ...config.units, angle: defaultConfig.units.angle } })}
          isDefault={config.units.angle === defaultConfig.units.angle}
          data-testid="setting-angle-unit"
        >
          <Select
            value={config.units.angle}
            onChange={handleAngleChange}
            options={[
              { value: 'degrees', label: t('settings.units.angle.options.degrees') },
              { value: 'gradians', label: t('settings.units.angle.options.gradians') },
            ]}
            className="w-32"
            aria-label={t('settings.units.angle.aria')}
            data-testid="select-angle-unit"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.units.sections.precision')} data-testid="section-precision">
        {precisionFields.map(({ key, i18nKey }) => (
          <SettingRow
            key={key}
            label={t(`settings.units.precision.${i18nKey}.label`)}
            helpText={t(`settings.units.precision.${i18nKey}.help`)}
            isChanged={isFieldChanged(changedFields, `units.decimal_precision.${key}`)}
            onReset={() => onChange({ units: { ...config.units, decimal_precision: { ...config.units.decimal_precision, [key]: defaultConfig.units.decimal_precision[key] } } })}
            isDefault={config.units.decimal_precision[key] === defaultConfig.units.decimal_precision[key]}
            data-testid={`setting-precision-${key}`}
          >
            <input
              type="number" inputMode="decimal"
              min={0}
              max={6}
              value={config.units.decimal_precision[key]}
              onChange={(e) => handlePrecisionChange(key, e.target.value)}
              className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t(`settings.units.precision.${i18nKey}.aria`)}
              data-testid={`input-precision-${key}`}
            />
          </SettingRow>
        ))}
      </SettingsSection>
    </div>
  );
}
