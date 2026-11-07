import { useState, useEffect, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { invokeIpc } from '../../../utils/ipc';
import { Toggle } from '../../common/Toggle';
import { Button } from '../../common/Button';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import { isFieldChanged, createNumericHandler, createToggleHandler, parseIntRadix10 } from './types';
import type { CacheStats } from '../../../types/cache';
import { defaultConfig } from '../../../types/config';
import { formatFixed, formatNumber } from '../../../i18n/format';
import { clamp } from '../../../utils/clamp';

const MIN_CACHE_SIZE_MB = 100;
const MAX_CACHE_SIZE_MB = 10000;

/**
 * Format bytes to human-readable string (e.g., "12.5 MB")
 */
function formatBytes(value: number): string {
  if (value === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(value) / Math.log(k));
  const scaled = value / Math.pow(k, i);
  return `${formatFixed(scaled, undefined, 1)} ${sizes[i]}`;
}

/**
 * Cache settings category content.
 * Maps to config.cache: tile_cache_enabled, tile_cache_max_mb, tile_ttl_days.
 * Also displays cache statistics and clear button with preserved data-testid attributes.
 */
export function CacheSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const [cacheStats, setCacheStats] = useState<CacheStats | null>(null);
  const [isLoadingStats, setIsLoadingStats] = useState(false);
  const [isClearing, setIsClearing] = useState(false);
  const [maxSizeInput, setMaxSizeInput] = useState<string>(String(config.cache.tile_cache_max_mb));
  const mountedRef = useRef(true);

  // Validation state
  const hasErrorRef = useRef(false);

  // Validate on mount and when config changes
  useEffect(() => {
    const errors: string[] = [];

    // Validate cache size
    if (config.cache.tile_cache_max_mb < MIN_CACHE_SIZE_MB) {
      errors.push(t('settings.cache.validation.minSize', { value: MIN_CACHE_SIZE_MB }));
    }
    if (config.cache.tile_cache_max_mb > MAX_CACHE_SIZE_MB) {
      errors.push(t('settings.cache.validation.maxSize', { value: MAX_CACHE_SIZE_MB }));
    }

    // Validate TTL
    if (config.cache.tile_ttl_days < 1 || config.cache.tile_ttl_days > 365) {
      errors.push(t('settings.cache.validation.ttl'));
    }

    const hasError = errors.length > 0;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('cache', hasError);
    }
  }, [config.cache, onValidationError, t]);

  // Sync maxSizeInput when config changes externally
  useEffect(() => {
    setMaxSizeInput(String(config.cache.tile_cache_max_mb));
  }, [config.cache.tile_cache_max_mb]);

  // Cleanup on unmount
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Load cache stats
  const loadCacheStats = useCallback(async () => {
    if (!mountedRef.current) return;
    setIsLoadingStats(true);
    try {
      const stats = await invokeIpc('get_cache_stats', undefined, {
        purpose: 'load tile cache statistics',
      });
      if (mountedRef.current) {
        setCacheStats(stats);
      }
    } catch (error) {
      console.error('[CacheSettings] Failed to load cache stats:', error);
      if (mountedRef.current) {
        setCacheStats({
          total_tiles: 0,
          total_bytes: 0,
          providers: {},
        });
      }
    } finally {
      if (mountedRef.current) {
        setIsLoadingStats(false);
      }
    }
  }, []);

  // Load stats on mount
  useEffect(() => {
    loadCacheStats();
  }, [loadCacheStats]);

  // Handle clearing the cache
  const handleClearCache = async () => {
    setIsClearing(true);
    try {
      await invokeIpc('clear_tile_cache', { provider: null }, {
        purpose: 'clear tile cache',
        fields: { provider: null },
      });
      await loadCacheStats();
    } catch (error) {
      console.error('[CacheSettings] Failed to clear cache:', error);
    } finally {
      if (mountedRef.current) {
        setIsClearing(false);
      }
    }
  };

  const handleCacheEnabledChange = createToggleHandler(onChange,
    (v) => ({ cache: { ...config.cache, tile_cache_enabled: v } }));

  const handleMaxSizeInputChange = (value: string) => {
    setMaxSizeInput(value);
    // Immediately update config (validation will highlight errors)
    const parsed = Number(value);
    if (Number.isFinite(parsed) && parsed > 0) {
      onChange({
        cache: {
          ...config.cache,
          tile_cache_max_mb: Math.round(parsed),
        },
      });
    }
  };

  const handleMaxSizeBlur = () => {
    const parsed = Number(maxSizeInput);
    let finalValue: number;

    if (!Number.isFinite(parsed) || parsed <= 0) {
      finalValue = config.cache.tile_cache_max_mb;
    } else {
      finalValue = clamp(Math.round(parsed), MIN_CACHE_SIZE_MB, MAX_CACHE_SIZE_MB);
    }

    setMaxSizeInput(String(finalValue));
    onChange({
      cache: {
        ...config.cache,
        tile_cache_max_mb: finalValue,
      },
    });
  };

  const handleTtlChange = createNumericHandler(onChange,
    (n) => ({ cache: { ...config.cache, tile_ttl_days: n } }), parseIntRadix10);

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'cache')}
      aria-labelledby={getTabId(panelIdBase, 'cache')}
      data-testid="settings-panel-cache"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.cache.title')}</h2>

      <SettingsSection title={t('settings.cache.sections.tileCache')} data-testid="section-tile-cache">
        {/* Cache Statistics - Preserve data-testid="cache-stats" */}
        <div
          className="bg-background-50 rounded-lg p-4 space-y-3 mb-4"
          data-testid="cache-stats"
        >
          {isLoadingStats ? (
            <p className="text-text-secondary">{t('settings.cache.stats.loading')}</p>
          ) : cacheStats ? (
            <>
              <div className="flex justify-between">
                <span className="text-text-secondary">{t('settings.cache.stats.cachedTiles')}</span>
                <span data-testid="cache-tile-count">{formatNumber(cacheStats.total_tiles)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-text-secondary">{t('settings.cache.stats.cacheSize')}</span>
                <span data-testid="cache-size">{formatBytes(cacheStats.total_bytes)}</span>
              </div>
              {Object.entries(cacheStats.providers).length > 0 && (
                <div className="pt-2 border-t border-border">
                  <span className="text-xs text-text-secondary">{t('settings.cache.stats.byProvider')}</span>
                  {Object.entries(cacheStats.providers).map(([provider, stats]) => (
                    <div key={provider} className="flex justify-between text-sm mt-1">
                      <span className="text-text-secondary capitalize">{provider}:</span>
                      <span>
                        {t('settings.cache.stats.providerTiles', {
                          tiles: formatNumber(stats.tiles),
                          size: formatBytes(stats.bytes),
                        })}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </>
          ) : (
            <p className="text-text-secondary">{t('settings.cache.stats.unavailable')}</p>
          )}
        </div>

        {/* Clear cache button - Preserve data-testid="clear-cache-button" */}
        <div className="mb-4">
          <Button
            variant="danger"
            onClick={handleClearCache}
            // A plain number, as the JSON invoke() delivers it (#576 C6: the
            // generated type said bigint, and `=== 0n` never matched).
            disabled={isClearing || (cacheStats?.total_tiles ?? 0) === 0}
            data-testid="clear-cache-button"
          >
            {isClearing ? t('settings.cache.clear.clearing') : t('settings.cache.clear.button')}
          </Button>
        </div>
      </SettingsSection>

      <SettingsSection title={t('settings.cache.sections.configuration')} data-testid="section-cache-config">
        <SettingRow
          label={t('settings.cache.enabled.label')}
          helpText={t('settings.cache.enabled.help')}
          isChanged={isFieldChanged(changedFields, 'cache.tile_cache_enabled')}
          onReset={() => onChange({ cache: { ...config.cache, tile_cache_enabled: defaultConfig.cache.tile_cache_enabled } })}
          isDefault={config.cache.tile_cache_enabled === defaultConfig.cache.tile_cache_enabled}
          data-testid="setting-cache-enabled"
        >
          <Toggle
            checked={config.cache.tile_cache_enabled}
            onChange={handleCacheEnabledChange}
            aria-label={t('settings.cache.enabled.aria')}
            data-testid="cache-enabled-toggle"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.cache.maxSize.label')}
          helpText={t('settings.cache.maxSize.help', {
            min: MIN_CACHE_SIZE_MB,
            max: MAX_CACHE_SIZE_MB,
          })}
          isChanged={isFieldChanged(changedFields, 'cache.tile_cache_max_mb')}
          onReset={() => onChange({ cache: { ...config.cache, tile_cache_max_mb: defaultConfig.cache.tile_cache_max_mb } })}
          isDefault={config.cache.tile_cache_max_mb === defaultConfig.cache.tile_cache_max_mb}
          data-testid="setting-cache-max-size"
        >
          <div className="flex items-center gap-2">
            <input
              type="number" inputMode="decimal"
              min={MIN_CACHE_SIZE_MB}
              max={MAX_CACHE_SIZE_MB}
              step={100}
              value={maxSizeInput}
              onChange={(e) => handleMaxSizeInputChange(e.target.value)}
              onBlur={handleMaxSizeBlur}
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t('settings.cache.maxSize.aria')}
              data-testid="cache-max-size-input"
            />
            <span className="text-text-secondary">{t('settings.cache.maxSize.unit')}</span>
          </div>
        </SettingRow>

        <SettingRow
          label={t('settings.cache.ttl.label')}
          helpText={t('settings.cache.ttl.help')}
          isChanged={isFieldChanged(changedFields, 'cache.tile_ttl_days')}
          onReset={() => onChange({ cache: { ...config.cache, tile_ttl_days: defaultConfig.cache.tile_ttl_days } })}
          isDefault={config.cache.tile_ttl_days === defaultConfig.cache.tile_ttl_days}
          data-testid="setting-cache-ttl"
        >
          <div className="flex items-center gap-2">
            <input
              type="number" inputMode="decimal"
              min={1}
              max={365}
              value={config.cache.tile_ttl_days}
              onChange={(e) => handleTtlChange(e.target.value)}
              className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              aria-label={t('settings.cache.ttl.aria')}
              data-testid="input-cache-ttl"
            />
            <span className="text-text-secondary">{t('settings.cache.ttl.unit')}</span>
          </div>
        </SettingRow>
      </SettingsSection>
    </div>
  );
}
