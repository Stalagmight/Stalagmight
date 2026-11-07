import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

/**
 * [#576 D34] Autosave must not write the open project while a "leave this project"
 * prompt is open, nor while "Discard Changes" is on its way to a new project.
 *
 * Before the fix the debounce armed by the last edit ran on under the Unsaved
 * Changes prompt, and after Discard it ran on while the user named the new
 * project - writing the edits the prompt said would be lost into the old file.
 * Backing out of New Project returns to the project with its edits, and autosave
 * re-arms (D34-D1).
 *
 * Time is the page's mocked clock (`page.clock`), never a real wait: the mocked
 * config's 30 s debounce is jumped over with `fastForward`.
 */

const SECTION: SectionSeed = {
  name: 'SectionA',
  shots: [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: 1, depth_out: 2 }],
};

type ProbeWindow = Window & {
  __saveProjectCalls__?: number;
  __TAURI_INTERNALS__: { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
  __E2E_TEST__: {
    projectStore: {
      getState: () => {
        isDirty: boolean;
        project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> };
        updateShot: (sectionId: number, shotId: number, updates: unknown) => Promise<unknown>;
      };
    };
  };
};

/** Count `save_project` IPC calls in the page, at the moment they are issued. */
async function countSaveProjectCalls(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as ProbeWindow;
    w.__saveProjectCalls__ = 0;
    const invoke = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      if (cmd === 'save_project') w.__saveProjectCalls__ = (w.__saveProjectCalls__ ?? 0) + 1;
      return invoke(cmd, args);
    };
  });
}

const saveProjectCalls = (page: Page) =>
  page.evaluate(() => (window as unknown as ProbeWindow).__saveProjectCalls__ ?? 0);

const isDirty = (page: Page) =>
  page.evaluate(() => (window as unknown as ProbeWindow).__E2E_TEST__.projectStore.getState().isDirty);

test.describe('Autosave and the leave-project prompts (#576 D34)', () => {
  test('sends no save_project while Unsaved Changes and the discard-to-new dialog are open; Cancel resumes', async ({ page }) => {
    await page.clock.install();
    await setupMockedPage(page);
    await createProject(page, 'CaveA');
    await seedProjectWithSections(page, [SECTION]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    // Start clean, then one edit arms the debounce.
    await page.keyboard.press('Control+s');
    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toContainText(/saved/i);
    await countSaveProjectCalls(page);
    await page.evaluate(async () => {
      const st = (window as unknown as ProbeWindow).__E2E_TEST__.projectStore.getState();
      const section = st.project.sections[0]!;
      await st.updateShot(section.id, section.shots[0]!.id, { length: 15.5 });
    });
    await expect.poll(() => isDirty(page)).toBe(true);
    await expect(saveStatus).toContainText(/auto-save in/i);

    // File > New over the dirty project: the prompt holds autosave.
    await page.keyboard.press('Control+n');
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();
    await page.clock.fastForward('01:00');
    await expect(saveStatus).toContainText(/unsaved changes/i);
    expect(await saveProjectCalls(page)).toBe(0);
    expect(await isDirty(page)).toBe(true);

    // Discard: the New Project dialog opens over the old project, still held.
    await page.getByTestId('unsaved-changes-discard').click();
    await expect(page.getByTestId('new-project-dialog')).toBeVisible();
    await page.getByTestId('new-project-name-input').fill('CaveB');
    await page.clock.fastForward('01:00');
    await expect(saveStatus).toContainText(/unsaved changes/i);
    expect(await saveProjectCalls(page)).toBe(0);
    expect(await isDirty(page)).toBe(true);

    // Cancel New Project: back on CaveA with its edits, and autosave re-arms.
    await page.getByTestId('new-project-cancel-button').click();
    await expect(page.getByTestId('new-project-dialog')).toBeHidden();
    await expect(saveStatus).toContainText(/auto-save in/i);
    await page.clock.fastForward('00:31');
    await expect.poll(() => saveProjectCalls(page)).toBe(1);
    await expect.poll(() => isDirty(page)).toBe(false);
  });
});
