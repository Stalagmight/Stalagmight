/**
 * #576 RV8c-2-D3: a pinned stage drop the dive turns short of is drawn at its pin, greyed, as
 * "not reached" - never red, never orange. The engine reports it past the turnaround (its
 * `stage_drop_route_indices` entry beyond the route's last station); the marker reads that and
 * nothing else.
 *
 * maplibre's `Marker` is replaced by a minimal stand-in that mounts the element into the document:
 * the shared `globalViewMap` fake has no `on()`, which every draggable drop marker subscribes with.
 */
import { describe, it, expect, vi } from 'vitest';
import { fireEvent, renderHook } from '@testing-library/react';
import type * as maplibregl from 'maplibre-gl';

vi.mock('maplibre-gl', () => {
  class Marker {
    private el: HTMLElement;
    constructor(opts: { element: HTMLElement }) {
      this.el = opts.element;
    }
    setLngLat() {
      return this;
    }
    addTo() {
      document.body.appendChild(this.el);
      return this;
    }
    on() {
      return this;
    }
    getLngLat() {
      return { lng: 0, lat: 0 };
    }
    remove() {
      this.el.remove();
    }
  }
  return { Marker };
});

import '../../../i18n';
import { useGasPlanDropMarkers } from '../useGasPlanDropMarkers';
import { useGasPlanStore } from '../../../store/gasPlanStore';
import { makeStation, stationRef } from '../../../test/factories/survey';
import type { GasReachField } from '../../../types/ipc.generated';

/** Route 1:0 -> 1:1 -> 1:2 (two legs). Drop 1 at 1:1, on the route; drop 2 pinned at 1:4, past
 *  the turnaround at 1:2 - the engine's not-reached answer. */
function field(): GasReachField {
  const leg = (from: number, to: number) => ({
    from: stationRef(1, from),
    to: stationRef(1, to),
    is_jump: false,
  });
  return {
    station_refs: [0, 1, 2, 3, 4].map((i) => stationRef(1, i)),
    gas_to_reach: [0, 100, 200, 300, 400],
    reachable: [true, true, true, false, false],
    turn_frontier: [],
    max_reachable_gas: 200,
    budget_litres: 250,
    route: [leg(0, 1), leg(1, 2)],
    stage_drop_refs: [stationRef(1, 1), stationRef(1, 4)],
    final_turn_ref: stationRef(1, 2),
    turn_toward_endpoint: false,
    backgas_turn_bar: 150,
    stage_drop_bars: [140, 120],
    stage_drop_over_reserve: [false, false],
    stage_drop_distances_m: [10, 40],
    cylinder_usable_bars: [],
    route_line_length_m: 20,
    route_max_depth_m: 0,
    route_avg_depth_m: 0,
    waypoint_gas_litres: [],
    route_profile: [],
    stage_drop_route_indices: [1, 4],
    waypoint_route_indices: [],
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: 2,
    turn_on_scooter_range: false,
  };
}

describe('useGasPlanDropMarkers: a drop the dive does not reach (#576 RV8c-2-D3)', () => {
  it('draws the pinned drop past the turnaround grey and "not reached", the other one as before', () => {
    const map = { project: () => ({ x: 0, y: 0 }) } as unknown as maplibregl.Map;
    const stations = [0, 1, 2, 3, 4].map((i) =>
      makeStation({ station_ref: stationRef(1, i), coordinates: { x: 10 * i, y: 0, z: 0 } }),
    );
    const { unmount } = renderHook(() =>
      useGasPlanDropMarkers({
        map,
        mapLoaded: true,
        showGasPlan: true,
        gasReachField: field(),
        activeStations: stations,
        gpsOrigin: null,
        setStageDropOverride: vi.fn(),
      }),
    );
    const markers = Array.from(document.querySelectorAll<HTMLElement>('[data-testid="gas-drop-marker"]'));
    expect(markers).toHaveLength(2);
    const [onRoute, past] = markers as [HTMLElement, HTMLElement];
    expect(onRoute.getAttribute('data-reached')).toBe('true');
    expect(onRoute.querySelector('path[stroke="#ffffff"]')?.getAttribute('fill')).toBe('#e08a1e');
    expect(past.getAttribute('data-reached')).toBe('false');
    expect(past.getAttribute('data-over-reserve')).toBe('false');
    expect(past.querySelector('path[stroke="#ffffff"]')?.getAttribute('fill')).toBe('#8a8f98');
    expect(past.title).toContain('not reached');
    unmount();
    expect(document.querySelectorAll('[data-testid="gas-drop-marker"]')).toHaveLength(0);
  });

  it('puts a not-reached drop past the end of its slider: Left pulls it onto the turnaround, Right does nothing (#576 RV9b-4)', () => {
    const map = { project: () => ({ x: 0, y: 0 }) } as unknown as maplibregl.Map;
    const stations = [0, 1, 2, 3, 4].map((i) =>
      makeStation({ station_ref: stationRef(1, i), coordinates: { x: 10 * i, y: 0, z: 0 } }),
    );
    const setStageDropOverride = vi.fn();
    useGasPlanStore.setState({ stageDropOverrides: [null, stationRef(1, 4)] });
    const { unmount } = renderHook(() =>
      useGasPlanDropMarkers({
        map,
        mapLoaded: true,
        showGasPlan: true,
        gasReachField: field(),
        activeStations: stations,
        gpsOrigin: null,
        setStageDropOverride,
      }),
    );
    const markers = Array.from(document.querySelectorAll<HTMLElement>('[data-testid="gas-drop-marker"]'));
    const [onRoute, past] = markers as [HTMLElement, HTMLElement];
    // Route 1:0 -> 1:1 -> 1:2: the slider runs 0..2. The pin at 1:4 is beyond the turnaround.
    expect(past.getAttribute('aria-valuemax')).toBe('2');
    expect(past.getAttribute('aria-valuenow')).toBe('2');
    expect(past.getAttribute('aria-valuetext')).toContain('not reached');
    fireEvent.keyDown(past, { key: 'ArrowRight' });
    fireEvent.keyDown(past, { key: 'ArrowUp' });
    expect(setStageDropOverride).not.toHaveBeenCalled();
    fireEvent.keyDown(past, { key: 'ArrowLeft' });
    expect(setStageDropOverride).toHaveBeenCalledTimes(1);
    expect(setStageDropOverride).toHaveBeenCalledWith(1, stationRef(1, 2));
    // A drop on the route keeps its own position and steps both ways as before.
    expect(onRoute.getAttribute('aria-valuenow')).toBe('1');
    expect(onRoute.hasAttribute('aria-valuetext')).toBe(false);
    fireEvent.keyDown(onRoute, { key: 'ArrowRight' });
    expect(setStageDropOverride).toHaveBeenLastCalledWith(0, stationRef(1, 2));
    fireEvent.keyDown(onRoute, { key: 'ArrowLeft' });
    expect(setStageDropOverride).toHaveBeenLastCalledWith(0, stationRef(1, 0));
    unmount();
    useGasPlanStore.setState({ stageDropOverrides: [] });
  });
});
