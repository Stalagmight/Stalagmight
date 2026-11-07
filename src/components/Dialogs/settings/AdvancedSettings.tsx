import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { invokeIpc } from '../../../utils/ipc';
import { chainSettingWrite, SETTING_KEYS } from '../../../utils/settingWriteChain';
import { Toggle } from '../../common/Toggle';
import { Select } from '../../common/Select';
import { Button } from '../../common/Button';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createNumericHandler, createToggleHandler, createSelectHandler, parseIntRadix10 } from './types';
import { defaultConfig } from '../../../types/config';
import type { Config, PlacementSuggestionConfig } from '../../../types/config';
import { useProjectStore } from '../../../store/projectStore';
import { PANEL_SIZE_BOUNDS, DATA_TABLE_MIN_HEIGHT, DATA_TABLE_MAX_HEIGHT } from '../../SingleSystemView/layoutConstants';

/** Reusable row for placement suggestion numeric fields. */
function PsRow({ field, i18nKey, step, min, max, config, changedFields, onChange, onInput, t }: {
  field: keyof PlacementSuggestionConfig;
  i18nKey: string;
  step: number;
  min: number;
  max: number;
  config: Config;
  changedFields: Set<string>;
  onChange: (updates: Partial<Config>) => void;
  onInput: (v: string) => void;
  t: (key: string) => string;
}) {
  const ps = config.placement_suggestion ?? defaultConfig.placement_suggestion;
  const dps = defaultConfig.placement_suggestion;
  const path = `placement_suggestion.${field}`;
  return (
    <SettingRow
      label={t(`settings.advanced.placementSuggestion.${i18nKey}.label`)}
      helpText={t(`settings.advanced.placementSuggestion.${i18nKey}.help`)}
      isChanged={isFieldChanged(changedFields, path)}
      onReset={() => onChange({ placement_suggestion: { ...ps, [field]: dps[field] } })}
      isDefault={ps[field] === dps[field]}
      data-testid={`setting-ps-${field}`}
    >
      <input
        type="number" inputMode="decimal"
        min={min}
        max={max}
        step={step}
        value={ps[field]}
        onChange={(e) => onInput(e.target.value)}
        className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
        aria-label={t(`settings.advanced.placementSuggestion.${i18nKey}.aria`)}
        data-testid={`input-ps-${field}`}
      />
    </SettingRow>
  );
}

/**
 * Compute differences between two config objects for reload display.
 */
