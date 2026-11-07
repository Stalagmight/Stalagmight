/**
 * #576 NF-DMPS-2: a section the LATE import landed must not be sent again by a
 * second late import.
 *
 * The run records which project section each DMP section became
 * (`earlyImported`), and every later batch skips what is recorded there. The
 * late import did not record its sections. The path that reaches a second
 * late import, walked here end to end:
 *
 * 1. The queue drains with A placed onto an existing section (a direct
 *    placement) and B connected only to C, which the user discarded.
 * 2. Late import #1 lands A. B is still unreachable (its one connection leads
 *    to a discarded section), so the wizard re-enters manual placing with B.
 * 3. The user drops B on the map; the reopened wizard imports it (D13) and,
 *    with the queue empty again, runs late import #2 - whose batch is rebuilt
 *    from every placement so far, A's included.
 *
 * All data is synthetic (dmpWizardHarness).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { failOnRefreshErrors } from '../../../test/refreshGuard';
import { act, waitFor } from '@testing-library/react';

// SectionPreview and the map panel pull maplibre-gl (WebGL) at module load,
// which jsdom cannot execute. Nothing here asserts on anything drawn.
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

import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { receipt, renderReopenedWizard, renderWizard, savedState } from './dmpWizardHarness';

/** Every `import_sections` batch, as the section names it carried. */
let batches: string[][] = [];

beforeEach(() => {
  batches = [];
  let nextSectionId = 100;
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string, args?: { sections?: Array<{ name: string }> }) => {
    if (cmd === 'import_sections') {
      const names = (args?.sections ?? []).map((s) => s.name);
      batches.push(names);
      return {
        success: true,
        section_ids: names.map(() => nextSectionId++),
        import_receipt: receipt({ sections_imported: names.length }),
        warnings: [],
      };
    }
    return null;
  });
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null });
  useProjectStore.setState({ project: null });
});

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

describe('DMP wizard: a second late import (#576 NF-DMPS-2)', () => {
  it('does not send a section the first late import already landed', async () => {
    // Step 1: the queue has drained. A sits on existing section 3; B's only
    // connection is to C, which was discarded.
    const first = renderWizard(
      savedState({
        directPlacements: [
          {
            anchorSectionIndex: 0,
            anchorStationIndex: 0,
            placement: { type: 'from_existing', import_station: 0, existing_station: { section_id: 3, station_index: 0 } },
          },
        ],
        connections: [
          { importSectionIndex: 1, importStationIndex: 0, targetType: 'importing', targetImportIndex: 2, targetImportStationIndex: 0 },
        ],
        discardedIndices: [2],
        segmentQueue: [],
      }),
      { selected: [0, 1, 2] },
    );

    // Step 2: late import #1 lands A, and B comes back for manual placing,
    // handed straight to the map.
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    expect(batches).toEqual([['A']]);

    // Step 3: B is dropped on the map; the reopened wizard imports it and runs
    // the late import again.
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
    await waitFor(() => expect(onComplete).toHaveBeenCalled());

    // A (late import #1) and B (the dropped segment) each reached the backend
    // once. (What late import #2 does with C is pinned by the next case.)
    const sent = batches.flat();
    expect(sent.filter((name) => name === 'B')).toHaveLength(1);
    expect(sent.filter((name) => name === 'A')).toHaveLength(1);
  });

  /**
   * #576 DMP-DISCARD / NF-W5L-1, decided: "A discarded section is never imported by a
   * later placement in the same run". C was discarded, but placing B (whose one
   * connection leads to C) made C reachable again, and late import #2 - rebuilt from
   * every placement so far - imported it.
   */
  it('never imports the section the user discarded, even once a placement reaches it', async () => {
    const first = renderWizard(
      savedState({
        directPlacements: [
          {
            anchorSectionIndex: 0,
            anchorStationIndex: 0,
            placement: { type: 'from_existing', import_station: 0, existing_station: { section_id: 3, station_index: 0 } },
          },
        ],
        connections: [
          { importSectionIndex: 1, importStationIndex: 0, targetType: 'importing', targetImportIndex: 2, targetImportStationIndex: 0 },
        ],
        discardedIndices: [2],
        segmentQueue: [],
      }),
      { selected: [0, 1, 2] },
    );
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
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
    await waitFor(() => expect(onComplete).toHaveBeenCalled());

    console.log('[DMP-DISCARD] import_sections batches:', JSON.stringify(batches));
    expect(batches.flat()).not.toContain('C');
  });
});
