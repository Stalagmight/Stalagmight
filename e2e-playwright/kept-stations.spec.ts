import { test, expect } from './fixtures/coverage';
import type { Page } from '@playwright/test';
import { setupMockedPage, createProject, seedProjectWithLines, waitForMapIdle, getMapLabels } from './helpers/app';
import type { LineSeed } from './helpers/app';

/**
 * Kept and archived stations in the four dialogs (plan 2026-11-23, RS 6 and
 * 7), through the mocked IPC: Merge Stations asks which station is kept, and
 * the other line keeps its own station of the kept name (a foreign slot or a
 * loop end counts for no name uniqueness, RS-FX5); Detach Line from Station
 * brings the archived station back and answers its collision; Create Loop
 * Closure keeps one end or both names; Remove Closure says what comes back;
 * the data table marks a foreign slot with its home line's dot.
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
      createLoopClosure: (
        a: Ref,
        b: Ref,
        strategy: string,
        answers?: unknown,
      ) => Promise<{ success: boolean; error: string | null }>;
      setZeroDatum: (r: Ref, gps: unknown, options: unknown) => Promise<unknown>;
      refreshProjectState: () => Promise<void>;
    };
  };
  uiStore: { getState: () => { openDialog: (name: string, data: unknown) => void; setSelection: (s: unknown) => void } };
  gasPlanStore: { getState: () => { setEndpoint: (r: Ref | null) => void } };
}

/** Create Loop Closure through the store, keeping `keep`, with no renames
 *  (a closure makes loop ends only, which repeat no name, RS-FX5). */
