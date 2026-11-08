import type * as maplibregl from 'maplibre-gl';
import { useMapDevStats } from './useMapDevStats';
import { DevStatsPanel } from './DevStatsPanel';
import { MapStatsPanel } from './MapStatsPanel';
import { ZoomStatsPanel } from './ZoomStatsPanel';

interface MapDevStatsPanelsProps {
  map: maplibregl.Map;
  showDevStats: boolean;
  showMapStats: boolean;
  showZoomStats: boolean;
}

/**
 * Composes the Dev Stats / Map Stats / Zoom Stats debug panels (hamburger
 * menu ▸ Debug), each independently toggleable. All three read from a single
 * useMapDevStats() call so the WebGL draw-call counter and the cache/tile
 * IPC polling are installed once regardless of how many panels are shown.
 * Enabled panels stack bottom-right without overlapping.
 */
export function MapDevStatsPanels({ map, showDevStats, showMapStats, showZoomStats }: MapDevStatsPanelsProps) {
  const data = useMapDevStats(map);

  return (
    <div className="absolute bottom-2 right-2 flex flex-col gap-2 items-end pointer-events-none">
      {showDevStats && (
        <div className="pointer-events-auto">
          <DevStatsPanel data={data} />
        </div>
      )}
      {showMapStats && (
        <div className="pointer-events-auto">
          <MapStatsPanel data={data} />
        </div>
      )}
      {showZoomStats && (
        <div className="pointer-events-auto">
          <ZoomStatsPanel map={map} data={data} />
        </div>
      )}
    </div>
  );
}
