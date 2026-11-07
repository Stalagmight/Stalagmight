/**
 * #576 D15 (decided "One TS walker, parity-tested"): the DMP wizard draws the
 * EXISTING project sections it shows with the walk the backend uses - each with
 * its own north reference, else the project default (`azimuthReferenceFor`) -
 * and draws every one of them the same way, so a merge preview and the context
 * drawn around it agree.
 *
 * The wizard used to walk existing sections with no reference at all, so a
 * section recorded with a declination (or inheriting one) was drawn rotated by
 * it - off the backend's own segment geometry, and off its merge partners.
 *
 * Every number here is checked against `walkStations` itself, the walk pinned
 * to the backend's by `stationWalkVectors.json`. The fixture's shots read two
 * different bearings (40 / 50, 350 / 10), drop or climb, and the sections carry
 * a magnetic and a grid reference with corrections - each of which the old
 * walk got wrong. All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const previewProps = vi.fn();
// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. The stub records what the wizard hands it - the contract here - and
// keeps the real walk adapters the wizard imports from the same module.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: (props: Record<string, unknown>) => {
    previewProps(props);
    return null;
  },
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../WizardMapPanel', () => ({
  WizardMapPanel: ({ children }: { children?: unknown }) => children ?? null,
  MAP_PANEL_DEFAULT_WIDTH: 400,
  MAP_PANEL_MAX_WIDTH: 800,
}));
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import { invokeIpcMock, stubGetErrorMessage } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');
stubGetErrorMessage((e, fallback) => (e instanceof Error ? e.message : (fallback ?? String(e))));

import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { walkStations } from '../../../utils/stationWalk';
import { DMP_AZIMUTH_REFERENCE } from '../../../utils/importSectionConfig';
import type { AzimuthReference } from '../../../types/ipc/AzimuthReference';
import type { CrossSectionSuggestion } from '../../../types/ipc/CrossSectionSuggestion';
import type { ContextSectionGeometry } from '../SectionPreview';
import type { Project } from '../../../types/project';
import { renderWizard, savedState } from './dmpWizardHarness';

Element.prototype.scrollIntoView = vi.fn();

const PROJECT_DEFAULT: AzimuthReference = { north: 'magnetic', correction: 7, source: 'manual' };
const NEIGHBOR_REFERENCE: AzimuthReference = { north: 'grid', correction: -2, source: 'manual' };

type WalkRow = { azimuth_in: number; azimuth_out: number; length: number; depth_in: number; depth_out: number };
const TARGET_SHOTS: WalkRow[] = [
  { azimuth_in: 40, azimuth_out: 50, length: 10, depth_in: 0, depth_out: 6 },
  { azimuth_in: 350, azimuth_out: 10, length: 8, depth_in: 6, depth_out: 4 },
];
const NEIGHBOR_SHOTS: WalkRow[] = [{ azimuth_in: 100, azimuth_out: 110, length: 6, depth_in: 4, depth_out: 9 }];

function section(id: number, name: string, reference: AzimuthReference | null, shots: WalkRow[]) {
  return {
    id,
    name,
    surveyor: null,
    date: null,
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    next_shot_id: shots.length,
    reverse_parity: false,
    gauge_shot_uuid: null,
    gauge_neighbour_uuid: null,
    azimuth_reference: reference,
    shots: shots.map((s, i) => ({
      id: id * 100000 + i,
      ...s,
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
    })),
  };
}

// Target 5 inherits the project default; neighbour 6 carries its own grid
// reference and is merged at target station 2 = its station 0.
const PROJECT = {
  sections: [section(5, 'Target', null, TARGET_SHOTS), section(6, 'Neighbor', NEIGHBOR_REFERENCE, NEIGHBOR_SHOTS)],
  station_merges: [
    { station_a: { section_id: 5, station_index: 2 }, station_b: { section_id: 6, station_index: 0 }, merged_at: '2026-01-01T00:00:00Z' },
  ],
  default_azimuth_reference: PROJECT_DEFAULT,
  created_at: '2026-01-01T00:00:00Z',
} as unknown as Project;

const T = walkStations(TARGET_SHOTS, PROJECT_DEFAULT);
const N = walkStations(NEIGHBOR_SHOTS, NEIGHBOR_REFERENCE);

const close = (got: number, want: number) => expect(got).toBeCloseTo(want, 9);

/** The last props SectionPreview got that carry `key`. */
function lastPropsWith<K extends string>(key: K): Record<K, unknown> & Record<string, unknown> {
  const calls = previewProps.mock.calls.map((c) => c[0] as Record<string, unknown>);
  const hit = [...calls].reverse().find((p) => p[key] != null);
  if (!hit) throw new Error(`SectionPreview never received ${key}`);
  return hit as Record<K, unknown> & Record<string, unknown>;
}

beforeEach(() => {
  previewProps.mockClear();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null });
  useProjectStore.setState({ project: PROJECT });
});

