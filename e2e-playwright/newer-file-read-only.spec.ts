import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, waitForMapIdle } from './helpers/app';

/**
 * E2E for a project saved by a NEWER Stalagmight (#576 B6-D1).
 *
 * The backend opens such a file read-only - no migration, no undo purge, every edit
 * and every save refused, the file never written - and reports the load's verdict on
 * the binary envelope (`newer_schema_version`). The mock reports it when seeded with
 * __MOCK_SET_PROJECT_NEWER_SCHEMA__ and refuses `save_project` for that project, as the
 * backend does. This pins the UI half: the banner, the canonical edit gate, and a
 * refused save surfacing as a read-only notice rather than an error dialog.
 */

async function seedNewerSchema(page: Page, version: number | null): Promise<void> {
  await page.evaluate((v) => {
    (window as unknown as { __MOCK_SET_PROJECT_NEWER_SCHEMA__: (v: number | null) => void })
      .__MOCK_SET_PROJECT_NEWER_SCHEMA__(v);
  }, version);
}

/** Open a project through the real load path (load_project_full_binary). */
async function loadProject(page: Page, path: string): Promise<void> {
  await page.evaluate(async (p) => {
    await (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { loadProject: (p: string) => Promise<unknown> } } };
    }).__E2E_TEST__.projectStore.getState().loadProject(p);
  }, path);
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  await waitForMapIdle(page);
}

async function selectFirstShot(page: Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__: {
        projectStore: {
          getState: () => { project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> } };
        };
        uiStore: { getState: () => { setSelection: (s: unknown) => void } };
      };
    }).__E2E_TEST__;
    const section = e2e.projectStore.getState().project.sections[0]!; // the seeded section
    e2e.uiStore.getState().setSelection({ type: 'shot', sectionId: section.id, shotId: section.shots[0]!.id });
  });
}

test.describe('A project saved by a newer version', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('opens read-only with a banner that says why', async ({ page }) => {
    await seedNewerSchema(page, 19);
    await loadProject(page, '/mock/CaveA.stalx');

    const banner = page.getByTestId('newer-file-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('saved by a newer version of Stalagmight');
    await expect(banner).toContainText('Update Stalagmight to edit it');
    // An ongoing condition with its remedy outside the app: nothing to click.
    await expect(banner.getByRole('button')).toHaveCount(0);

    // The canonical edit gate is closed: the DetailPanel's edit verbs are disabled.
    await selectFirstShot(page);
    await expect(page.getByTestId('shot-delete-button')).toBeDisabled();
    await expect(page.getByTestId('shot-kind-cycle-button')).toBeDisabled();
  });

  test('a save is refused with a read-only notice, not an error dialog', async ({ page }) => {
    await seedNewerSchema(page, 19);
    await loadProject(page, '/mock/CaveA.stalx');

    await page.keyboard.press('Control+s');

    await expect(page.getByTestId('toast')).toContainText(
      'Not saved: this project was saved by a newer version of Stalagmight.'
    );
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);
    // The project stays as it was: nothing became dirty, nothing to autosave.
    const isDirty = await page.evaluate(
      () =>
        (window as unknown as {
          __E2E_TEST__: { projectStore: { getState: () => { isDirty: boolean } } };
        }).__E2E_TEST__.projectStore.getState().isDirty
    );
    expect(isDirty).toBe(false);
  });

  test('the next ordinary project is editable and has no banner', async ({ page }) => {
    await seedNewerSchema(page, 19);
    await loadProject(page, '/mock/CaveA.stalx');
    await expect(page.getByTestId('newer-file-banner')).toBeVisible();

    await seedNewerSchema(page, null);
    await loadProject(page, '/mock/CaveB.stalx');

    await expect(page.getByTestId('newer-file-banner')).toHaveCount(0);
    await selectFirstShot(page);
    await expect(page.getByTestId('shot-delete-button')).toBeEnabled();
  });
});