async function createClosure(page: Page, a: Ref, b: Ref, keep: number): Promise<{ success: boolean; error: string | null }> {
  return page.evaluate(
    ({ a, b, keep }) =>
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore
        .getState()
        .createLoopClosure(a, b, 'Batch', { keep, renames: [] }),
    { a, b, keep },
  );
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
  // RS-FX5, "dotted slots don't collide": Main / 3 kept at Side's start
  // shows 3 on Side with Main's dot, and Side's own 3 further on keeps its
  // name, with no letter and nothing to answer.
  test("merge keeping Main's station leaves Side its own station of that name", async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    // Side's second station is named 3: keeping Main / 3 shows 3 on Side too.
    await seedNames(page, [[sideId, 1, '3']]);
    await openMerge(page, ref(sideId, 0), ref(mainId, 3));

    // Main / 3 is first in line order: the default keep.
    await expect(page.getByTestId(`merge-keep-${mainId * 100_000 + 3}`)).toBeChecked();
    await expect(page.getByTestId('merge-hidden')).toHaveText('Side / 0 is archived.');
    await expect(page.getByTestId('merge-renamed-lines')).toHaveText('Side will show Main / 3.');
    // No collision rows: the dotted 3 repeats no name.
    await expect(page.getByTestId('merge-collisions')).toHaveCount(0);
    // RS-FX6, P2-6: the dialog says Side will list 3 twice, before the user
    // confirms (shot 01 said only "Side will show Main / 3").
    await expect(page.getByTestId(`merge-repeat-${sideId}-${sideId * 100_000 + 1}`)).toHaveText(
      `Side already has a station 3 (Station ID ${sideId * 100_000 + 1}); it keeps its name, and Side shows 3 twice, the junction with Main's dot.`,
    );

    await page.getByTestId('merge-confirm').click();
    await expect(page.getByTestId('merge-dialog')).toBeHidden();
    await expect.poll(() => nameAt(page, ref(sideId, 0))).toBe('3');
    await expect.poll(() => nameAt(page, ref(sideId, 1))).toBe('3');
    await expect.poll(() => nameAt(page, ref(mainId, 3))).toBe('3');

    // The data table shows both 3s on Side: the start with Main's dot and
    // Main's Station ID, Side's own 3 plain with its own.
    await page.evaluate((lineId) => {
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore
        .getState()
        .setSelection({ type: 'line', lineId });
    }, sideId);
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
    await page.getByTestId('data-table-view-stations').click();
    await expect(page.getByTestId('data-table-view-stations')).toHaveAttribute('aria-checked', 'true');
    // The gutter pills name both 3 (the dot says whose the first one is).
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-0`).locator('.sh-lpill-main')).toHaveText('3');
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-1`).locator('.sh-lpill-main')).toHaveText('3');
    // So does the Name column (hidden by default; the eye menu shows it).
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('data-table-columns-dropdown').getByTestId('col-vis-name').check();
    await page.keyboard.press('Escape');
    // RS-FX6, P2-5: the dotted one says whose it is, muted.
    await expect(page.getByTestId('grid-cell-0-name')).toHaveText('3 (Main)');
    await expect(page.getByTestId('grid-cell-1-name')).toHaveText('3');
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-0-home-dot`)).toBeVisible();
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-1-home-dot`)).toHaveCount(0);
    await expect(page.getByTestId('grid-cell-0-_stationId')).toHaveText(String(mainId * 100_000 + 3));
    await expect(page.getByTestId('grid-cell-1-_stationId')).toHaveText(String(sideId * 100_000 + 1));
  });

  test("merge keeping Side's station shows its 0 on Main beside Main's own 0", async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    await openMerge(page, ref(sideId, 0), ref(mainId, 3));
    // Keeping Side / 0 shows 0 on Main through a foreign slot, where Main / 0
    // shows its own 0: no collision.
    await page.getByTestId(`merge-keep-${sideId * 100_000}`).click();
    await expect(page.getByTestId('merge-hidden')).toHaveText('Main / 3 is archived.');
    await expect(page.getByTestId('merge-renamed-lines')).toHaveText('Main will show Side / 0.');
    await expect(page.getByTestId('merge-collisions')).toHaveCount(0);
    // RS-FX6, P2-6: Main's own 0 keeps its name beside the dotted 0.
    await expect(page.getByTestId(`merge-repeat-${mainId}-${mainId * 100_000}`)).toHaveText(
      `Main already has a station 0 (Station ID ${mainId * 100_000}); it keeps its name, and Main shows 0 twice, the junction with Side's dot.`,
    );

    await page.getByTestId('merge-confirm').click();
    await expect(page.getByTestId('merge-dialog')).toBeHidden();
    await expect.poll(() => nameAt(page, ref(mainId, 3))).toBe('0');
    await expect.poll(() => nameAt(page, ref(sideId, 0))).toBe('0');
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
    const row = page.getByTestId('unmerge-stations-collision-0');
    await expect(row).toContainText('On Side, 0 would name two stations');
    // RS-FX6, P1-1: the two answers rename two stations named 0; each says
    // which, by its role and Station ID (both read "Rename Side / 0 to 0a").
    await expect(page.getByTestId('unmerge-stations-collision-0-letter').locator('..')).toHaveText(
      `Rename the station that comes back (Side / 0, Station ID ${sideId * 100_000}) to 0a`,
    );
    await expect(page.getByTestId('unmerge-stations-collision-0-alternative').locator('..')).toHaveText(
      `Rename the 0 that is on Side now (Station ID ${sideId * 100_000 + 1}) to 0a`,
    );
    // RS-FX6, P2-2: the lead follows the answer: rule L's letter first, then
    // the station's own name once the other one takes the letter.
    await expect(page.getByTestId('unmerge-stations-naming')).toContainText(
      `Side comes off: it gets back 0a (Station ID ${sideId * 100_000})`,
    );
    // Rename the station that reused the name instead.
    await page.getByTestId('unmerge-stations-collision-0-alternative').click();
    await expect(page.getByTestId('unmerge-stations-naming')).toContainText(
      `Side comes off: it gets back 0 (Station ID ${sideId * 100_000})`,
    );
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(dialog).toBeHidden();

    await expect.poll(() => nameAt(page, ref(sideId, 0))).toBe('0');
    await expect.poll(() => nameAt(page, ref(sideId, 1))).toBe('0a');
    await expect.poll(() => nameAt(page, ref(mainId, 3))).toBe('3');
  });

  test('detach gives a junction merged away back whole, with its archived station inside it', async ({ page }) => {
    // RS-FX4: Side / 0 and Third / 0 are one station (Side's kept, Third's
    // archived in it); merging that junction into Main / 3 keeping Main's
    // archives Side / 0 with Third's station still inside it.
    await setupMockedPage(page);
    await createProject(page, 'Kept Stations');
    const third: LineSeed = { name: 'Third', shots: [{ azimuth_in: 180, azimuth_out: 180, length: 7, depth_in: 3, depth_out: 3 }] };
    await seedProjectWithLines(page, [main, side, third]);
    await waitForMapIdle(page);
    const [mainId, sideId, thirdId] = await page.evaluate(() => {
      const e2e = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__;
      return e2e.projectStore.getState().project!.lines.map((l) => l.id);
    });
    const merge = (a: Ref, b: Ref, keep: number) =>
      page.evaluate(
        async ({ a, b, keep }) => {
          const e2e = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__;
          await e2e.projectStore.getState().mergeStations(a, b, { keep, renames: [] });
        },
        { a, b, keep },
      );
    await merge(ref(thirdId!, 0), ref(sideId!, 0), sideId! * 100_000);
    await merge(ref(sideId!, 0), ref(mainId!, 3), mainId! * 100_000 + 3);
    await expect.poll(() => nameAt(page, ref(thirdId!, 0))).toBe('3');
    await page.evaluate((at) => {
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore
        .getState()
        .setSelection({ type: 'station', stationRef: at });
    }, ref(mainId!, 3));
    // "Archived here" lists Third's station inside Side's.
    await expect(page.getByTestId(`station-detail-record-${sideId! * 100_000}-inside`)).toContainText(
      `0 (Station ID ${thirdId! * 100_000}), Third's own station`,
    );

    // Detaching Third gives back Side / 0, on Side and Third together.
    await page.getByTestId(`detail-unmerge-${thirdId}-0`).click();
    const dialog = page.getByTestId('unmerge-stations-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('unmerge-stations-naming')).toHaveText(
      'Third comes off and gets back the station it had here before the merge.',
    );
    await expect(page.getByTestId('unmerge-stations-comes-back')).toContainText(
      `It gets back 0 (Station ID ${sideId! * 100_000}).`,
    );
    await expect(page.getByTestId('unmerge-stations-together')).toHaveText(
      'Side comes off with Third: they were one station before the merge.',
    );
    // RS-FX6, P3-12: Third's own station stays inside the one that comes
    // back, and a second Detach gives it back.
    await expect(page.getByTestId(`unmerge-stations-inside-${thirdId! * 100_000}`)).toHaveText(
      `Third / 0 (Station ID ${thirdId! * 100_000}) stays archived inside it: detach Third again to get it back.`,
    );
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(dialog).toBeHidden();

    await expect.poll(() => nameAt(page, ref(mainId!, 3))).toBe('3');
    await expect.poll(() => nameAt(page, ref(sideId!, 0))).toBe('0');
    // Third shows Side's station again: its own is still inside it.
    await expect.poll(() => nameAt(page, ref(thirdId!, 0))).toBe('0');
    const shownId = await page.evaluate(
      (at) =>
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore
          .getState()
          .stations.find((s) => s.station_ref.line_id === at.line_id && s.station_ref.station_index === at.station_index)
          ?.shown?.id,
      ref(thirdId!, 0),
    );
    expect(shownId).toBe(sideId! * 100_000);
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
      `Station ID #${mainId * 100_000 + 3}`,
    );
    // Side's own stations carry no dot.
    await expect(page.getByTestId(`data-table-station-pill-${sideId}-1-home-dot`)).toHaveCount(0);
  });

  // ui-1, ui-8: the remaining-junction picker. Main keeps a junction of three
  // lines, so detaching Main leaves Side and North together on one of their
  // own stations; the mock now applies the record `restKeep` picks.
  test('detaching the kept line: the pick shows on the lines that come off, and leaving the line clears it', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Kept Stations Rest');
    const north: LineSeed = {
      name: 'North',
      shots: [{ azimuth_in: 0, azimuth_out: 0, length: 7, depth_in: 3, depth_out: 3 }],
    };
    await seedProjectWithLines(page, [main, side, north]);
    await waitForMapIdle(page);
    const [mainId, sideId, northId] = await page.evaluate(() =>
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore.getState().project!.lines.map((l) => l.id),
    );
    await seedNames(page, [
      [sideId!, 0, 'S'],
      [northId!, 0, 'N'],
    ]);
    await page.evaluate(
      async ({ m, s, n, keep }) => {
        const store = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore.getState();
        await store.mergeStations(m, s, { keep, renames: [] });
        await store.mergeStations(m, n, { keep, renames: [] });
      },
      { m: ref(mainId!, 3), s: ref(sideId!, 0), n: ref(northId!, 0), keep: mainId! * 100_000 + 3 },
    );
    await page.evaluate(
      ({ at, others }) => {
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore
          .getState()
          .openDialog('unmerge-stations', { stationRef: at, mergedStations: others, chosenLine: at.line_id });
      },
      { at: ref(mainId!, 3), others: [ref(sideId!, 0), ref(northId!, 0)] },
    );
    const dialog = page.getByTestId('unmerge-stations-dialog');
    await expect(dialog).toBeVisible();
    const sideRecord = sideId! * 100_000;
    const northRecord = northId! * 100_000;
    await expect(page.getByTestId(`unmerge-stations-rest-${sideRecord}`)).toBeChecked();
    await expect(page.getByTestId('unmerge-stations-comes-back')).toContainText('S (Station ID');
    // RS-FX10: the picker sits above what it decides, under the sentence it
    // answers.
    const top = async (testId: string) => (await page.getByTestId(testId).boundingBox())?.y ?? Number.NaN;
    expect(await top('unmerge-stations-naming')).toBeLessThan(await top('unmerge-stations-rest'));
    expect(await top('unmerge-stations-rest')).toBeLessThan(await top('unmerge-stations-comes-back'));
    expect(await top('unmerge-stations-comes-back')).toBeLessThan(await top('unmerge-stations-results'));

    // Pick North's, leave the line and come back: the default is back on screen.
    await page.getByTestId(`unmerge-stations-rest-${northRecord}`).click();
    await expect(page.getByTestId('unmerge-stations-comes-back')).toContainText('N (Station ID');
    await page.getByTestId(`unmerge-station-option-${sideId}-0`).click();
    await page.getByTestId(`unmerge-station-option-${mainId}-3`).click();
    await expect(page.getByTestId(`unmerge-stations-rest-${sideRecord}`)).toBeChecked();
    await expect(page.getByTestId('unmerge-stations-comes-back')).toContainText('S (Station ID');

    // Pick North's for real: Side and North come off showing N.
    await page.getByTestId(`unmerge-stations-rest-${northRecord}`).click();
    await expect(page.getByTestId('unmerge-stations-comes-back')).toContainText('N (Station ID');
    await page.getByTestId('unmerge-stations-confirm').click();
    await expect(dialog).toBeHidden();
    await expect.poll(() => nameAt(page, ref(sideId!, 0))).toBe('N');
    await expect.poll(() => nameAt(page, ref(northId!, 0))).toBe('N');
    await expect.poll(() => nameAt(page, ref(mainId!, 3))).toBe('3');
  });

  // ui-6, ui-8: the mock models C1, so the note on rule K's refused end is
  // reachable. RS-FX5: a loop end counts for no name uniqueness, so a
  // closure never repeats a name.
  test("a closure whose rule K end may not be kept says why; a loop end repeats no name", async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    // Main / 1 - Side / 2 keeping Main / 1: Side / 2 is a loop end showing 1
    // with the loop mark, beside Side's own 1.
    expect((await createClosure(page, ref(mainId, 1), ref(sideId, 2), mainId * 100_000 + 1)).success).toBe(true);
    await expect.poll(() => nameAt(page, ref(sideId, 2))).toBe('1');
    await expect.poll(() => nameAt(page, ref(sideId, 1))).toBe('1');

    // Main / 2 - Side / 2: keeping Main / 2 (rule K, the first end in the
    // mock) would hide Side / 2 a second time (C1).
    await page.evaluate(
      ({ a, b }) => {
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore.getState().openDialog('closure', {
          stationA: a,
          stationB: b,
          preview: { path_length: 20, misclosure_distance: 0.4, error_percentage: 2, horizontal_misclosure: 0.3, vertical_misclosure: 0.2 },
        });
      },
      { a: ref(mainId, 2), b: ref(sideId, 2) },
    );
    await expect(page.getByTestId('closure-dialog')).toBeVisible();
    await expect(page.getByTestId('closure-keep-not-allowed')).toHaveText(
      'Main / 2 cannot be kept here: it would hide Side / 2, which another loop closure already shows as Main / 1.',
    );
    await expect(page.getByTestId('closure-keep-both')).toBeChecked();
    await expect(page.getByTestId('closure-keep-outcome')).toHaveText(
      'No name changes. Side / 2 stays shown as Main / 1 through another loop closure.',
    );
    await page.getByTestId('closure-cancel').click();
    await expect(page.getByTestId('closure-dialog')).toBeHidden();
  });

  // RS 6, RS-FX6 P3-10: a Plan pin on a loop end names the loop end's own
  // station, then the station it is shown as ("Main / 1 (loop end)" read as
  // if Main / 1 were the loop end).
  test('a Plan endpoint on a loop end names it and the station it is shown as', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    expect((await createClosure(page, ref(mainId, 1), ref(sideId, 2), mainId * 100_000 + 1)).success).toBe(true);
    await page.evaluate(
      (at) =>
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.projectStore
          .getState()
          .setZeroDatum(at, { latitude: 24.55, longitude: -81.78, depth_reference: 0 }, { fitToBounds: false }),
      ref(mainId, 0),
    );
    await page.getByTestId('mode-plan').click();
    const entrance = page.getByTestId('gas-start-entrance');
    if (await entrance.isVisible().catch(() => false)) await entrance.click();
    await page.evaluate((at) => {
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.gasPlanStore.getState().setEndpoint(at);
    }, ref(sideId, 2));
    await expect(page.getByTestId('gas-endpoint-readout')).toHaveText('Side / 2 (loop end, shown as Main / 1)');
  });
});