describe('DMP wizard: existing sections walked with their north reference (#576 D15)', () => {
  it('hands the merge preview the target reference, and offsets the import-side context by the same walk', async () => {
    // A (0) connects to B (1) first; then B is offered onto existing section 5,
    // so A is import-side context drawn around B and must be offset by the SAME
    // target walk the preview aligns B with.
    const toB: CrossSectionSuggestion = {
      existingStationIndex: 0,
      importStationIndex: 2,
      score: 2,
      matchedShots: 0,
      existingMatchedShots: 0,
      preferredMergeOffset: 0,
      direction: 'forward',
      isPartial: false,
      confidence: 'high',
      existingSectionName: 'B',
      importSectionIndex: 0,
      importSectionName: 'A',
      targetType: 'importing',
      existingSectionId: null,
      targetImportIndex: 1,
    };
    const onto5: CrossSectionSuggestion = {
      ...toB,
      existingStationIndex: 1,
      importStationIndex: 0,
      score: 1,
      existingSectionName: 'Target',
      importSectionIndex: 1,
      importSectionName: 'B',
      targetType: 'existing',
      existingSectionId: 5,
      targetImportIndex: null,
    };
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'suggest_all_import_placements') return [toB, onto5];
      if (cmd === 'get_segment_geometry') return { sections: [] };
      return null;
    });
    renderWizard(undefined, { selected: [0, 1] });

    await screen.findByTestId('wizard-suggestion');
    await act(async () => {
      await userEvent.click(screen.getByTestId('confirm-button'));
    });
    await waitFor(() => expect(lastPropsWith('mergePreview').mergePreview).toBeTruthy());

    const props = lastPropsWith('mergePreview');
    const merge = props.mergePreview as { targetAzimuthReference?: AzimuthReference | null; targetStationIndex: number; importStationIndex: number };
    expect(merge.targetAzimuthReference).toEqual(PROJECT_DEFAULT);

    // A, drawn in B's frame, is shifted so B's merge station sits on target
    // station 1 - as walked with the target's reference.
    await waitFor(() => expect(lastPropsWith('contextSections').contextSections).toBeTruthy());
    const context = lastPropsWith('contextSections').contextSections as ContextSectionGeometry[];
    const bStations = walkStations(
      [0, 1].map(() => ({ azimuth_in: 10, azimuth_out: 10, length: 5, depth_in: 0, depth_out: 0 })),
      DMP_AZIMUTH_REFERENCE,
    );
    const aStations = bStations; // the harness sections are identical
    // A's station 2 is B's station 0 (the confirmed connection).
    const aOffset = { x: bStations[0]!.x - aStations[2]!.x, y: bStations[0]!.y - aStations[2]!.y };
    const shift = { x: T[1]!.x - bStations[0]!.x, y: T[1]!.y - bStations[0]!.y };
    expect(context).toHaveLength(1);
    const first = context[0]!.shots[0]!;
    close(first.from[0], aStations[0]!.x + aOffset.x + shift.x);
    close(first.from[1], aStations[0]!.y + aOffset.y + shift.y);
  });

  it('draws the From Existing context with each section\'s own walk, merge partners coinciding', async () => {
    renderWizard(savedState({ segmentQueue: [[0]], segmentAnchor: { sectionIndex: 0, stationIndex: 0 } }), {
      selected: [0],
    });
    await userEvent.click(await screen.findByTestId('segment-mode-from-existing-input'));
    await userEvent.click(screen.getByTestId('from-existing-section-select'));
    await userEvent.click(screen.getByTestId('from-existing-section-select-option-5'));

    await waitFor(() => expect(lastPropsWith('contextSections').contextSections).toBeTruthy());
    const context = lastPropsWith('contextSections').contextSections as ContextSectionGeometry[];
    const target = context.find((c) => c.color === '#95a5a6')!;
    const neighbor = context.find((c) => c.color === '#6b7280')!;
    expect(target.shots).toHaveLength(2);
    expect(neighbor.shots).toHaveLength(1);

    // Each shot spans exactly what its section's own walk says it spans.
    target.shots.forEach((s, i) => {
      close(s.to[0] - s.from[0], T[i + 1]!.x - T[i]!.x);
      close(s.to[1] - s.from[1], T[i + 1]!.y - T[i]!.y);
    });
    close(neighbor.shots[0]!.to[0] - neighbor.shots[0]!.from[0], N[1]!.x - N[0]!.x);
    close(neighbor.shots[0]!.to[1] - neighbor.shots[0]!.from[1], N[1]!.y - N[0]!.y);
    // And the merged pair is drawn on one spot: target station 2, neighbour station 0.
    close(neighbor.shots[0]!.from[0], target.shots[1]!.to[0]);
    close(neighbor.shots[0]!.from[1], target.shots[1]!.to[1]);
  });

  it('hands map placement the reference the DMP sections will be recorded with', async () => {
    renderWizard(savedState({ segmentQueue: [[0]], segmentAnchor: { sectionIndex: 0, stationIndex: 0 } }), {
      selected: [0],
      autoStartManualPlacement: true,
    });
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    expect(useUIStore.getState().placementMode!.azimuthReference).toEqual(DMP_AZIMUTH_REFERENCE);
  });
});
