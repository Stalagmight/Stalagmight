/**
 * #487: the DMP wizard's manual-placement path surfaced no import receipt.
 *
 * Manual map placement CLOSES the wizard (`handleStartSegmentPlacement` ->
 * `closeDialog`) and reopens a fresh instance through `openDialog` with a
 * `SavedManualPlacingState`. Every `import_receipt` banked before that handoff
 * lives in a component ref, so it died with the old instance; the reopened one
 * completed the wizard with an empty list, `aggregateImportReceipts` returned
 * `undefined`, and App.tsx's `if (receipt)` skipped the presentation entirely.
 *
 * The two halves of the carry get one test each: what the handoff SAVES, and
 * what the reopened instance COMPLETES with.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { failOnRefreshErrors } from '../../../test/refreshGuard';
import { act, render, screen, waitFor } from '@testing-library/react';
import type { DmpFile } from '../../../types/commands';
import type { ImportReceipt } from '../../../types/ipc/ImportReceipt';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

// SectionPreview and the map panel pull maplibre-gl (WebGL) at module load,
// which jsdom cannot execute. This test exercises the receipt plumbing, not
// anything drawn.
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
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import { emptyBinaryRefresh, invokeIpcMock, stubGetErrorMessage } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');
// [#576 NF-W6A-5] The refresh every edit/import ends with reads bytes; a test that
// gives none answers "no project open" rather than making the decoder throw.
emptyBinaryRefresh();
stubGetErrorMessage((e, fallback) =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : (fallback ?? String(e)),
);

import DmpNewImportDialog, { type SavedManualPlacingState } from '../DmpNewImportDialog';
import type { ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { convertToEditableShots } from '../../../utils/shotConversion';
import { useUIStore } from '../../../store/uiStore';
import { renderReopenedWizard } from './dmpWizardHarness';

const receipt = (overrides: Partial<ImportReceipt> = {}): ImportReceipt => ({
  source_format: 'mNemo DMP',
  file_name: 'dive.dmp',
  sections_found: 2,
  sections_imported: 1,
  shot_count: 3,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: [],
  info: [],
  zero_datum_created: null,
  ...overrides,
});

/** The receipt banked by the early import, before the map-placement handoff. */
const EARLY_RECEIPT = receipt({ sections_imported: 1, shot_count: 3 });
/** What the backend returns for the segment the user drops on the map. */
const SEGMENT_RECEIPT = receipt({ sections_imported: 1, shot_count: 4 });

