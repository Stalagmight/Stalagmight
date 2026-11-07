import { beforeEach, describe, expect, it, vi } from 'vitest';

const evalInAppMock = vi.hoisted(() => vi.fn());
vi.mock('./page', () => ({ evalInApp: evalInAppMock }));

import { mapFeatureClick } from './mapControls';
import type { Browser } from 'webdriverio';

function pointerBrowser() {
  const canvas = { waitForDisplayed: vi.fn(), moveTo: vi.fn() };
  const pointer = {
    move: vi.fn().mockReturnThis(),
    down: vi.fn().mockReturnThis(),
    up: vi.fn().mockReturnThis(),
    perform: vi.fn(),
  };
  return { $: vi.fn(() => canvas), action: vi.fn(() => pointer) } as unknown as Browser;
}

beforeEach(() => vi.clearAllMocks());

describe('mapFeatureClick', () => {
  it('resolves a semantic shot, clicks the fresh point, and verifies selection', async () => {
    const browser = pointerBrowser();
    evalInAppMock
      .mockResolvedValueOnce({
        featureType: 'shot',
        sectionId: 7,
        shotId: 42,
        lngLat: [1, 2],
        canvasPoint: { x: 20, y: 30 },
        canvasSize: { width: 200, height: 100 },
        point: { x: 120, y: 230 },
        clearancePx: 9.5,
        interactionSequenceBefore: 4,
      })
      .mockResolvedValueOnce('canvas.maplibregl-canvas')
      .mockResolvedValueOnce({
        interactionSequence: 5,
        selection: { type: 'shot', sectionId: 7, shotId: 42 },
      })
      .mockResolvedValueOnce(undefined);

    await expect(mapFeatureClick(browser, { featureType: 'shot', sectionId: 7, shotId: 42 })).resolves.toMatchObject({
      point: { x: 120, y: 230 },
      selectionVerified: true,
    });
    expect(evalInAppMock.mock.calls[0]?.[1]).toContain('__stalagmight_geometry__');
    expect(evalInAppMock.mock.calls[0]?.[1]).not.toContain('map.fire');
    expect(evalInAppMock.mock.calls[1]?.[1]).toContain('document.elementFromPoint');
    expect(evalInAppMock.mock.calls[3]?.[1]).toContain('__stalagmight_clear_map_target__');
  });

  it('fails when overlapping geometry selects a different feature', async () => {
    const browser = pointerBrowser();
    evalInAppMock
      .mockResolvedValueOnce({
        featureType: 'station',
        sectionId: 3,
        stationIndex: 8,
        lngLat: [1, 2],
        canvasPoint: { x: 20, y: 30 },
        canvasSize: { width: 200, height: 100 },
        point: { x: 120, y: 230 },
        clearancePx: 0,
        interactionSequenceBefore: 8,
      })
      .mockResolvedValueOnce('canvas.maplibregl-canvas')
      .mockResolvedValueOnce({
        interactionSequence: 9,
        selection: { type: 'station', stationRef: { section_id: 3, station_index: 9 } },
      })
      .mockResolvedValueOnce(undefined);

    await expect(
      mapFeatureClick(browser, { featureType: 'station', sectionId: 3, stationIndex: 8 }),
    ).rejects.toThrow('instead of station 3:8');
  });

  it("keeps the app's selection off the line a runner prints, and refuses an unknown feature type (#576 W51)", async () => {
    const browser = pointerBrowser();
    evalInAppMock
      .mockResolvedValueOnce({
        featureType: 'station',
        sectionId: 3,
        stationIndex: 8,
        lngLat: [1, 2],
        canvasPoint: { x: 20, y: 30 },
        canvasSize: { width: 200, height: 100 },
        point: { x: 120, y: 230 },
        clearancePx: 0,
        interactionSequenceBefore: 8,
      })
      .mockResolvedValueOnce('div[data-testid=system-card-qzCaveA]')
      .mockResolvedValueOnce({ interactionSequence: 9, selection: { type: 'section', name: 'qzCaveA' } })
      .mockResolvedValueOnce(undefined);
    const err = await mapFeatureClick(browser, { featureType: 'station', sectionId: 3, stationIndex: 8 }).then(
      () => new Error('resolved'),
      (e: unknown) => e as Error,
    );
    expect(err.message.split('\n')[0]).toBe('map_feature_click selected something else instead of station 3:8');
    expect(err.message.split('\n')[0]).not.toContain('qzCaveA');

    await expect(
      mapFeatureClick(browser, { featureType: 'qzCaveA' as 'shot', sectionId: 3, shotId: 1 }),
    ).rejects.toThrow(new Error('map_feature_click needs a featureType of shot or station'));
  });

  it('allows non-selection map tools to skip selection verification', async () => {
    const browser = pointerBrowser();
    evalInAppMock
      .mockResolvedValueOnce({
        featureType: 'station',
        sectionId: 3,
        stationIndex: 8,
        lngLat: [1, 2],
        canvasPoint: { x: 20, y: 30 },
        canvasSize: { width: 200, height: 100 },
        point: { x: 120, y: 230 },
        clearancePx: 0,
        interactionSequenceBefore: 10,
      })
      .mockResolvedValueOnce('canvas.maplibregl-canvas')
      .mockResolvedValueOnce({ interactionSequence: 11, selection: null })
      .mockResolvedValueOnce(undefined);

    await expect(
      mapFeatureClick(browser, {
        featureType: 'station',
        sectionId: 3,
        stationIndex: 8,
        button: 'right',
        verifySelection: false,
      }),
    ).resolves.toMatchObject({ selectionVerified: false });
    expect(evalInAppMock).toHaveBeenCalledTimes(4);
  });

  it('rejects a click when stale selection makes it look successful', async () => {
    const browser = pointerBrowser();
    evalInAppMock
      .mockResolvedValueOnce({
        featureType: 'station',
        sectionId: 3,
        stationIndex: 8,
        lngLat: [1, 2],
        canvasPoint: { x: 20, y: 30 },
        canvasSize: { width: 200, height: 100 },
        point: { x: 120, y: 230 },
        clearancePx: 0,
        interactionSequenceBefore: 12,
      })
      .mockResolvedValueOnce('canvas.maplibregl-canvas')
      .mockResolvedValueOnce({
        interactionSequence: 12,
        selection: { type: 'station', stationRef: { section_id: 3, station_index: 8 } },
      })
      .mockResolvedValueOnce(undefined);

    await expect(
      mapFeatureClick(browser, { featureType: 'station', sectionId: 3, stationIndex: 8 }),
    ).rejects.toThrow('did not reach a semantic map interaction handler');
  });

  it('clears an armed semantic target when pointer dispatch fails', async () => {
    const browser = pointerBrowser();
    evalInAppMock
      .mockResolvedValueOnce({
        featureType: 'station',
        sectionId: 3,
        stationIndex: 8,
        lngLat: [1, 2],
        canvasPoint: { x: 20, y: 30 },
        canvasSize: { width: 200, height: 100 },
        point: { x: 120, y: 230 },
        clearancePx: 0,
        interactionSequenceBefore: 12,
      })
      .mockRejectedValueOnce(new Error('pointer dispatch failed'))
      .mockResolvedValueOnce(undefined);

    await expect(
      mapFeatureClick(browser, { featureType: 'station', sectionId: 3, stationIndex: 8 }),
    ).rejects.toThrow('pointer dispatch failed');
    expect(evalInAppMock.mock.calls[2]?.[1]).toContain('__stalagmight_clear_map_target__');
  });
});
