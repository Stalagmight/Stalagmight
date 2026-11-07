/**
 * Make a controller test's fake map lose its style the way MapLibre does (#576
 * NF-MAP-3): `Map.remove()` deletes `map.style` and a WebGL context loss sets it
 * to null, after which every style-backed call (`getLayer`, `addLayer`,
 * `removeLayer`) reads the missing style and throws.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';

export function dropStyle(map: MaplibreMap): void {
  const m = map as unknown as Record<string, unknown>;
  m.style = null;
  const thrower = (name: string) => () => {
    throw new TypeError(`Cannot read properties of null (reading '${name}')`);
  };
  m.getLayer = thrower('getLayer');
  m.addLayer = thrower('addLayer');
  m.removeLayer = thrower('removeLayer');
}
