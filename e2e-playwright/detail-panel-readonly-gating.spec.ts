/**
 * E2E: the DetailPanel action-footer buttons are gated when editing is blocked (#260).
 *
 * The footer verbs (station Edit, shot Mark-as-<kind> / Delete, line Edit line)
 * all sit behind `useEditingBlocked()`. This spec asserts that gating through the
 * stable `data-testid`s rather than by scraping label text, so a copy change can't
 * silently turn the read-only assertion into a no-op.
 *
 * Two independent read-only sources are covered, because they compose differently:
 * - a remote SpeleoDB project open WITHOUT the write lock (`speleoStore.link`), and
 * - the closure-adjusted geometry view (`uiStore.geometryMode`), which has NO server
 *   backstop at all - read-only there rests entirely on this UI gating.
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */

import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapIdle,
  type LineSeed,
} from './helpers/app';

const lineSeeds: LineSeed[] = [
  {
    name: 'Main Tunnel',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
    ],
  },
];

/** The four footer verbs this spec gates, by the testids added in DetailPanel. */
const STATION_EDIT = 'station-edit-button';
const SHOT_KIND_CYCLE = 'shot-kind-cycle-button';
const SHOT_DELETE = 'shot-delete-button';
const LINE_EDIT = 'line-edit-button';

interface SeededIds {
  lineId: number;
  shotId: number;
}

async function seedIds(page: Page): Promise<SeededIds> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            project: { lines: Array<{ id: number; shots: Array<{ id: number }> }> } | null;
          };
        };
      };
    }).__E2E_TEST__;
    const line = stores?.projectStore.getState().project?.lines[0];
    return { lineId: line?.id ?? -1, shotId: line?.shots[0]?.id ?? -1 };
  });
}

async function select(page: Page, selection: Record<string, unknown>): Promise<void> {
  await page.evaluate((sel) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { setSelection: (s: unknown) => void } } };
    }).__E2E_TEST__;
    stores?.uiStore.getState().setSelection(sel);
  }, selection);
}

/**
 * Toggle the SpeleoDB read-only source.
 *
 * READ-ONLY is a remote link opened WITHOUT the write lock: `setLinkOnOpen`
 * seeds `editState: 'read-only'`, which `selectSpeleoReadOnly` reads as blocked.
 *
 * EDITABLE is the confirmed-LOCAL state, `syncProjectLink(false)` - NOT
 * `clearLink()`. The gate fails closed (#270): `clearLink` drops back to
 * `projectLinkKind: 'unknown'`, and everything that is not a confirmed-local
 * project or a held write lock reads as read-only. Clearing the link therefore
 * does not restore an editable baseline; only the same call `loadProject` makes
 * on a local open does.
 */
async function setSpeleoReadOnly(page: Page, readOnly: boolean): Promise<void> {
  await page.evaluate((on) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        speleoStore: {
          getState: () => {
            setLinkOnOpen: (a: { permission: string | null }) => void;
            syncProjectLink: (isRemote: boolean, forPath?: string | null) => void;
          };
        };
      };
    }).__E2E_TEST__;
    const speleo = stores?.speleoStore.getState();
    if (on) speleo?.setLinkOnOpen({ permission: 'READ_AND_WRITE' });
    else speleo?.syncProjectLink(false);
  }, readOnly);
}

/**
 * Give the project one closure the adjustment solve would actually move.
 *
 * Without this the closure-adjusted view cannot be entered at all: MapCanvas
 * reverts it to `measured` on any project with no ELIGIBLE closure (the adjusted
 * geometry would be identical to measured), so a spec that just flips the mode
 * silently asserts nothing. `Batch` is one of the two eligible strategies -
 * see `hasEligibleClosures` in src/utils/closures.ts.
 */
