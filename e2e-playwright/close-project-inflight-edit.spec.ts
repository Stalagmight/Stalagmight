import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';
import { getIpcCalls, recordIpcCalls } from './helpers/ipcRecorder';

/**
 * [#576 W84] Close Project waits for an edit still in flight before it decides
 * whether to ask about unsaved changes, as quitting does (W83, app-update.spec.ts).
 *
 * Leaving the grid for the menu is the blur that commits the cell being edited.
 * On a clean project that edit is still in flight when Close is clicked, so the
 * close read the clean flag and went ahead; the close itself waits for the edit,
 * so it landed on the project being closed, which then closed with it unsaved.
 */

/** The page-side handle `holdShotEdit` leaves for releasing the held edit. */
interface EditHoldWindow {
  __TAURI_INTERNALS__: { invoke: (cmd: string, args?: unknown, opts?: unknown) => Promise<unknown> };
  __E2E_HELD_EDIT__?: { release: () => void };
  __E2E_TEST__: {
    projectStore: {
      getState: () => { isDirty: boolean; project: { sections: Array<{ shots: Array<{ length: number }> }> } | null };
      setState: (s: unknown) => void;
    };
  };
}

/**
 * Hold the next `update_shot` until the spec releases it through
 * `window.__E2E_HELD_EDIT__`, then hand it to the mock as issued: a real edit's
 * IPC and the refresh after it take a moment (longer on a large project), and the
 * mock otherwise answers at once, which hides the window a close can fall into.
 */
async function holdShotEdit(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as EditHoldWindow;
    const original = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = (cmd, args, opts) => {
      if (cmd !== 'update_shot' || w.__E2E_HELD_EDIT__) return original(cmd, args, opts);
      return new Promise((resolve, reject) => {
        w.__E2E_HELD_EDIT__ = { release: () => void original(cmd, args, opts).then(resolve, reject) };
      });
    };
  });
}

test.describe('Close Project with an edit in flight', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Close Edit Test');
  });

  test('Close pressed while a grid cell edit is in flight prompts once that edit lands', async ({ page }) => {
    await seedProjectWithSections(page, [
      {
        name: 'Close Edit Section',
        shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0 }],
      },
    ]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible();
    // Saved: nothing for the guard to protect yet.
    await page.evaluate(() => {
      (window as unknown as EditHoldWindow).__E2E_TEST__.projectStore.setState({ isDirty: false });
    });
    await holdShotEdit(page);
    await recordIpcCalls(page, ['update_shot', 'close_project']);
    const count = async (cmd: string) => (await getIpcCalls(page, cmd)).length;
    const isDirty = () =>
      page.evaluate(() => (window as unknown as EditHoldWindow).__E2E_TEST__.projectStore.getState().isDirty);

    // Type a new length; the editor is still open when the menu is opened.
    await page.getByTestId('grid-cell-0-length').click();
    await page.keyboard.type('42');
    await expect(page.getByTestId('grid-cell-editor')).toHaveValue('42');
    await page.getByTestId('app-menu-trigger').click();
    await expect(page.getByTestId('app-menu')).toBeVisible();
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
    // The blur committed the cell: its edit is on its way, not answered yet.
    await expect.poll(() => count('update_shot')).toBe(1);
    await expect
      .poll(() => page.evaluate(() => (window as unknown as EditHoldWindow).__E2E_HELD_EDIT__ != null))
      .toBe(true);
    expect(await isDirty()).toBe(false);

    await page.getByTestId('back-button').click();
    await page.evaluate(() => (window as unknown as EditHoldWindow).__E2E_HELD_EDIT__?.release());

    // The close saw the edit: the prompt is up, and the project is still open with it.
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();
    expect(await count('close_project')).toBe(0);
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    expect(await isDirty()).toBe(true);
    expect(
      await page.evaluate(
        () => (window as unknown as EditHoldWindow).__E2E_TEST__.projectStore.getState().project?.sections[0]?.shots[0]?.length,
      ),
    ).toBe(42);
  });
});
