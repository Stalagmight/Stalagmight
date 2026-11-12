/**
 * #685: the wizard's "Merge at" opens on the end of the repeat.
 *
 * A section whose first two shots repeat survey already in the project gets a
 * suggestion whose matcher pick (`preferredMergeOffset`) is the station INSIDE
 * the repeat. Joining there would leave the repeated shot after it imported as
 * an ordinary shot, so the wizard must show - and a confirm must record - the
 * last option instead. Everything here is synthetic.
 *
 * What this can and cannot see: the up-front value `computeDefaultMergeOffset`
 * sets lives for one render before the reset effect runs, and React's `act()`
 * flushes that effect before any assertion, so the rule itself is pinned on the
 * pure `defaultMergeOffset` (overlapHelpers.test.ts). This test pins what the
 * user sees and what Confirm sends.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { DmpFile } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { CrossSectionSuggestion } from '../../../types/ipc/CrossSectionSuggestion';

vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../WizardMapPanel', () => ({
  WizardMapPanel: () => null,
  MAP_PANEL_DEFAULT_WIDTH: 400,
  MAP_PANEL_MAX_WIDTH: 800,
}));
vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock('../../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
  setSource: vi.fn(),
}));

const invokeIpcMock = vi.fn();
vi.mock('../../../utils/ipc', () => ({
  invokeIpc: (...args: unknown[]) => invokeIpcMock(...args),
  getErrorMessage: (e: unknown, fallback: string) => (e instanceof Error ? e.message : fallback),
}));

import DmpNewImportDialog from '../DmpNewImportDialog';
import type { ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { convertToEditableShots } from '../../../hooks/useImportState';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';

const shot = (heading: number, length: number) => ({
  shot_type: 'Standard',
  heading_in: heading,
  heading_out: heading,
  length,
  depth_in: 3,
  depth_out: 4,
  pitch_in: 0,
  pitch_out: 0,
  left: 1,
  right: 1,
  up: 0.5,
  down: 0.5,
  temperature: 20,
  hour: 10,
  minute: 0,
  second: 0,
  marker_index: 0,
});

// AAA is the anchor; BBB's first two shots repeat AAA's last two.
const dmpFile = {
  version: 5,
  sections: [
    { version: 5, name: 'AAA', direction: 'in', date: '2026-10-01', content_hash: 'h_a', shots: [shot(10, 12), shot(40, 9), shot(75, 15), shot(120, 8)] },
    { version: 5, name: 'BBB', direction: 'in', date: '2026-10-01', content_hash: 'h_b', shots: [shot(75, 15), shot(120, 8), shot(160, 10), shot(200, 7)] },
  ],
} as unknown as DmpFile;

const selectionConfig: DmpSelectionConfig = {
  selectedIndices: new Set([0, 1]),
  defaultSurveyor: 'S',
  defaultDate: null,
  fileHash: 'file-hash',
  sectionHashes: ['h_a', 'h_b'],
};

const zeroDatum = (): ZeroDatumConfigResult => ({
  sectionIndex: 0,
  stationIndex: 0,
  gps: { latitude: 20.5, longitude: -87.3, depth_reference: 0 },
  metadata: { name: 'AAA', surveyor: 'S', date: '2026-10-01', color: '#4ecdc4', direction: 'in', quality: 1, reverse: false },
  shots: convertToEditableShots(dmpFile.sections[0]!.shots),
});

const ANCHOR_ID = 7;

// The matcher's view: BBB station 0 onto AAA station 2, two shots matched, its
// own pick one station in (offset 1, mid-repeat).
const suggestion: CrossSectionSuggestion = {
  existingStationIndex: 2,
  importStationIndex: 0,
  score: 0.95,
  matchedShots: 2,
  existingMatchedShots: 2,
  preferredMergeOffset: 1,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'AAA',
  importSectionIndex: 1,
  importSectionName: 'BBB',
  targetType: 'existing',
  existingSectionId: ANCHOR_ID,
  targetImportIndex: null,
};

let importCalls: Array<Record<string, unknown>> = [];

describe('DMP wizard: the default merge station is the end of the repeat (#685)', () => {
  beforeEach(() => {
    importCalls = [];
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'import_sections') {
        importCalls.push(args ?? {});
        const configs = (args?.sections as unknown[] | undefined) ?? [];
        const ids = configs.map((_, i) => ANCHOR_ID + importCalls.length - 1 + i);
        return { success: true, section_ids: ids, import_receipt: null, warnings: [] };
      }
      if (cmd === 'suggest_all_import_placements') return [suggestion];
      // The temporal rerank asks for distances from the anchor; none known.
      if (cmd === 'get_station_distances_from') return { station_refs: [], distances: [], max_reachable: 0 };
      if (cmd === 'execute_batch' || cmd === 'save_project') return { success: true, warnings: [] };
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, importContext: null });
    useProjectStore.setState({ project: null });
  });

  it('shows the end of the repeat, not the matcher pick, when the suggestion appears', async () => {
    render(
      <DmpNewImportDialog
        dmpFile={dmpFile}
        selectionConfig={selectionConfig}
        project={null}
        stations={[]}
        onCancel={vi.fn()}
        onBack={vi.fn()}
        onComplete={vi.fn()}
        zeroDatumConfig={zeroDatum()}
        autoStartManualPlacement={false}
      />,
    );

    const select = await screen.findByTestId('merge-station-select');
    await waitFor(() => expect(select).toHaveTextContent('stn 2'));
    expect(select).toHaveTextContent('stn 4');
    expect(select).not.toHaveTextContent('stn 1 →');

    // And Confirm imports BBB joined at the end of the repeat: placed from
    // AAA's station 4, carrying only its two shots past the repeat. Joined
    // mid-repeat it would hang off AAA station 3 and bring a repeated shot.
    fireEvent.click(screen.getByTestId('confirm-button'));
    await waitFor(() => expect(importCalls.length).toBeGreaterThan(1));
    const bbb = (importCalls[importCalls.length - 1]!.sections as Array<Record<string, unknown>>)[0]!;
    expect(bbb.name).toBe('BBB');
    expect(bbb.placement).toEqual({
      type: 'from_existing',
      import_station: 0,
      existing_station: { section_id: ANCHOR_ID, station_index: 4 },
    });
    expect((bbb.shots as Array<{ azimuth_in: number }>).map((s) => s.azimuth_in)).toEqual([160, 200]);
  });
});
