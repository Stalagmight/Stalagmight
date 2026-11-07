/**
 * #576 D12: the merge station a suggestion opens with is the backend-scored
 * `preferredMergeOffset`, not the deepest overlap.
 *
 * `pickFromPool` set the preferred offset, and then an effect keyed on the
 * suggestion overwrote it with the deepest one on the next render - so unless
 * the user noticed the dropdown, the section was joined (and its overlap shots
 * trimmed) at a station the matcher had scored lower, possibly one it skipped
 * for crossing another passage.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import type { DmpFile } from '../../../types/commands';
import type { CrossSectionSuggestion } from '../../../types/ipc/CrossSectionSuggestion';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
// Real i18n: the merge-station option text ("stn 1 → A stn 3") asserted below
// renders from en.json.
import '../../../i18n';

// SectionPreview and the map panel pull maplibre-gl (WebGL) at module load,
// which jsdom cannot execute.
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
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import { invokeIpcMock } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');

import DmpNewImportDialog from '../DmpNewImportDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';

// Fully synthetic sections, five importable shots each.
const dmpSection = (name: string) => ({
  version: 5,
  name,
  direction: 'in',
  date: '2026-01-01',
  content_hash: `hash_${name}`,
  shots: Array.from({ length: 5 }, () => ({
    shot_type: 'Standard',
    heading_in: 10,
    heading_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 20,
    hour: 12,
    minute: 0,
    second: 0,
    marker_index: 0,
  })),
});

const dmpFile = { version: 5, sections: [dmpSection('A'), dmpSection('B')], warnings: [] } as unknown as DmpFile;

const selectionConfig: DmpSelectionConfig = {
  selectedIndices: new Set([0, 1]),
  defaultSurveyor: 'S',
  defaultDate: null,
  fileHash: null,
  sectionHashes: [],
};

// B's station 0 onto A's station 2, three shots of overlap: offsets 0..3 are
// on offer (`computeMergeOptions`), the deepest is 3, the matcher prefers 1.
const SUGGESTION: CrossSectionSuggestion = {
  existingStationIndex: 2,
  importStationIndex: 0,
  score: 1.4,
  matchedShots: 3,
  existingMatchedShots: 3,
  preferredMergeOffset: 1,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'A',
  importSectionIndex: 1,
  importSectionName: 'B',
  targetType: 'importing',
  existingSectionId: null,
  targetImportIndex: 0,
};

describe('DMP wizard: the merge station a suggestion opens with (#576 D12)', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'suggest_all_import_placements') return [SUGGESTION];
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null });
    useProjectStore.setState({ project: null });
  });

  it("opens on the backend's preferred offset, not the deepest", async () => {
    render(
      <DmpNewImportDialog
        dmpFile={dmpFile}
        selectionConfig={selectionConfig}
        project={null}
        stations={[]}
        onCancel={vi.fn()}
        onBack={vi.fn()}
        onComplete={vi.fn()}
        autoStartManualPlacement={false}
      />,
    );

    const select = await screen.findByTestId('merge-station-select');
    await waitFor(() => expect(screen.getByTestId('wizard-merge-station-control')).toBeInTheDocument());
    // Let every render and passive effect queued by the suggestion's arrival run:
    // an awaited act() drains React's work queue, so the old overriding effect
    // would have rewritten the select by the time it returns (the waitFor above,
    // which RTL wraps in act, already does this; the explicit flush says so).
    // Not a fixed wait (#576 RV2-FE-D5), and not a waitFor on the text either:
    // the FIRST render already shows the preferred offset, so that would pass
    // before such an effect ran.
    await act(async () => {});
    // Offset 1: B stn 1 onto A stn 3. The deepest (offset 3) would read "stn 3 -> A stn 5".
    expect(select.textContent).toMatch(/^stn 1 → A stn 3/);
  });
});
