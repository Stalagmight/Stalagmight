import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

type StationRef = { line_id: number; station_index: number };

type MenuTargets = {
  lineId: number;
  lineName: string;
  shotIds: number[];
  stationRefs: StationRef[];
};

function buildShot(index: number, overrides?: Partial<Record<string, number>>) {
  const base = index + 1;
  return {
    azimuth_in: 10 * base,
    azimuth_out: 180 + 10 * base,
    length: 5 + base,
    depth_in: -1 * base,
    depth_out: -1.5 * base,
    left: 0.5 * base,
    right: 0.7 * base,
    up: 0.9 * base,
    down: 0.4 * base,
    ...overrides,
  };
}

async function setupProjectWithShots(page: Page, shotCount: number): Promise<MenuTargets> {
  await setupMockedPage(page);
  await createProject(page, 'Context Menu Coverage');

  const shots = Array.from({ length: shotCount }, (_, idx) => buildShot(idx));
  await seedProjectWithLines(page, [
    {
      name: 'Context Line',
      direction: 'in',
      shots,
    },
  ]);

  await waitForMapReady(page);
  await waitForNoDialogs(page);

  const ids = await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: any; stations: any[] } };
      };
    }).__E2E_TEST__;

    if (!e2e) return null;

    const state = e2e.projectStore.getState();
    const line = state.project?.lines?.[0];
    const stations = (state.stations ?? [])
      .filter((s) => s.station_ref.line_id === line?.id)
      .sort((a, b) => a.station_ref.station_index - b.station_ref.station_index);

    return {
      lineId: line?.id ?? null,
      lineName: line?.name ?? `Line ${line?.id ?? 0}`,
      shotIds: (line?.shots ?? []).map((s: any) => s.id),
      stationRefs: stations.map((s) => s.station_ref),
    };
  });

  if (!ids || !ids.lineId || ids.shotIds.length < 1 || ids.stationRefs.length < 1) {
    throw new Error('Failed to resolve IDs for context menu tests.');
  }

  return ids as MenuTargets;
}

async function setupProjectWithMergedStation(
  page: Page,
  mergeCount: 1 | 2
): Promise<{
  targetRef: StationRef;
  mergeRefA: StationRef;
  mergeRefB: StationRef | null;
}> {
  await setupMockedPage(page);
  await createProject(page, 'Context Menu Unmerge Coverage');

  await seedProjectWithLines(page, [
    { name: 'Merge A', direction: 'in', shots: [buildShot(0)] },
    { name: 'Merge B', direction: 'in', shots: [buildShot(1)] },
    { name: 'Merge C', direction: 'in', shots: [buildShot(2)] },
  ]);

  await waitForMapReady(page);
  await waitForNoDialogs(page);

  const refs = await page.evaluate(async (count) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            project: { lines: Array<{ id: number }> } | null;
            mergeStations: (a: StationRef, b: StationRef) => Promise<unknown>;
          };
        };
      };
    }).__E2E_TEST__;

    if (!e2e) return null;
    const state = e2e.projectStore.getState();
    const lineIds = (state.project?.lines ?? []).map((line) => line.id);
    if (lineIds.length < 3) return null;

    const targetRef = { line_id: lineIds[0]!, station_index: 0 };
    const mergeRefA = { line_id: lineIds[1]!, station_index: 0 };
    const mergeRefB = { line_id: lineIds[2]!, station_index: 0 };

    await state.mergeStations(targetRef, mergeRefA);
    if (count > 1) {
      await state.mergeStations(targetRef, mergeRefB);
    }

    return {
      targetRef,
      mergeRefA,
      mergeRefB: count > 1 ? mergeRefB : null,
    };
  }, mergeCount);

  if (!refs) {
    throw new Error('Failed to prepare merged station test data');
  }

  return refs;
}

