/**
 * #576 D15 (decided "One TS walker, parity-tested"): the map placement preview
 * walks the section being placed with the walk the backend will place it with -
 * `walkStations`, pinned to `compute_station_coordinates` by
 * `stationWalkVectors.json` - and with the north reference the section will be
 * recorded with: its own (a DMP import records magnetic, no correction), else
 * the project default (a section typed in the New Section dialog records none).
 *
 * Its own walker took `azimuth_in` alone, walked the slope length and applied no
 * reference, so the preview the user aims with was not where the section lands.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import PlacementOverlay from '../PlacementOverlay';
import { useProjectStore } from '../../../store/projectStore';
import { walkStations } from '../../../utils/stationWalk';
import { DMP_AZIMUTH_REFERENCE } from '../../../utils/importSectionConfig';
import type { AzimuthReference } from '../../../types/ipc/AzimuthReference';
import type { Project, SectionData, ShotData } from '../../../types/project';

const PROJECT_DEFAULT: AzimuthReference = { north: 'magnetic', correction: 7, source: 'manual' };

function shotData(azimuth_in: number, azimuth_out: number, length: number, depth_in: number, depth_out: number): ShotData {
  return {
    azimuth_in,
    azimuth_out,
    length,
    depth_in,
    depth_out,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
  } as unknown as ShotData;
}

// Two readings that disagree (40 / 50, and 350 / 10 across north), a 6 m drop
// on a 10 m tape, a climb on the second.
const SHOTS = [shotData(40, 50, 10, 0, 6), shotData(350, 10, 8, 6, 4)];
const ADDITIONAL = [shotData(120, 130, 6, 4, 9)];

const sectionData = (shots: ShotData[]): SectionData => ({
  name: 'CaveA',
  surveyor: null,
  date: null,
  color: '#3b82f6',
  direction: null,
  quality: 1,
  shots,
});

const CURSOR = { x: 400, y: 300 };
const PPM = 10;

/** Screen position of walked station `i`, anchored on station `anchor` at the cursor. */
function expectedScreen(stations: Array<{ x: number; y: number }>, i: number, anchor: number, offset = { x: 0, y: 0 }) {
  return {
    x: CURSOR.x + (stations[i]!.x + offset.x - stations[anchor]!.x) * PPM,
    y: CURSOR.y - (stations[i]!.y + offset.y - stations[anchor]!.y) * PPM,
  };
}

function circles(container: HTMLElement, selector: string) {
  return Array.from(container.querySelectorAll<SVGCircleElement>(selector)).map((c) => ({
    x: Number(c.getAttribute('cx')),
    y: Number(c.getAttribute('cy')),
  }));
}

beforeEach(() => {
  useProjectStore.setState({ project: { default_azimuth_reference: PROJECT_DEFAULT } as unknown as Project });
});

describe('PlacementOverlay station walk (#576 D15)', () => {
  it('walks a section that records no reference with the project default', () => {
    const { container } = render(
      <PlacementOverlay
        sectionData={sectionData(SHOTS)}
        alignStationIndex={1}
        cursorPosition={CURSOR}
        isSnapped={false}
        pixelsPerMeter={{ east: PPM, north: PPM }}
      />,
    );
    const want = walkStations(SHOTS, PROJECT_DEFAULT);
    const got = circles(container, '[data-testid="placement-preview"] > circle');
    expect(got).toHaveLength(want.length);
    got.forEach((p, i) => {
      const e = expectedScreen(want, i, 1);
      expect(p.x).toBeCloseTo(e.x, 6);
      expect(p.y).toBeCloseTo(e.y, 6);
    });
  });

  it("walks with the section's own reference over the default, segment sections included", () => {
    const offset = { x: 3, y: -4 };
    const { container } = render(
      <PlacementOverlay
        sectionData={sectionData(SHOTS)}
        alignStationIndex={0}
        cursorPosition={CURSOR}
        isSnapped={false}
        pixelsPerMeter={{ east: PPM, north: PPM }}
        azimuthReference={DMP_AZIMUTH_REFERENCE}
        additionalSections={[{ sectionData: sectionData(ADDITIONAL), color: '#ff0000', offsetX: offset.x, offsetY: offset.y }]}
      />,
    );
    const primary = walkStations(SHOTS, DMP_AZIMUTH_REFERENCE);
    const got = circles(container, '[data-testid="placement-preview"] > circle');
    got.forEach((p, i) => {
      const e = expectedScreen(primary, i, 0);
      expect(p.x).toBeCloseTo(e.x, 6);
      expect(p.y).toBeCloseTo(e.y, 6);
    });

    const extra = walkStations(ADDITIONAL, DMP_AZIMUTH_REFERENCE);
    const gotExtra = circles(container, '[data-testid="placement-preview"] > g > circle');
    expect(gotExtra).toHaveLength(extra.length);
    gotExtra.forEach((p, i) => {
      // An additional section's stations sit at `offset` from the primary's station 0.
      const e = {
        x: CURSOR.x + (extra[i]!.x + offset.x - primary[0]!.x) * PPM,
        y: CURSOR.y - (extra[i]!.y + offset.y - primary[0]!.y) * PPM,
      };
      expect(p.x).toBeCloseTo(e.x, 6);
      expect(p.y).toBeCloseTo(e.y, 6);
    });
  });
});
