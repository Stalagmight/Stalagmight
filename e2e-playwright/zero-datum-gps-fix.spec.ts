/**
 * #741: a second zero datum on a station merge-connected to a piece that is
 * already placed. The Edit Station dialog no longer refuses it: it previews the
 * datum as a GPS fix (bends the Closures Computed View, leaves the Raw Data View
 * alone), shows the gap, and saves with the entered accuracy.
 */
import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithSections, waitForMapIdle, type SectionSeed } from './helpers/app';

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Entrance A',
    direction: 'in',
    shots: [
      { azimuth_in: 90, azimuth_out: 270, length: 20, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 20, depth_in: -3, depth_out: -4 },
    ],
  },
  {
    name: 'Entrance B',
    direction: 'in',
    shots: [{ azimuth_in: 180, azimuth_out: 0, length: 15, depth_in: -4, depth_out: -2 }],
  },
];

type StationRef = { section_id: number; station_index: number };
type E2EStores = {
  projectStore: {
    getState: () => {
      project: {
        sections: Array<{ id: number }>;
        zero_datums: Array<{ station_ref: StationRef; role: string; accuracy_m?: number }>;
      } | null;
      setZeroDatum: (ref: StationRef, gps: Record<string, number>) => Promise<unknown>;
      mergeStations: (a: StationRef, b: StationRef) => Promise<unknown>;
    };
  };
  uiStore: { getState: () => { openDialog: (name: string, data: Record<string, unknown>) => void } };
};

/** Place section A by a datum, and merge B's start onto A's end. */
async function seedPlacedConnectedSurvey(page: Page): Promise<{ a: number; b: number }> {
  return page.evaluate(async () => {
    const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
    const state = e2e.projectStore.getState();
    const [a, b] = state.project!.sections.map((s) => s.id) as [number, number];
    await state.setZeroDatum({ section_id: a, station_index: 0 }, { latitude: 20.5, longitude: -87.4, depth_reference: 0 });
    await state.mergeStations({ section_id: a, station_index: 2 }, { section_id: b, station_index: 0 });
    return { a, b };
  });
}

test.describe('Second zero datum in a connected survey (#741)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('previews a GPS fix with its gap and saves it with the accuracy', async ({ page }) => {
    await createProject(page, 'Two Entrances');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapIdle(page);
    const { b } = await seedPlacedConnectedSurvey(page);

    // The context menu's "Set Zero Datum" opens the dialog this way.
    await page.evaluate((sectionId) => {
      const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
      e2e.uiStore.getState().openDialog('edit-station', {
        stationRef: { section_id: sectionId, station_index: 1 },
        forceEnableZeroDatum: true,
        focusGpsInput: true,
      });
    }, b);

    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('station-datum-accuracy-input')).toHaveValue('0');
    await dialog.getByTestId('station-gps-input').fill('20.5004, -87.3996');

    await expect(dialog.getByTestId('station-datum-fix-notice')).toContainText(
      'This survey already has a zero datum.'
    );
    await expect(dialog.getByTestId('station-datum-gap')).toHaveText('GPS is 40 m NE of the surveyed position.');
    await expect(dialog.getByTestId('station-datum-gap-warning')).toHaveCount(0);

    await dialog.getByTestId('station-datum-accuracy-input').fill('3');
    await dialog.getByTestId('edit-station-save').click();
    await expect(dialog).toBeHidden();

    const saved = await page.evaluate(() => {
      const win = window as unknown as { __MOCK_SET_ZERO_DATUM_CALLS__?: Array<Record<string, unknown>> };
      return win.__MOCK_SET_ZERO_DATUM_CALLS__?.at(-1) ?? null;
    });
    expect(saved, 'the save reached set_zero_datum with the entered accuracy').toMatchObject({
      stationRef: { section_id: b, station_index: 1 },
      accuracyM: 3,
    });

    // The refreshed project carries both datums; the new one is a GPS fix.
    await expect
      .poll(() =>
        page.evaluate(() => {
          const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
          return e2e.projectStore.getState().project?.zero_datums.map((d) => d.role) ?? [];
        })
      )
      .toEqual(['Placement', 'GpsFix']);
  });

  test('warns, without blocking, when the gap is past the threshold', async ({ page }) => {
    await page.evaluate(() => {
      (window as unknown as Record<string, unknown>).__MOCK_DATUM_GAP__ = {
        gap_m: 60,
        bearing_deg: 200,
        survey_distance_m: 1400,
        threshold_m: 43,
        exceeds_threshold: true,
      };
    });
    await createProject(page, 'Two Entrances Far');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapIdle(page);
    const { b } = await seedPlacedConnectedSurvey(page);

    await page.evaluate((sectionId) => {
      const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
      e2e.uiStore.getState().openDialog('edit-station', {
        stationRef: { section_id: sectionId, station_index: 1 },
        forceEnableZeroDatum: true,
      });
    }, b);

    const dialog = page.getByTestId('edit-station-dialog');
    await dialog.getByTestId('station-gps-input').fill('20.4995, -87.4002');
    await expect(dialog.getByTestId('station-datum-gap')).toHaveText('GPS is 60 m S of the surveyed position.');
    await expect(dialog.getByTestId('station-datum-gap-warning')).toContainText('more than the 43 m expected');
    await expect(dialog.getByTestId('edit-station-save')).toBeEnabled();
    await dialog.getByTestId('edit-station-save').click();
    await expect(dialog).toBeHidden();
  });
});
