import { describe, it, expect, beforeEach } from 'vitest';
import { useUIStore, type MeasureEndpoint } from '../uiStore';

const station = (line_id: number, station_index: number): MeasureEndpoint => ({
  kind: 'station',
  line_id,
  station_index,
  coords: { x: 0, y: 0, z: 0 },
});

describe('uiStore measure reset funnels (#26)', () => {
  beforeEach(() => {
    useUIStore.setState({
      activeTool: 'measure',
      geometryMode: 'measured',
      activeDialog: null,
      placementMode: null,
    });
    useUIStore.getState().clearMeasure();
  });

  it('clearMeasure wipes every piece of measure state', () => {
    const s = useUIStore.getState();
    s.setMeasureStart(station(1, 0));
    s.setMeasureEnd(station(1, 1));
    s.setMeasureDirect({ distance: 5, depthDiff: 0 });
    s.setMeasureAlongLine({ alongLine: 5, pathShots: [{ line_id: 1, shot_id: 0 }], pathStations: [{ line_id: 1, station_index: 0 }] });
    s.setMeasureAlongLinePending(true);
    s.setMeasureSnapShot({ line_id: 1, shot_id: 0, t: 0.5 });

    useUIStore.getState().clearMeasure();

    const after = useUIStore.getState();
    expect(after.measureStart).toBeNull();
    expect(after.measureEnd).toBeNull();
    expect(after.measureDirect).toBeNull();
    expect(after.measureAlongLine).toBeNull();
    expect(after.measureAlongLinePending).toBe(false);
    expect(after.measureSnapShot).toBeNull();
  });

  it('switching to a different tool clears measure', () => {
    const s = useUIStore.getState();
    s.setMeasureStart(station(1, 0));
    s.setMeasureEnd(station(1, 1));
    useUIStore.getState().setActiveTool('select');
    expect(useUIStore.getState().measureStart).toBeNull();
    expect(useUIStore.getState().measureEnd).toBeNull();
  });

  it('opening a dialog clears measure', () => {
    useUIStore.getState().setMeasureStart(station(1, 0));
    useUIStore.getState().openDialog('settings');
    expect(useUIStore.getState().measureStart).toBeNull();
  });

  it('changing geometry mode clears measure', () => {
    useUIStore.getState().setMeasureStart(station(1, 0));
    useUIStore.getState().setGeometryMode('closureAdjusted');
    expect(useUIStore.getState().measureStart).toBeNull();
  });

  it('setMeasureAlongLine resolves the pending flag', () => {
    useUIStore.getState().setMeasureAlongLinePending(true);
    useUIStore.getState().setMeasureAlongLine({ alongLine: null, pathShots: [], pathStations: [] });
    expect(useUIStore.getState().measureAlongLinePending).toBe(false);
  });
});