function computeConfigDiff(
  oldConfig: Config,
  newConfig: Config
): Array<{ path: string; oldValue: string; newValue: string }> {
  const changes: Array<{ path: string; oldValue: string; newValue: string }> = [];

  // Check debug.map settings
  if (oldConfig.debug.map.show_hitboxes !== newConfig.debug.map.show_hitboxes) {
    changes.push({
      path: 'debug.map.show_hitboxes',
      oldValue: String(oldConfig.debug.map.show_hitboxes),
      newValue: String(newConfig.debug.map.show_hitboxes),
    });
  }
  if (oldConfig.debug.map.log_hit_detection !== newConfig.debug.map.log_hit_detection) {
    changes.push({
      path: 'debug.map.log_hit_detection',
      oldValue: String(oldConfig.debug.map.log_hit_detection),
      newValue: String(newConfig.debug.map.log_hit_detection),
    });
  }

  // Check logging settings
  if (oldConfig.logging.level !== newConfig.logging.level) {
    changes.push({
      path: 'logging.level',
      oldValue: oldConfig.logging.level,
      newValue: newConfig.logging.level,
    });
  }
  if (oldConfig.logging.max_directory_size_mb !== newConfig.logging.max_directory_size_mb) {
    changes.push({
      path: 'logging.max_directory_size_mb',
      oldValue: String(oldConfig.logging.max_directory_size_mb),
      newValue: String(newConfig.logging.max_directory_size_mb),
    });
  }

  // Check panel settings
  if (oldConfig.panels.sections_panel_width !== newConfig.panels.sections_panel_width) {
    changes.push({
      path: 'panels.sections_panel_width',
      oldValue: String(oldConfig.panels.sections_panel_width),
      newValue: String(newConfig.panels.sections_panel_width),
    });
  }
  if (oldConfig.panels.detail_panel_width !== newConfig.panels.detail_panel_width) {
    changes.push({
      path: 'panels.detail_panel_width',
      oldValue: String(oldConfig.panels.detail_panel_width),
      newValue: String(newConfig.panels.detail_panel_width),
    });
  }
  if (oldConfig.panels.data_table_height !== newConfig.panels.data_table_height) {
    changes.push({
      path: 'panels.data_table_height',
      oldValue: String(oldConfig.panels.data_table_height),
      newValue: String(newConfig.panels.data_table_height),
    });
  }

  // Check closure adjustment settings
  if (oldConfig.closure_adjustment.base_sigma_length_m !== newConfig.closure_adjustment.base_sigma_length_m) {
    changes.push({
      path: 'closure_adjustment.base_sigma_length_m',
      oldValue: String(oldConfig.closure_adjustment.base_sigma_length_m),
      newValue: String(newConfig.closure_adjustment.base_sigma_length_m),
    });
  }
  if (oldConfig.closure_adjustment.base_sigma_azimuth_deg !== newConfig.closure_adjustment.base_sigma_azimuth_deg) {
    changes.push({
      path: 'closure_adjustment.base_sigma_azimuth_deg',
      oldValue: String(oldConfig.closure_adjustment.base_sigma_azimuth_deg),
      newValue: String(newConfig.closure_adjustment.base_sigma_azimuth_deg),
    });
  }
  if (oldConfig.closure_adjustment.base_sigma_depth_m !== newConfig.closure_adjustment.base_sigma_depth_m) {
    changes.push({
      path: 'closure_adjustment.base_sigma_depth_m',
      oldValue: String(oldConfig.closure_adjustment.base_sigma_depth_m),
      newValue: String(newConfig.closure_adjustment.base_sigma_depth_m),
    });
  }
  // Check placement suggestion settings
  const oldPs = oldConfig.placement_suggestion;
  const newPs = newConfig.placement_suggestion;
  if (oldPs && newPs) {
    const psFields: Array<keyof PlacementSuggestionConfig> = [
      'azimuth_sigma', 'length_sigma', 'depth_sigma',
      'azimuth_weight', 'length_weight', 'depth_in_weight', 'depth_out_weight',
      'min_shot_score', 'min_aggregate_score', 'max_reference_shots', 'max_suggestions',
      'min_partial_ratio', 'short_shot_threshold', 'single_shot_penalty',
      'max_azimuth_error_full', 'max_azimuth_error_partial', 'max_depth_error',
      'max_length_error_fraction', 'max_length_error_floor',
    ];
    for (const f of psFields) {
      if (oldPs[f] !== newPs[f]) {
        changes.push({
          path: `placement_suggestion.${f}`,
          oldValue: String(oldPs[f]),
          newValue: String(newPs[f]),
        });
      }
    }
  }
  return changes;
}

/**
 * Advanced settings category content.
 * Maps to config.debug + config.logging + config.panels, plus Reload Config button.
 */
