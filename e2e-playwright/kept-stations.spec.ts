import { test, expect } from './fixtures/coverage';
import type { Page } from '@playwright/test';
import { setupMockedPage, createProject, seedProjectWithLines, waitForMapIdle } from './helpers/app';
import type { LineSeed } from './helpers/app';

/**
 * Kept and archived stations in the four dialogs (plan 2026-11-23, RS 6 and
 * 7), through the mocked IPC: Merge Stations asks which station is kept and
 * answers a name collision; Detach Line from Station brings the archived
 * station back and answers its collision; Create Loop Closure keeps one end
 * or both names; Remove Closure says what comes back; the data table marks a
 * foreign slot with its home line's dot.
 *
 * The mock keeps a junction's first slot in line order unless a merge names
 * `keep`, gives each slot the Station ID `lineId * 100000 + index`, and names
 * a station by its index unless renamed.
 */

const main: LineSeed = {
  name: 'Main',
  shots: [
    { azimuth_in: 0, azimuth_out: 0, length: 10, depth_in: 0, depth_out: 1 },
    { azimuth_in: 10, azimuth_out: 10, length: 8, depth_in: 1, depth_out: 2 },
    { azimuth_in: 20, azimuth_out: 20, length: 6, depth_in: 2, depth_out: 3 },
  ],
};
const side: LineSeed = {
  name: 'Side',
  shots: [
    { azimuth_in: 90, azimuth_out: 90, length: 5, depth_in: 3, depth_out: 3.5 },
    { azimuth_in: 100, azimuth_out: 100, length: 4, depth_in: 3.5, depth_out: 4 },
  ],
};

type Ref = { line_id: number; station_index: number };
const ref = (line_id: number, station_index: number): Ref => ({ line_id, station_index });

interface E2E {
  projectStore: {
    getState: () => {
      project: { lines: Array<{ id: number }>; loop_closures: Array<{ id: bigint; archived?: unknown }> } | null;
      stations: Array<{ station_ref: Ref; name: string; id: number; shown?: { id: number } }>;
      checkMergeConnectivity: (a: Ref, b: Ref) => Promise<{ merge_preview?: unknown }>;
      mergeStations: (a: Ref, b: Ref, answers?: unknown) => Promise<unknown>;
      refreshProjectState: () => Promise<void>;
    };
  };
  uiStore: { getState: () => { openDialog: (name: string, data: unknown) => void; setSelection: (s: unknown) => void } };
}

async function setUp(page: Page): Promise<[number, number]> {
  await setupMockedPage(page);
  await createProject(page, 'Kept Stations');
  await seedProjectWithLines(page, [main, side]);
  await waitForMapIdle(page);
  return page.evaluate(() => {
    const e2e = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__;
    const ids = e2e.projectStore.getState().project!.lines.map((l) => l.id);
    return [ids[0]!, ids[1]!] as [number, number];
  });
}

/** Rename slots through the mock's seed hook, then refresh. */
async function seedNames(page: Page, names: Array<[number, number, string]>): Promise<void> {
  await page.evaluate(async (list) => {
    const w = window as unknown as {
      __MOCK_SET_STATION_NAME__: (l: number, i: number, n: string | null) => void;
      __E2E_TEST__: E2E;
    };
    for (const [l, i, n] of list) w.__MOCK_SET_STATION_NAME__(l, i, n);
    await w.__E2E_TEST__.projectStore.getState().refreshProjectState();
  }, names);
}

/** Open Merge Stations as the merge tool does: the check, then the dialog with its preview. */
async function openMerge(page: Page, a: Ref, b: Ref): Promise<void> {
  await page.evaluate(
    async ({ a, b }) => {
      const e2e = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__;
      const result = await e2e.projectStore.getState().checkMergeConnectivity(a, b);
      e2e.uiStore.getState().openDialog('merge', { stationA: a, stationB: b, mergePreview: result.merge_preview });
    },
    { a, b },
  );
  await expect(page.getByTestId('merge-dialog')).toBeVisible();
}

