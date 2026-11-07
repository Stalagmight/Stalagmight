import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

type StationRef = { section_id: number; station_index: number };

type MenuTargets = {
  sectionId: number;
  sectionName: string;
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
  await seedProjectWithSections(page, [
    {
      name: 'Context Section',
      direction: 'in',
      shots,
    },
  ]);

  await waitForMapReady(page);
  await waitForNoDialogs(page);

  const ids = await page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;

    if (!e2e) return null;

    const state = e2e.projectStore.getState();
    const section = state.project?.sections?.[0];
    const stations = (state.stations ?? [])
      .filter((s) => s.station_ref.section_id === section?.id)
      .sort((a, b) => a.station_ref.station_index - b.station_ref.station_index);

    return {
      sectionId: section?.id ?? null,
      sectionName: section?.name ?? `Section ${section?.id ?? 0}`,
      shotIds: (section?.shots ?? []).map((s) => s.id),
      stationRefs: stations.map((s) => s.station_ref),
    };
  });

  if (!ids || !ids.sectionId || ids.shotIds.length < 1 || ids.stationRefs.length < 1) {
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

  await seedProjectWithSections(page, [
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
            project: { sections: Array<{ id: number }> } | null;
            mergeStations: (a: StationRef, b: StationRef) => Promise<unknown>;
          };
        };
      };
    }).__E2E_TEST__;

    if (!e2e) return null;
    const state = e2e.projectStore.getState();
    const sectionIds = (state.project?.sections ?? []).map((section) => section.id);
    if (sectionIds.length < 3) return null;

    const targetRef = { section_id: sectionIds[0]!, station_index: 0 };
    const mergeRefA = { section_id: sectionIds[1]!, station_index: 0 };
    const mergeRefB = { section_id: sectionIds[2]!, station_index: 0 };

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

  await seedProjectWithSections(page, [
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
              sections: Array<{ id: number }>;
              loop_closures: Array<{
                id: number;
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
    const sectionIds = (state.project?.sections ?? []).map((section) => section.id);
    if (sectionIds.length < 3) return null;

    const targetRef = { section_id: sectionIds[0]!, station_index: 0 };
    const closureRefA = { section_id: sectionIds[1]!, station_index: 0 };
    const closureRefB = { section_id: sectionIds[2]!, station_index: 0 };

    await state.createLoopClosure(targetRef, closureRefA, 'batch');
    if (count > 1) {
      await state.createLoopClosure(targetRef, closureRefB, 'batch');
    }

    const closures = e2e.projectStore.getState().project?.loop_closures ?? [];
    const findByPair = (a: StationRef, b: StationRef) => {
      return closures.find((closure) => {
        const direct =
          closure.station_a.section_id === a.section_id &&
          closure.station_a.station_index === a.station_index &&
          closure.station_b.section_id === b.section_id &&
          closure.station_b.station_index === b.station_index;
        const reverse =
          closure.station_a.section_id === b.section_id &&
          closure.station_a.station_index === b.station_index &&
          closure.station_b.section_id === a.section_id &&
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
      (window as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target,
        position: { x: 140, y: 140 },
      });
    },
    { target }
  );
}

test.describe('Context Menu Coverage', () => {
  test('shot menu sets the shot kind and routes delete dialogs', async ({ page }) => {
    const { sectionId, shotIds } = await setupProjectWithShots(page, 3);

    const [firstShotId, midShotId] = shotIds;

    await openContextMenu(page, { type: 'shot', sectionId, shotId: firstShotId });
    await expect(page.getByRole('button', { name: 'Edit Shot' })).toBeVisible();

    // #27: the menu offers the two kinds the shot is not, outright.
    await page.getByRole('button', { name: 'Mark as Reference' }).click();

    await expect.poll(async () => {
      return page.evaluate(({ sectionId, shotId }) => {
        const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
        const section = state.project!.sections.find((s) => s.id === sectionId);
        const shot = section?.shots.find((s) => s.id === shotId);
        return shot?.kind ?? null;
      }, { sectionId, shotId: firstShotId });
    }).toBe('Reference');

    await openContextMenu(page, { type: 'shot', sectionId, shotId: midShotId });
    await page.getByRole('button', { name: 'Delete Shot' }).click();

    const deleteChoice = page.getByTestId('delete-shot-choice-dialog');
    await expect(deleteChoice).toBeVisible();
    await deleteChoice.getByRole('button', { name: 'Cancel' }).click();
    await expect(deleteChoice).not.toBeVisible({ timeout: 5000 });

    await openContextMenu(page, { type: 'shot', sectionId, shotId: firstShotId });
    await page.getByRole('button', { name: 'Delete Shot' }).click();

    const confirmDialog = page.getByTestId('confirm-dialog');
    await expect(confirmDialog).toBeVisible();
    await confirmDialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirmDialog).not.toBeVisible({ timeout: 5000 });

    await waitForNoDialogs(page);
  });

  test('datatable shot menu uses delete helper for mid-shot', async ({ page }) => {
    const { sectionId, shotIds, sectionName } = await setupProjectWithShots(page, 3);
    const midShotId = shotIds[1];

    await openContextMenu(page, {
      type: 'datatable-shot',
      sectionId,
      shotId: midShotId,
      isFirst: false,
      isLast: false,
      totalShots: 3,
      sectionName,
    });

    await page.getByRole('button', { name: 'Delete Shot' }).click();
    await expect(page.getByTestId('delete-shot-choice-dialog')).toBeVisible();
  });

  test('station menu exposes add-shot options and merge tool', async ({ page }) => {
    const { stationRefs } = await setupProjectWithShots(page, 3);
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
        const uiState = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
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

  test('section menu opens adjust position dialog', async ({ page }) => {
    const { sectionId } = await setupProjectWithShots(page, 2);

    await openContextMenu(page, { type: 'section', sectionId });
    await page.getByRole('button', { name: 'Adjust Position...' }).click();

    const dialog = page.getByTestId('section-adjust-dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });

  test('station menu unmerges immediately when exactly one merge exists', async ({ page }) => {
    const { targetRef } = await setupProjectWithMergedStation(page, 1);

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await expect(page.getByRole('button', { name: 'Unmerge Stations' })).toBeVisible();
    await page.getByRole('button', { name: 'Unmerge Stations' }).click();

    await expect(page.getByTestId('unmerge-stations-dialog')).toBeHidden();

    await expect.poll(async () => {
      return page.evaluate(() => {
        const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
        return state.project?.station_merges?.length ?? 0;
      });
    }).toBe(0);
  });

  test('station menu opens picker dialog when multiple merges exist', async ({ page }) => {
    const { targetRef, mergeRefB } = await setupProjectWithMergedStation(page, 2);
    if (!mergeRefB) {
      throw new Error('Expected second merge target');
    }

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await expect(page.getByRole('button', { name: 'Unmerge Stations' })).toBeVisible();
    await page.getByRole('button', { name: 'Unmerge Stations' }).click();

    const unmergeDialog = page.getByTestId('unmerge-stations-dialog');
    await expect(unmergeDialog).toBeVisible();

    await page.getByTestId(`unmerge-station-option-${mergeRefB.section_id}-${mergeRefB.station_index}`).click();
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(unmergeDialog).not.toBeVisible({ timeout: 5000 });

    await expect.poll(async () => {
      return page.evaluate(({ target, removed }) => {
        const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
        const merges = state.project?.station_merges ?? [];
        const hasRemovedPair = merges.some((merge) => {
          const direct =
            merge.station_a.section_id === target.section_id &&
            merge.station_a.station_index === target.station_index &&
            merge.station_b.section_id === removed.section_id &&
            merge.station_b.station_index === removed.station_index;
          const reverse =
            merge.station_b.section_id === target.section_id &&
            merge.station_b.station_index === target.station_index &&
            merge.station_a.section_id === removed.section_id &&
            merge.station_a.station_index === removed.station_index;
          return direct || reverse;
        });
        return {
          mergeCount: merges.length,
          hasRemovedPair,
        };
      }, { target: targetRef, removed: mergeRefB });
    }).toEqual({ mergeCount: 1, hasRemovedPair: false });
  });

  test('station menu removes closure immediately when exactly one direct closure exists', async ({ page }) => {
    const { targetRef } = await setupProjectWithStationClosures(page, 1);

    await openContextMenu(page, { type: 'station', ref: targetRef });
    await expect(page.getByRole('button', { name: 'Remove Closure' })).toBeVisible();
    await page.getByRole('button', { name: 'Remove Closure' }).click();

    await expect(page.getByTestId('remove-closure-dialog')).toBeHidden();

    await expect.poll(async () => {
      return page.evaluate(() => {
        const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
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
        const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
        const closures = state.project?.loop_closures ?? [];
        const hasRemovedClosure = closures.some((closure) => closure.id.toString() === removedClosureId);
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
    const { sectionId, shotIds, sectionName } = await setupProjectWithShots(page, 3);
    const firstShotId = shotIds[0];

    await openContextMenu(page, { type: 'shot', sectionId, shotId: firstShotId });
    const insertItem = page.getByRole('button', { name: 'Insert Station...' });
    await expect(insertItem).toBeVisible();
    await expect(insertItem).toBeEnabled();
    await insertItem.click();

    const dialog = page.getByTestId('insert-station-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(sectionName);
    // buildShot(0) yields length 6.
    await expect(page.getByTestId('insert-station-preview')).toContainText('Total shot length: 6.00');

    await page.getByTestId('insert-station-distance').fill('2.5');
    await page.getByTestId('insert-station-confirm').click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const call = await page.evaluate(() => {
      const calls = (window as E2EWindow).__MOCK_SPLIT_SHOT_CALLS__ ?? [];
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toEqual({ sectionId, shotId: firstShotId, distance: 2.5 });
  });

  test('a zero-length shot disables Insert Station', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Zero Length Shot');
    await seedProjectWithSections(page, [
      { name: 'Zero Section', direction: 'in', shots: [buildShot(0, { length: 0 })] },
    ]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    const ids = await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      const section = e2e?.projectStore.getState().project?.sections?.[0];
      return { sectionId: section?.id ?? null, shotId: section?.shots?.[0]?.id ?? null };
    });
    if (!ids.sectionId || !ids.shotId) {
      throw new Error('Failed to resolve the zero-length shot');
    }

    await openContextMenu(page, { type: 'shot', sectionId: ids.sectionId, shotId: ids.shotId });
    await expect(page.getByRole('button', { name: 'Insert Station...' })).toBeDisabled();
  });

  test('Insert Station is absent for a shot target in Plan mode', async ({ page }) => {
    const { sectionId, shotIds } = await setupProjectWithShots(page, 3);

    await page.locator('[data-testid="mode-plan"]').click();
    await expect.poll(async () => {
      return page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().appMode);
    }).toBe('plan');

    await openContextMenu(page, { type: 'shot', sectionId, shotId: shotIds[0] });
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
