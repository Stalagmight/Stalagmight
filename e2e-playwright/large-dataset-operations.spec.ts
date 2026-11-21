import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  waitForActiveTool,
  waitForMapIdle,
} from './helpers/app';

test.describe('Large Dataset Operations', () => {
  test('keeps core interactions responsive with a large seeded project', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Large Dataset Scenario');

    await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: Record<string, unknown> | null;
              refreshProjectState: () => Promise<void>;
            };
            setState: (partial: Record<string, unknown>) => void;
          };
        };
        __E2E_MOCKS__?: {
          setCurrentProject: (project: unknown) => void;
        };
      }).__E2E_TEST__;

      const mocks = (window as unknown as {
        __E2E_MOCKS__?: {
          setCurrentProject: (project: unknown) => void;
        };
      }).__E2E_MOCKS__;

      if (!e2e?.projectStore || !mocks?.setCurrentProject) {
        throw new Error('E2E project hooks unavailable');
      }

      const state = e2e.projectStore.getState();
      const baseProject = state.project;
      if (!baseProject) {
        throw new Error('Project must exist before seeding large dataset');
      }

      const sections: Array<Record<string, unknown>> = [];
      for (let sectionId = 1; sectionId <= 24; sectionId += 1) {
        const shots: Array<Record<string, unknown>> = [];
        for (let shotId = 1; shotId <= 40; shotId += 1) {
          shots.push({
            id: shotId,
            azimuth_in: (sectionId * 12 + shotId * 3) % 360,
            azimuth_out: (sectionId * 12 + shotId * 3) % 360,
            length: 3 + (shotId % 5),
            depth_in: -(shotId * 0.15),
            depth_out: -((shotId + 1) * 0.15),
            pitch_in: null,
            pitch_out: null,
            left: null,
            right: null,
            up: null,
            down: null,
            comment: null,
            kind: 'Normal',
            marker_index: null,
            temperature: null,
          });
        }

        sections.push({
          id: sectionId,
          name: `Dataset Section ${sectionId}`,
          surveyor: 'Load Tester',
          date: '2026-02-10',
          color: sectionId % 2 === 0 ? '#4a90d9' : '#22c55e',
          direction: 'in',
          shots,
        });
      }

      const nextProject = {
        ...baseProject,
        sections,
        station_merges: [],
        zero_datums: [
          {
            station_ref: { section_id: 1, station_index: 0 },
            gps: { latitude: 24.55, longitude: -81.78, depth_reference: 0 },
          },
        ],
        loop_closures: [],
        section_offsets: {},
      };

      mocks.setCurrentProject(nextProject);
      e2e.projectStore.setState({ project: nextProject });
      await e2e.projectStore.getState().refreshProjectState();
    });

    // The sections panel is virtualized (a 14k-section project must not mount a
    // <li> per section), so only the visible window of rows is in the DOM at
    // once. Assert the seed loaded all 24 sections via the store, not by DOM
    // node count, then reveal the last row before interacting with it.
    const seededSectionCount = await page.evaluate(() =>
      (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { project: { sections: unknown[] } | null } } };
      }).__E2E_TEST__.projectStore.getState().project?.sections.length ?? 0,
    );
    expect(seededSectionCount).toBe(24);

    await page.getByTestId('sections-list').evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await expect(page.getByTestId('section-row-24')).toBeVisible();
    await page.getByTestId('section-row-24').click();
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 10000 });

    // Merge / measure rather than zoom / pan: the navigate group is hidden by
    // default (#409), so Z and P are inert and would prove nothing here.
    await page.keyboard.press('m');
    await waitForActiveTool(page, 'merge');

    await page.keyboard.press('r');
    await waitForActiveTool(page, 'measure');

    await page.keyboard.press('f');
    await waitForMapIdle(page);

    // Ctrl+E enters the Export workspace on its image export and live preview.
    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('export-type-image')).toBeChecked();
    await expect(page.getByTestId('export-preview')).toBeVisible({ timeout: 10000 });
  });
});
