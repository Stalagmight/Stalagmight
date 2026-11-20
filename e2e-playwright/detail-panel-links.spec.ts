/**
 * E2E tests for detail panel hyperlinks and center button functionality.
 *
 * Tests the bidirectional navigation between survey elements:
 * - Shot Detail: Start/End Station links, Line link
 * - Station Detail: Previous/Next Shot links, Line link, Merged station links
 * - Line Detail: First/Last Station links (Endpoints)
 * - Center buttons for all detail types
 * - Keyboard navigation (Tab + Enter)
 * - ARIA live announcements for selection changes
 */

import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapIdle,
  type LineSeed,
} from './helpers/app';

/**
 * Test data with multiple lines, shots, and stations.
 * Includes a station that will be set up with merged_with references.
 */
const lineSeeds: LineSeed[] = [
  {
    name: 'Main Tunnel',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      { azimuth_in: 120, azimuth_out: 300, length: 8, depth_in: -6, depth_out: -8 },
    ],
  },
  {
    name: 'Side Passage',
    direction: 'out',
    shots: [
      { azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -2, depth_out: -4 },
      { azimuth_in: 200, azimuth_out: 20, length: 5, depth_in: -4, depth_out: -5 },
    ],
  },
  {
    name: 'Single Station Line',
    direction: 'in',
    shots: [], // Empty line - single station only
  },
];

/**
 * Helper to create a project with test lines.
 */
async function createProjectWithLines(page: Page): Promise<void> {
  await createProject(page, 'Detail Links Test');
  await seedProjectWithLines(page, lineSeeds);
  await waitForMapIdle(page);
}

/**
 * Helper to get line IDs from the project store.
 */
async function getLineIds(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { lines: Array<{ id: number }> } | null } };
      };
    }).__E2E_TEST__;
    return stores?.projectStore.getState().project?.lines.map(s => s.id) ?? [];
  });
}

/**
 * Helper to get the current selection state.
 */
async function getSelectionState(page: Page): Promise<{
  type: string;
  lineId?: number;
  stationRef?: { line_id: number; station_index: number };
  shotId?: number;
} | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { selection: unknown } };
      };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().selection as {
      type: string;
      lineId?: number;
      stationRef?: { line_id: number; station_index: number };
      shotId?: number;
    } | null;
  });
}

/**
 * Helper to poll for selection state changes.
 */
async function pollSelectionState(
  page: Page,
  predicate: (sel: Awaited<ReturnType<typeof getSelectionState>>) => boolean,
  timeout = 5000
): Promise<Awaited<ReturnType<typeof getSelectionState>>> {
  let result: Awaited<ReturnType<typeof getSelectionState>> = null;
  await expect.poll(
    async () => {
      const sel = await getSelectionState(page);
      if (predicate(sel)) {
        result = sel;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to select a line by clicking its row.
 */
async function selectLineById(page: Page, lineId: number): Promise<void> {
  const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
  await expect(lineRow).toBeVisible();
  await lineRow.click();
  await pollSelectionState(page, (sel) => sel?.type === 'line' && sel?.lineId === lineId);
}

/**
 * Helper to select a station by clicking on the map or via store.
 */
async function selectStation(page: Page, lineId: number, stationIndex: number): Promise<void> {
  await page.evaluate(({ secId, idx }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { setSelection: (sel: unknown) => void } };
      };
    }).__E2E_TEST__;
    if (stores) {
      stores.uiStore.getState().setSelection({
        type: 'station',
        stationRef: { line_id: secId, station_index: idx },
      });
    }
  }, { secId: lineId, idx: stationIndex });

  await pollSelectionState(
    page,
    (sel) =>
      sel?.type === 'station' &&
      sel?.stationRef?.line_id === lineId &&
      sel?.stationRef?.station_index === stationIndex
  );
}

/**
 * Helper to select a shot via store.
 */
async function selectShot(page: Page, lineId: number, shotId: number): Promise<void> {
  await page.evaluate(({ secId, sId }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { setSelection: (sel: unknown) => void } };
      };
    }).__E2E_TEST__;
    if (stores) {
      stores.uiStore.getState().setSelection({
        type: 'shot',
        lineId: secId,
        shotId: sId,
      });
    }
  }, { secId: lineId, sId: shotId });

  await pollSelectionState(
    page,
    (sel) => sel?.type === 'shot' && sel?.lineId === lineId && sel?.shotId === shotId
  );
}

/**
 * Helper to get shot IDs for a line.
 */
async function getShotIdsForLine(page: Page, lineId: number): Promise<number[]> {
  return page.evaluate((secId) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            project: { lines: Array<{ id: number; shots: Array<{ id: number }> }> } | null;
          };
        };
      };
    }).__E2E_TEST__;
    const line = stores?.projectStore.getState().project?.lines.find(s => s.id === secId);
    return line?.shots.map(shot => shot.id) ?? [];
  }, lineId);
}

