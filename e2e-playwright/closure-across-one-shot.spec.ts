/**
 * E2E (#737): loop closures across one shot, in the mocked-IPC tier.
 *
 * - The Create Loop Closure dialog shows the backend's refusal and offers no
 *   Create, so a closure tying the two ends of one shot is never sent.
 * - The Closures Computed View shows what its solve left out (a planted
 *   adjustment report) beside the read-only pill, and Show warnings opens the
 *   Warnings tab.
 * - A shift delete that removed a closure says so in one warning toast.
 *
 * Synthetic data only. Parallel-safe: all state is per-page (mocked IPC +
 * `window.__E2E_TEST__` stores).
 */
import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

const REFUSAL =
  'This closure would join the two ends of one shot (0:1 to 0:2) and squeeze that shot to zero length. Pick two stations that are not the ends of a single shot.';

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Synthetic loop',
    direction: 'in',
    shots: [
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -1, depth_out: -2 },
      { azimuth_in: 0, azimuth_out: 180, length: 8, depth_in: -2, depth_out: -2 },
      { azimuth_in: 270, azimuth_out: 90, length: 15, depth_in: -2, depth_out: -1 },
    ],
  },
];

// Every page.evaluate body runs in the browser, so each reaches the stores
// through `window.__E2E_TEST__` itself (a Node-side helper is not in scope there).
async function sectionId(page: Page): Promise<number> {
  return page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().project.sections[0].id as number);
}

test.describe('Loop closures across one shot (#737)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Across one shot');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapIdle(page);
    await waitForNoDialogs(page);
  });

  test('the closure dialog shows the refusal and offers no Create', async ({ page }) => {
    const secId = await sectionId(page);
    await page.evaluate(
      ({ secId, refusal }) => {
        (window as any).__E2E_TEST__.uiStore.getState().openDialog('closure', {
          stationA: { section_id: secId, station_index: 1 },
          stationB: { section_id: secId, station_index: 2 },
          preview: {
            path_length: 10,
            misclosure_distance: 10,
            error_percentage: 100,
            horizontal_misclosure: 10,
            vertical_misclosure: 0,
          },
          closureRefusal: refusal,
        });
      },
      { secId, refusal: REFUSAL }
    );

    const dialog = page.getByTestId('closure-dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });
    await expect(dialog.getByTestId('closure-refusal')).toHaveText(REFUSAL);
    const create = dialog.getByTestId('closure-create');
    await expect(create).toBeDisabled();
    await create.click({ force: true });
    // Nothing was created: the project still has no loop closure.
    const closures = await page.evaluate(
      () => (window as any).__E2E_TEST__.projectStore.getState().project.loop_closures.length as number
    );
    expect(closures).toBe(0);
    await dialog.getByTestId('closure-cancel').click();
    await expect(dialog).toBeHidden();
  });

  test('the computed view says what it left out, and Show warnings opens the tab', async ({ page }) => {
    const secId = await sectionId(page);
    await page.evaluate((secId) => {
      (window as any).__MOCK_ADJUSTMENT_REPORT__ = {
        skipped_closures: [
          {
            closure_id: 2,
            station_a: { section_id: secId, station_index: 1 },
            station_b: { section_id: secId, station_index: 2 },
            shot_section_id: secId,
            shot_index: 1,
          },
        ],
        failures: [
          {
            anchor: { section_id: secId, station_index: 0 },
            section_ids: [secId],
            reason: 'loop closures tie two zero datums to the same point.',
          },
        ],
      };
      const { projectStore, uiStore } = (window as any).__E2E_TEST__;
      const project = projectStore.getState().project;
      const closure = (id: number, a: number, b: number) => ({
        id, station_a: { section_id: secId, station_index: a }, station_b: { section_id: secId, station_index: b },
        path_length: 20, misclosure_distance: 0.3, error_percentage: 1.5, horizontal_misclosure: 0.3,
        vertical_misclosure: 0, strategy: 'Batch', is_aligned: false,
      });
      projectStore.setState({ project: { ...project, loop_closures: [closure(1, 0, 4), closure(2, 1, 2)] } });
      uiStore.getState().setGeometryMode('closureAdjusted', 'e2e');
    }, secId);

    const notice = page.getByTestId('adjustment-notice');
    await expect(notice).toBeVisible({ timeout: 10000 });
    await expect(notice.getByTestId('adjustment-notice-skipped')).toHaveText(
      '1 loop closure skipped: it spans a single shot.'
    );
    await expect(notice.getByTestId('adjustment-notice-failed')).toHaveText(
      'Part of the survey could not be adjusted and is shown as measured.'
    );
    // Under the read-only pill in the notice column, left-aligned with it, not
    // squeezing it: the pill keeps its words on one line at this width.
    const pill = (await page.getByTestId('geometry-readonly-banner').boundingBox())!;
    const box = (await notice.boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(pill.y + pill.height - 1);
    expect(Math.abs(box.x - pill.x)).toBeLessThanOrEqual(1);
    expect(pill.height).toBeLessThan(40);

    await notice.getByTestId('adjustment-notice-show-warnings').click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().detailPanelTab as string))
      .toBe('warnings');

    // Back to the Raw Data View: the notice goes with the adjusted geometry.
    await page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().setGeometryMode('measured', 'e2e'));
    await expect(notice).toBeHidden();
  });

  test('a shift delete that removed a closure says so in one warning toast', async ({ page }) => {
    const message =
      'Removed loop closure 0:1 to 0:5: closing the gap left its two stations at the two ends of one shot.';
    await page.evaluate((message) => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_range_shift = {
        success: true,
        section_ids: [],
        warnings: [{ code: 'CLOSURE_REMOVED_ACROSS_ONE_SHOT', message, severity: 'warning' }],
      };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    }, message);

    const result = await page.evaluate(async () => {
      const { projectStore } = (window as any).__E2E_TEST__;
      const section = projectStore.getState().project.sections[0];
      return projectStore
        .getState()
        .deleteShotRangeShift(section.id, section.shots[1].id, section.shots[2].id) as Promise<{ success: boolean }>;
    });
    expect(result.success).toBe(true);
    const toast = page.getByTestId('toast').filter({ hasText: message });
    await expect(toast).toHaveCount(1);
  });
});
