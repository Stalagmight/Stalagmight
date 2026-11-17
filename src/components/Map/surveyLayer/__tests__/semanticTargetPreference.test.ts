import type { MapMouseEvent } from 'maplibre-gl';
import { describe, expect, it, vi } from 'vitest';
import type { SemanticMapGeometry } from '../semanticTarget';
import { SemanticTargetPreference } from '../semanticTargetPreference';

const geometry: SemanticMapGeometry = {
  stations: [{ lineId: 1, stationIndex: 2, lngLat: [10, 20] }],
  shots: [{ lineId: 1, shotId: 3, from: [0, 20], to: [20, 20] }],
};
const event = { point: { x: 10, y: 20 } } as MapMouseEvent;

function preference(visibility: { stations: boolean; shots: boolean }) {
  const callbacks = { onStationClick: vi.fn(), onShotClick: vi.fn() };
  return {
    callbacks,
    target: new SemanticTargetPreference({
      geometry: () => geometry,
      project: ([x, y]) => ({ x, y }),
      visibility: () => visibility,
      callbacks: () => callbacks,
    }),
  };
}

describe('SemanticTargetPreference', () => {
  it('rejects a station hidden by the display toggle', () => {
    const { callbacks, target } = preference({ stations: false, shots: true });
    target.set({ featureType: 'station', lineId: 1, stationIndex: 2 });

    expect(target.handleClick(event, 10, 4, false, false)).toBe(false);
    expect(callbacks.onStationClick).not.toHaveBeenCalled();
  });

  it('rejects a shot hidden by the display toggle', () => {
    const { callbacks, target } = preference({ stations: true, shots: false });
    target.set({ featureType: 'shot', lineId: 1, shotId: 3 });

    expect(target.handleClick(event, 10, 4, false, false)).toBe(false);
    expect(callbacks.onShotClick).not.toHaveBeenCalled();
  });
});