/**
 * Helper to get the text content of the ARIA live region.
 */
async function getAnnouncerText(page: Page): Promise<string> {
  const announcer = page.locator('#selection-announcer');
  return (await announcer.textContent()) ?? '';
}

/**
 * Helper to set up merged station relationships in the store.
 * This is needed because seedProjectWithLines creates new stations without merge data.
 * The IPC mock's merged station setup only applies to the initial load, not dynamically created lines.
 */
async function setupMergedStations(
  page: Page,
  mainLineId: number,
  mainLastStationIndex: number,
  sideLineId: number
): Promise<void> {
  await page.evaluate(
    ({ mainSecId, mainLastIdx, sideSecId }) => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              stations: Array<{
                station_ref: { line_id: number; station_index: number };
                merged_with: Array<{ line_id: number; station_index: number }>;
              }>;
            };
            setState: (state: { stations: unknown[] }) => void;
          };
        };
      }).__E2E_TEST__;

      if (!stores) return;

      const state = stores.projectStore.getState();
      const updatedStations = state.stations.map((station) => {
        // Last station of main line merges with first station of side line
        if (
          station.station_ref.line_id === mainSecId &&
          station.station_ref.station_index === mainLastIdx
        ) {
          return {
            ...station,
            merged_with: [{ line_id: sideSecId, station_index: 0 }],
          };
        }
        // First station of side line merges back to last station of main line
        if (
          station.station_ref.line_id === sideSecId &&
          station.station_ref.station_index === 0
        ) {
          return {
            ...station,
            merged_with: [{ line_id: mainSecId, station_index: mainLastIdx }],
          };
        }
        return station;
      });

      stores.projectStore.setState({ stations: updatedStations });
    },
    { mainSecId: mainLineId, mainLastIdx: mainLastStationIndex, sideSecId: sideLineId }
  );
}

async function createDirectMerge(
  page: Page,
  stationA: { line_id: number; station_index: number },
  stationB: { line_id: number; station_index: number }
): Promise<void> {
  await page.evaluate(async ({ a, b }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { mergeStations: (left: typeof a, right: typeof b) => Promise<unknown> };
        };
      };
    }).__E2E_TEST__;
    if (!stores) return;
    await stores.projectStore.getState().mergeStations(a, b);
  }, { a: stationA, b: stationB });
}

async function createDirectClosure(
  page: Page,
  stationA: { line_id: number; station_index: number },
  stationB: { line_id: number; station_index: number }
): Promise<string> {
  const closureId = await page.evaluate(async ({ a, b }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            createLoopClosure: (left: typeof a, right: typeof b, strategy: string) => Promise<unknown>;
            project: {
              loop_closures: Array<{
                id: bigint;
                station_a: { line_id: number; station_index: number };
                station_b: { line_id: number; station_index: number };
              }>;
            } | null;
          };
        };
      };
    }).__E2E_TEST__;
    if (!stores) return null;

    const state = stores.projectStore.getState();
    await state.createLoopClosure(a, b, 'batch');

    const closures = stores.projectStore.getState().project?.loop_closures ?? [];
    const closure = closures.find((entry) => {
      const direct =
        entry.station_a.line_id === a.line_id &&
        entry.station_a.station_index === a.station_index &&
        entry.station_b.line_id === b.line_id &&
        entry.station_b.station_index === b.station_index;
      const reverse =
        entry.station_a.line_id === b.line_id &&
        entry.station_a.station_index === b.station_index &&
        entry.station_b.line_id === a.line_id &&
        entry.station_b.station_index === a.station_index;
      return direct || reverse;
    });

    return closure ? closure.id.toString() : null;
  }, { a: stationA, b: stationB });

  if (!closureId) {
    throw new Error('Failed to create closure');
  }
  return closureId;
}

