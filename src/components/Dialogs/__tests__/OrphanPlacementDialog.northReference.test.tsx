/**
 * The orphan placement preview walks both sections with the north reference
 * the backend walks them with (#576 D15): each section's own, else the
 * project's default. Both sections are already in the project, so the backend
 * has rotated each by its correction; a preview that walks the raw readings
 * shows the orphan's merge partners off the target's stations by the
 * difference between the two corrections.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';

const previewProps = vi.fn();
// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. The stub records what the dialog hands it, which is the contract here.
vi.mock('../SectionPreview', () => ({
  default: (props: Record<string, unknown>) => {
    previewProps(props);
    return null;
  },
}));
vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryWith());

import OrphanPlacementDialog from '../OrphanPlacementDialog';
import { useProjectStore } from '../../../store/projectStore';
import type { OrphanPlacementSuggestion } from '../../../types/commands';
import type { AzimuthReference } from '../../../types/ipc/AzimuthReference';
import type { Project } from '../../../types/project';

const ORPHAN_REFERENCE: AzimuthReference = { north: 'magnetic', correction: 7, source: 'file_declared' };
const PROJECT_DEFAULT: AzimuthReference = { north: 'grid', correction: -2, source: 'manual' };

function section(id: number, azimuthReference: AzimuthReference | null) {
  return {
    id,
    name: `Section${id}`,
    surveyor: null,
    date: null,
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    next_shot_id: 1,
    reverse_parity: false,
    gauge_shot_uuid: null,
    gauge_neighbour_uuid: null,
    azimuth_reference: azimuthReference,
    shots: [
      {
        id: 0,
        azimuth_in: 0,
        azimuth_out: 0,
        length: 10,
        depth_in: 0,
        depth_out: 0,
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
      },
    ],
  };
}

const suggestion: OrphanPlacementSuggestion = {
  orphanSectionId: 1,
  orphanSectionName: 'Section1',
  orphanStationIndex: 0,
  targetSectionId: 2,
  targetSectionName: 'Section2',
  targetStationIndex: 1,
  score: 0.9,
  matchedShots: 0,
  existingMatchedShots: 0,
  preferredMergeOffset: 0,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
};

beforeEach(() => {
  previewProps.mockClear();
  useProjectStore.setState({
    project: {
      sections: [section(1, ORPHAN_REFERENCE), section(2, null)],
      default_azimuth_reference: PROJECT_DEFAULT,
    } as unknown as Project,
  });
});

describe('OrphanPlacementDialog preview north reference', () => {
  it('walks the orphan with its own reference and the target with the inherited default', () => {
    render(<OrphanPlacementDialog suggestions={[suggestion]} />);

    const calls = previewProps.mock.calls;
    const props = calls[calls.length - 1]?.[0] as {
      azimuthReference?: AzimuthReference | null;
      mergePreview?: { targetAzimuthReference?: AzimuthReference | null } | null;
    };
    expect(props.azimuthReference).toEqual(ORPHAN_REFERENCE);
    expect(props.mergePreview?.targetAzimuthReference).toEqual(PROJECT_DEFAULT);
  });
});
