import { describe, it, expect, beforeEach } from 'vitest';
import { useUIStore } from '../uiStore';
import type { SectionData } from '../../types/project';

/**
 * updatePlacementSnap runs on every placement pointer move (#576 D9). A move
 * that does not change the snap must not write the store: every write replaces
 * `placementMode` and re-renders the subscribed map.
 */
describe('uiStore updatePlacementSnap', () => {
  beforeEach(() => {
    useUIStore.setState({ placementMode: null, activeDialog: null });
    useUIStore.getState().startPlacementMode({ name: 'CaveA', shots: [] } as unknown as SectionData, 0);
  });

  it('an unchanged snap is no store write', () => {
    let writes = 0;
    const unsubscribe = useUIStore.subscribe(() => {
      writes++;
    });
    const s = useUIStore.getState();
    s.updatePlacementSnap(null);
    s.updatePlacementSnap({ section_id: 1, station_index: 2 });
    s.updatePlacementSnap({ section_id: 1, station_index: 2 }); // equal, fresh object
    s.updatePlacementSnap({ section_id: 1, station_index: 3 });
    s.updatePlacementSnap(null);
    s.updatePlacementSnap(null);
    unsubscribe();
    expect(writes).toBe(3);
    expect(useUIStore.getState().placementMode?.snappedToStation).toBeNull();
  });

  it('still records the snap', () => {
    useUIStore.getState().updatePlacementSnap({ section_id: 4, station_index: 1 });
    expect(useUIStore.getState().placementMode?.snappedToStation).toEqual({ section_id: 4, station_index: 1 });
  });
});