async function seedEligibleClosure(page: Page, lineId: number): Promise<void> {
  await page.evaluate((secId) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { project: { loop_closures: unknown[] } | null };
          setState: (s: unknown) => void;
        };
      };
    }).__E2E_TEST__;
    const project = stores?.projectStore.getState().project;
    if (!project) return;
    stores?.projectStore.setState({
      project: {
        ...project,
        loop_closures: [
          ...project.loop_closures,
          {
            id: 1,
            station_a: { line_id: secId, station_index: 0 },
            station_b: { line_id: secId, station_index: 2 },
            path_length: 22,
            misclosure_distance: 0.3,
            error_percentage: 1.4,
            horizontal_misclosure: 0.25,
            vertical_misclosure: 0.15,
            strategy: 'Batch',
            is_aligned: false,
          },
        ],
      },
    });
  }, lineId);
}

async function readGeometryMode(page: Page): Promise<string | undefined> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { geometryMode: string } } };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().geometryMode;
  });
}

async function setClosureAdjusted(page: Page, on: boolean): Promise<void> {
  await page.evaluate((adjusted) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: {
          getState: () => { setGeometryMode: (m: string, source?: string) => void };
        };
      };
    }).__E2E_TEST__;
    stores?.uiStore.getState().setGeometryMode(adjusted ? 'closureAdjusted' : 'measured', 'e2e');
  }, on);
}

test.describe('DetailPanel footer verbs are gated when editing is blocked', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Read-only Gating');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
  });

  test('station Edit is enabled while editable and disabled read-only', async ({ page }) => {
    const { lineId } = await seedIds(page);
    await select(page, {
      type: 'station',
      stationRef: { line_id: lineId, station_index: 0 },
    });

    const edit = page.getByTestId(STATION_EDIT);
    await expect(edit).toBeEnabled();

    await setSpeleoReadOnly(page, true);
    await expect(edit).toBeDisabled();

    await setSpeleoReadOnly(page, false);
    await expect(edit).toBeEnabled();
  });

  test('shot Mark-as-kind and Delete are disabled read-only', async ({ page }) => {
    const { lineId, shotId } = await seedIds(page);
    await select(page, { type: 'shot', lineId, shotId });

    const markKind = page.getByTestId(SHOT_KIND_CYCLE);
    const del = page.getByTestId(SHOT_DELETE);
    await expect(markKind).toBeEnabled();
    await expect(del).toBeEnabled();

    await setSpeleoReadOnly(page, true);
    await expect(markKind).toBeDisabled();
    await expect(del).toBeDisabled();

    await setSpeleoReadOnly(page, false);
    await expect(markKind).toBeEnabled();
    await expect(del).toBeEnabled();
  });

  test('line Edit-line is disabled read-only', async ({ page }) => {
    const { lineId } = await seedIds(page);
    await select(page, { type: 'line', lineId });

    const editLine = page.getByTestId(LINE_EDIT);
    await expect(editLine).toBeEnabled();

    await setSpeleoReadOnly(page, true);
    await expect(editLine).toBeDisabled();
  });

  test('the closure-adjusted view disables every footer verb too', async ({ page }) => {
    const { lineId, shotId } = await seedIds(page);
    // The mode only STICKS on a project with an eligible closure; without one
    // MapCanvas reverts it and this test would assert the measured view instead.
    await seedEligibleClosure(page, lineId);
    await setClosureAdjusted(page, true);
    await expect.poll(() => readGeometryMode(page)).toBe('closureAdjusted');

    await select(page, {
      type: 'station',
      stationRef: { line_id: lineId, station_index: 0 },
    });
    await expect(page.getByTestId(STATION_EDIT)).toBeDisabled();

    await select(page, { type: 'shot', lineId, shotId });
    await expect(page.getByTestId(SHOT_KIND_CYCLE)).toBeDisabled();
    await expect(page.getByTestId(SHOT_DELETE)).toBeDisabled();

    await select(page, { type: 'line', lineId });
    await expect(page.getByTestId(LINE_EDIT)).toBeDisabled();

    // Still the adjusted view at the end: a mid-test revert to `measured` would
    // leave the SpeleoDB term as the only thing gating these buttons, quietly
    // turning this case into a duplicate of the ones above.
    expect(await readGeometryMode(page)).toBe('closureAdjusted');
  });
});
