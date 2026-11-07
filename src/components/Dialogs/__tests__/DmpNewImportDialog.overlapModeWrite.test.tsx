/**
 * #576 RV6b-1: the overlap mode the user picks LAST is the one saved.
 *
 * The wizard persisted a change by handing `updateConfig` the whole config
 * with the new mode, and `updateConfig` sends only what differs from the
 * store's config. That config is re-read only once a save has landed, so
 * switching the mode and switching it straight back compared the second choice
 * against the ORIGINAL mode, found nothing to send, and the first choice was
 * what the next launch read. The wizard now sends the one setting itself
 * (`patchConfig`), and the config writes run in order, so the last one wins.
 *
 * The oracle is a fake backend config that `patch_config` really mutates. All
 * data is synthetic (dmpWizardHarness).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { failOnRefreshErrors } from '../../../test/refreshGuard';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

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

import type { CrossSectionSuggestion } from '../../../types/ipc/CrossSectionSuggestion';
import type { Config } from '../../../types/config';
import type { ConfigPatch } from '../../../types/ipc/ConfigPatch';
import { defaultConfig } from '../../../types/ipc/defaultConfig';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { renderWizard } from './dmpWizardHarness';

// The dropdown scrolls its highlighted option into view; jsdom has no layout.
Element.prototype.scrollIntoView = vi.fn();

// B's station 0 onto A's station 1 with one shot of overlap, so the overlap
// mode picker is offered (it is only shown when shots overlap).
const SUGGESTION: CrossSectionSuggestion = {
  existingStationIndex: 1,
  importStationIndex: 0,
  score: 1.4,
  matchedShots: 1,
  existingMatchedShots: 1,
  preferredMergeOffset: 0,
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

/** The backend's config, as `patch_config` leaves it. */
let backendConfig: Config;
/** Every `patch_config` the wizard sent, in order. */
let patches: ConfigPatch[][] = [];
/** Holds the FIRST save open, so the second choice is made before it lands. */
let releaseFirstSave: (() => void) | null = null;

function applyPatch(config: Config, patch: ConfigPatch): Config {
  const next = structuredClone(config) as unknown as Record<string, Record<string, unknown>>;
  const [group, key] = patch.path as [string, string];
  next[group]![key] = patch.value;
  return next as unknown as Config;
}

beforeEach(() => {
  backendConfig = structuredClone(defaultConfig);
  backendConfig.import.overlap_mode = 'import_data';
  patches = [];
  releaseFirstSave = null;
  let firstSave: Promise<void> | null = new Promise<void>((resolve) => {
    releaseFirstSave = resolve;
  });
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string, args?: { patches?: ConfigPatch[] }) => {
    if (cmd === 'suggest_all_import_placements') return [SUGGESTION];
    if (cmd === 'patch_config') {
      const sent = args?.patches ?? [];
      patches.push(sent);
      if (firstSave) {
        const gate = firstSave;
        firstSave = null;
        await gate;
      }
      for (const patch of sent) backendConfig = applyPatch(backendConfig, patch);
      return { keychain_residue: null, persistent_residue: null };
    }
    if (cmd === 'get_config') return structuredClone(backendConfig);
    return null;
  });
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null });
  useProjectStore.setState({ project: null, config: structuredClone(backendConfig) });
});

afterEach(async () => {
  releaseFirstSave?.();
  await act(async () => {});
});

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

async function chooseOverlapMode(value: string): Promise<void> {
  await userEvent.click(screen.getByTestId('overlap-mode-select'));
  await userEvent.click(await screen.findByTestId(`overlap-mode-select-option-${value}`));
}

describe('DMP wizard: the overlap mode chosen last is the one saved (#576 RV6b-1)', () => {
  it('switching the mode and straight back saves the second choice', async () => {
    renderWizard(undefined, { selected: [0, 1] });
    await screen.findByTestId('overlap-mode-select');

    // Interpolate, then back to import data while the first save is still open.
    await chooseOverlapMode('interpolate');
    await chooseOverlapMode('import_data');

    await act(async () => {
      releaseFirstSave?.();
    });
    await waitFor(() => expect(useProjectStore.getState().config?.import.overlap_mode).toBe('import_data'));

    expect(
      patches.map((sent) => sent.map((p) => `${p.path.join('.')}=${String(p.value)}`)),
      'the config writes the wizard sent',
    ).toEqual([['import.overlap_mode=interpolate'], ['import.overlap_mode=import_data']]);
    expect(backendConfig.import.overlap_mode, 'what the next launch reads').toBe('import_data');
  });
});
