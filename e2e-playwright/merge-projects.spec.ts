import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';
import type { MockReply } from './mocks/ipc/types';

/**
 * E2E tests for the Merge Projects dialog.
 *
 * Tests the merge dialog which combines multiple projects into one.
 * The dialog is opened via uiStore.openDialog('merge-projects', { selectedProjects }).
 *
 * Mock recent projects provide the selectedProjects data:
 *   Alpha:  5 sections, 1250.5m
 *   Beta:   3 sections, 820.0m
 *   Gamma:  8 sections, 2100.3m
 */

const MOCK_PROJECTS = [
  {
    name: 'Cave System Alpha',
    location: 'Yucatan, Mexico',
    file_path: '/projects/cave-alpha.stalx',
    modified_at: new Date().toISOString(),
    section_count: 5,
    total_length: 1250.5,
    gps_center: [-87.4631, 20.2137],
    gps_bounds: null,
  },
  {
    name: 'Underwater Cave Beta',
    location: 'Florida, USA',
    file_path: '/projects/cave-beta.stalx',
    modified_at: new Date(Date.now() - 86400000).toISOString(),
    section_count: 3,
    total_length: 820.0,
    gps_center: [-82.4584, 29.6516],
    gps_bounds: null,
  },
  {
    name: 'Deep Cave Gamma',
    location: 'Bahamas',
    file_path: '/projects/cave-gamma.stalx',
    modified_at: new Date(Date.now() - 172800000).toISOString(),
    section_count: 8,
    total_length: 2100.3,
    gps_center: [-77.3963, 25.0343],
    gps_bounds: null,
  },
];

/** Open merge-projects dialog with the given projects. */
async function openMergeDialog(page: Page, projects = MOCK_PROJECTS): Promise<void> {
  await page.evaluate(
    (selectedProjects) => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      e2e.uiStore.getState().openDialog('merge-projects', { selectedProjects });
    },
    projects,
  );
  await expect(page.getByTestId('merge-projects-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Merge Projects Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('dialog renders with selected project list @smoke', async ({ page }) => {
    await openMergeDialog(page);

    const dialog = page.getByTestId('merge-projects-dialog');
    await expect(dialog).toContainText('Cave System Alpha');
    await expect(dialog).toContainText('Underwater Cave Beta');
    await expect(dialog).toContainText('Deep Cave Gamma');
  });

  test('default output name is first project name with _merged suffix', async ({ page }) => {
    await openMergeDialog(page);

    const nameInput = page.getByTestId('merge-output-name-input');
    await expect(nameInput).toHaveValue('Cave System Alpha_merged');
  });

  test('projects list shows section counts and lengths', async ({ page }) => {
    await openMergeDialog(page);

    const dialog = page.getByTestId('merge-projects-dialog');
    // Each project should show its section count
    await expect(dialog).toContainText('5 sections');
    await expect(dialog).toContainText('3 sections');
    await expect(dialog).toContainText('8 sections');
  });

  test('total sections and length are summed correctly', async ({ page }) => {
    await openMergeDialog(page);

    const dialog = page.getByTestId('merge-projects-dialog');
    // Total sections: 5 + 3 + 8 = 16
    await expect(dialog).toContainText('16');
    // Total length: 1250.5 + 820 + 2100.3 = 4170.8m = ~4.17 km
    // Should show as km since total > 1000m
    await expect(dialog).toContainText('km');
  });

  test('empty output name disables confirm button', async ({ page }) => {
    await openMergeDialog(page);

    const nameInput = page.getByTestId('merge-output-name-input');
    const confirmBtn = page.getByTestId('merge-projects-confirm-button');

    await nameInput.fill('');
    await expect(confirmBtn).toBeDisabled();
  });

  test('archive originals checkbox is toggleable', async ({ page }) => {
    await openMergeDialog(page);

    const checkbox = page.getByTestId('merge-archive-checkbox');
    // Default: unchecked
    await expect(checkbox).not.toBeChecked();

    await checkbox.check();
    await expect(checkbox).toBeChecked();

    await checkbox.uncheck();
    await expect(checkbox).not.toBeChecked();
  });

  test('open after merge checkbox defaults to checked', async ({ page }) => {
    await openMergeDialog(page);

    const checkbox = page.getByTestId('merge-open-checkbox');
    await expect(checkbox).toBeChecked();
  });

  test('confirm button calls merge IPC and closes dialog @smoke', async ({ page }) => {
    // Set up mock override for merge_projects
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.merge_projects = { warnings: [], source_repairs: [] } satisfies MockReply<'merge_projects'>;
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openMergeDialog(page);

    // Uncheck "open after merge" to avoid project load flow
    await page.getByTestId('merge-open-checkbox').uncheck();

    await page.getByTestId('merge-projects-confirm-button').click();

    // Dialog should close on success
    await expect(page.getByTestId('merge-projects-dialog')).toBeHidden({ timeout: 10000 });
  });

  // [#576 W57-NF2] The merge reads each source as an open does; what that read reset
  // or dropped is not in the merged file, so the load-repairs notice names the source.
  test('the load-repairs notice names what reading a source dropped', async ({ page }) => {
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.merge_projects = {
        warnings: [],
        source_repairs: [
          {
            path: '/projects/cave-beta.stalx',
            name: 'Underwater Cave Beta',
            repairs: { shots: 0, section_offsets: 0, station_offsets: 0, gps_points: 0, station_comments: 1 },
          },
        ],
      } satisfies MockReply<'merge_projects'>;
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openMergeDialog(page);
    await page.getByTestId('merge-projects-confirm-button').click();
    await expect(page.getByTestId('merge-projects-dialog')).toBeHidden({ timeout: 10000 });

    const source = page.getByTestId('load-repairs-merge-source-cave-beta.stalx');
    await expect(source).toHaveCount(1, { timeout: 10000 });
    await expect(source).toHaveText(
      'Some stored values in Underwater Cave Beta (cave-beta.stalx) could not be used and were ' +
        'changed in the merged project: 1 station comment past the end of its section dropped.',
    );
    await expect(page.getByTestId('load-repairs-banner')).toContainText('The source files are unchanged.');
    await page.getByTestId('load-repairs-dismiss-button').click();
    await expect(page.getByTestId('load-repairs-banner')).toBeHidden();
  });

  test('IPC error shows error in dialog', async ({ page }) => {
    // Override merge_projects to throw
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.merge_projects = { __throw: 'Disk space insufficient for merge' };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openMergeDialog(page);
    await page.getByTestId('merge-projects-confirm-button').click();

    const dialog = page.getByTestId('merge-projects-dialog');
    await expect(dialog).toContainText('Disk space insufficient', { timeout: 5000 });
  });

  test('cancel closes without action', async ({ page }) => {
    await openMergeDialog(page);

    const dialog = page.getByTestId('merge-projects-dialog');
    await dialog.getByRole('button', { name: /Cancel/i }).click();

    await expect(dialog).toBeHidden({ timeout: 3000 });
  });
});