// Fully synthetic, in the real `DmpShot` shape: `shot_type: 'Standard'` and a
// length other than 1 are what `isImportableDmpShot` keeps, and a section whose
// shots are all filtered out emits no import config at all.
const dmpSection = (name: string) => ({
  version: 1,
  name,
  direction: 'in',
  date: '2026-01-01',
  content_hash: `hash_${name}`,
  shots: Array.from({ length: 2 }, () => ({
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

const dmpFile = { version: 1, sections: [dmpSection('A'), dmpSection('B')] } as unknown as DmpFile;

const selectionConfig = (selected: number[]): DmpSelectionConfig => ({
  selectedIndices: new Set(selected),
  defaultSurveyor: 'S',
  defaultDate: null,
  fileHash: null,
  sectionHashes: [],
});

const savedState = (overrides: Partial<SavedManualPlacingState>): SavedManualPlacingState => ({
  connections: [],
  skippedIndices: [],
  segmentQueue: [],
  segmentAnchor: { sectionIndex: 0, stationIndex: 0 },
  directPlacements: [],
  skippedSuggestions: [],
  segmentSkipCounts: [],
  sectionEdits: [],
  ...overrides,
});

const renderWizard = (
  saved: SavedManualPlacingState | undefined,
  props: {
    onComplete: (ids?: number[], r?: ImportReceipt) => void;
    autoStartManualPlacement?: boolean;
    selected?: number[];
    zeroDatumConfig?: ZeroDatumConfigResult;
    file?: DmpFile;
  },
) =>
  render(
    <DmpNewImportDialog
      dmpFile={props.file ?? dmpFile}
      selectionConfig={selectionConfig(props.selected ?? [0])}
      project={null}
      stations={[]}
      onCancel={vi.fn()}
      onBack={vi.fn()}
      onComplete={props.onComplete}
      savedManualPlacingState={saved}
      zeroDatumConfig={props.zeroDatumConfig}
      autoStartManualPlacement={props.autoStartManualPlacement ?? false}
    />,
  );

/** The single-section case: the zero datum IS the whole import. */
const zeroDatumOnly = (): ZeroDatumConfigResult => ({
  sectionIndex: 0,
  stationIndex: 0,
  gps: { latitude: 20, longitude: -87, depth_reference: 0 },
  metadata: {
    name: 'A',
    surveyor: 'S',
    date: '2026-01-01',
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    reverse: false,
  },
  shots: convertToEditableShots(dmpFile.sections[0]!.shots, dmpFile.sections[0]!.version),
});

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

describe('DMP wizard receipt carry across manual placement', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'import_sections') {
        return { success: true, section_ids: [7], import_receipt: SEGMENT_RECEIPT, warnings: [] };
      }
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null });
  });

  /**
   * #576 D13 (decided "Import in reopened wizard"): the drop only records the
   * placement and reopens the wizard, which imports the segment itself - so the
   * reopen carries the run's receipts and the placed segment, and the receipt of
   * the segment import is banked by the instance that completes the run.
   */
  it('carries the receipts to the reopened wizard, which imports the dropped segment', async () => {
    const first = renderWizard(
      savedState({
        // Section 0 landed in the early import, through a confirmed suggestion.
        connections: [
          { importSectionIndex: 0, importStationIndex: 0, targetType: 'existing', existingSectionId: 3, existingStationIndex: 0 },
        ],
        earlyImported: [[0, 100]],
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedReceipts: [EARLY_RECEIPT],
      }),
      { onComplete: vi.fn(), autoStartManualPlacement: true, selected: [0, 1] },
    );

    // The auto-start handoff is a setTimeout(0) that closes the dialog and puts
    // the app in placement mode.
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());

    // The user drops the section on the map: the wizard reopens with the
    // placement pending and the receipts banked so far.
    await act(async () => {
      useUIStore.getState().placementMode!.onComplete!({
        position: { x: 1, y: 2, z: 0 },
        snappedToStation: null,
      });
    });

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('dmp-import-new'));
    const reopened = useUIStore.getState().dialogData as {
      savedManualPlacingState?: SavedManualPlacingState & { placedSegment?: { segment: number[] } };
    };
    expect(reopened.savedManualPlacingState?.importedReceipts).toEqual([EARLY_RECEIPT]);
    expect(reopened.savedManualPlacingState?.placedSegment?.segment).toEqual([1]);
    // Nothing was imported by the instance that closed.
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).not.toContain('import_sections');

    first.unmount();
    const onComplete = vi.fn();
    renderReopenedWizard({ onComplete });

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(invokeIpcMock.mock.calls.filter((c) => c[0] === 'import_sections')).toHaveLength(1);
    expect(onComplete.mock.calls[0]![0]).toEqual([7]);
    expect(onComplete.mock.calls[0]![1]).toMatchObject({
      sections_imported: EARLY_RECEIPT.sections_imported + SEGMENT_RECEIPT.sections_imported,
      shot_count: EARLY_RECEIPT.shot_count + SEGMENT_RECEIPT.shot_count,
    });
  });

  /**
   * #576 D13. The import used to run in the instance that had already closed, so
   * its error went nowhere and the reopened wizard showed the same segment as if
   * nothing had happened. It now fails where the user is looking: on that
   * segment's manual-placing screen, with the segment still queued and nothing
   * restarted behind it.
   */
  it('shows a failed segment import on the reopened manual-placing screen', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'import_sections') {
        throw { code: 'FAILED', message: 'Segment refused (test)' };
      }
      return null;
    });
    const first = renderWizard(
      savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 1, stationIndex: 0 } }),
      { onComplete: vi.fn(), autoStartManualPlacement: true, selected: [0, 1] },
    );
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    // The drop, as MapCanvas confirms it: placement mode ends, then the callback.
    await act(async () => {
      useUIStore.getState().confirmPlacement()!.onComplete!({
        position: { x: 1, y: 2, z: 0 },
        snappedToStation: null,
      });
    });
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('dmp-import-new'));
    first.unmount();

    const onComplete = vi.fn();
    renderReopenedWizard({ onComplete });

    await waitFor(() => expect(screen.getByTestId('wizard-error')).toHaveTextContent('Segment refused (test)'));
    expect(screen.getByTestId('wizard-manual-placing')).toBeVisible();
    expect(screen.getByTestId('wizard-manual-placing')).toHaveTextContent('B');
    expect(screen.getByTestId('cancel-button')).toBeEnabled();
    // Not restarted behind the error, and not completed.
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    expect(useUIStore.getState().placementMode).toBeNull();
    expect(onComplete).not.toHaveBeenCalled();
    expect(invokeIpcMock.mock.calls.filter((c) => c[0] === 'import_sections')).toHaveLength(1);
  });

  it('completes with the whole run aggregated when reopened after placement', async () => {
    const onComplete = vi.fn();

    // The state the reopened wizard gets after the last segment landed: queue
    // drained, the placed section already imported (so the late import has
    // nothing to send), and both receipts carried over.
    renderWizard(
      savedState({
        segmentQueue: [],
        directPlacements: [
          {
            anchorSectionIndex: 0,
            anchorStationIndex: 0,
            placement: { type: 'manual', align_station_index: 0, offset: { x: 0, y: 0, z: 0 } },
          },
        ],
        earlyImported: [[0, 101]],
        importedReceipts: [EARLY_RECEIPT, SEGMENT_RECEIPT],
      }),
      { onComplete, selected: [0] },
    );

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    const delivered = onComplete.mock.calls[0]![1] as ImportReceipt | undefined;
    expect(delivered).toBeDefined();
    expect(delivered).toMatchObject({
      source_format: 'mNemo DMP',
      file_name: 'dive.dmp',
      sections_found: 2,
      sections_imported: 2,
      shot_count: 7,
    });
    // Nothing left to import, so the aggregate is exactly what was carried.
    expect(invokeIpcMock).not.toHaveBeenCalledWith(
      'import_sections',
      expect.anything(),
      expect.anything(),
    );
  });

  /**
   * The fourth completion site, the 'done' phase, was the one that passed no
   * receipt at all. It is reached with one already banked - a file whose only
   * selected section IS the zero datum, or a selection with nothing left to
   * suggest - so that import went silent too, without any manual placement.
   */
  it('completes with a receipt when the zero datum is the whole import', async () => {
    const onComplete = vi.fn();

    renderWizard(undefined, {
      onComplete,
      selected: [0],
      zeroDatumConfig: zeroDatumOnly(),
    });

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).toContain('import_sections');
    expect(onComplete.mock.calls[0]![1]).toMatchObject({
      source_format: 'mNemo DMP',
      sections_imported: SEGMENT_RECEIPT.sections_imported,
      shot_count: SEGMENT_RECEIPT.shot_count,
    });
  });
});

