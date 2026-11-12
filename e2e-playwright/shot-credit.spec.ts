/**
 * Per-shot survey credit (#707) in the shot Detail panel.
 *
 * A shot whose surveyor or date differs from its section's (a section combined
 * from two trips) shows who surveyed it; a shot crediting exactly what its
 * section does shows no extra row.
 */

import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
} from './helpers/app';

async function shotIds(page: Page): Promise<{ sectionId: number; ids: number[] }> {
  return page.evaluate(() => {
    const state = (window as unknown as {
      __E2E_TEST__: {
        projectStore: {
          getState: () => { project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> } };
        };
      };
    }).__E2E_TEST__.projectStore.getState();
    const section = state.project.sections[0]!;
    return { sectionId: section.id, ids: section.shots.map((s) => s.id) };
  });
}

async function selectShot(page: Page, sectionId: number, shotId: number): Promise<void> {
  await page.evaluate(
    ({ sectionId, shotId }) => {
      (window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => { setSelection: (sel: unknown) => void } } };
      }).__E2E_TEST__.uiStore.getState().setSelection({ type: 'shot', sectionId, shotId });
    },
    { sectionId, shotId },
  );
}

test.describe('Shot credit in the Detail panel', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Shot Credit');
    await seedProjectWithSections(page, [
      {
        name: 'Main',
        surveyor: 'Bob',
        date: '2026-03-02',
        shots: [
          { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: 1, depth_out: 2 },
          {
            azimuth_in: 95,
            azimuth_out: 275,
            length: 8,
            depth_in: 2,
            depth_out: 3,
            credit: { surveyor: 'Ann', date: '2026-04-11' },
          },
        ],
      },
    ]);
    await waitForMapIdle(page);
  });

  test('a shot with its own credit shows who surveyed it and when', async ({ page }) => {
    const { sectionId, ids } = await shotIds(page);
    await selectShot(page, sectionId, ids[1]!);

    const credit = page.getByTestId('shot-credit');
    await expect(credit).toBeVisible();
    await expect(credit).toContainText('Ann · 2026-04-11');
    await expect(credit).toContainText('(section: Bob)');
  });

  test('a shot crediting what its section does shows no credit row', async ({ page }) => {
    const { sectionId, ids } = await shotIds(page);
    await selectShot(page, sectionId, ids[0]!);

    // The shot's details rendered (its start-station link), so the missing
    // credit row is a real absence, not a panel that never drew.
    await expect(page.getByTestId(`station-${sectionId}-0-link`)).toBeVisible();
    await expect(page.getByTestId('shot-credit')).toHaveCount(0);
  });
});