/** The name the slot shows now. */
async function nameAt(page: Page, at: Ref): Promise<string | undefined> {
  return page.evaluate(
    (at) =>
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore
        .getState()
        .stations.find((s) => s.station_ref.line_id === at.line_id && s.station_ref.station_index === at.station_index)?.name,
    at,
  );
}

test.describe('Kept and archived stations (RS)', () => {
  test("merge keeping Main's station letters the colliding station on Side", async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    // Side's second station is named 3: keeping Main / 3 shows 3 on Side too.
    await seedNames(page, [[sideId, 1, '3']]);
    await openMerge(page, ref(sideId, 0), ref(mainId, 3));

    // Main / 3 is first in line order: the default keep.
    await expect(page.getByTestId(`merge-keep-${mainId * 100_000 + 3}`)).toBeChecked();
    await expect(page.getByTestId('merge-hidden')).toHaveText('Side / 0 is archived.');
    await expect(page.getByTestId('merge-renamed-lines')).toHaveText('Side will show Main / 3.');
    const row = page.getByTestId('merge-collision-0');
    await expect(row).toContainText('On Side, 3 would name two stations');
    await expect(page.getByTestId('merge-collision-0-letter')).toBeChecked();
    await expect(row).toContainText('Rename Side / 3 to 3a');

    await page.getByTestId('merge-confirm').click();
    await expect(page.getByTestId('merge-dialog')).toBeHidden();
    await expect.poll(() => nameAt(page, ref(sideId, 0))).toBe('3');
    await expect.poll(() => nameAt(page, ref(sideId, 1))).toBe('3a');
    await expect.poll(() => nameAt(page, ref(mainId, 3))).toBe('3');
  });

  test("merge keeping Side's station, renaming the kept station instead", async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    await openMerge(page, ref(sideId, 0), ref(mainId, 3));
    // Keeping Side / 0 shows 0 on Main, where Main / 0 already shows it.
    await page.getByTestId(`merge-keep-${sideId * 100_000}`).click();
    await expect(page.getByTestId('merge-hidden')).toHaveText('Main / 3 is archived.');
    await expect(page.getByTestId('merge-renamed-lines')).toHaveText('Main will show Side / 0.');
    await expect(page.getByTestId('merge-collision-0')).toContainText('On Main, 0 would name two stations');
    await page.getByTestId('merge-collision-0-alternative').click();

    await page.getByTestId('merge-confirm').click();
    await expect(page.getByTestId('merge-dialog')).toBeHidden();
    // The kept station took the letter; Main's own 0 kept its name.
    await expect.poll(() => nameAt(page, ref(mainId, 3))).toBe('0a');
    await expect.poll(() => nameAt(page, ref(sideId, 0))).toBe('0a');
    await expect.poll(() => nameAt(page, ref(mainId, 0))).toBe('0');
  });

  test('detach brings the archived station back and answers its collision', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    await page.evaluate(
      async ({ a, b, keep }) => {
        const e2e = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__;
        await e2e.projectStore.getState().mergeStations(a, b, { keep, renames: [] });
      },
      { a: ref(mainId, 3), b: ref(sideId, 0), keep: mainId * 100_000 + 3 },
    );
    // Side's own 0 is archived, so its name is free on Side: reuse it.
    await seedNames(page, [[sideId, 1, '0']]);
    await page.evaluate((at) => {
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore
        .getState()
        .setSelection({ type: 'station', stationRef: at });
    }, ref(mainId, 3));
    await expect(page.getByTestId('station-detail')).toBeVisible();

    // The "Also on" button asks first: Side's own station comes back.
    await page.getByTestId(`detail-unmerge-${sideId}-0`).click();
    const dialog = page.getByTestId('unmerge-stations-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('unmerge-stations-naming')).toContainText(
      `Side comes off: it gets back 0 (Station ID ${sideId * 100_000})`,
    );
    const row = page.getByTestId('unmerge-stations-collision-0');
    await expect(row).toContainText('On Side, 0 would name two stations');
    await expect(row).toContainText('Rename Side / 0 to 0a');
    // Rename the station that reused the name instead.
    await page.getByTestId('unmerge-stations-collision-0-alternative').click();
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(dialog).toBeHidden();

    await expect.poll(() => nameAt(page, ref(sideId, 0))).toBe('0');
    await expect.poll(() => nameAt(page, ref(sideId, 1))).toBe('0a');
    await expect.poll(() => nameAt(page, ref(mainId, 3))).toBe('3');
  });

  test('create a loop closure keeping one end, then keeping both names', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    const openClosure = async () => {
      await page.evaluate(
        ({ a, b }) => {
          (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore.getState().openDialog('closure', {
            stationA: a,
            stationB: b,
            preview: { path_length: 20, misclosure_distance: 0.4, error_percentage: 2, horizontal_misclosure: 0.3, vertical_misclosure: 0.2 },
          });
        },
        { a: ref(mainId, 1), b: ref(sideId, 2) },
      );
      await expect(page.getByTestId('closure-dialog')).toBeVisible();
      await expect(page.getByTestId('closure-keep')).toContainText('Both ends show:');
    };
    const closures = () =>
      page.evaluate(() =>
        ((window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore.getState().project?.loop_closures ?? []).map(
          (c) => ({ id: String(c.id), archived: c.archived !== undefined }),
        ),
      );

    await openClosure();
    await expect(page.getByTestId(`closure-keep-${mainId * 100_000 + 1}`)).toBeChecked();
    await expect(page.getByTestId('closure-hidden')).toContainText('Side / 2 is hidden');
    await page.getByTestId('closure-create').click();
    await expect(page.getByTestId('closure-dialog')).toBeHidden();
    await expect.poll(closures).toEqual([{ id: '1', archived: true }]);
    // The hidden end shows the kept end's name with the loop mark.
    await expect.poll(() => nameAt(page, ref(sideId, 2))).toBe('1');

    // The same pair again, keeping both names (the mock refuses a duplicate,
    // so remove the first one through its dialog first).
    await page.evaluate(
      ({ anchor, other }) => {
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore.getState().openDialog('remove-closure', {
          anchorStation: anchor,
          closures: [{ closureId: 1n, stationRef: other }],
        });
      },
      { anchor: ref(mainId, 1), other: ref(sideId, 2) },
    );
    await expect(page.getByTestId('remove-closure-option-1-back')).toHaveText('(2 comes back)');
    await expect(page.getByTestId('remove-closure-comes-back')).toContainText('Side / 2 comes back');
    await page.getByTestId('remove-closure-confirm').click();
    await expect(page.getByTestId('remove-closure-dialog')).toBeHidden();
    await expect.poll(closures).toEqual([]);
    await expect.poll(() => nameAt(page, ref(sideId, 2))).toBe('2');

    await openClosure();
    await page.getByTestId('closure-keep-both').click();
    await expect(page.getByTestId('closure-keep-outcome')).toHaveText('Each end keeps its own name.');
    await page.getByTestId('closure-create').click();
    await expect(page.getByTestId('closure-dialog')).toBeHidden();
    await expect.poll(closures).toEqual([{ id: '1', archived: false }]);
    await expect.poll(() => nameAt(page, ref(sideId, 2))).toBe('2');
  });

  test('the data table marks a foreign slot with its home line dot and shows the kept Station ID', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    await page.evaluate(
      async ({ a, b }) => {
        await (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore.getState().mergeStations(a, b);
      },
      { a: ref(mainId, 3), b: ref(sideId, 0) },
    );
    await page.evaluate((lineId) => {
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore
        .getState()
        .setSelection({ type: 'line', lineId });
    }, sideId);
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
    await page.getByTestId('data-table-view-stations').click();
    await expect(page.getByTestId('data-table-view-stations')).toHaveAttribute('aria-checked', 'true');
    // Side 0 shows Main / 3: its pill carries Main's dot and Main's Station ID.
    const pill = page.getByTestId(`data-table-station-pill-${sideId}-0`);
    await expect(pill).toBeVisible();
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-0-home-dot`)).toBeVisible();
    // (The superscript carries sr-only words before the number.)
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-0-station-id`)).toHaveText(
      `Station ID ${mainId * 100_000 + 3}`,
    );
    // Side's own stations carry no dot.
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-1-home-dot`)).toHaveCount(0);
  });
});
