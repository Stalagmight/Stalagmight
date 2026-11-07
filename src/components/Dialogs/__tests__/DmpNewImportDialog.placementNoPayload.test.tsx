/**
 * #576 NF-W9FE-3: a map-placement handoff that cannot build its payload closed the
 * wizard.
 *
 * `handleStartSegmentPlacement` hands a segment to the map: it builds the overlay
 * payload, starts placement mode and closes the wizard (a drop reopens it). With no
 * payload - the anchor section is not in the file - it reopened the wizard
 * (`openDialog`) and then went on to the unconditional `closeDialog()`. Reached from
 * the hidden auto-start handoff, the run vanished: no wizard, no placement, no
 * late import. It now leaves the wizard where it is, so the settled handoff shows
 * it on the segment's manual-placing screen (the screen a declined handoff ends
 * on), where the user can skip the segment or go back.
 *
 * All data is synthetic (dmpWizardHarness).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { failOnRefreshErrors } from '../../../test/refreshGuard';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import en from '../../../i18n/locales/en.json';

// SectionPreview and the map panel pull maplibre-gl (WebGL) at module load,
// which jsdom cannot execute. Nothing here asserts on anything drawn.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
// jsdom does not implement scrollIntoView; the portal Select calls it on open.
Element.prototype.scrollIntoView = vi.fn();
vi.mock('../WizardMapPanel', () => ({
  WizardMapPanel: () => null,
  MAP_PANEL_DEFAULT_WIDTH: 400,
  MAP_PANEL_MAX_WIDTH: 800,
}));
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import { emptyBinaryRefresh, invokeIpcMock } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');
// [#576 NF-W6A-5] The refresh every edit/import ends with reads bytes; a test that
// gives none answers "no project open" rather than making the decoder throw.
emptyBinaryRefresh();

import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { renderWizard, savedState } from './dmpWizardHarness';

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  useProjectStore.setState({ project: null });
});

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

describe('DMP wizard: a handoff with no placement payload (#576 NF-W9FE-3)', () => {
  it('leaves the wizard open on the manual-placing screen instead of reopening and closing it', async () => {
    // The wizard as a drop reopened it: the active dialog, its data in dialogData,
    // the next segment to hand off at once. Its anchor names a section the file
    // does not have, so no overlay payload can be built.
    const dialogData = { filePath: '/tmp/dive.dmp' };
    useUIStore.setState({ activeDialog: 'dmp-import-new', dialogData, placementMode: null });
    const openDialog = vi.spyOn(useUIStore.getState(), 'openDialog');
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    renderWizard(
      savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }),
      { selected: [0, 1], autoStartManualPlacement: true },
    );

    // The handoff was attempted and settled: the manual-placing screen shows.
    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toBeVisible());

    // Soft, so a regression reports all of it: reopened, then closed.
    const ui = useUIStore.getState();
    expect.soft(openDialog).not.toHaveBeenCalled();
    expect.soft(ui.activeDialog).toBe('dmp-import-new');
    expect.soft(ui.dialogData).toBe(dialogData);
    expect.soft(ui.placementMode).toBeNull();
    expect.soft(warn).toHaveBeenCalledWith('[dmpImport] no placement payload for anchor section', 9);
    openDialog.mockRestore();
    warn.mockRestore();
  });
});

/**
 * #576 NF-W9FE-3-D1: the manual-placing screen says why the segment cannot go on the
 * map. The one condition that leaves no placement payload is an anchor naming a
 * section the file does not have (`anchorSectionIn`, shared with the payload
 * builder); the line shows exactly then, and goes away when the anchor moves to a
 * section the file has.
 */
describe('DMP wizard: the manual-placing screen says why a segment cannot be placed on the map (#576 NF-W9FE-3-D1)', () => {
  const REASON = 'wizard-cannot-place-on-map';
  const KEY = 'dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMap';

  beforeEach(() => {
    useUIStore.setState({ activeDialog: 'dmp-import-new', dialogData: { filePath: '/tmp/dive.dmp' }, placementMode: null });
  });

  it('states the reason when the anchor names a section the file does not have', async () => {
    renderWizard(savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1],
    });

    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toBeVisible());
    expect(screen.queryByTestId(REASON)).toHaveTextContent(KEY);
  });

  it('after the auto-start handoff found nothing to place, the settled screen states it', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    renderWizard(savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1],
      autoStartManualPlacement: true,
    });

    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toBeVisible());
    expect(screen.queryByTestId(REASON)).toHaveTextContent(KEY);
    warn.mockRestore();
  });

  it('is not shown when the anchor section is in the file', async () => {
    renderWizard(savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 1, stationIndex: 0 } }), {
      selected: [0, 1],
    });

    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toBeVisible());
    expect(screen.queryByTestId(REASON)).toBeNull();
  });

  it('goes away once the anchor moves to a section the file has', async () => {
    renderWizard(savedState({ segmentQueue: [[1, 2]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1, 2],
    });
    await waitFor(() => expect(screen.queryByTestId(REASON)).toBeVisible());

    await userEvent.click(screen.getByTestId('manual-import-section-select'));
    await userEvent.click(screen.getByTestId('manual-import-section-select-option-2'));

    await waitFor(() => expect(screen.queryByTestId(REASON)).toBeNull());
  });

  it('reads as one plain line with the way on (en.json)', () => {
    expect(en.dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMap).toBe(
      'This segment cannot be placed on the map: its anchor section is not in the DMP file. ' +
        'Click a station in the preview to set another anchor, or choose Do not place.',
    );
  });
});