async function setupProjectWithStationClosures(
  page: Page,
  closureCount: 1 | 2
): Promise<{
  targetRef: StationRef;
  closureRefA: StationRef;
  closureRefB: StationRef | null;
  closureAId: string;
  closureBId: string | null;
}> {
  await setupMockedPage(page);
  await createProject(page, 'Context Menu Closure Coverage');

  await seedProjectWithLines(page, [
    { name: 'Closure A', direction: 'in', shots: [buildShot(0)] },
    { name: 'Closure B', direction: 'in', shots: [buildShot(1)] },
    { name: 'Closure C', direction: 'in', shots: [buildShot(2)] },
  ]);

  await waitForMapReady(page);
  await waitForNoDialogs(page);

  const refs = await page.evaluate(async (count) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            project: {
              lines: Array<{ id: number }>;
              loop_closures: Array<{
                id: bigint;
                station_a: StationRef;
                station_b: StationRef;
              }>;
            } | null;
            createLoopClosure: (a: StationRef, b: StationRef, strategy: string) => Promise<unknown>;
          };
        };
      };
    }).__E2E_TEST__;

    if (!e2e) return null;
    const state = e2e.projectStore.getState();
    const lineIds = (state.project?.lines ?? []).map((line) => line.id);
    if (lineIds.length < 3) return null;

    const targetRef = { line_id: lineIds[0]!, station_index: 0 };
    const closureRefA = { line_id: lineIds[1]!, station_index: 0 };
    const closureRefB = { line_id: lineIds[2]!, station_index: 0 };

    await state.createLoopClosure(targetRef, closureRefA, 'batch');
    if (count > 1) {
      await state.createLoopClosure(targetRef, closureRefB, 'batch');
    }

    const closures = e2e.projectStore.getState().project?.loop_closures ?? [];
    const findByPair = (a: StationRef, b: StationRef) => {
      return closures.find((closure: any) => {
        const direct =
          closure.station_a.line_id === a.line_id &&
          closure.station_a.station_index === a.station_index &&
          closure.station_b.line_id === b.line_id &&
          closure.station_b.station_index === b.station_index;
        const reverse =
          closure.station_a.line_id === b.line_id &&
          closure.station_a.station_index === b.station_index &&
          closure.station_b.line_id === a.line_id &&
          closure.station_b.station_index === a.station_index;
        return direct || reverse;
      });
    };

    const closureAResolved = findByPair(targetRef, closureRefA);
    const closureBResolved = count > 1 ? findByPair(targetRef, closureRefB) : null;
    if (!closureAResolved || (count > 1 && !closureBResolved)) {
      return null;
    }

    return {
      targetRef,
      closureRefA,
      closureRefB: count > 1 ? closureRefB : null,
      closureAId: closureAResolved.id.toString(),
      closureBId: closureBResolved ? closureBResolved.id.toString() : null,
    };
  }, closureCount);

  if (!refs) {
    throw new Error('Failed to prepare closure test data');
  }

  return refs;
}

async function openContextMenu(page: Page, target: Record<string, unknown>) {
  await page.evaluate(
    ({ target }) => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target,
        position: { x: 140, y: 140 },
      });
    },
    { target }
  );
}

