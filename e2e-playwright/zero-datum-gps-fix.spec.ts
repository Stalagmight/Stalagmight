/**
 * #741: a second zero datum on a station merge-connected to a piece that is
 * already placed. The Edit Station dialog no longer refuses it: it previews the
 * datum as a GPS fix (bends the Closures Computed View, leaves the Raw Data View
 * alone), shows the gap, and saves with the entered accuracy.
 */
import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForDrawnStations,
  waitForMapIdle,
  type LineSeed,
} from './helpers/app';

const lineSeeds: LineSeed[] = [
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

type StationRef = { line_id: number; station_index: number };
type E2EStores = {
  projectStore: {
    getState: () => {
      project: {
        lines: Array<{ id: number }>;
        zero_datums: Array<{ station_ref: StationRef; role: string; accuracy_m?: number }>;
      } | null;
      setZeroDatum: (ref: StationRef, gps: Record<string, number>) => Promise<unknown>;
      mergeStations: (a: StationRef, b: StationRef) => Promise<unknown>;
    };
  };
  uiStore: {
    getState: () => {
      openDialog: (name: string, data: Record<string, unknown>) => void;
      setSelection: (sel: unknown) => void;
      setGeometryMode: (mode: string, source: string) => void;
      setActiveTool: (tool: string) => void;
    };
  };
};

/** Place line A by a datum, and merge B's start onto A's end. */
async function seedPlacedConnectedSurvey(page: Page): Promise<{ a: number; b: number }> {
  return page.evaluate(async () => {
    const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
    const state = e2e.projectStore.getState();
    const [a, b] = state.project!.lines.map((s) => s.id) as [number, number];
    await state.setZeroDatum({ line_id: a, station_index: 0 }, { latitude: 20.5, longitude: -87.4, depth_reference: 0 });
    await state.mergeStations({ line_id: a, station_index: 2 }, { line_id: b, station_index: 0 });
    return { a, b };
  });
}

test.describe('Second zero datum in a connected survey (#741)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('previews a GPS fix with its gap and saves it with the accuracy', async ({ page }) => {
    await createProject(page, 'Two Entrances');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
    const { b } = await seedPlacedConnectedSurvey(page);

    // The context menu's "Set Zero Datum" opens the dialog this way.
    await page.evaluate((lineId) => {
      const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
      e2e.uiStore.getState().openDialog('edit-station', {
        stationRef: { line_id: lineId, station_index: 1 },
        forceEnableZeroDatum: true,
        focusGpsInput: true,
      });
    }, b);

    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();
    // The accuracy only weighs a GPS fix, so it shows once the preview says so.
    await expect(dialog.getByTestId('station-datum-accuracy-input')).toHaveCount(0);
    await dialog.getByTestId('station-gps-input').fill('20.5004, -87.3996');
    await expect(dialog.getByTestId('station-datum-accuracy-input')).toHaveValue('0');

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
      stationRef: { line_id: b, station_index: 1 },
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
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
    const { b } = await seedPlacedConnectedSurvey(page);

    await page.evaluate((lineId) => {
      const e2e = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__;
      e2e.uiStore.getState().openDialog('edit-station', {
        stationRef: { line_id: lineId, station_index: 1 },
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

  test('a GPS fix shows its role and gap in Details, and its own map marker', async ({ page }) => {
    await createProject(page, 'Fix In Details');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
    const { a, b } = await seedPlacedConnectedSurvey(page);
    await page.evaluate(async (lineId) => {
      await (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.projectStore.getState().setZeroDatum(
        { line_id: lineId, station_index: 1 },
        { latitude: 20.5004, longitude: -87.3996, depth_reference: 0 }
      );
    }, b);

    const select = (line_id: number, station_index: number) =>
      page.evaluate(
        (ref) => (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.uiStore.getState().setSelection({ type: 'station', stationRef: ref }),
        { line_id, station_index }
      );
    const detail = page.getByTestId('station-datum-detail');

    await select(b, 1);
    await expect(detail.getByTestId('station-datum-detail-role')).toContainText('GPS fix');
    await expect(detail.getByTestId('station-datum-detail-gap')).toHaveText(
      'GPS is 40 m NE of the surveyed position.'
    );
    await expect(detail.getByTestId('station-datum-detail-gps-reading')).toBeVisible();

    await select(a, 0);
    await expect(detail.getByTestId('station-datum-detail-role')).toContainText('Placement datum');
    await expect(detail.getByTestId('station-datum-detail-gap')).toHaveCount(0);

    // The marker kind follows the role: only the fix carries the GPS-fix flag
    // (STATION_FLAG_IS_GPS_FIX = 0b10) that draws it as a hollow ring.
    const fixFlags = () =>
      page.evaluate(() => {
        const data = (
          window as unknown as {
            __E2E_TEST__: { projectStore: { getState: () => { surveyData: { stationCount: number; stationFlags: Uint8Array; stationLineIds: Uint32Array; stationIndices: Uint32Array } | null } } };
          }
        ).__E2E_TEST__.projectStore.getState().surveyData;
        if (!data) return [];
        const out: string[] = [];
        for (let g = 0; g < data.stationCount; g++) {
          if ((data.stationFlags[g]! & 0b10) !== 0) out.push(`${data.stationLineIds[g]}:${data.stationIndices[g]}`);
        }
        return out;
      });
    await expect.poll(fixFlags).toEqual([`${b}:1`]);
  });

  test('the computed view names a far GPS fix it is bent to', async ({ page }) => {
    await createProject(page, 'Far Fix Notice');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
    const { b } = await seedPlacedConnectedSurvey(page);
    await page.evaluate(async (lineId) => {
      // The backend decides which fixes are far; the mock is told.
      (window as unknown as Record<string, unknown>).__MOCK_ADJUSTMENT_REPORT__ = {
        far_gps_fixes: [{ station: { line_id: lineId, station_index: 1 }, gap_m: 43.2, threshold_m: 25 }],
      };
      await (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.projectStore.getState().setZeroDatum(
        { line_id: lineId, station_index: 1 },
        { latitude: 20.5004, longitude: -87.3996, depth_reference: 0 }
      );
      (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.uiStore.getState().setGeometryMode('closureAdjusted', 'e2e');
    }, b);

    const notice = page.getByTestId('adjustment-notice');
    await expect(notice.getByTestId('adjustment-notice-far-gps-fix')).toHaveText(
      `Bent to a GPS fix at ${b}:1, 43 m from its surveyed position. Check it.`,
      { timeout: 10000 }
    );
    await page.evaluate(() => (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.uiStore.getState().setGeometryMode('measured', 'e2e'));
    await expect(notice).toBeHidden();
  });

  test('merging two placed pieces says which one moves and that its datum becomes a fix', async ({ page }) => {
    await createProject(page, 'Two Placed Pieces');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
    const { a, b } = await page.evaluate(async () => {
      const state = (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.projectStore.getState();
      const [a, b] = state.project!.lines.map((s) => s.id) as [number, number];
      await state.setZeroDatum({ line_id: a, station_index: 0 }, { latitude: 20.5, longitude: -87.4, depth_reference: 0 });
      await state.setZeroDatum({ line_id: b, station_index: 0 }, { latitude: 20.501, longitude: -87.4, depth_reference: 0 });
      return { a, b };
    });
    await page.evaluate((lineId) => {
      // What the backend's merge check answers for two placed pieces.
      (window as unknown as Record<string, unknown>).__MOCK_CONNECTIVITY_RESULT__ = {
        connected: false,
        preview: null,
        can_merge: true,
        reason: null,
        datum_shift: {
          moving_line_ids: [lineId],
          shift_m: 12.047,
          datum_station: { line_id: lineId, station_index: 0 },
        },
      };
    }, b);

    // The real merge tool: click one station of each piece on the map.
    await page.evaluate(() => (window as unknown as { __E2E_TEST__: E2EStores }).__E2E_TEST__.uiStore.getState().setActiveTool('merge'));
    const stations = await waitForDrawnStations(page, 5);
    const canvas = (await page.locator('canvas.maplibregl-canvas').boundingBox())!;
    const clickStation = async (lineId: number, stationIndex: number) => {
      const st = stations.find((s) => s.lineId === lineId && s.stationIndex === stationIndex)!;
      await page.mouse.click(canvas.x + st.x, canvas.y + st.y);
    };
    await clickStation(a, 2);
    await clickStation(b, 1);

    const dialog = page.getByTestId('merge-dialog');
    await expect(dialog).toBeVisible({ timeout: 10000 });
    await expect(dialog.getByTestId('merge-datum-shift')).toHaveText(
      `Entrance B is placed by its own GPS. It moves 12 m to meet the other piece, and its zero datum at ${b}:0 becomes a GPS fix.`
    );
  });
});
