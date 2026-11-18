/**
 * E2E for the per-project settings dialog (#57): General (name) + Validation
 * thresholds, stored in the .stalx. Opened from the project overview.
 */
import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithLines, waitForMapIdle, type LineSeed } from './helpers/app';

const lineSeeds: LineSeed[] = [
  {
    name: 'Main',
    direction: 'in',
    shots: [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 }],
  },
];

test.describe('Project Settings', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('edits per-project name + validation thresholds', async ({ page }) => {
    await createProject(page, 'Proj Settings');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    // Project overview (no selection) on the Details tab carries the button.
    await page.getByTestId('detail-tab-details').click();
    await page.getByTestId('open-project-settings').click();
    await expect(page.getByTestId('project-settings-dialog')).toBeVisible();

    // General tab shows the project name.
    await expect(page.getByTestId('project-settings-name')).toHaveValue('Proj Settings');

    // Validation tab exposes the same thresholds as the global settings.
    await page.getByTestId('project-settings-tab-validation').click();
    const minInput = page.getByTestId('input-shot-length-min');
    await expect(minInput).toBeVisible();
    await minInput.fill('2');

    await page.getByTestId('project-settings-save').click();
    await expect(page.getByTestId('project-settings-dialog')).toHaveCount(0);

    // The change is reflected in the project's per-project validation config.
    await expect
      .poll(async () =>
        page.evaluate(
          () =>
            (
              window as unknown as {
                __E2E_TEST__?: {
                  projectStore: {
                    getState: () => { project: { validation?: { warn_shot_length_min: number } } | null };
                  };
                };
              }
            ).__E2E_TEST__?.projectStore.getState().project?.validation?.warn_shot_length_min,
        ),
      )
      .toBe(2);
  });

  test('edits the entrance elevation (#465), including clearing it back to not recorded', async ({ page }) => {
    await createProject(page, 'Proj Elevation');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    await page.getByTestId('detail-tab-details').click();
    await page.getByTestId('open-project-settings').click();
    await expect(page.getByTestId('project-settings-dialog')).toBeVisible();

    // Not recorded by default: empty field, "Not recorded" placeholder.
    const elevationInput = page.getByTestId('project-settings-elevation');
    await expect(elevationInput).toHaveValue('');
    await expect(elevationInput).toHaveAttribute('placeholder', 'Not recorded');

    await elevationInput.fill('42.5');
    await page.getByTestId('project-settings-save').click();
    await expect(page.getByTestId('project-settings-dialog')).toHaveCount(0);

    await expect
      .poll(async () =>
        page.evaluate(
          () =>
            (
              window as unknown as {
                __E2E_TEST__?: {
                  projectStore: {
                    getState: () => { project: { surface_elevation?: number | null } | null };
                  };
                };
              }
            ).__E2E_TEST__?.projectStore.getState().project?.surface_elevation,
        ),
      )
      .toBe(42.5);

    // Reopen, clear it back to "not recorded".
    await page.getByTestId('open-project-settings').click();
    await expect(elevationInput).toHaveValue('42.5');
    await elevationInput.fill('');
    await page.getByTestId('project-settings-save').click();
    await expect(page.getByTestId('project-settings-dialog')).toHaveCount(0);

    await expect
      .poll(async () =>
        page.evaluate(
          () =>
            (
              window as unknown as {
                __E2E_TEST__?: {
                  projectStore: {
                    getState: () => { project: { surface_elevation?: number | null } | null };
                  };
                };
              }
            ).__E2E_TEST__?.projectStore.getState().project?.surface_elevation,
        ),
      )
      .toBe(null);
  });

  test('Cancel discards a staged name edit', async ({ page }) => {
    // Both surviving tabs stage their edits in local state and commit only inside
    // handleSave, so Cancel must leave the .stalx untouched. That contract had no
    // coverage left after the collab-toggle test (its only exercise of
    // project-settings-cancel) was removed with collaboration mode, and nothing
    // would catch a General input that started writing through on change.
    await createProject(page, 'Proj Settings');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    await page.getByTestId('detail-tab-details').click();
    await page.getByTestId('open-project-settings').click();
    await page.getByTestId('project-settings-name').fill('Renamed Away');
    await page.getByTestId('project-settings-cancel').click();
    await expect(page.getByTestId('project-settings-dialog')).toHaveCount(0);

    const name = async () =>
      page.evaluate(
        () =>
          (
            window as unknown as {
              __E2E_TEST__?: {
                projectStore: { getState: () => { project: { name: string } | null } };
              };
            }
          ).__E2E_TEST__?.projectStore.getState().project?.name,
      );
    await expect.poll(name).toBe('Proj Settings');

    // Reopening re-seeds from the project, so the discarded text is gone.
    await page.getByTestId('open-project-settings').click();
    await expect(page.getByTestId('project-settings-name')).toHaveValue('Proj Settings');
  });
});
