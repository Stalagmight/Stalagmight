import { describe, expect, it } from 'vitest';
import { buildGhostHighlights, type GhostHighlightHost } from '../dragGhostHighlight';

// A LineString shot feature (2 points) so shotPrim yields a primitive.
const shotFeature = (): GeoJSON.Feature => ({
  type: 'Feature',
  geometry: { type: 'LineString', coordinates: [[0, 0], [1, 1]] },
  properties: { opacity: 1 },
});
// A Point station feature so stationPrim yields a primitive.
const stationFeature = (): GeoJSON.Feature => ({
  type: 'Feature',
  geometry: { type: 'Point', coordinates: [2, 2] },
  properties: { opacity: 1 },
});

function makeHost(over: Partial<GhostHighlightHost> = {}): GhostHighlightHost {
  return {
    hoveredShotId: null,
    selectedShotId: null,
    hoveredStationId: null,
    selectedStationId: null,
    shotsVisible: true,
    stationsVisible: true,
    selShotWidth: 3,
    hoverShotWidth: 4,
    selRadius: 5,
    hoverRadius: 6,
    hoverStroke: [255, 255, 255, 255],
    selStroke: [255, 255, 255, 255],
    movingEndForShot: () => undefined,
    isMovingStation: () => false,
    parseFid: () => 42,
    isAnchored: () => false,
    isShotImmovable: () => false,
    isStationImmovable: () => false,
    shotFeature,
    stationFeature,
    colorFromProps: () => [10, 20, 30, 255],
    ...over,
  };
}

describe('buildGhostHighlights selected-branch anchored/immovable guard', () => {
  it('highlights a moving selected shot/station that is not anchored or immovable', () => {
    const { shots, stations } = buildGhostHighlights(
      makeHost({
        selectedShotId: 'S1',
        selectedStationId: 'ST1',
        movingEndForShot: (id) => (id === 'S1' ? 'to' : undefined),
        isMovingStation: (id) => id === 'ST1',
      }),
    );
    expect(shots).toHaveLength(1);
    expect(stations).toHaveLength(1);
  });

  it('excludes an anchored selected shot even when it reports as moving', () => {
    const { shots } = buildGhostHighlights(
      makeHost({
        selectedShotId: 'S1',
        movingEndForShot: (id) => (id === 'S1' ? 'to' : undefined),
        isAnchored: () => true, // anchored: must be filtered like the hover branch
      }),
    );
    expect(shots).toHaveLength(0);
  });

  it('excludes an immovable selected shot even when it reports as moving', () => {
    const { shots } = buildGhostHighlights(
      makeHost({
        selectedShotId: 'S1',
        movingEndForShot: (id) => (id === 'S1' ? 'to' : undefined),
        isShotImmovable: () => true,
      }),
    );
    expect(shots).toHaveLength(0);
  });

  it('excludes an anchored/immovable selected station even when it reports as moving', () => {
    const anchored = buildGhostHighlights(
      makeHost({ selectedStationId: 'ST1', isMovingStation: () => true, isAnchored: () => true }),
    );
    expect(anchored.stations).toHaveLength(0);

    const immovable = buildGhostHighlights(
      makeHost({
        selectedStationId: 'ST1',
        isMovingStation: () => true,
        isStationImmovable: () => true,
      }),
    );
    expect(immovable.stations).toHaveLength(0);
  });
});

// #301: isAnchored is keyed by the feature's "lineId:scopedId" id, NOT by its
// numeric fid. A shot's scoped id is shot.id, which is itself line-encoded
// (lineId * FEATURE_ID_STRIDE + seq), so the fid of a line-1 shot is
// 2 * FEATURE_ID_STRIDE and the old floor(fid / STRIDE) decode called it line 2.
// These pin the id-string contract, so a host that gates on the real line id
// filters the highlight instead of leaking it through.
describe('buildGhostHighlights anchored gating uses the line id, not a decoded fid', () => {
  // Line 1, shot.id already 1 * FEATURE_ID_STRIDE; parseFid returns the
  // double-encoded fid SurveyLayer would build (1 * STRIDE + 100000).
  const LINE_1_SHOT = '1:100000';
  const LINE_1_STATION = '1:0';
  const anchoredLine1 = (id: string): boolean => Number(id.split(':')[0]) === 1;

  it('excludes a moving shot in an anchored line >= 1', () => {
    const { shots } = buildGhostHighlights(
      makeHost({
        selectedShotId: LINE_1_SHOT,
        movingEndForShot: () => 'to',
        parseFid: () => 200000,
        isAnchored: anchoredLine1,
      }),
    );
    expect(shots).toHaveLength(0);
  });

  it('excludes a moving station in an anchored line >= 1', () => {
    const { stations } = buildGhostHighlights(
      makeHost({
        selectedStationId: LINE_1_STATION,
        isMovingStation: () => true,
        parseFid: () => 100000,
        isAnchored: anchoredLine1,
      }),
    );
    expect(stations).toHaveLength(0);
  });

  it('keeps a moving shot whose line is not anchored', () => {
    const { shots } = buildGhostHighlights(
      makeHost({
        selectedShotId: '2:200000',
        movingEndForShot: () => 'to',
        parseFid: () => 400000,
        isAnchored: anchoredLine1,
      }),
    );
    expect(shots).toHaveLength(1);
  });

  it('passes the hovered/selected id through verbatim', () => {
    const seen: string[] = [];
    buildGhostHighlights(
      makeHost({
        hoveredShotId: LINE_1_SHOT,
        hoveredStationId: LINE_1_STATION,
        movingEndForShot: () => 'to',
        isMovingStation: () => true,
        isAnchored: (id) => {
          seen.push(id);
          return false;
        },
      }),
    );
    expect(seen).toEqual([LINE_1_SHOT, LINE_1_STATION]);
  });
});
