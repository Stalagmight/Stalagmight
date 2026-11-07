import { useEffect, useState, type RefObject } from 'react';
import type { Map as MaplibreMap } from 'maplibre-gl';

/**
 * A counter that moves once the map's style is back after a WebGL context
 * restore (#576 NF-MAP-5).
 *
 * While a context is lost MapLibre has no style, so every effect that writes
 * the style (adds a source or layer, sets a paint property) has to skip its
 * write - reading the missing style throws, and a throw in an effect takes the
 * whole map panel down. The restore brings back the style as it was at the loss,
 * so a write skipped in between is simply missing. The same goes for the frame
 * in which the restored style is in place but still loading: it refuses every
 * write. Effects that skip in either window (`canWriteStyle`) list this counter
 * as a dependency to run again once the restored style has loaded. (The custom
 * layers are back before any of those effects runs:
 * surveyLayer/customRender/styleRestore.ts re-adds them synchronously in its own
 * `style.load` listener, while this counter only schedules a re-render.)
 */
export function useStyleEpoch(map: RefObject<MaplibreMap | null>, mapLoaded: boolean): number {
  const [epoch, setEpoch] = useState(0);
  useEffect(() => {
    const current = map.current;
    if (!current || !mapLoaded) return undefined;
    let pendingStyleLoad: (() => void) | null = null;
    // MapLibre re-applies the serialized style on restore; it loads next frame.
    const onRestored = (): void => {
      if (pendingStyleLoad) return;
      pendingStyleLoad = () => {
        pendingStyleLoad = null;
        setEpoch((e) => e + 1);
      };
      current.once('style.load', pendingStyleLoad);
    };
    current.on('webglcontextrestored', onRestored);
    return () => {
      current.off('webglcontextrestored', onRestored);
      if (pendingStyleLoad) current.off('style.load', pendingStyleLoad);
    };
  }, [map, mapLoaded]);
  return epoch;
}
