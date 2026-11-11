import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * #674: a `.stalx` picked through the in-project Import door asks whether to
 * open it as a project or add its survey data to the open one. The mock's
 * `add_project_file` only records the path and appends a section, so this
 * covers the frontend routing and the receipt, not the merge itself (that is
 * pinned in the Rust `project_add_tests`).
 */

const PICKED = '/mock/other-cave.stalx';

/** Answer the next native file picker with `path` (the mock reads the override). */
async function setPickedPath(page: Page, path: string): Promise<void> {
  await page.evaluate((selectedPath) => {
    const win = window as unknown as Record<string, unknown>;
    const overrides = (win.__MOCK_IPC_OVERRIDES__ as Record<string, unknown> | undefined) ?? {};
    overrides['plugin:dialog|open'] = selectedPath;
    win.__MOCK_IPC_OVERRIDES__ = overrides;
  }, path);
}

function addCalls(page: Page): Promise<unknown> {
  return page.evaluate(
    () => (window as unknown as Record<string, unknown>).__MOCK_ADD_PROJECT_FILE_CALLS__ ?? [],
  );
}

function openProjectPath(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const state = (window as unknown as {
      __E2E_TEST__?: { projectStore: { getState: () => { project: { file_path: string | null } | null } } };
    }).__E2E_TEST__?.projectStore.getState();
    return state?.project?.file_path ?? null;
  });
}

test.describe('Add a .stalx to the open project (#674)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Host Cave');
    await setPickedPath(page, PICKED);
    await page.getByTestId('toolbar-import-button').click();
    await expect(page.getByTestId('add-or-open-project-dialog')).toBeVisible({ timeout: 10000 });
  });

  test('Add sends the picked path and surfaces the receipt @smoke', async ({ page }) => {
    await expect(page.getByTestId('add-or-open-project-add')).toContainText('Host Cave');
    await page.getByTestId('add-or-open-project-add').click();

    await expect.poll(() => addCalls(page)).toEqual([{ path: PICKED }]);
    // Warning-free receipt: the toast, not the modal. `.filter` because the
    // toast testid is per-toast and strict mode errors on a concurrent one.
    await expect(page.getByTestId('toast').filter({ hasText: 'Stalagmight import' })).toBeVisible({
      timeout: 10000,
    });
    await expect(page.getByTestId('add-or-open-project-dialog')).toBeHidden();
    expect(await openProjectPath(page)).not.toBe(PICKED);
  });

  test('"Open as project" takes the existing open flow and adds nothing', async ({ page }) => {
    await page.getByTestId('add-or-open-project-open').click();

    await expect(page.getByTestId('add-or-open-project-dialog')).toBeHidden();
    await expect.poll(() => openProjectPath(page), { timeout: 10000 }).toBe(PICKED);
    expect(await addCalls(page)).toEqual([]);
  });
});
