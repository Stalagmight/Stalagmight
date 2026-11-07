/**
 * Unit tests for station/shot centering actions in uiStore.
 * Tests request state management for triggerCenterOnStation and triggerCenterOnShot.
 */

import { describe, it, expect, beforeEach } from 'vitest';

import { useUIStore } from '../uiStore';

describe('Station Centering Actions', () => {
  beforeEach(() => {
    // Reset centering request state
    useUIStore.setState({
      requestCenterOnStation: null,
      requestCenterOnShot: null,
    });
  });

  describe('triggerCenterOnStation', () => {
    it('sets requestCenterOnStation to the provided station ref', () => {
      const stationRef = { section_id: 1, station_index: 2 };

      useUIStore.getState().triggerCenterOnStation(stationRef);

      const request = useUIStore.getState().requestCenterOnStation;
      expect(request).toEqual(stationRef);
      expect(request?.section_id).toBe(1);
      expect(request?.station_index).toBe(2);
    });

    it('overwrites previous request with new station ref', () => {
      const firstRef = { section_id: 1, station_index: 0 };
      const secondRef = { section_id: 2, station_index: 5 };

      useUIStore.getState().triggerCenterOnStation(firstRef);
      expect(useUIStore.getState().requestCenterOnStation).toEqual(firstRef);

      useUIStore.getState().triggerCenterOnStation(secondRef);
      expect(useUIStore.getState().requestCenterOnStation).toEqual(secondRef);
    });

    it('handles section_id 0 correctly', () => {
      const stationRef = { section_id: 0, station_index: 0 };

      useUIStore.getState().triggerCenterOnStation(stationRef);

      const request = useUIStore.getState().requestCenterOnStation;
      expect(request).not.toBeNull();
      expect(request?.section_id).toBe(0);
      expect(request?.station_index).toBe(0);
    });
  });

  describe('clearCenterOnStationRequest', () => {
    it('clears requestCenterOnStation to null', () => {
      const stationRef = { section_id: 1, station_index: 2 };
      useUIStore.getState().triggerCenterOnStation(stationRef);
      expect(useUIStore.getState().requestCenterOnStation).not.toBeNull();

      useUIStore.getState().clearCenterOnStationRequest();

      expect(useUIStore.getState().requestCenterOnStation).toBeNull();
    });

    it('is idempotent when already null', () => {
      expect(useUIStore.getState().requestCenterOnStation).toBeNull();

      useUIStore.getState().clearCenterOnStationRequest();

      expect(useUIStore.getState().requestCenterOnStation).toBeNull();
    });
  });
});

describe('Shot Centering Actions', () => {
  beforeEach(() => {
    // Reset centering request state
    useUIStore.setState({
      requestCenterOnStation: null,
      requestCenterOnShot: null,
    });
  });

  describe('triggerCenterOnShot', () => {
    it('sets requestCenterOnShot to the provided section/shot ids', () => {
      useUIStore.getState().triggerCenterOnShot(1, 5);

      const request = useUIStore.getState().requestCenterOnShot;
      expect(request).toEqual({ sectionId: 1, shotId: 5 });
    });

    it('overwrites previous request with new shot', () => {
      useUIStore.getState().triggerCenterOnShot(1, 0);
      expect(useUIStore.getState().requestCenterOnShot).toEqual({ sectionId: 1, shotId: 0 });

      useUIStore.getState().triggerCenterOnShot(2, 3);
      expect(useUIStore.getState().requestCenterOnShot).toEqual({ sectionId: 2, shotId: 3 });
    });

    it('handles sectionId 0 correctly', () => {
      useUIStore.getState().triggerCenterOnShot(0, 0);

      const request = useUIStore.getState().requestCenterOnShot;
      expect(request).not.toBeNull();
      expect(request?.sectionId).toBe(0);
      expect(request?.shotId).toBe(0);
    });
  });

  describe('clearCenterOnShotRequest', () => {
    it('clears requestCenterOnShot to null', () => {
      useUIStore.getState().triggerCenterOnShot(1, 5);
      expect(useUIStore.getState().requestCenterOnShot).not.toBeNull();

      useUIStore.getState().clearCenterOnShotRequest();

      expect(useUIStore.getState().requestCenterOnShot).toBeNull();
    });

    it('is idempotent when already null', () => {
      expect(useUIStore.getState().requestCenterOnShot).toBeNull();

      useUIStore.getState().clearCenterOnShotRequest();

      expect(useUIStore.getState().requestCenterOnShot).toBeNull();
    });
  });
});

describe('Centering Actions Independence', () => {
  beforeEach(() => {
    // Reset centering request state
    useUIStore.setState({
      requestCenterOnStation: null,
      requestCenterOnShot: null,
    });
  });

  it('station and shot centering requests are independent', () => {
    const stationRef = { section_id: 1, station_index: 2 };

    // Set both requests
    useUIStore.getState().triggerCenterOnStation(stationRef);
    useUIStore.getState().triggerCenterOnShot(3, 4);

    expect(useUIStore.getState().requestCenterOnStation).toEqual(stationRef);
    expect(useUIStore.getState().requestCenterOnShot).toEqual({ sectionId: 3, shotId: 4 });

    // Clear station request - shot request should remain
    useUIStore.getState().clearCenterOnStationRequest();

    expect(useUIStore.getState().requestCenterOnStation).toBeNull();
    expect(useUIStore.getState().requestCenterOnShot).toEqual({ sectionId: 3, shotId: 4 });
  });

  it('clearing shot request does not affect station request', () => {
    const stationRef = { section_id: 5, station_index: 10 };

    useUIStore.getState().triggerCenterOnStation(stationRef);
    useUIStore.getState().triggerCenterOnShot(2, 1);

    // Clear shot request - station request should remain
    useUIStore.getState().clearCenterOnShotRequest();

    expect(useUIStore.getState().requestCenterOnStation).toEqual(stationRef);
    expect(useUIStore.getState().requestCenterOnShot).toBeNull();
  });
});
