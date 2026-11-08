import * as maplibregl from 'maplibre-gl';

/**
 * `new maplibregl.Map(options)`, or null when the machine cannot give MapLibre a
 * WebGL2 context. MapLibre 6 throws `GPUInitializationError` from the
 * constructor in that case (5 fell back to WebGL1); the caller shows
 * `MapUnavailable` in place of the map. Any other error propagates.
 */
export function createMap(options: maplibregl.MapOptions): maplibregl.Map | null {
  try {
    return new maplibregl.Map(options);
  } catch (e) {
    if (!(e instanceof maplibregl.GPUInitializationError)) throw e;
    console.warn('[map] WebGL2 unavailable; map disabled', e.statusMessage ?? '');
    return null;
  }
}
