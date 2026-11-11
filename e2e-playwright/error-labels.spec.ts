import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
  waitForDrawnShots,
  getMapLabels,
  LABEL_KIND_SHOT_ERROR,
  LABEL_KIND_CLOSURE_ERROR,
  type MapLabel,
  type SectionSeed,
} from './helpers/app';

/**
 * The Errors labels (Display > Errors): one label per shot holding how far its
 * readings disagree, one per loop closure with its error, and - where labels
 * collide - the worst error is the one shown. Labels have no MapLibre features,
 * so the oracle is the label layer's own read-out (`__stalagmight_labels__`).
 *
 * The seed: A runs 10 m east with its azimuths 50° apart (score 2.5 at the 20°
 * default), B comes 1 m back west 6° apart (score 0.3), so their midpoints are
 * 4.5 m apart and the labels overlap until the map is zoomed far in; C agrees.
 */
const seeds: SectionSeed[] = [
  {
    name: 'Errors',
    direction: 'in',
    shots: [
      { azimuth_in: 65, azimuth_out: 115, length: 10, depth_in: -1, depth_out: -1 },
      { azimuth_in: 267, azimuth_out: 273, length: 1, depth_in: -1, depth_out: -1 },
      { azimuth_in: 0, azimuth_out: 0, length: 8, depth_in: -1, depth_out: -1 },
    ],
  },
];

interface MapHandle {
  getZoom: () => number;
  jumpTo: (o: { zoom: number; center?: unknown }) => void;
  getCenter: () => unknown;
}

async function setZoom(page: Page, zoom: number): Promise<void> {
  await page.evaluate((z) => {
    const map = (window as unknown as { __stalagmight_map__?: MapHandle }).__stalagmight_map__;
    map?.jumpTo({ zoom: z, center: map.getCenter() });
  }, zoom);
  await waitForMapIdle(page);
}

async function setErrorsOn(page: Page): Promise<void> {
  await page.evaluate(() => {
    const ui = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { showShotErrors: boolean; toggleDisplay: (k: string) => void } } };
    }).__E2E_TEST__?.uiStore;
    if (ui && !ui.getState().showShotErrors) ui.getState().toggleDisplay('showShotErrors');
  });
}

const errorLabels = async (page: Page, kind = LABEL_KIND_SHOT_ERROR): Promise<MapLabel[]> =>
  (await getMapLabels(page)).filter((l) => l.kind === kind);

test.describe('Errors labels', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Errors Labels Test');
    await seedProjectWithSections(page, seeds);
    await waitForMapIdle(page);
    await waitForDrawnShots(page, 3);
  });

  test('label each shot that disagrees, and only those', async ({ page }) => {
    await setErrorsOn(page);
    await setZoom(page, 18);
    await expect
      .poll(async () => (await errorLabels(page)).map((l) => l.text).sort())
      .toEqual(['Δaz 50.0°', 'Δaz 6.0°']);
  });

  test('where two labels collide the worse error is shown first', async ({ page }) => {
    // Section names are placed from z10 and keep their place, so take this
    // section's name out of the contest to compare the two Errors labels only.
    await page.evaluate(() => {
      const ui = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { toggleDisplay: (k: string) => void } } } })
        .__E2E_TEST__?.uiStore;
      ui?.getState().toggleDisplay('showSectionNames');
    });
    await setErrorsOn(page);
    await setZoom(page, 15);
    await expect.poll(async () => (await errorLabels(page)).some((l) => l.text === 'Δaz 50.0°')).toBe(true);
    const labels = await errorLabels(page);
    const strong = labels.find((l) => l.text === 'Δaz 50.0°')!;
    const weak = labels.find((l) => l.text === 'Δaz 6.0°');
    expect(strong.revealZoom).toBe(14);
    // The weaker label only appears once the two have room, or (past the
    // reachable zoom) not at all.
    if (weak) {
      expect(weak.revealZoom).toBeGreaterThan(strong.revealZoom);
      expect(strong.score).toBeGreaterThan(weak.score!);
    }
  });

  test('turning Errors off removes the labels; Shots off hides them', async ({ page }) => {
    await setErrorsOn(page);
    await setZoom(page, 18);
    await expect.poll(async () => (await errorLabels(page)).length).toBe(2);
    await page.evaluate(() => {
      const ui = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { toggleDisplay: (k: string) => void } } } })
        .__E2E_TEST__?.uiStore;
      ui?.getState().toggleDisplay('showShots');
    });
    await expect.poll(async () => (await errorLabels(page)).every((l) => !l.kindVisible)).toBe(true);
    await page.evaluate(() => {
      const ui = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { toggleDisplay: (k: string) => void } } } })
        .__E2E_TEST__?.uiStore;
      ui?.getState().toggleDisplay('showShots');
      ui?.getState().toggleDisplay('showShotErrors');
    });
    await expect.poll(async () => (await errorLabels(page)).length).toBe(0);
  });

  test('a loop closure gets its error percentage and misclosure', async ({ page }) => {
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => { project: { sections: Array<{ id: number }>; loop_closures: unknown[] } | null };
            setState: (s: unknown) => void;
          };
        };
      }).__E2E_TEST__!;
      const project = e2e.projectStore.getState().project!;
      const sid = project.sections[0]!.id;
      e2e.projectStore.setState({
        project: {
          ...project,
          loop_closures: [
            {
              id: BigInt(1),
              station_a: { section_id: sid, station_index: 0 },
              station_b: { section_id: sid, station_index: 3 },
              path_length: 19,
              misclosure_distance: 0.42,
              error_percentage: 2.21,
              horizontal_misclosure: 0.42,
              vertical_misclosure: 0,
              strategy: 'Batch',
              is_aligned: false,
            },
          ],
        },
      });
    });
    await setErrorsOn(page);
    await setZoom(page, 18);
    await expect
      .poll(async () => (await errorLabels(page, LABEL_KIND_CLOSURE_ERROR)).map((l) => l.text))
      .toEqual(['2.2% 0.4m']);
  });
});