/**
 * #576 B13: what the parser could not read (`DmpFile.warnings`) leads the
 * receipt. The wizard hands it to `import_sections` (`sourceWarnings`) with the
 * run's FIRST import only - a run's receipts are concatenated at completion, so
 * sending it on every call would repeat it once per batch.
 */
describe('DMP wizard: the file parse warnings ride the first import', () => {
  const PARSE_WARNING =
    'Section "B" (#2 in the file) has no end-of-section marker, so the download may have been cut short: 2 shots were read, the rest of the file was not.';
  const fileWithWarnings = { ...dmpFile, warnings: [PARSE_WARNING] } as DmpFile;
  const importArgs = () =>
    invokeIpcMock.mock.calls
      .filter((c) => c[0] === 'import_sections')
      .map((c) => c[1] as Record<string, unknown>);

  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'import_sections') {
        return { success: true, section_ids: [7], import_receipt: SEGMENT_RECEIPT, warnings: [] };
      }
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null });
  });

  it('sends them with the first import of the run', async () => {
    const onComplete = vi.fn();
    renderWizard(undefined, {
      onComplete,
      selected: [0],
      zeroDatumConfig: zeroDatumOnly(),
      file: fileWithWarnings,
    });

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(importArgs().map((a) => a.sourceWarnings)).toEqual([[PARSE_WARNING]]);
  });

  it('does not send them again once the run has banked a receipt', async () => {
    const first = renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedReceipts: [EARLY_RECEIPT],
      }),
      { onComplete: vi.fn(), autoStartManualPlacement: true, selected: [0, 1], file: fileWithWarnings },
    );
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    await act(async () => {
      useUIStore.getState().placementMode!.onComplete!({
        position: { x: 1, y: 2, z: 0 },
        snappedToStation: null,
      });
    });
    // The segment is imported by the wizard the drop reopens (#576 D13).
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('dmp-import-new'));
    first.unmount();
    renderReopenedWizard();

    await waitFor(() => expect(importArgs()).toHaveLength(1));
    expect(importArgs()[0]!.sourceWarnings ?? null).toBeNull();
  });
});