test.describe('Context Menu Coverage', () => {
  test('shot menu sets the shot kind and routes delete dialogs', async ({ page }) => {
    const { lineId, shotIds, lineName } = await setupProjectWithShots(page, 3);

    const [firstShotId, midShotId] = shotIds;

    await openContextMenu(page, { type: 'shot', lineId, shotId: firstShotId });
    await expect(page.getByRole('button', { name: 'Edit Shot' })).toBeVisible();

    // #27: the menu offers the two kinds the shot is not, outright.
    await page.getByRole('button', { name: 'Mark as Reference' }).click();

    await expect.poll(async () => {
      return page.evaluate(({ lineId, shotId }) => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        const line = state.project.lines.find((s: any) => s.id === lineId);
        const shot = line?.shots.find((s: any) => s.id === shotId);
        return shot?.kind ?? null;
      }, { lineId, shotId: firstShotId });
    }).toBe('Reference');

    await openContextMenu(page, { type: 'shot', lineId, shotId: midShotId });
    await page.getByRole('button', { name: 'Delete Shot' }).click();

    const deleteChoice = page.getByTestId('delete-shot-choice-dialog');
    await expect(deleteChoice).toBeVisible();
    await deleteChoice.getByRole('button', { name: 'Cancel' }).click();
    await expect(deleteChoice).not.toBeVisible({ timeout: 5000 });

    await openContextMenu(page, { type: 'shot', lineId, shotId: firstShotId });
    await page.getByRole('button', { name: 'Delete Shot' }).click();

    const confirmDialog = page.getByTestId('confirm-dialog');
    await expect(confirmDialog).toBeVisible();
    await confirmDialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirmDialog).not.toBeVisible({ timeout: 5000 });

    await waitForNoDialogs(page);
  });

  test('datatable shot menu uses delete helper for mid-shot', async ({ page }) => {
    const { lineId, shotIds, lineName } = await setupProjectWithShots(page, 3);
    const midShotId = shotIds[1];

    await openContextMenu(page, {
      type: 'datatable-shot',
      lineId,
      shotId: midShotId,
      isFirst: false,
      isLast: false,
      totalShots: 3,
      lineName,
    });

    await page.getByRole('button', { name: 'Delete Shot' }).click();
    await expect(page.getByTestId('delete-shot-choice-dialog')).toBeVisible();
  });

  test('station menu exposes add-shot options and merge tool', async ({ page }) => {
    const { lineId, stationRefs } = await setupProjectWithShots(page, 3);
    const firstStation = stationRefs[0];
    const lastStation = stationRefs[stationRefs.length - 1];

    await openContextMenu(page, { type: 'station', ref: firstStation });
    await expect(page.getByRole('button', { name: 'Add Shot Before' })).toBeVisible();
    await page.getByRole('button', { name: 'Edit Station' }).click();

    const editStation = page.getByTestId('edit-station-dialog');
    await expect(editStation).toBeVisible();
    await editStation.getByRole('button', { name: 'Cancel' }).click();
    await expect(editStation).not.toBeVisible({ timeout: 5000 });

    await openContextMenu(page, { type: 'station', ref: lastStation });
    await expect(page.getByRole('button', { name: 'Add Shot After' })).toBeVisible();
    await page.getByRole('button', { name: 'Add Shot After' }).click();

    const drawShot = page.getByTestId('draw-shot-dialog');
    await expect(drawShot).toBeVisible();
    await drawShot.getByRole('button', { name: 'Cancel' }).click();
    await expect(drawShot).not.toBeVisible({ timeout: 5000 });

    await openContextMenu(page, { type: 'station', ref: firstStation });
    await page.getByRole('button', { name: 'Merge With...' }).click();

    await expect.poll(async () => {
      return page.evaluate(() => {
        const uiState = (window as any).__E2E_TEST__.uiStore.getState();
        return {
          activeTool: uiState.activeTool,
          mergeStationA: uiState.mergeStationA,
        };
      });
    }).toEqual({
      activeTool: 'merge',
      mergeStationA: firstStation,
    });
  });

  test('line menu opens adjust position dialog', async ({ page }) => {
    const { lineId } = await setupProjectWithShots(page, 2);

    await openContextMenu(page, { type: 'line', lineId });
    await page.getByRole('button', { name: 'Adjust Position...' }).click();

    const dialog = page.getByTestId('line-adjust-dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });

  // On the station graph a merge is ONE station two or more lines pass; any one
  // line can be detached from it (plan 5.2). On two lines where either choice
  // gives the same result (the clicked line keeps the station, the merged
  // line gets back the station it had) the dialog offers no choice: it says
  // which line comes off and detaches that one (N-2).
  test('station menu detaches the line that comes off a two-line station', async ({ page }) => {
    const { targetRef, mergeRefA } = await setupProjectWithMergedStation(page, 1);

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await expect(page.getByRole('button', { name: 'Unmerge Stations' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Detach Line from Station...' }).click();

    const dialog = page.getByTestId('unmerge-stations-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('unmerge-stations-naming')).toHaveText(
      /^Merge B comes off: it gets back .+ \(Station ID \d+\); Merge A keeps .+\.$/,
    );
    await expect(page.getByTestId(`unmerge-station-option-${targetRef.line_id}-${targetRef.station_index}`)).toHaveCount(0);
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // The pair form detaches its partner: Merge B, the line that came off.
    const calls = await page.evaluate(() =>
      (window as unknown as { __MOCK_GET_UNMERGE_STATION_PAIR_CALLS__: () => Array<{ stationB: StationRef }> })
        .__MOCK_GET_UNMERGE_STATION_PAIR_CALLS__(),
    );
    expect(calls.map((c) => c.stationB)).toEqual([mergeRefA]);

    await expect.poll(async () => {
      return page.evaluate(() => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        return state.project?.station_merges?.length ?? 0;
      });
    }).toBe(0);
  });

  test('station menu lets the user pick which line of a three-line station to detach', async ({ page }) => {
    const { targetRef, mergeRefA, mergeRefB } = await setupProjectWithMergedStation(page, 2);
    if (!mergeRefB) {
      throw new Error('Expected second merge target');
    }

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await page.getByRole('button', { name: 'Detach Line from Station...' }).click();

    const dialog = page.getByTestId('unmerge-stations-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('is where 3 lines meet');

    await page.getByTestId(`unmerge-station-option-${mergeRefB.line_id}-${mergeRefB.station_index}`).click();
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // The chosen line left; the other two are still one station.
    await expect.poll(async () => {
      return page.evaluate(({ target, kept, removed }) => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        const at = (ref: { line_id: number; station_index: number }) =>
          state.stations.find(
            (s: any) => s.station_ref.line_id === ref.line_id && s.station_ref.station_index === ref.station_index,
          );
        return {
          mergeCount: state.project?.station_merges?.length ?? 0,
          targetWith: at(target)?.merged_with.map((r: any) => r.line_id) ?? null,
          removedWith: at(removed)?.merged_with.length ?? null,
          sharedId: at(target)?.id === at(kept)?.id,
          detachedOwnId: at(removed)?.id !== at(target)?.id,
        };
      }, { target: targetRef, kept: mergeRefA, removed: mergeRefB });
    }).toEqual({
      mergeCount: 1,
      targetWith: [mergeRefA.line_id],
      removedWith: 0,
      sharedId: true,
      detachedOwnId: true,
    });
  });

  test('station menu removes closure immediately when exactly one direct closure exists', async ({ page }) => {
    const { targetRef } = await setupProjectWithStationClosures(page, 1);

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await expect(page.getByRole('button', { name: 'Remove Closure' })).toBeVisible();
    await page.getByRole('button', { name: 'Remove Closure' }).click();

    await expect(page.getByTestId('remove-closure-dialog')).toBeHidden();

    await expect.poll(async () => {
      return page.evaluate(() => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        return state.project?.loop_closures?.length ?? 0;
      });
    }).toBe(0);
  });

  test('station menu opens closure picker when multiple direct closures exist', async ({ page }) => {
    const { targetRef, closureBId } = await setupProjectWithStationClosures(page, 2);
    if (!closureBId) {
      throw new Error('Expected second closure id');
    }

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await expect(page.getByRole('button', { name: 'Remove Closure' })).toBeVisible();
    await page.getByRole('button', { name: 'Remove Closure' }).click();

    const removeDialog = page.getByTestId('remove-closure-dialog');
    await expect(removeDialog).toBeVisible();

    await page.getByTestId(`remove-closure-option-${closureBId}`).click();
    await page.getByTestId('remove-closure-confirm').click();
    await expect(removeDialog).not.toBeVisible({ timeout: 5000 });

    await expect.poll(async () => {
      return page.evaluate((removedClosureId) => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        const closures = state.project?.loop_closures ?? [];
        const hasRemovedClosure = closures.some((closure: any) => closure.id.toString() === removedClosureId);
        return {
          closureCount: closures.length,
          hasRemovedClosure,
        };
      }, closureBId);
    }).toEqual({ closureCount: 1, hasRemovedClosure: false });
  });
});

test.describe('Insert Station (#59)', () => {
  test('shot menu offers Insert Station, opens the dialog, and confirms a split', async ({ page }) => {
    const { lineId, shotIds, lineName } = await setupProjectWithShots(page, 3);
    const firstShotId = shotIds[0];

    await openContextMenu(page, { type: 'shot', lineId, shotId: firstShotId });
    const insertItem = page.getByRole('button', { name: 'Insert Station...' });
    await expect(insertItem).toBeVisible();
    await expect(insertItem).toBeEnabled();
    await insertItem.click();

    const dialog = page.getByTestId('insert-station-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(lineName);
    // buildShot(0) yields length 6.
    await expect(page.getByTestId('insert-station-preview')).toContainText('Total shot length: 6.00');

    await page.getByTestId('insert-station-distance').fill('2.5');
    await page.getByTestId('insert-station-confirm').click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_SPLIT_SHOT_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toEqual({ lineId, shotId: firstShotId, distance: 2.5 });
  });

  test('a zero-length shot disables Insert Station', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Zero Length Shot');
    await seedProjectWithLines(page, [
      { name: 'Zero Line', direction: 'in', shots: [buildShot(0, { length: 0 })] },
    ]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    const ids = await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: { projectStore: { getState: () => { project: any } } };
      }).__E2E_TEST__;
      const line = e2e?.projectStore.getState().project?.lines?.[0];
      return { lineId: line?.id ?? null, shotId: line?.shots?.[0]?.id ?? null };
    });
    if (!ids.lineId || !ids.shotId) {
      throw new Error('Failed to resolve the zero-length shot');
    }

    await openContextMenu(page, { type: 'shot', lineId: ids.lineId, shotId: ids.shotId });
    await expect(page.getByRole('button', { name: 'Insert Station...' })).toBeDisabled();
  });

  test('Insert Station is absent for a shot target in Plan mode', async ({ page }) => {
    const { lineId, shotIds } = await setupProjectWithShots(page, 3);

    await page.locator('[data-testid="mode-plan"]').click();
    await expect.poll(async () => {
      return page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().appMode);
    }).toBe('plan');

    await openContextMenu(page, { type: 'shot', lineId, shotId: shotIds[0] });
    await expect(page.getByRole('button', { name: 'Insert Station...' })).toHaveCount(0);
  });

  // Note: the data table's SpreadsheetGrid renders its own internal
  // right-click menu (grid-context-menu), which is a live DOM row interaction
  // rather than the synthetic openContextMenu() dialog helper used above. None
  // of this file's helpers drive the Data dock panel, so covering that path
  // (open the Data panel, right-click a shot row, assert "Insert Station..."
  // in the SpreadsheetGrid's context menu) is a separate, heavier setup, not
  // added here to keep this file's synthetic-target pattern consistent.
});