/**
 * #576 RV10b-2: while the cannot-place line shows, the screen offered "Place on
 * map" anyway - a primary button whose click only logged a console warning. It is
 * now disabled, with that line as its accessible description, and enabled again
 * once the anchor names a section the file has.
 *
 * #576 RV10b-6: the line tells the user to click a station in the preview, but
 * when none of the segment's sections is in the file the preview draws nothing.
 * That sub-case drops the preview clause and keeps the Do not place way out.
 */
describe('DMP wizard: Place on map while the segment cannot go on the map (#576 RV10b-2, RV10b-6)', () => {
  const REASON = 'wizard-cannot-place-on-map';
  const LONG = 'dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMap';
  const SHORT = 'dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMapNoPreview';
  const PLACE_ON_MAP = 'segment-place-on-map-button';

  beforeEach(() => {
    useUIStore.setState({ activeDialog: 'dmp-import-new', dialogData: { filePath: '/tmp/dive.dmp' }, placementMode: null });
  });

  const chooseManualPlacement = async () => {
    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toBeVisible());
    await userEvent.click(screen.getByTestId('segment-mode-manual-input'));
    return screen.getByTestId(PLACE_ON_MAP);
  };

  it('disables Place on map and names the line as its reason', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    renderWizard(savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1],
    });
    const button = await chooseManualPlacement();
    const reason = screen.getByTestId(REASON);

    expect.soft(button).toBeDisabled();
    expect.soft(reason.id).not.toBe('');
    expect.soft(button).toHaveAttribute('aria-describedby', reason.id);
    expect.soft(button).toHaveAccessibleDescription(LONG);

    // A click on the disabled button reaches nothing: no placement, the wizard stays.
    await userEvent.click(button);
    expect.soft(useUIStore.getState().placementMode).toBeNull();
    expect.soft(useUIStore.getState().activeDialog).toBe('dmp-import-new');
    expect.soft(warn).not.toHaveBeenCalled();
    warn.mockRestore();
  });

  it('enables Place on map again once the anchor moves to a section the file has', async () => {
    renderWizard(savedState({ segmentQueue: [[1, 2]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1, 2],
    });
    const button = await chooseManualPlacement();
    expect(button).toBeDisabled();

    await userEvent.click(screen.getByTestId('manual-import-section-select'));
    await userEvent.click(screen.getByTestId('manual-import-section-select-option-2'));

    await waitFor(() => expect(screen.queryByTestId(REASON)).toBeNull());
    expect.soft(screen.getByTestId(PLACE_ON_MAP)).toBeEnabled();
    expect.soft(screen.getByTestId(PLACE_ON_MAP)).not.toHaveAttribute('aria-describedby');
  });

  it('keeps Place on map enabled, with no description, when the anchor section is in the file', async () => {
    renderWizard(savedState({ segmentQueue: [[1]], segmentAnchor: { sectionIndex: 1, stationIndex: 0 } }), {
      selected: [0, 1],
    });
    const button = await chooseManualPlacement();
    expect.soft(button).toBeEnabled();
    expect.soft(button).not.toHaveAttribute('aria-describedby');
  });

  it('points at the preview while it draws one of the segment sections', async () => {
    renderWizard(savedState({ segmentQueue: [[1, 9]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1],
    });
    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toBeVisible());
    expect(screen.getByTestId(REASON)).toHaveTextContent(LONG);
  });

  it('leaves the preview out when none of the segment sections is in the file', async () => {
    renderWizard(savedState({ segmentQueue: [[9]], segmentAnchor: { sectionIndex: 9, stationIndex: 0 } }), {
      selected: [0, 1],
    });
    const button = await chooseManualPlacement();
    expect.soft(screen.getByTestId(REASON)).toHaveTextContent(SHORT);
    expect.soft(button).toBeDisabled();
    expect.soft(button).toHaveAccessibleDescription(SHORT);
  });

  it('the short line keeps the Do not place way out and does not mention the preview (en.json)', () => {
    const line: string = en.dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMapNoPreview;
    expect.soft(line).toBe(
      'This segment cannot be placed on the map: its anchor section is not in the DMP file. Choose Do not place.',
    );
    expect.soft(line).not.toMatch(/preview/i);
  });
});