export function AdvancedSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const setConfig = useProjectStore((s) => s.setConfig);
  const [isReloading, setIsReloading] = useState(false);
  const [reloadMessage, setReloadMessage] = useState<string | null>(null);
  const mountedRef = useRef(true);

  // Validation state
  const hasErrorRef = useRef(false);

  // Validate on mount and when config changes
  useEffect(() => {
    const errors: string[] = [];

    // Validate logging max directory size
    if (config.logging.max_directory_size_mb < 1 || config.logging.max_directory_size_mb > 10000) {
      errors.push(t('settings.advanced.validation.logSize'));
    }

    // Validate panel dimensions against the same bounds the backend clamps to
    // (PANEL_SIZE_BOUNDS / DATA_TABLE_MIN_HEIGHT / DATA_TABLE_MAX_HEIGHT), so
    // validation can never accept a value the save round-trip then silently
    // snaps back (#357 item 18).
    if (config.panels.sections_panel_width < PANEL_SIZE_BOUNDS.sections.min || config.panels.sections_panel_width > PANEL_SIZE_BOUNDS.sections.max) {
      errors.push(t('settings.advanced.validation.sectionsPanel', { min: PANEL_SIZE_BOUNDS.sections.min, max: PANEL_SIZE_BOUNDS.sections.max }));
    }
    if (config.panels.detail_panel_width < PANEL_SIZE_BOUNDS.detail.min || config.panels.detail_panel_width > PANEL_SIZE_BOUNDS.detail.max) {
      errors.push(t('settings.advanced.validation.detailPanel', { min: PANEL_SIZE_BOUNDS.detail.min, max: PANEL_SIZE_BOUNDS.detail.max }));
    }
    if (config.panels.data_table_height < DATA_TABLE_MIN_HEIGHT || config.panels.data_table_height > DATA_TABLE_MAX_HEIGHT) {
      errors.push(t('settings.advanced.validation.dataTable', { min: DATA_TABLE_MIN_HEIGHT, max: DATA_TABLE_MAX_HEIGHT }));
    }

    if (config.closure_adjustment.base_sigma_length_m <= 0) {
      errors.push(t('settings.advanced.validation.closureSigmaLength'));
    }
    if (config.closure_adjustment.base_sigma_azimuth_deg <= 0) {
      errors.push(t('settings.advanced.validation.closureSigmaAzimuth'));
    }
    if (config.closure_adjustment.base_sigma_depth_m <= 0) {
      errors.push(t('settings.advanced.validation.closureSigmaDepth'));
    }

    // Validate placement suggestion sigma values
    const ps = config.placement_suggestion;
    if (ps) {
      if (ps.azimuth_sigma <= 0 || ps.length_sigma <= 0 || ps.depth_sigma <= 0) {
        errors.push(t('settings.advanced.validation.placementSigma'));
      }
    }

    const hasError = errors.length > 0;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('advanced', hasError);
    }
  }, [config.logging, config.panels, config.closure_adjustment, config.placement_suggestion, onValidationError, t]);

  const logLevelOptions = useMemo(() => [
    { value: 'trace', label: t('settings.advanced.logging.level.options.trace') },
    { value: 'debug', label: t('settings.advanced.logging.level.options.debug') },
    { value: 'info', label: t('settings.advanced.logging.level.options.info') },
    { value: 'warn', label: t('settings.advanced.logging.level.options.warn') },
    { value: 'error', label: t('settings.advanced.logging.level.options.error') },
  ], [t]);

  // Cleanup on unmount
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // --- Debug settings ---
  const handleShowHitboxesChange = createToggleHandler(onChange,
    (v) => ({ debug: { ...config.debug, map: { ...config.debug.map, show_hitboxes: v } } }));
  const handleLogHitDetectionChange = createToggleHandler(onChange,
    (v) => ({ debug: { ...config.debug, map: { ...config.debug.map, log_hit_detection: v } } }));

  // --- Logging settings ---
  const handleLogLevelChange = createSelectHandler(onChange,
    (v) => ({ logging: { ...config.logging, level: v } }));
  const handleMaxLogSizeChange = createNumericHandler(onChange,
    (n) => ({ logging: { ...config.logging, max_directory_size_mb: n } }), parseIntRadix10);

  // --- Panel settings ---
  const handleSectionsPanelWidthChange = createNumericHandler(onChange,
    (n) => ({ panels: { ...config.panels, sections_panel_width: n } }), parseIntRadix10);
  const handleDetailPanelWidthChange = createNumericHandler(onChange,
    (n) => ({ panels: { ...config.panels, detail_panel_width: n } }), parseIntRadix10);
  const handleDataTableHeightChange = createNumericHandler(onChange,
    (n) => ({ panels: { ...config.panels, data_table_height: n } }), parseIntRadix10);

  // --- Closure adjustment settings ---
  const handleSigmaLengthChange = createNumericHandler(onChange,
    (n) => ({ closure_adjustment: { ...config.closure_adjustment, base_sigma_length_m: n } }));
  const handleSigmaAzimuthChange = createNumericHandler(onChange,
    (n) => ({ closure_adjustment: { ...config.closure_adjustment, base_sigma_azimuth_deg: n } }));
  const handleSigmaDepthChange = createNumericHandler(onChange,
    (n) => ({ closure_adjustment: { ...config.closure_adjustment, base_sigma_depth_m: n } }));

  // --- Placement suggestion settings ---
  const psBase = config.placement_suggestion ?? defaultConfig.placement_suggestion;
  const psUpdate = (field: string, n: number) =>
    ({ placement_suggestion: { ...psBase, [field]: n } });
  const handlePsAzimuthWeight = createNumericHandler(onChange, (n) => psUpdate('azimuth_weight', n));
  const handlePsLengthWeight = createNumericHandler(onChange, (n) => psUpdate('length_weight', n));
  const handlePsDepthInWeight = createNumericHandler(onChange, (n) => psUpdate('depth_in_weight', n));
  const handlePsDepthOutWeight = createNumericHandler(onChange, (n) => psUpdate('depth_out_weight', n));
  const handlePsAzimuthSigma = createNumericHandler(onChange, (n) => psUpdate('azimuth_sigma', n));
  const handlePsLengthSigma = createNumericHandler(onChange, (n) => psUpdate('length_sigma', n));
  const handlePsDepthSigma = createNumericHandler(onChange, (n) => psUpdate('depth_sigma', n));
  const handlePsMinShotScore = createNumericHandler(onChange, (n) => psUpdate('min_shot_score', n));
  const handlePsMinAggregateScore = createNumericHandler(onChange, (n) => psUpdate('min_aggregate_score', n));
  const handlePsSingleShotPenalty = createNumericHandler(onChange, (n) => psUpdate('single_shot_penalty', n));
  const handlePsMaxAzErrorFull = createNumericHandler(onChange, (n) => psUpdate('max_azimuth_error_full', n));
  const handlePsMaxAzErrorPartial = createNumericHandler(onChange, (n) => psUpdate('max_azimuth_error_partial', n));
  const handlePsMaxDepthError = createNumericHandler(onChange, (n) => psUpdate('max_depth_error', n));
  const handlePsMaxLenErrorFrac = createNumericHandler(onChange, (n) => psUpdate('max_length_error_fraction', n));
  const handlePsMaxLenErrorFloor = createNumericHandler(onChange, (n) => psUpdate('max_length_error_floor', n));
  const handlePsMaxRefShots = createNumericHandler(onChange, (n) => psUpdate('max_reference_shots', n), parseIntRadix10);
  const handlePsMaxSuggestions = createNumericHandler(onChange, (n) => psUpdate('max_suggestions', n), parseIntRadix10);
  const handlePsMinPartialRatio = createNumericHandler(onChange, (n) => psUpdate('min_partial_ratio', n));
  const handlePsShortShotThresh = createNumericHandler(onChange, (n) => psUpdate('short_shot_threshold', n));

  // Reload config from disk
  const handleReloadConfig = useCallback(async () => {
    setIsReloading(true);
    setReloadMessage(null);
    try {
      // [#576 RV9b-3] A config re-read, so in the config chain like every other
      // (`reloadConfigInOrder`): issued while a settings save is in flight, it
      // could answer with the file before the save and land in the store after
      // the save's re-read. The store write is part of the same step.
      // [#576 NF-W10FE-2] Unconditional: the backend has reloaded whether or not
      // this tab is still open, and the store is global - skipping it after an
      // unmount left the app on the old config until the next re-read.
      const newConfig = await chainSettingWrite(SETTING_KEYS.config, async () => {
        const fresh = await invokeIpc('reload_config', undefined, { purpose: 'reload config from disk' });
        setConfig(fresh);
        return fresh;
      });

      if (!mountedRef.current) return;

      const changes = computeConfigDiff(config, newConfig);

      // The store took the reloaded config above; the local dialog state follows
      // via onChange, so the UI reflects it and unsaved changes are cleared.
      onChange(newConfig);

      if (changes.length > 0) {
        const changeList = changes
          .map((c) => `${c.path}: ${c.oldValue} -> ${c.newValue}`)
          .join('\n');
        setReloadMessage(t('settings.advanced.reload.success', { changes: changeList }));
      } else {
        setReloadMessage(t('settings.advanced.reload.noChanges'));
      }
    } catch (error) {
      console.error('[AdvancedSettings] Failed to reload config:', error);
      if (mountedRef.current) {
        setReloadMessage(t('settings.advanced.reload.error', { error: String(error) }));
      }
    } finally {
      if (mountedRef.current) {
        setIsReloading(false);
      }
    }
  }, [config, setConfig, onChange, t]);

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'advanced')}
      aria-labelledby={getTabId(panelIdBase, 'advanced')}
      data-testid="settings-panel-advanced"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.advanced.title')}</h2>

      <SettingsSection title={t('settings.advanced.sections.debug')} data-testid="section-debug">
        <SettingRow
          label={t('settings.advanced.debug.showHitboxes.label')}
          helpText={t('settings.advanced.debug.showHitboxes.help')}
          isChanged={isFieldChanged(changedFields, 'debug.map.show_hitboxes')}
          onReset={() => onChange({ debug: { ...config.debug, map: { ...config.debug.map, show_hitboxes: defaultConfig.debug.map.show_hitboxes } } })}
          isDefault={config.debug.map.show_hitboxes === defaultConfig.debug.map.show_hitboxes}
          data-testid="setting-show-hitboxes"
        >
          <Toggle
            checked={config.debug.map.show_hitboxes}
            onChange={handleShowHitboxesChange}
            aria-label={t('settings.advanced.debug.showHitboxes.aria')}
            data-testid="toggle-show-hitboxes"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.advanced.debug.logHitDetection.label')}
          helpText={t('settings.advanced.debug.logHitDetection.help')}
          isChanged={isFieldChanged(changedFields, 'debug.map.log_hit_detection')}
          onReset={() => onChange({ debug: { ...config.debug, map: { ...config.debug.map, log_hit_detection: defaultConfig.debug.map.log_hit_detection } } })}
          isDefault={config.debug.map.log_hit_detection === defaultConfig.debug.map.log_hit_detection}
          data-testid="setting-log-hit-detection"
        >
          <Toggle
            checked={config.debug.map.log_hit_detection}
            onChange={handleLogHitDetectionChange}
            aria-label={t('settings.advanced.debug.logHitDetection.aria')}
            data-testid="toggle-log-hit-detection"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.advanced.sections.logging')} data-testid="section-logging">
        <SettingRow
          label={t('settings.advanced.logging.level.label')}
          helpText={t('settings.advanced.logging.level.help')}
          isChanged={isFieldChanged(changedFields, 'logging.level')}
          onReset={() => onChange({ logging: { ...config.logging, level: defaultConfig.logging.level } })}
          isDefault={config.logging.level === defaultConfig.logging.level}
          requiresRestart
          data-testid="setting-log-level"
        >
          <Select
            value={config.logging.level}
            onChange={handleLogLevelChange}
            options={logLevelOptions}
            className="w-48"
            aria-label={t('settings.advanced.logging.level.aria')}
            data-testid="select-log-level"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.advanced.logging.maxSize.label')}
          helpText={t('settings.advanced.logging.maxSize.help')}
          isChanged={isFieldChanged(changedFields, 'logging.max_directory_size_mb')}
          onReset={() => onChange({ logging: { ...config.logging, max_directory_size_mb: defaultConfig.logging.max_directory_size_mb } })}
          isDefault={config.logging.max_directory_size_mb === defaultConfig.logging.max_directory_size_mb}
          data-testid="setting-max-log-size"
        >
          <div className="flex items-center gap-2">
            <input
              type="number" inputMode="decimal"
              min={1}
              max={10000}
              value={config.logging.max_directory_size_mb}
              onChange={(e) => handleMaxLogSizeChange(e.target.value)}
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t('settings.advanced.logging.maxSize.aria')}
              data-testid="input-max-log-size"
            />
            <span className="text-text-secondary">{t('settings.advanced.logging.maxSize.unit')}</span>
          </div>
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.advanced.sections.closureAdjustment')} data-testid="section-closure-adjustment">
        <SettingRow
          label={t('settings.advanced.closureAdjustment.sigmaLength.label')}
          helpText={t('settings.advanced.closureAdjustment.sigmaLength.help')}
          isChanged={isFieldChanged(changedFields, 'closure_adjustment.base_sigma_length_m')}
          onReset={() => onChange({ closure_adjustment: { ...config.closure_adjustment, base_sigma_length_m: defaultConfig.closure_adjustment.base_sigma_length_m } })}
          isDefault={config.closure_adjustment.base_sigma_length_m === defaultConfig.closure_adjustment.base_sigma_length_m}
          data-testid="setting-closure-sigma-length"
        >
          <input
            type="number" inputMode="decimal"
            min={0.001}
            step={0.01}
            value={config.closure_adjustment.base_sigma_length_m}
            onChange={(e) => handleSigmaLengthChange(e.target.value)}
            className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.advanced.closureAdjustment.sigmaLength.aria')}
            data-testid="input-closure-sigma-length"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.advanced.closureAdjustment.sigmaAzimuth.label')}
          helpText={t('settings.advanced.closureAdjustment.sigmaAzimuth.help')}
          isChanged={isFieldChanged(changedFields, 'closure_adjustment.base_sigma_azimuth_deg')}
          onReset={() => onChange({ closure_adjustment: { ...config.closure_adjustment, base_sigma_azimuth_deg: defaultConfig.closure_adjustment.base_sigma_azimuth_deg } })}
          isDefault={config.closure_adjustment.base_sigma_azimuth_deg === defaultConfig.closure_adjustment.base_sigma_azimuth_deg}
          data-testid="setting-closure-sigma-azimuth"
        >
          <input
            type="number" inputMode="decimal"
            min={0.01}
            step={0.1}
            value={config.closure_adjustment.base_sigma_azimuth_deg}
            onChange={(e) => handleSigmaAzimuthChange(e.target.value)}
            className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.advanced.closureAdjustment.sigmaAzimuth.aria')}
            data-testid="input-closure-sigma-azimuth"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.advanced.closureAdjustment.sigmaDepth.label')}
          helpText={t('settings.advanced.closureAdjustment.sigmaDepth.help')}
          isChanged={isFieldChanged(changedFields, 'closure_adjustment.base_sigma_depth_m')}
          onReset={() => onChange({ closure_adjustment: { ...config.closure_adjustment, base_sigma_depth_m: defaultConfig.closure_adjustment.base_sigma_depth_m } })}
          isDefault={config.closure_adjustment.base_sigma_depth_m === defaultConfig.closure_adjustment.base_sigma_depth_m}
          data-testid="setting-closure-sigma-depth"
        >
          <input
            type="number" inputMode="decimal"
            min={0.001}
            step={0.01}
            value={config.closure_adjustment.base_sigma_depth_m}
            onChange={(e) => handleSigmaDepthChange(e.target.value)}
            className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.advanced.closureAdjustment.sigmaDepth.aria')}
            data-testid="input-closure-sigma-depth"
          />
        </SettingRow>

      </SettingsSection>

      <SettingsSection title={t('settings.advanced.sections.placementSuggestion')} data-testid="section-placement-suggestion">
        {/* Scoring Weights */}
        <p className="text-xs text-text-secondary font-medium mt-1 mb-2">{t('settings.advanced.placementSuggestion.groups.weights')}</p>
        <PsRow field="azimuth_weight" i18nKey="azimuthWeight" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsAzimuthWeight} t={t} />
        <PsRow field="length_weight" i18nKey="lengthWeight" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsLengthWeight} t={t} />
        <PsRow field="depth_in_weight" i18nKey="depthInWeight" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsDepthInWeight} t={t} />
        <PsRow field="depth_out_weight" i18nKey="depthOutWeight" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsDepthOutWeight} t={t} />

        {/* Scoring Sensitivity (Sigma) */}
        <p className="text-xs text-text-secondary font-medium mt-3 mb-2">{t('settings.advanced.placementSuggestion.groups.sigma')}</p>
        <PsRow field="azimuth_sigma" i18nKey="azimuthSigma" step={1} min={0.1} max={180}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsAzimuthSigma} t={t} />
        <PsRow field="length_sigma" i18nKey="lengthSigma" step={0.1} min={0.01} max={100}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsLengthSigma} t={t} />
        <PsRow field="depth_sigma" i18nKey="depthSigma" step={0.05} min={0.01} max={100}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsDepthSigma} t={t} />

        {/* Score Thresholds */}
        <p className="text-xs text-text-secondary font-medium mt-3 mb-2">{t('settings.advanced.placementSuggestion.groups.thresholds')}</p>
        <PsRow field="min_shot_score" i18nKey="minShotScore" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMinShotScore} t={t} />
        <PsRow field="min_aggregate_score" i18nKey="minAggregateScore" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMinAggregateScore} t={t} />
        <PsRow field="single_shot_penalty" i18nKey="singleShotPenalty" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsSingleShotPenalty} t={t} />

        {/* Hard Cutoffs */}
        <p className="text-xs text-text-secondary font-medium mt-3 mb-2">{t('settings.advanced.placementSuggestion.groups.cutoffs')}</p>
        <PsRow field="max_azimuth_error_full" i18nKey="maxAzimuthErrorFull" step={1} min={0} max={180}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxAzErrorFull} t={t} />
        <PsRow field="max_azimuth_error_partial" i18nKey="maxAzimuthErrorPartial" step={1} min={0} max={180}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxAzErrorPartial} t={t} />
        <PsRow field="max_depth_error" i18nKey="maxDepthError" step={0.05} min={0} max={100}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxDepthError} t={t} />
        <PsRow field="max_length_error_fraction" i18nKey="maxLengthErrorFraction" step={0.01} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxLenErrorFrac} t={t} />
        <PsRow field="max_length_error_floor" i18nKey="maxLengthErrorFloor" step={0.1} min={0} max={100}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxLenErrorFloor} t={t} />

        {/* Limits */}
        <p className="text-xs text-text-secondary font-medium mt-3 mb-2">{t('settings.advanced.placementSuggestion.groups.limits')}</p>
        <PsRow field="max_reference_shots" i18nKey="maxReferenceShots" step={1} min={1} max={50}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxRefShots} t={t} />
        <PsRow field="max_suggestions" i18nKey="maxSuggestions" step={1} min={1} max={100}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMaxSuggestions} t={t} />
        <PsRow field="min_partial_ratio" i18nKey="minPartialRatio" step={0.05} min={0} max={1}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsMinPartialRatio} t={t} />
        <PsRow field="short_shot_threshold" i18nKey="shortShotThreshold" step={0.1} min={0} max={100}
          config={config} changedFields={changedFields} onChange={onChange} onInput={handlePsShortShotThresh} t={t} />
      </SettingsSection>

      <SettingsSection title={t('settings.advanced.sections.panels')} data-testid="section-panels">
        <SettingRow
          label={t('settings.advanced.panels.sectionsWidth.label')}
          helpText={t('settings.advanced.panels.sectionsWidth.help')}
          isChanged={isFieldChanged(changedFields, 'panels.sections_panel_width')}
          onReset={() => onChange({ panels: { ...config.panels, sections_panel_width: defaultConfig.panels.sections_panel_width } })}
          isDefault={config.panels.sections_panel_width === defaultConfig.panels.sections_panel_width}
          data-testid="setting-sections-panel-width"
        >
          <div className="flex items-center gap-2">
            <input
              type="number" inputMode="decimal"
              min={PANEL_SIZE_BOUNDS.sections.min}
              max={PANEL_SIZE_BOUNDS.sections.max}
              value={config.panels.sections_panel_width}
              onChange={(e) => handleSectionsPanelWidthChange(e.target.value)}
              className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t('settings.advanced.panels.sectionsWidth.aria')}
              data-testid="input-sections-panel-width"
            />
            <span className="text-text-secondary">{t('settings.advanced.panels.sectionsWidth.unit')}</span>
          </div>
        </SettingRow>

        <SettingRow
          label={t('settings.advanced.panels.detailWidth.label')}
          helpText={t('settings.advanced.panels.detailWidth.help')}
          isChanged={isFieldChanged(changedFields, 'panels.detail_panel_width')}
          onReset={() => onChange({ panels: { ...config.panels, detail_panel_width: defaultConfig.panels.detail_panel_width } })}
          isDefault={config.panels.detail_panel_width === defaultConfig.panels.detail_panel_width}
          data-testid="setting-detail-panel-width"
        >
          <div className="flex items-center gap-2">
            <input
              type="number" inputMode="decimal"
              min={PANEL_SIZE_BOUNDS.detail.min}
              max={PANEL_SIZE_BOUNDS.detail.max}
              value={config.panels.detail_panel_width}
              onChange={(e) => handleDetailPanelWidthChange(e.target.value)}
              className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t('settings.advanced.panels.detailWidth.aria')}
              data-testid="input-detail-panel-width"
            />
            <span className="text-text-secondary">{t('settings.advanced.panels.detailWidth.unit')}</span>
          </div>
        </SettingRow>

        <SettingRow
          label={t('settings.advanced.panels.dataTableHeight.label')}
          helpText={t('settings.advanced.panels.dataTableHeight.help')}
          isChanged={isFieldChanged(changedFields, 'panels.data_table_height')}
          onReset={() => onChange({ panels: { ...config.panels, data_table_height: defaultConfig.panels.data_table_height } })}
          isDefault={config.panels.data_table_height === defaultConfig.panels.data_table_height}
          data-testid="setting-data-table-height"
        >
          <div className="flex items-center gap-2">
            <input
              type="number" inputMode="decimal"
              min={DATA_TABLE_MIN_HEIGHT}
              max={DATA_TABLE_MAX_HEIGHT}
              value={config.panels.data_table_height}
              onChange={(e) => handleDataTableHeightChange(e.target.value)}
              className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t('settings.advanced.panels.dataTableHeight.aria')}
              data-testid="input-data-table-height"
            />
            <span className="text-text-secondary">{t('settings.advanced.panels.dataTableHeight.unit')}</span>
          </div>
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.advanced.sections.configFile')} data-testid="section-config-file">
        <div className="space-y-3">
          <Button
            variant="secondary"
            onClick={handleReloadConfig}
            disabled={isReloading}
            data-testid="reload-config-button"
          >
            {isReloading ? t('settings.advanced.reload.loading') : t('settings.advanced.reload.button')}
          </Button>
          <p className="text-xs text-text-secondary">
            {t('settings.advanced.reload.help')}
          </p>
          {reloadMessage && (
            <pre className="text-xs p-2 bg-background rounded border border-border whitespace-pre-wrap">
              {reloadMessage}
            </pre>
          )}
        </div>
      </SettingsSection>
    </div>
  );
}