/**
 * RS-FX6, P3-18 (shot 52 of the RS capture): at 1024x640 the Detach dialog's
 * body scrolled with Cancel and Detach at its end, below the fold. The four
 * kept-station dialogs pin their action row to the bottom of the body, so
 * both buttons are on screen however long the body is.
 */
test.describe('Kept-station dialogs at 1024x640 (RS-FX6)', () => {
  test.use({ viewport: { width: 1024, height: 640 } });

  test('Merge, Detach, Create Loop Closure and Remove Closure keep their buttons in view', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    const inView = async (...ids: string[]) => {
      for (const id of ids) await expect(page.getByTestId(id)).toBeInViewport({ ratio: 1 });
    };

    await seedNames(page, [[sideId, 1, '3']]);
    await openMerge(page, ref(sideId, 0), ref(mainId, 3));
    await inView('merge-cancel', 'merge-confirm');
    await page.getByTestId('merge-confirm').click();
    await expect(page.getByTestId('merge-dialog')).toBeHidden();

    // Reuse Side's archived 0 so the Detach body carries a collision too.
    await seedNames(page, [[sideId, 2, '0']]);
    await page.evaluate((at) => {
      (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore
        .getState()
        .setSelection({ type: 'station', stationRef: at });
    }, ref(mainId, 3));
    await page.getByTestId(`detail-unmerge-${sideId}-0`).click();
    await expect(page.getByTestId('unmerge-stations-dialog')).toBeVisible();
    await expect(page.getByTestId('unmerge-stations-collision-0')).toBeVisible();
    await inView('unmerge-stations-cancel', 'unmerge-stations-confirm');
    // A shorter window makes the body scroll for sure: scrolled to its top,
    // it still shows the action row.
    await page.setViewportSize({ width: 1024, height: 420 });
    const body = page.getByTestId('unmerge-stations-dialog-body');
    await expect.poll(() => body.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
    await body.evaluate((el) => el.scrollTo(0, 0));
    await inView('unmerge-stations-cancel', 'unmerge-stations-confirm');
    await page.setViewportSize({ width: 1024, height: 640 });
    await page.getByTestId('unmerge-stations-cancel').click();
    await expect(page.getByTestId('unmerge-stations-dialog')).toBeHidden();

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
    await expect(page.getByTestId('closure-keep')).toBeVisible();
    await inView('closure-cancel', 'closure-create');
    await page.getByTestId('closure-create').click();
    await expect(page.getByTestId('closure-dialog')).toBeHidden();

    await page.evaluate(
      ({ anchor, other }) => {
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore.getState().openDialog('remove-closure', {
          anchorStation: anchor,
          closures: [{ closureId: 1n, stationRef: other }],
        });
      },
      { anchor: ref(mainId, 1), other: ref(sideId, 2) },
    );
    await expect(page.getByTestId('remove-closure-comes-back')).toBeVisible();
    await inView('remove-closure-cancel', 'remove-closure-confirm');
  });
});

/**
 * RS-FX8, N1 (shots 03, 10, 55 of the RS capture): after a merge, Side starts
 * on the junction Main / 3, and its line name sat on the junction's station
 * label: the two overlapped at every zoom, so one of them never showed (in
 * the capture the junction's label, here Side's name), and turning names off
 * did not bring a lost station label back. The name now sits on Side's first
 * own station, so both show, and a line-name toggle shows the station label
 * a name covers without a rebuild. The oracle is the label layer's read-out
 * (`__stalagmight_labels__`).
 */
test.describe('Map labels at a junction (RS-FX8)', () => {
  const STATION_LABEL = 1;
  const LINE_NAME = 2;
  type Ui = { getState: () => { toggleDisplay: (k: string) => void } };
  const toggle = (page: Page, key: string) =>
    page.evaluate((k) => {
      (window as unknown as { __E2E_TEST__: { uiStore: Ui } }).__E2E_TEST__.uiStore.getState().toggleDisplay(k);
    }, key);
  /** Whether a label of `kind` reading `text` is drawn at zoom `z`. */
  const drawn = async (page: Page, kind: number, text: string, z: number) =>
    (await getMapLabels(page)).some((l) => l.kind === kind && l.text === text && l.kindVisible && l.revealZoom <= z);

  test('the junction keeps its label beside the branch name, and a name toggle needs no rebuild', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    await page.evaluate(
      async ({ a, b, keep }) => {
        const e2e = (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__;
        await e2e.projectStore.getState().mergeStations(a, b, { keep, renames: [] });
      },
      { a: ref(sideId, 0), b: ref(mainId, 3), keep: mainId * 100_000 + 3 },
    );
    await toggle(page, 'showStationLabels');
    // Center the map on the junction at z18.
    await page.evaluate((lineId) => {
      const w = window as unknown as {
        __stalagmight_map__?: { jumpTo: (o: { zoom: number; center: unknown }) => void };
        __stalagmight_geometry__?: () => { stations: Array<{ lineId: number; stationIndex: number; lngLat: unknown }> };
      };
      const junction = w.__stalagmight_geometry__?.().stations.find((s) => s.lineId === lineId && s.stationIndex === 3);
      w.__stalagmight_map__?.jumpTo({ zoom: 18, center: junction?.lngLat });
    }, mainId);
    await waitForMapIdle(page);

    const junction = String(mainId * 100_000 + 3);
    const mainFirst = String(mainId * 100_000);
    await expect.poll(() => drawn(page, STATION_LABEL, junction, 18)).toBe(true);
    await expect.poll(() => drawn(page, LINE_NAME, 'Side', 18)).toBe(true);
    // Main's name covers Main / 0's label...
    expect(await drawn(page, STATION_LABEL, mainFirst, 18)).toBe(false);
    // ...which line names off brings back, and on again hides; the junction
    // stays labelled throughout.
    await toggle(page, 'showLineNames');
    await expect.poll(() => drawn(page, STATION_LABEL, mainFirst, 18)).toBe(true);
    expect(await drawn(page, STATION_LABEL, junction, 18)).toBe(true);
    await toggle(page, 'showLineNames');
    await expect.poll(() => drawn(page, STATION_LABEL, mainFirst, 18)).toBe(false);
    expect(await drawn(page, STATION_LABEL, junction, 18)).toBe(true);
  });
});

/**
 * RS-FX8, N2 (shot 54 of the RS capture): Remove Closure said "(2 comes
 * back)" and "Side / 2 comes back" while its default answer letters the
 * returning station, and the removal gave 2a. Both follow the chosen answer.
 */
test.describe('Remove Closure follows the collision answer (RS-FX8)', () => {
  test('says 2a with the default answer, 2 with the other, and removes as it said', async ({ page }) => {
    const [mainId, sideId] = await setUp(page);
    expect((await createClosure(page, ref(mainId, 1), ref(sideId, 2), mainId * 100_000 + 1)).success).toBe(true);
    // Side / 2 is hidden behind Main / 1: its name is free on Side, so reuse it.
    await seedNames(page, [[sideId, 1, '2']]);
    await page.evaluate(
      ({ anchor, other }) => {
        (window as unknown as { __E2E_TEST__: E2E }).__E2E_TEST__.uiStore.getState().openDialog('remove-closure', {
          anchorStation: anchor,
          closures: [{ closureId: 1n, stationRef: other }],
        });
      },
      { anchor: ref(mainId, 1), other: ref(sideId, 2) },
    );
    await expect(page.getByTestId('remove-closure-collision-0')).toBeVisible();
    await expect(page.getByTestId('remove-closure-collision-0-letter').locator('..')).toHaveText(
      `Rename the station that comes back (Side / 2, Station ID ${sideId * 100_000 + 2}) to 2a`,
    );
    await expect(page.getByTestId('remove-closure-option-1-back')).toHaveText('(2a comes back)');
    await expect(page.getByTestId('remove-closure-comes-back')).toContainText('Side / 2a comes back');

    await page.getByTestId('remove-closure-collision-0-alternative').click();
    await expect(page.getByTestId('remove-closure-option-1-back')).toHaveText('(2 comes back)');
    await expect(page.getByTestId('remove-closure-comes-back')).toContainText('Side / 2 comes back');
    await page.getByTestId('remove-closure-confirm').click();
    await expect(page.getByTestId('remove-closure-dialog')).toBeHidden();
    await expect.poll(() => nameAt(page, ref(sideId, 2))).toBe('2');
    await expect.poll(() => nameAt(page, ref(sideId, 1))).toBe('2a');
  });
});
