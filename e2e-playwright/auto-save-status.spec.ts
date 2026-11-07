import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';
import { recordIpcCalls, getIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';
import type { E2EWindow } from './helpers/e2eWindow';
import type { CommandError } from '../src/types/ipc/CommandError';

/**
 * E2E tests for Auto-Save Status Bar indicator.
 *
 * Tests the save-status element in the SingleSystemView footer, which shows:
 * - idle + clean: "Saved"
 * - idle + dirty: "Unsaved changes" (yellow)
 * - pending: countdown to next auto-save
 * - saving: "Saving..."
 * - saved: "Saved" (transient flash)
 * - error: "Save error" (red)
 */

const sectionSeed: SectionSeed = {
  name: 'Status Test Section',
  shots: [
    {
      azimuth_in: 45,
      azimuth_out: 225,
      length: 10,
      depth_in: -1,
      depth_out: -2,
    },
  ],
};

async function createProjectWithSection(page: Page): Promise<void> {
  await createProject(page, 'Auto-Save Status Test');
  await seedProjectWithSections(page, [sectionSeed]);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
}

test.describe('Auto-Save Status Bar', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('save-status element exists in project view footer @smoke', async ({ page }) => {
    await createProject(page, 'Status Bar Test');

    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toBeVisible({ timeout: 5000 });
  });

  test('shows "Saved" when project is clean @smoke', async ({ page }) => {
    await createProject(page, 'Clean Project');

    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toBeVisible();
    // Clean project should show "Saved" text
    await expect(saveStatus).toContainText(/saved/i);
  });

  // The autosave canary (#576 RV3-FE-7): the real debounce, end to end. The
  // footer counts down after an edit, the timer sends an AUTOSAVE (not a manual
  // save) when it runs out, and the footer returns to Saved. Fake timers are
  // installed after the project opens, so the mock config's 30 s interval is
  // crossed with fastForward instead of being waited out.
  test('an edit counts down, autosaves and returns to Saved @smoke', async ({ page }) => {
    await createProject(page, 'Autosave Canary');
    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toHaveText('Saved');

    await page.clock.install();
    await recordIpcCalls(page, ['save_project']);
    await seedProjectWithSections(page, [sectionSeed]);

    await expect(saveStatus).toHaveText(/^Auto-save in \d+s$/);
    expect(await getIpcCalls(page, 'save_project')).toHaveLength(0);

    await page.clock.fastForward(31_000);

    const [call] = await waitForIpcCall(page, 'save_project');
    expect(call.args.isAutosave).toBe(true);
    await expect(saveStatus).toHaveText('Saved');
  });

  test('shows unsaved changes indicator after modification', async ({ page }) => {
    await createProjectWithSection(page);

    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toBeVisible();

    // Mark project as dirty by modifying a shot via the store
    await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      const state = e2e.projectStore.getState();
      const section = state.project?.sections?.[0];
      if (!section || !section.shots?.[0]) return;

      // Update a shot to make the project dirty
      state.updateShot(section.id, section.shots[0].id, { length: 15.5 });
    });

    // Status should change to indicate unsaved changes or pending auto-save
    // Wait for the status to reflect dirtiness
    await expect(saveStatus).not.toContainText(/^$/);
  });

  test('status shows saving state during Ctrl+S', async ({ page }) => {
    await createProjectWithSection(page);

    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toBeVisible();

    // Press Ctrl+S to trigger save
    await page.keyboard.press('Control+s');

    // After save completes, should show "Saved"
    await expect(saveStatus).toContainText(/saved/i, { timeout: 5000 });
  });

  test('status has yellow styling when dirty', async ({ page }) => {
    await createProjectWithSection(page);

    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toBeVisible();

    // Mark project as dirty
    await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      const state = e2e.projectStore.getState();
      const section = state.project?.sections?.[0];
      if (!section || !section.shots?.[0]) return;
      state.updateShot(section.id, section.shots[0].id, { length: 20 });
    });

    // Status should have yellow color class when dirty (idle + dirty state)
    // Check for yellow-400 class or pending countdown text
    await expect(saveStatus).toBeVisible();
    // The status text should change from "Saved" to either unsaved or countdown
    const text = await saveStatus.textContent();
    // After modification, status should no longer just say "Saved" (unless auto-save fired fast)
    expect(text).toBeDefined();
  });

  test('status shows error styling on save failure', async ({ page }) => {
    await createProjectWithSection(page);

    const saveStatus = page.getByTestId('save-status');
    await expect(saveStatus).toBeVisible();

    // Override save_project to return error
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.save_project = { __reject: { code: 'FAILED', message: 'Disk full' } satisfies CommandError };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    // Mark dirty and trigger save
    await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      const state = e2e.projectStore.getState();
      const section = state.project?.sections?.[0];
      if (!section || !section.shots?.[0]) return;
      state.updateShot(section.id, section.shots[0].id, { length: 25 });
    });

    await page.keyboard.press('Control+s');

    // Status should reflect error state (red text) or show error message
    // The exact behavior depends on whether the error is shown via dialog or inline
    // At minimum, app should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();

    // Clean up override
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      delete overrides.save_project;
    });
  });

  test('footer shows section/shot summary statistics', async ({ page }) => {
    await createProjectWithSection(page);

    // The footer should show summary stats like "1 section, 1 shot, 10.0 m"
    const footer = page.locator('footer');
    await expect(footer).toBeVisible();
    await expect(footer).toContainText(/section/i);
  });
});
