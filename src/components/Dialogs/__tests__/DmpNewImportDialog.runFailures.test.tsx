/**
 * #576 W4: where the DMP wizard puts each failure of its run, and what it offers
 * next. The wizard used to keep ONE error string for every step, written by
 * eleven call sites and rendered by whichever panel happened to be on screen -
 * so some failures had no panel at all (a blank window) and others landed
 * behind a dialog. Each case here is a failure the user could not see, or could
 * not act on, before.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { failOnRefreshErrors } from '../../../test/refreshGuard';
import { screen, fireEvent, waitFor, act } from '@testing-library/react';

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
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys('keyAndOptions'));
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

import { receipt, renderWizard, savedState, zeroDatumOn0 } from './dmpWizardHarness';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';

/** What `import_sections` answers with; `null` = success with fresh ids. */
let importAnswer: Record<string, unknown> | Error | null = null;
/** What `suggest_all_import_placements` does on each call, in order. */
let suggestAnswers: Array<unknown[] | Error> = [];
let ipcOrder: string[] = [];
let nextSectionId = 7;

beforeEach(() => {
  importAnswer = null;
  suggestAnswers = [];
  ipcOrder = [];
  nextSectionId = 7;
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    ipcOrder.push(cmd);
    if (cmd === 'import_sections') {
      if (importAnswer instanceof Error) throw importAnswer;
      // A refusal is the backend's CommandError rejection (#576 C4).
      if (importAnswer) throw importAnswer;
      const configs = (args?.sections as unknown[] | undefined) ?? [];
      return { success: true, section_ids: configs.map(() => nextSectionId++), import_receipt: receipt(), warnings: [] };
    }
    if (cmd === 'suggest_all_import_placements') {
      const answer = suggestAnswers.shift() ?? [];
      if (answer instanceof Error) throw answer;
      return answer;
    }
    return null;
  });
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, importContext: null, toasts: [] });
  useProjectStore.setState({ project: null });
});

afterEach(async () => {
  await act(async () => {});
});

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

/**
 * NF-AN4-N1. Phase `init` had no error view: the zero-datum import failing left
 * the wizard's body empty - a blank window with Cancel and Back and no word of
 * what went wrong.
 */
describe('DMP wizard: a failure while the wizard starts is shown (#576 NF-AN4-N1)', () => {
  it('shows the backend refusal of the zero-datum import', async () => {
    importAnswer = { code: 'FAILED', message: 'Anchor refused (test)' };
    renderWizard(undefined, { selected: [0, 1], zeroDatumConfig: zeroDatumOn0() });

    await waitFor(() => expect(screen.getByTestId('wizard-error')).toHaveTextContent('Anchor refused (test)'));
    expect(screen.getByTestId('wizard-error')).toBeVisible();
    // The way out is open: nothing is in flight.
    expect(screen.getByTestId('cancel-button')).toBeEnabled();
    expect(screen.getByTestId('back-button')).toBeEnabled();
  });

  it('shows a thrown zero-datum import too', async () => {
    importAnswer = new Error('disk unplugged (test)');
    renderWizard(undefined, { selected: [0, 1], zeroDatumConfig: zeroDatumOn0() });

    await waitFor(() => expect(screen.getByTestId('wizard-error')).toHaveTextContent('disk unplugged (test)'));
  });
});

/**
 * NF-AN4-N3. "Skip segment" set the placement mode to 'origin', which the radio
 * group never renders and the Place gate rejects - so the next segment came up
 * with no option selected. Every other route into a segment selects one.
 */
describe('DMP wizard: Skip segment leaves a placement option selected (#576 NF-AN4-N3)', () => {
  it('selects manual placement on the segment it moves to', async () => {
    renderWizard(
      savedState({ segmentQueue: [[0], [1]], segmentAnchor: { sectionIndex: 0, stationIndex: 0 } }),
      { selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('segment-skip-button'));

    await waitFor(() => expect(screen.getByTestId('wizard-manual-placing')).toHaveTextContent('B'));
    expect(screen.getByTestId('segment-mode-manual-input')).toBeChecked();
    expect(screen.getByTestId('segment-place-on-map-button')).toBeEnabled();
  });
});

/**
 * #576 D24 (decided "Stop and offer Retry"). A failed `suggest_all_import_placements`
 * was swallowed: the wizard behaved as if nothing matched, hid itself and started
 * map placement of the first section - a broken matcher looked exactly like a
 * survey with no overlaps.
 */
describe('DMP wizard: failed placement suggestions stop and offer Retry (#576 D24)', () => {
  it('stays visible with the reason instead of starting map placement', async () => {
    suggestAnswers = [new Error('matcher crashed (test)')];
    renderWizard(undefined, { selected: [0, 1] });

    const failed = await screen.findByTestId('wizard-suggestions-failed');
    expect(failed).toHaveTextContent('dialogs.dmpNewImport.suggestionsFailed');
    expect(failed).toHaveTextContent('matcher crashed (test)');
    expect(failed).toBeVisible();
    expect(screen.getByTestId('suggestions-retry-button')).toBeEnabled();
    expect(screen.getByTestId('suggestions-place-manually-button')).toBeEnabled();
    // Nothing was started behind the user's back.
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    expect(useUIStore.getState().placementMode).toBeNull();
  });

  it('Retry asks the backend again and carries on when it answers', async () => {
    suggestAnswers = [new Error('matcher crashed (test)'), []];
    renderWizard(undefined, { selected: [0, 1] });
    await screen.findByTestId('wizard-suggestions-failed');

    fireEvent.click(screen.getByTestId('suggestions-retry-button'));

    await waitFor(() =>
      expect(ipcOrder.filter((c) => c === 'suggest_all_import_placements')).toHaveLength(2),
    );
    // Nothing matched on the retry: the ordinary no-match path, manual placement.
    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    expect(screen.queryByTestId('wizard-suggestions-failed')).not.toBeInTheDocument();
  });

  it('Place manually takes the no-match path without asking again', async () => {
    suggestAnswers = [new Error('matcher crashed (test)')];
    renderWizard(undefined, { selected: [0, 1] });
    await screen.findByTestId('wizard-suggestions-failed');

    fireEvent.click(screen.getByTestId('suggestions-place-manually-button'));

    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    expect(ipcOrder.filter((c) => c === 'suggest_all_import_placements')).toHaveLength(1);
  });

  it('a successful empty answer still goes straight to manual placement', async () => {
    suggestAnswers = [[]];
    renderWizard(undefined, { selected: [0, 1] });

    await waitFor(() => expect(useUIStore.getState().placementMode).not.toBeNull());
    expect(screen.queryByTestId('wizard-suggestions-failed')).not.toBeInTheDocument();
  });
});