test.describe('Detail Panel Links', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test.describe('Shot Detail Links', () => {
    test('clicking Start Station link changes selection to station', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Find and click the Start Station link
      // Shot at index 0 has Start Station at index 0 (position 0, 0-indexed)
      const startStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await expect(startStationLink).toBeVisible();
      await startStationLink.click();

      // Selection should change to station
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'station' && sel?.stationRef?.station_index === 0
      );
      expect(selection?.type).toBe('station');
      expect(selection?.stationRef?.line_id).toBe(mainLineId);
      expect(selection?.stationRef?.station_index).toBe(0);
    });

    test('clicking End Station link changes selection to station', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Find and click the End Station link
      // Shot at index 0 has End Station at index 1 (position 1, 0-indexed)
      const endStationLink = page.getByTestId(`station-${mainLineId}-1-link`);
      await expect(endStationLink).toBeVisible();
      await endStationLink.click();

      // Selection should change to station
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'station' && sel?.stationRef?.station_index === 1
      );
      expect(selection?.type).toBe('station');
      expect(selection?.stationRef?.line_id).toBe(mainLineId);
      expect(selection?.stationRef?.station_index).toBe(1);
    });

    test('clicking Line link changes selection and highlights in Sections Panel', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Find and click the Line link
      const lineLink = page.getByTestId(`line-${mainLineId}-link`);
      await expect(lineLink).toBeVisible();
      await lineLink.click();

      // Selection should change to line
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'line' && sel?.lineId === mainLineId
      );
      expect(selection?.type).toBe('line');
      expect(selection?.lineId).toBe(mainLineId);

      // Verify line row is highlighted in Sections Panel
      const lineRow = page.getByTestId(`line-row-${mainLineId}`);
      await expect(lineRow).toHaveAttribute('aria-selected', 'true');
    });
  });

  test.describe('Station Detail Links', () => {
    test('clicking Previous Shot link changes selection to correct shot', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);

      // Select station at index 2 (position 3) - has previous shot at index 1
      await selectStation(page, mainLineId, 2);

      // Find and click the Previous Shot link
      // Station at index 2 has Previous Shot at index 1 (position 2)
      const prevShotLink = page.getByTestId(`shot-${mainLineId}-2-link`);
      await expect(prevShotLink).toBeVisible();
      await prevShotLink.click();

      // Selection should change to the correct shot (shots[1])
      const expectedShotId = shotIds[1]!;
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'shot' && sel?.shotId === expectedShotId
      );
      expect(selection?.type).toBe('shot');
      expect(selection?.lineId).toBe(mainLineId);
      expect(selection?.shotId).toBe(expectedShotId);
    });

    test('clicking Next Shot link changes selection to correct shot', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);

      // Select station at index 1 (position 2) - has next shot at index 1
      await selectStation(page, mainLineId, 1);

      // Find and click the Next Shot link
      // Station at index 1 has Next Shot at index 1 (position 2)
      const nextShotLink = page.getByTestId(`shot-${mainLineId}-2-link`);
      await expect(nextShotLink).toBeVisible();
      await nextShotLink.click();

      // Selection should change to the correct shot (shots[1])
      const expectedShotId = shotIds[1]!;
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'shot' && sel?.shotId === expectedShotId
      );
      expect(selection?.type).toBe('shot');
      expect(selection?.lineId).toBe(mainLineId);
      expect(selection?.shotId).toBe(expectedShotId);
    });

    test('clicking Line link from Station changes selection to line', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select station at index 1
      await selectStation(page, mainLineId, 1);

      // Station detail should be visible
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Find and click the Line link
      const lineLink = page.getByTestId(`line-${mainLineId}-link`);
      await expect(lineLink).toBeVisible();
      await lineLink.click();

      // Selection should change to line
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'line' && sel?.lineId === mainLineId
      );
      expect(selection?.type).toBe('line');
      expect(selection?.lineId).toBe(mainLineId);
    });

    test('first station does not show Previous Shot link', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select station at index 0 (first station)
      await selectStation(page, mainLineId, 0);

      // Station detail should be visible
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Previous Shot label should not be visible
      const prevShotLabel = stationDetail.locator('text=Previous Shot');
      await expect(prevShotLabel).toBeHidden();

      // Next Shot should be visible since there are shots
      const nextShotLabel = stationDetail.locator('text=Next Shot');
      await expect(nextShotLabel).toBeVisible();
    });

    test('last station does not show Next Shot link', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Main line has 3 shots, so 4 stations (indices 0-3)
      // Last station is at index 3
      await selectStation(page, mainLineId, 3);

      // Station detail should be visible
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Next Shot label should not be visible
      const nextShotLabel = stationDetail.locator('text=Next Shot');
      await expect(nextShotLabel).toBeHidden();

      // Previous Shot should be visible
      const prevShotLabel = stationDetail.locator('text=Previous Shot');
      await expect(prevShotLabel).toBeVisible();
    });

    test('clicking merged station link changes selection', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const sideLineId = lineIds[1]!;

      // Set up bidirectional merged station relationships
      // Main line has 3 shots, so 4 stations (indices 0-3)
      // Last station is at index 3
      await setupMergedStations(page, mainLineId, 3, sideLineId);

      // Select the last station of main line (which has merged_with reference)
      await selectStation(page, mainLineId, 3);

      // Station detail should show merged station link
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Verify the "Merged with" line is displayed
      await expect(stationDetail.locator('text=Merged with')).toBeVisible();

      // Find and click the merged station link
      // The merged station is at sideLineId:0, displayed as position 0 (0-indexed)
      const mergedLink = page.getByTestId(`station-${sideLineId}-0-link`);
      await expect(mergedLink).toBeVisible();
      await mergedLink.click();

      // Selection should change to the merged station
      const selection = await pollSelectionState(
        page,
        (sel) =>
          sel?.type === 'station' &&
          sel?.stationRef?.line_id === sideLineId &&
          sel?.stationRef?.station_index === 0
      );
      expect(selection?.type).toBe('station');
      expect(selection?.stationRef?.line_id).toBe(sideLineId);
      expect(selection?.stationRef?.station_index).toBe(0);
    });

    test('merged station has bidirectional link back to original', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const sideLineId = lineIds[1]!;

      // Set up bidirectional merged station relationships
      // Main line has 3 shots, so last station is at index 3
      await setupMergedStations(page, mainLineId, 3, sideLineId);

      // Select the first station of the second line (which should link back to main line)
      await selectStation(page, sideLineId, 0);

      // Station detail should show merged station link
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Verify the "Merged with" line is displayed
      await expect(stationDetail.locator('text=Merged with')).toBeVisible();

      // Main line has 3 shots, so last station is at index 3 (position 3, 0-indexed)
      const mergedLink = page.getByTestId(`station-${mainLineId}-3-link`);
      await expect(mergedLink).toBeVisible();
      await mergedLink.click();

      // Selection should change back to the original merged station
      const selection = await pollSelectionState(
        page,
        (sel) =>
          sel?.type === 'station' &&
          sel?.stationRef?.line_id === mainLineId &&
          sel?.stationRef?.station_index === 3
      );
      expect(selection?.type).toBe('station');
      expect(selection?.stationRef?.line_id).toBe(mainLineId);
      expect(selection?.stationRef?.station_index).toBe(3);
    });

    test('clicking broken-chain icon unmerges the listed station pair', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const sideLineId = lineIds[1]!;

      await createDirectMerge(
        page,
        { line_id: mainLineId, station_index: 3 },
        { line_id: sideLineId, station_index: 0 }
      );

      await selectStation(page, mainLineId, 3);
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      const unmergeButton = page.getByTestId(`detail-unmerge-${sideLineId}-0`);
      await expect(unmergeButton).toBeVisible();
      await unmergeButton.click();

      await expect.poll(async () => {
        return page.evaluate(({ a, b }) => {
          const state = (window as any).__E2E_TEST__.projectStore.getState();
          const merges = state.project?.station_merges ?? [];
          return merges.some((merge: any) => {
            const direct =
              merge.station_a.line_id === a.line_id &&
              merge.station_a.station_index === a.station_index &&
              merge.station_b.line_id === b.line_id &&
              merge.station_b.station_index === b.station_index;
            const reverse =
              merge.station_b.line_id === a.line_id &&
              merge.station_b.station_index === a.station_index &&
              merge.station_a.line_id === b.line_id &&
              merge.station_a.station_index === b.station_index;
            return direct || reverse;
          });
        }, { a: { line_id: mainLineId, station_index: 3 }, b: { line_id: sideLineId, station_index: 0 } });
      }).toBe(false);
    });

    test('a junction lists the shots that meet it on the other lines, and shares its name', async ({ page }) => {
      // On the station graph a merge is ONE station two lines pass (plan 9.3):
      // its Connected section names the other line's shot too, and both slots
      // show the one Station ID.
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const sideLineId = lineIds[1]!;

      await createDirectMerge(
        page,
        { line_id: mainLineId, station_index: 3 },
        { line_id: sideLineId, station_index: 0 }
      );

      await selectStation(page, mainLineId, 3);
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();
      const otherShot = page.getByTestId(`station-detail-other-shot-${sideLineId}-0`);
      await expect(otherShot).toBeVisible();
      await expect(otherShot).toContainText('On ');
      const id = await page.getByTestId('station-detail-id').textContent();

      await selectStation(page, sideLineId, 0);
      await expect(page.getByTestId('station-detail-id')).toHaveText(id ?? '');
      await expect(page.getByTestId(`detail-unmerge-${mainLineId}-3`)).toHaveAttribute(
        'aria-label',
        /^Detach .+ from this station$/
      );
    });

    test('clicking broken-chain icon removes the listed closure', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const sideLineId = lineIds[1]!;

      const closureId = await createDirectClosure(
        page,
        { line_id: mainLineId, station_index: 3 },
        { line_id: sideLineId, station_index: 0 }
      );

      await selectStation(page, mainLineId, 3);
      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      const removeClosureButton = page.getByTestId(`detail-remove-closure-${closureId}`);
      await expect(removeClosureButton).toBeVisible();
      await removeClosureButton.click();

      await expect.poll(async () => {
        return page.evaluate((removedClosureId) => {
          const state = (window as any).__E2E_TEST__.projectStore.getState();
          const closures = state.project?.loop_closures ?? [];
          return closures.some((closure: any) => closure.id.toString() === removedClosureId);
        }, closureId);
      }).toBe(false);
    });
  });

  test.describe('Line Detail Links', () => {
    test('clicking First Station link changes selection', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select the line
      await selectLineById(page, mainLineId);

      // Line detail should be visible
      const lineDetail = page.getByTestId('line-detail');
      await expect(lineDetail).toBeVisible();

      // Find and click the First Station link (0-indexed, position 0)
      const firstStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await expect(firstStationLink).toBeVisible();
      await firstStationLink.click();

      // Selection should change to station
      const selection = await pollSelectionState(
        page,
        (sel) =>
          sel?.type === 'station' &&
          sel?.stationRef?.line_id === mainLineId &&
          sel?.stationRef?.station_index === 0
      );
      expect(selection?.type).toBe('station');
      expect(selection?.stationRef?.station_index).toBe(0);
    });

    test('clicking Last Station link changes selection', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select the line
      await selectLineById(page, mainLineId);

      // Line detail should be visible
      const lineDetail = page.getByTestId('line-detail');
      await expect(lineDetail).toBeVisible();

      // Main line has 3 shots, so last station is at index 3 (position 3, 0-indexed)
      const lastStationLink = page.getByTestId(`station-${mainLineId}-3-link`);
      await expect(lastStationLink).toBeVisible();
      await lastStationLink.click();

      // Selection should change to station
      const selection = await pollSelectionState(
        page,
        (sel) =>
          sel?.type === 'station' &&
          sel?.stationRef?.line_id === mainLineId &&
          sel?.stationRef?.station_index === 3
      );
      expect(selection?.type).toBe('station');
      expect(selection?.stationRef?.station_index).toBe(3);
    });

    test('single-station line shows Only Station', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const singleStationLineId = lineIds[2]!; // Third line has no shots

      // Select the single-station line
      await selectLineById(page, singleStationLineId);

      // Line detail should be visible
      const lineDetail = page.getByTestId('line-detail');
      await expect(lineDetail).toBeVisible();

      // Should show "Only Station" instead of "First Station" / "Last Station"
      const onlyStationLabel = lineDetail.locator('text=Only Station');
      await expect(onlyStationLabel).toBeVisible();

      // Should not show "First Station" or "Last Station"
      const firstStationLabel = lineDetail.locator('text=First Station');
      const lastStationLabel = lineDetail.locator('text=Last Station');
      await expect(firstStationLabel).toBeHidden();
      await expect(lastStationLabel).toBeHidden();
    });
  });

  test.describe('Center Buttons', () => {
    test('shot center button click triggers triggerCenterOnShot', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Find the center button
      const centerButton = page.getByTestId('center-shot-button');
      await expect(centerButton).toBeVisible();

      // Verify aria-label
      const ariaLabel = await centerButton.getAttribute('aria-label');
      // Named by its line and its two stations, never by ids (SPEC 4.2).
      expect(ariaLabel).toBe('Center map on shot Main Tunnel: 0 \u2192 1');

      // Verify button attributes for accessibility
      await expect(centerButton).toHaveAttribute('type', 'button');
      await expect(centerButton).toBeEnabled();

      // Set up a flag to capture when triggerCenterOnShot is called
      // This intercepts the trigger before map clears it
      let capturedRequest: { lineId: number; shotId: number } | null = null;
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: {
              getState: () => {
                triggerCenterOnShot: (lineId: number, shotId: number) => void;
              };
              setState: (state: unknown) => void;
            };
          };
        }).__E2E_TEST__;
        if (stores) {
          const originalTrigger = stores.uiStore.getState().triggerCenterOnShot;
          (window as unknown as { __capturedCenterOnShot?: unknown }).__capturedCenterOnShot = null;
          stores.uiStore.setState({
            triggerCenterOnShot: (lineId: number, shotId: number) => {
              (window as unknown as { __capturedCenterOnShot?: unknown }).__capturedCenterOnShot = { lineId, shotId };
              originalTrigger(lineId, shotId);
            },
          });
        }
      });

      // Click the center button
      await centerButton.click();

      // Verify the trigger was called with correct values
      capturedRequest = await page.evaluate(() => {
        return (window as unknown as { __capturedCenterOnShot?: { lineId: number; shotId: number } | null }).__capturedCenterOnShot ?? null;
      });
      expect(capturedRequest).not.toBeNull();
      expect(capturedRequest?.lineId).toBe(mainLineId);
      expect(capturedRequest?.shotId).toBe(firstShotId);
    });

    test('station center button click triggers triggerCenterOnStation', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select station at index 0
      await selectStation(page, mainLineId, 0);

      // Find the center button
      const centerButton = page.getByTestId('center-station-button');
      await expect(centerButton).toBeVisible();

      // Verify aria-label
      const ariaLabel = await centerButton.getAttribute('aria-label');
      expect(ariaLabel).toBe('Center map on station Main Tunnel / 0');

      // Verify button attributes for accessibility
      await expect(centerButton).toHaveAttribute('type', 'button');
      await expect(centerButton).toBeEnabled();

      // Set up a flag to capture when triggerCenterOnStation is called
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: {
              getState: () => {
                triggerCenterOnStation: (ref: { line_id: number; station_index: number }) => void;
              };
              setState: (state: unknown) => void;
            };
          };
        }).__E2E_TEST__;
        if (stores) {
          const originalTrigger = stores.uiStore.getState().triggerCenterOnStation;
          (window as unknown as { __capturedCenterOnStation?: unknown }).__capturedCenterOnStation = null;
          stores.uiStore.setState({
            triggerCenterOnStation: (ref: { line_id: number; station_index: number }) => {
              (window as unknown as { __capturedCenterOnStation?: unknown }).__capturedCenterOnStation = ref;
              originalTrigger(ref);
            },
          });
        }
      });

      // Click the center button
      await centerButton.click();

      // Verify the trigger was called with correct values
      const capturedRequest = await page.evaluate(() => {
        return (window as unknown as { __capturedCenterOnStation?: { line_id: number; station_index: number } | null }).__capturedCenterOnStation ?? null;
      });
      expect(capturedRequest).not.toBeNull();
      expect(capturedRequest?.line_id).toBe(mainLineId);
      expect(capturedRequest?.station_index).toBe(0);
    });

    test('line center button click triggers triggerZoomToLine', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select the line
      await selectLineById(page, mainLineId);

      // Find the center button
      const centerButton = page.getByTestId('center-line-button');
      await expect(centerButton).toBeVisible();

      // Verify aria-label
      const ariaLabel = await centerButton.getAttribute('aria-label');
      expect(ariaLabel).toContain('Center map on line');
      expect(ariaLabel).toContain('Main Tunnel');

      // Verify button attributes for accessibility
      await expect(centerButton).toHaveAttribute('type', 'button');
      await expect(centerButton).toBeEnabled();

      // Set up a flag to capture when triggerZoomToLine is called
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: {
              getState: () => {
                triggerZoomToLine: (lineId: number) => void;
              };
              setState: (state: unknown) => void;
            };
          };
        }).__E2E_TEST__;
        if (stores) {
          const originalTrigger = stores.uiStore.getState().triggerZoomToLine;
          (window as unknown as { __capturedZoomToLine?: unknown }).__capturedZoomToLine = null;
          stores.uiStore.setState({
            triggerZoomToLine: (lineId: number) => {
              (window as unknown as { __capturedZoomToLine?: unknown }).__capturedZoomToLine = lineId;
              originalTrigger(lineId);
            },
          });
        }
      });

      // Click the center button
      await centerButton.click();

      // Verify the trigger was called with correct value
      const capturedRequest = await page.evaluate(() => {
        return (window as unknown as { __capturedZoomToLine?: number | null }).__capturedZoomToLine ?? null;
      });
      expect(capturedRequest).toBe(mainLineId);
    });

    test('center button keyboard Enter triggers action', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select the line
      await selectLineById(page, mainLineId);

      // Find the center button
      const centerButton = page.getByTestId('center-line-button');
      await expect(centerButton).toBeVisible();

      // Verify the button is focusable
      await centerButton.focus();
      await expect(centerButton).toBeFocused();

      // Verify the button has type="button" (proper keyboard semantics)
      await expect(centerButton).toHaveAttribute('type', 'button');

      // Set up capture for triggerZoomToLine
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: {
              getState: () => {
                triggerZoomToLine: (lineId: number) => void;
              };
              setState: (state: unknown) => void;
            };
          };
        }).__E2E_TEST__;
        if (stores) {
          const originalTrigger = stores.uiStore.getState().triggerZoomToLine;
          (window as unknown as { __capturedZoomToLine?: unknown }).__capturedZoomToLine = null;
          stores.uiStore.setState({
            triggerZoomToLine: (lineId: number) => {
              (window as unknown as { __capturedZoomToLine?: unknown }).__capturedZoomToLine = lineId;
              originalTrigger(lineId);
            },
          });
        }
      });

      // Press Enter to activate
      await page.keyboard.press('Enter');

      // Verify the trigger was called
      const capturedRequest = await page.evaluate(() => {
        return (window as unknown as { __capturedZoomToLine?: number | null }).__capturedZoomToLine ?? null;
      });
      expect(capturedRequest).toBe(mainLineId);
    });
  });

  test.describe('Keyboard Navigation', () => {
    test('Tab navigates through links in detail panel', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Focus the Start Station link first
      const startStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await startStationLink.focus();
      await expect(startStationLink).toBeFocused();

      // Tab to next element (End Station link)
      await page.keyboard.press('Tab');

      const endStationLink = page.getByTestId(`station-${mainLineId}-1-link`);
      await expect(endStationLink).toBeFocused();

      // Tab to next element (Line link)
      await page.keyboard.press('Tab');

      const lineLink = page.getByTestId(`line-${mainLineId}-link`);
      await expect(lineLink).toBeFocused();
    });

    test('Enter activates focused link', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Focus the Start Station link
      const startStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await startStationLink.focus();
      await expect(startStationLink).toBeFocused();

      // Press Enter to activate
      await page.keyboard.press('Enter');

      // Selection should change to station
      const selection = await pollSelectionState(
        page,
        (sel) => sel?.type === 'station' && sel?.stationRef?.station_index === 0
      );
      expect(selection?.type).toBe('station');
    });

    test('links have visible focus ring when focused', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Focus the Start Station link
      const startStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await startStationLink.focus();
      await expect(startStationLink).toBeFocused();

      // Check that the element has a visible focus ring via computed styles
      // Tailwind's focus:ring-* generates box-shadow for focus rings
      const focusRingStyles = await startStationLink.evaluate((el) => {
        const styles = window.getComputedStyle(el);
        return {
          boxShadow: styles.boxShadow,
          outline: styles.outline,
          outlineWidth: styles.outlineWidth,
        };
      });

      // The focus ring should have a non-none box-shadow (Tailwind ring-* uses box-shadow)
      // or a visible outline
      const hasVisibleFocusIndicator =
        (focusRingStyles.boxShadow && focusRingStyles.boxShadow !== 'none') ||
        (focusRingStyles.outline && focusRingStyles.outline !== 'none') ||
        (focusRingStyles.outlineWidth && focusRingStyles.outlineWidth !== '0px');

      expect(hasVisibleFocusIndicator).toBe(true);
    });
  });

  test.describe('Station names', () => {
    test('shot ends, merged stations and the station view show station names', async ({ page }) => {
      await createProjectWithLines(page);
      const [mainLineId, sideLineId] = (await getLineIds(page)) as [number, number];
      await createDirectMerge(
        page,
        { line_id: mainLineId, station_index: 1 },
        { line_id: sideLineId, station_index: 0 },
      );
      await page.evaluate(
        async ({ main, side }) => {
          const setName = (window as unknown as {
            __MOCK_SET_STATION_NAME__: (s: number, i: number, n: string | null) => void;
          }).__MOCK_SET_STATION_NAME__;
          setName(main, 0, '7');
          setName(main, 1, '8');
          setName(side, 0, '0');
          const e2e = (window as unknown as {
            __E2E_TEST__: { projectStore: { getState: () => { refreshProjectState: () => Promise<void> } } };
          }).__E2E_TEST__;
          await e2e.projectStore.getState().refreshProjectState();
        },
        { main: mainLineId, side: sideLineId },
      );

      const shotIds = await getShotIdsForLine(page, mainLineId);
      await selectShot(page, mainLineId, shotIds[0]!);
      // The shot's ends are named, never `line:index`; the aria-label adds the line.
      await expect(page.getByTestId('shot-detail-label')).toHaveText('Shot:7 \u2192 8');
      await expect(page.getByTestId(`station-${mainLineId}-0-link`)).toHaveText('7');
      await expect(page.getByTestId(`station-${mainLineId}-0-link`)).toHaveAttribute('aria-label', 'Select station Main Tunnel / 7');
      await expect(page.getByTestId(`station-${mainLineId}-1-link`)).toHaveText('8');

      await selectStation(page, mainLineId, 1);
      await expect(page.getByTestId('station-detail-name')).toHaveText('Name:8');
      // The merge partner is the SAME station on the other line (the station
      // graph, plan 9.3): it shows the junction's one name, with that line.
      const partner = page.getByTestId(`station-${sideLineId}-0-link`);
      await expect(partner).toHaveAttribute('aria-label', 'Select station Side Passage / 8');
      await expect(partner).toContainText('8');
      await expect(partner).toContainText('Side Passage');

      // Every station has a name: one never renamed keeps the one seeded from its index.
      await selectStation(page, mainLineId, 2);
      await expect(page.getByTestId('station-detail-name')).toHaveText('Name:2');
      await expect(page.getByTestId('station-detail-line')).toHaveText('Line:Main Tunnel');
    });
  });

  test.describe('Accessibility', () => {
    test('all links have aria-labels', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Check Start Station link aria-label
      const startStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      const startAriaLabel = await startStationLink.getAttribute('aria-label');
      expect(startAriaLabel).toMatch(/Select station/i);

      // Check End Station link aria-label
      const endStationLink = page.getByTestId(`station-${mainLineId}-1-link`);
      const endAriaLabel = await endStationLink.getAttribute('aria-label');
      expect(endAriaLabel).toMatch(/Select station/i);

      // Check Line link aria-label
      const lineLink = page.getByTestId(`line-${mainLineId}-link`);
      const lineAriaLabel = await lineLink.getAttribute('aria-label');
      expect(lineAriaLabel).toMatch(/Select line/i);
    });

    test('ARIA live region announces selection changes', async ({ page }) => {
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select the first shot
      await selectShot(page, mainLineId, firstShotId);

      // Click the Start Station link
      const startStationLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await startStationLink.click();

      // Check the ARIA live region for the announcement
      await expect.poll(async () => {
        const text = await getAnnouncerText(page);
        return text.toLowerCase().includes('selected') && text.toLowerCase().includes('station');
      }, { timeout: 5000 }).toBe(true);
    });

    test('ARIA live region exists and has correct attributes', async ({ page }) => {
      await createProjectWithLines(page);

      // Check that the ARIA live region exists
      const announcer = page.locator('#selection-announcer');
      await expect(announcer).toBeAttached();

      // Verify ARIA attributes
      await expect(announcer).toHaveAttribute('role', 'status');
      await expect(announcer).toHaveAttribute('aria-live', 'polite');
      await expect(announcer).toHaveAttribute('aria-atomic', 'true');
    });
  });

  test.describe('Edge Cases', () => {
    test('verifies shot at index 0 links to correct stations', async ({ page }) => {
      // Concrete example from spec:
      // Shot at index 0 (display "1") -> Start Station index 0 (display "1"), End Station index 1 (display "2")
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;
      const shotIds = await getShotIdsForLine(page, mainLineId);
      const firstShotId = shotIds[0]!;

      // Select shot at index 0
      await selectShot(page, mainLineId, firstShotId);

      // The start station is station 0, named "0" (seeded from its index).
      const startLink = page.getByTestId(`station-${mainLineId}-0-link`);
      await expect(startLink).toBeVisible();
      await expect(startLink).toHaveText('0');
      await expect(startLink).toHaveAttribute('aria-label', 'Select station Main Tunnel / 0');

      // The end station is station 1, named "1".
      const endLink = page.getByTestId(`station-${mainLineId}-1-link`);
      await expect(endLink).toBeVisible();
      await expect(endLink).toHaveText('1');
    });

    test('verifies station at index 0 has no previous shot', async ({ page }) => {
      // Station index 0: No previous shot, Next shot = shots[0] (display "1")
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select station at index 0
      await selectStation(page, mainLineId, 0);

      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Should not have Previous Shot
      await expect(stationDetail.locator('text=Previous Shot')).toBeHidden();

      // Should have Next Shot with position 1 (shots are 1-indexed)
      await expect(stationDetail.locator('text=Next Shot')).toBeVisible();
      const nextShotLink = page.getByTestId(`shot-${mainLineId}-1-link`);
      await expect(nextShotLink).toBeVisible();
    });

    test('verifies station at middle index has both prev and next shots', async ({ page }) => {
      // Station index 1: Previous shot = shots[0] (display "1"), Next shot = shots[1] (display "2")
      // Note: shot positions are still 1-indexed
      await createProjectWithLines(page);

      const lineIds = await getLineIds(page);
      const mainLineId = lineIds[0]!;

      // Select station at index 1
      await selectStation(page, mainLineId, 1);

      const stationDetail = page.getByTestId('station-detail');
      await expect(stationDetail).toBeVisible();

      // Should have Previous Shot with position 1 (shots are 1-indexed)
      await expect(stationDetail.locator('text=Previous Shot')).toBeVisible();
      const prevShotLink = page.getByTestId(`shot-${mainLineId}-1-link`);
      await expect(prevShotLink).toBeVisible();

      // Should have Next Shot with position 2 (shots are 1-indexed)
      await expect(stationDetail.locator('text=Next Shot')).toBeVisible();
      const nextShotLink = page.getByTestId(`shot-${mainLineId}-2-link`);
      await expect(nextShotLink).toBeVisible();
    });
  });
});
