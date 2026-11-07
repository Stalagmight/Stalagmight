import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';
import type { MockReply } from './mocks/ipc/types';

/**
 * E2E tests for the section split workflow.
 *
 * Tests the Split Section dialog which opens when a mid-section shot is deleted,
 * allowing the user to name the two resulting sections.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Main Line',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: 0, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -2, depth_out: -4 },
      { azimuth_in: 135, azimuth_out: 315, length: 8, depth_in: -4, depth_out: -6 },
      { azimuth_in: 180, azimuth_out: 0, length: 15, depth_in: -6, depth_out: -8 },
    ],
  },
];

/** Get section and shot IDs from the project store. */
async function getIds(page: Page): Promise<{ sectionId: number; shotId: number; sectionName: string }> {
  const ids = await page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    if (!e2e) return null;
    const state = e2e.projectStore.getState();
    const section = state.project?.sections?.[0];
    // Pick a mid-section shot (index 1) for splitting
    const shot = section?.shots?.[1];
    return {
      sectionId: section?.id ?? 0,
      shotId: shot?.id ?? 0,
      sectionName: section?.name ?? '',
    };
  });
  if (!ids) throw new Error('Could not retrieve section/shot IDs');
  return ids;
}

/** Open split-section dialog programmatically via UI store. */
async function openSplitDialog(page: Page, sectionId: number, shotId: number, sectionName: string): Promise<void> {
  await page.evaluate(
    ({ sectionId, shotId, sectionName }) => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      e2e.uiStore.getState().openDialog('split-section', { sectionId, shotId, sectionName });
    },
    { sectionId, shotId, sectionName },
  );
  await expect(page.getByTestId('split-section-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Section Split Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Split Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
  });

  test('dialog renders with correct default names', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    const nameA = page.getByTestId('split-name-a');
    const nameB = page.getByTestId('split-name-b');

    await expect(nameA).toHaveValue('Main Line-A');
    await expect(nameB).toHaveValue('Main Line-B');
  });

  test('both name inputs are pre-filled from section name', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    const nameA = page.getByTestId('split-name-a');
    const nameB = page.getByTestId('split-name-b');

    // Both should contain the section name as prefix
    await expect(nameA).toHaveValue(/Main Line/);
    await expect(nameB).toHaveValue(/Main Line/);
  });

  test('empty name A disables confirm button', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    const nameA = page.getByTestId('split-name-a');
    const confirmBtn = page.getByTestId('split-confirm');

    await nameA.fill('');
    await expect(confirmBtn).toBeDisabled();
  });

  test('empty name B disables confirm button', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    const nameB = page.getByTestId('split-name-b');
    const confirmBtn = page.getByTestId('split-confirm');

    await nameB.fill('');
    await expect(confirmBtn).toBeDisabled();
  });

  test('custom names can be entered', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    const nameA = page.getByTestId('split-name-a');
    const nameB = page.getByTestId('split-name-b');

    await nameA.fill('North Passage');
    await nameB.fill('South Passage');

    await expect(nameA).toHaveValue('North Passage');
    await expect(nameB).toHaveValue('South Passage');

    // Confirm should be enabled with valid names
    await expect(page.getByTestId('split-confirm')).toBeEnabled();
  });

  test('confirm triggers IPC and closes dialog', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);

    // Set up mock override for delete_shot_and_split. `section_id` is section B's (a
    // SectionCreated always names it); the dialog keeps A selected and never reads it.
    await page.evaluate((id) => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_and_split = {
        section_id: id + 1,
        warnings: [],
        remote_link: null,
      } satisfies MockReply<'delete_shot_and_split'>;
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    }, sectionId);

    await openSplitDialog(page, sectionId, shotId, sectionName);
    await page.getByTestId('split-confirm').click();

    // Dialog should close on success
    await expect(page.getByTestId('split-section-dialog')).toBeHidden({ timeout: 5000 });
  });

  test('IPC error shows error message in dialog', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);

    // Set up mock override to throw error
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_and_split = { __throw: 'Section has too few shots to split' };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openSplitDialog(page, sectionId, shotId, sectionName);
    await page.getByTestId('split-confirm').click();

    // Error should display in the dialog
    const dialog = page.getByTestId('split-section-dialog');
    await expect(dialog).toContainText('too few shots', { timeout: 5000 });
  });

  test('cancel closes without action', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    // Click Cancel
    const dialog = page.getByTestId('split-section-dialog');
    await dialog.getByRole('button', { name: /Cancel/i }).click();

    await expect(dialog).toBeHidden({ timeout: 3000 });
  });

  test('dialog renders Delete & Split button text', async ({ page }) => {
    const { sectionId, shotId, sectionName } = await getIds(page);
    await openSplitDialog(page, sectionId, shotId, sectionName);

    const confirmBtn = page.getByTestId('split-confirm');
    await expect(confirmBtn).toContainText('Delete & Split');
  });
});
