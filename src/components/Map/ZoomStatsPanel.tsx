import type * as maplibregl from 'maplibre-gl';
import { useTranslation } from 'react-i18next';
import { formatFixed, formatNumber } from '../../i18n/format';
import { getFpsColor, type MapDevStatsData } from './useMapDevStats';
import { WheelChainStats } from './WheelChainStats';

interface ZoomStatsPanelProps {
  map: maplibregl.Map;
  data: MapDevStatsData;
}

/**
 * Zoom Stats panel: the current zoom level, the performance of the last
 * completed zoom gesture (fps, duration, levels crossed, worst frame time),
 * and the wheel-chain diagnostics (event/travel/classification for the
 * previous and live wheel gesture - originally a dev-build-only overlay,
 * now folded in here so it's only live when Zoom Stats is toggled on). One
 * of the three panels driven by useMapDevStats - see MapDevStatsPanels for
 * how they're composed.
 */
export function ZoomStatsPanel({ map, data }: ZoomStatsPanelProps) {
  const { t } = useTranslation();
  const { zoom, zoomFps, lastZoomDuration, lastZoomLevels, worstFrameTime } = data;

  return (
    <div
      className="relative bg-panel-90 text-text-primary text-xs font-mono p-2 rounded border border-border select-none"
      style={{ minWidth: '200px', maxWidth: '280px' }}
      data-testid="map-zoom-stats"
    >
      <div className="mb-1">
        <span className="text-text-secondary">{t('map.devStats.zoom')}:</span>{' '}
        <span className="text-text-primary">{formatFixed(zoom, undefined, 2)}</span>
      </div>

      {(zoomFps !== null || lastZoomDuration !== null) && (
        <div className="text-[10px]">
          <span className="text-text-secondary">{t('map.devStats.lastZoom')}:</span>{' '}
          {zoomFps !== null && (
            <>
              <span className={getFpsColor(zoomFps)}>
                {t('map.devStats.fpsValue', { value: formatNumber(zoomFps) })}
              </span>
              <span className="text-text-secondary opacity-80 mx-1">|</span>
            </>
          )}
          {lastZoomDuration !== null && (
            <span className="text-text-primary">
              {t('map.devStats.durationMs', { ms: formatNumber(lastZoomDuration) })}
            </span>
          )}
          {lastZoomLevels !== null && (
            <span className="text-text-secondary opacity-80 ml-1">
              {t('map.devStats.zoomLevels', {
                count: lastZoomLevels,
                countFormatted: formatNumber(lastZoomLevels),
              })}
            </span>
          )}
          {worstFrameTime !== null && worstFrameTime > 20 && (
            <span className={`ml-1 ${worstFrameTime > 50 ? 'text-error' : 'text-warning'}`}>
              {t('map.devStats.worstFrame', { ms: formatNumber(worstFrameTime) })}
            </span>
          )}
        </div>
      )}

      <div className="border-t border-border pt-1 mt-1 overflow-x-auto min-w-0">
        <WheelChainStats map={map} />
      </div>
    </div>
  );
}
