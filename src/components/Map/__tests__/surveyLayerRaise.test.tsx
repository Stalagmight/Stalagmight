/**
 * The selection's section draws above every other one and takes the click
 * where they overlap (#700 review). These pin SurveyLayer's two halves of that:
 * the selection setters push the section to the custom renderer, and the hit
 * resolvers pass the same section to the geometry index. `Object.create` skips
 * the heavy constructor; a map without `style` makes the interact overlay
 * refresh a no-op.
 */
import { describe, expect, it, vi } from 'vitest';

// maplibre-gl runs `window.URL.createObjectURL` at module-init, which jsdom
// lacks; nothing here touches maplibre.
vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';

interface Probe {
  setSelectedStation: (ref: { section_id: number; station_index: number } | null) => void;
  setSelectedShot: (sectionId: number | null, shotId: number | null) => void;
  setSelectedSection: (sectionId: number | null) => void;
  resolveStationAt: (x: number, y: number, sections: Set<number>, max: number) => unknown;
  resolveShotAt: (x: number, y: number, sections: Set<number>, max: number) => unknown;
}

function makeLayer() {
  const setRaisedSection = vi.fn((_sectionId: number | null) => {});
  const nearestStation = vi.fn((..._args: unknown[]) => null);
  const nearestShot = vi.fn((..._args: unknown[]) => null);
  const layer = Object.create(SurveyLayer.prototype) as Probe;
  Object.assign(layer, {
    map: { project: () => ({ x: 0, y: 0 }) },
    config: null,
    customRender: {
      setRaisedSection,
      setSectionSelect: vi.fn(),
      stationSizeMul: () => 1,
      shotWidthMul: () => 1,
    },
    geoIndex: { nearestStation, nearestShot },
    selectedStationId: null,
    selectedShotId: null,
    selectedSectionId: null,
    showJumpsCached: true,
    showReferenceShotsCached: true,
  });
  return { layer, setRaisedSection, nearestStation, nearestShot };
}

describe('SurveyLayer raises the selection\'s section', () => {
  it('pushes the selected section, or the section of the selected station or shot', () => {
    const { layer, setRaisedSection } = makeLayer();
    const last = () => setRaisedSection.mock.calls[setRaisedSection.mock.calls.length - 1]?.[0];
    layer.setSelectedStation({ section_id: 4, station_index: 2 });
    expect(last()).toBe(4);
    // MapCanvas's order for a shot: the new target first, then the clears.
    layer.setSelectedShot(9, 3);
    expect(last()).toBe(4); // the station still wins until it is cleared
    layer.setSelectedStation(null);
    expect(last()).toBe(9);
    layer.setSelectedSection(7);
    expect(last()).toBe(7);
    layer.setSelectedShot(null, null);
    layer.setSelectedSection(null);
    expect(last()).toBeNull();
  });

  it('passes the same section to the station and shot picks', () => {
    const { layer, nearestStation, nearestShot } = makeLayer();
    const sections = new Set([1, 2]);
    layer.resolveStationAt(5, 5, sections, 10);
    layer.resolveShotAt(5, 5, sections, 10);
    expect(nearestStation.mock.calls[0]![5]).toMatchObject({ raisedSectionId: null });
    expect(nearestShot.mock.calls[0]![5]).toMatchObject({ raisedSectionId: null });
    layer.setSelectedStation({ section_id: 2, station_index: 0 });
    layer.resolveStationAt(5, 5, sections, 10);
    layer.resolveShotAt(5, 5, sections, 10);
    expect(nearestStation.mock.calls[1]![5]).toMatchObject({ raisedSectionId: 2 });
    expect(nearestShot.mock.calls[1]![5]).toMatchObject({ raisedSectionId: 2 });
  });
});

describe('SurveyLayer preview sections', () => {
  it('highlights a dialog preview on top of the selection and clears it again', () => {
    const { layer } = makeLayer();
    const probe = layer as unknown as Probe & {
      setPreviewSections: (ids: readonly number[]) => void;
      customRender: { setSectionSelect: ReturnType<typeof vi.fn> };
    };
    const calls = () => probe.customRender.setSectionSelect.mock.calls;
    const last = () => calls()[calls().length - 1]?.[0];
    probe.setSelectedSection(3);
    expect(last()).toEqual([3]);
    probe.setPreviewSections([3, 8]);
    expect(last()).toEqual([3, 3, 8]);
    probe.setSelectedSection(null);
    expect(last()).toEqual([3, 8]);
    probe.setPreviewSections([]);
    expect(last()).toEqual([]);
  });
});
