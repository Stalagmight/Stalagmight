import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';
import { clearIpcOverrideKeys, setIpcOverrides } from './helpers/ipcOverrides';
import type { E2EWindow } from './helpers/e2eWindow';
import type { IpcCommands } from '../src/types/ipc/IpcCommands';
import type { CommandError } from '../src/types/ipc/CommandError';

/**
 * E2E tests for the data table's draft "new shot" rows.
 *
 * The shots grid always shows one empty draft row at the end; the row-gutter
 * context menu inserts more drafts before/after existing shots. A draft
 * commits via create_shot once length, both depths and an azimuth are
 * present, honoring its anchor (afterShotId/beforeShotId). Inserted drafts
 * mark their missing required cells red immediately; the end draft only once
 * it holds a value. Delete (or the "Remove draft row" menu entry) discards a
 * draft without any confirmation; the end draft resets instead.
 *
 * Rows are grid-row-{rowIndex}, cells grid-cell-{rowIndex}-{colKey}; draft
 * rows carry the sg-row-draft class, missing cells sg-cell-missing. The mock
 * create_shot mutates the project (insert position honors the anchors) and
 * records its calls in __MOCK_CREATE_SHOT_CALLS__.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Draft Test Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

interface ShotSnapshot {
  id: number;
  length: number;
  azimuth_in: number;
  depth_in: number;
  depth_out: number;
}

async function getShots(page: Page): Promise<ShotSnapshot[]> {
  return page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    const shots = e2e?.projectStore.getState().project?.sections?.[0]?.shots ?? [];
    return shots.map((s) => ({
      id: s.id,
      length: s.length,
      azimuth_in: s.azimuth_in,
      depth_in: s.depth_in,
      depth_out: s.depth_out,
    }));
  });
}

async function getCreateShotCalls(
  page: Page,
): Promise<Array<{ sectionId: number; afterShotId: number | null; beforeShotId: number | null }>> {
  return page.evaluate(() => {
    // Each entry is the args the app invoked create_shot with.
    const calls = ((window as E2EWindow).__MOCK_CREATE_SHOT_CALLS__ ?? []) as Array<
      IpcCommands['create_shot']['args']
    >;
    return calls.map((c) => ({
      sectionId: c?.sectionId ?? null,
      afterShotId: c?.afterShotId ?? null,
      beforeShotId: c?.beforeShotId ?? null,
    }));
  });
}

/** Open the DataTable via keyboard shortcut. */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  // 3 shots + the end draft row (auto-selects first section)
  await expect(page.getByTestId('grid-row-3')).toHaveClass(/sg-row-draft/, { timeout: 5000 });
}

/** Double-click a cell, type a value, commit with Enter. */
async function fillCell(page: Page, row: number, colKey: string, value: string): Promise<void> {
  await page.getByTestId(`grid-cell-${row}-${colKey}`).dblclick();
  const editor = page.getByTestId('grid-cell-editor');
  await expect(editor).toBeVisible();
  await editor.fill(value);
  await page.keyboard.press('Enter');
  await expect(editor).toHaveCount(0);
}

/** Fill a draft row's required fields (length, azimuth avg, both depths). */
async function fillDraftRow(page: Page, row: number, length: string): Promise<void> {
  await fillCell(page, row, 'length', length);
  await fillCell(page, row, '_azimuth_avg', '100');
  await fillCell(page, row, 'depth_in', '1');
  await fillCell(page, row, 'depth_out', '2');
}

/** Right-click the gutter of a row and click a context-menu entry by name. */
async function clickGutterMenuEntry(page: Page, row: number, entry: string | RegExp): Promise<void> {
  await page.getByTestId(`grid-gutter-${row}`).click({ button: 'right' });
  const menu = page.getByTestId('grid-context-menu');
  await expect(menu).toBeVisible();
  await menu.getByRole('button', { name: entry }).click();
  await expect(menu).toHaveCount(0);
}

test.describe('Draft shot rows', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Draft Row Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
  });

  test('end draft commits a new shot after the last one', async ({ page }) => {
    // The empty end draft stays quiet: no missing-red before any value.
    await expect(page.getByTestId('grid-cell-3-length')).not.toHaveClass(/sg-cell-missing/);

    await fillCell(page, 3, 'length', '5');
    // Now the still-missing required cells surface.
    await expect(page.getByTestId('grid-cell-3-depth_in')).toHaveClass(/sg-cell-missing/);
    await fillCell(page, 3, '_azimuth_avg', '100');
    await fillCell(page, 3, 'depth_in', '1');
    await fillCell(page, 3, 'depth_out', '2');

    // The draft committed: 4 shots + a fresh, empty end draft below them.
    await expect(page.getByTestId('grid-row-4')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-cell-4-length')).toHaveText('');
    await expect(page.getByTestId('grid-cell-4-length')).not.toHaveClass(/sg-cell-missing/);

    const shots = await getShots(page);
    expect(shots.map((s) => s.length)).toEqual([10, 20, 30, 5]);
    expect(shots[3]).toMatchObject({ azimuth_in: 100, depth_in: 1, depth_out: 2 });

    // Committed AFTER the previous last shot.
    const calls = await getCreateShotCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ afterShotId: shots[2]!.id, beforeShotId: null });
  });

  // #576 NF-W3UI-1: the backend mints a uuid for every shot it creates, and uuid
  // anchors (plan pins, the distance origin) resolve through it; the mock's
  // create_shot used to leave the new shot without one. Read off the mock backend's
  // project: the store refreshes through the binary envelope, which carries no uuid
  // (#576 A9-D2).
  test('the committed shot carries a uuid of its own, as the backend mints one', async ({ page }) => {
    await fillDraftRow(page, 3, '5');
    await expect(page.getByTestId('grid-row-4')).toHaveClass(/sg-row-draft/);
    const uuids = await page.evaluate(() => {
      const shots = (window as E2EWindow).__E2E_MOCKS__!.getCurrentProject()!.sections[0]!.shots as Array<{
        uuid?: string | null;
      }>;
      return shots.map((s) => s.uuid ?? null);
    });
    expect(uuids).toHaveLength(4);
    expect(uuids[3]).toEqual(expect.any(String));
    expect(new Set(uuids).size).toBe(4);
  });

  test('insert above the first shot anchors before it and marks missing cells immediately', async ({ page }) => {
    await clickGutterMenuEntry(page, 0, 'Insert shot before');

    // A draft row appears at the top, red on every missing required cell
    // before anything is typed (unlike the lazy end draft).
    await expect(page.getByTestId('grid-row-0')).toHaveClass(/sg-row-draft/);
    for (const colKey of ['length', '_azimuth_avg', 'depth_in', 'depth_out']) {
      await expect(page.getByTestId(`grid-cell-0-${colKey}`)).toHaveClass(/sg-cell-missing/);
    }
    // The shots below shifted down; the end draft is still last.
    await expect(page.getByTestId('grid-row-4')).toHaveClass(/sg-row-draft/);

    const before = await getShots(page);
    await fillDraftRow(page, 0, '5');

    // Committed BEFORE the first shot: it is now at index 0.
    await expect(page.getByTestId('grid-row-0')).not.toHaveClass(/sg-row-draft/);
    expect((await getShots(page)).map((s) => s.length)).toEqual([5, 10, 20, 30]);
    const calls = await getCreateShotCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ afterShotId: null, beforeShotId: before[0]!.id });
  });

  test('insert below a shot anchors after it', async ({ page }) => {
    await clickGutterMenuEntry(page, 0, 'Insert shot after');
    await expect(page.getByTestId('grid-row-1')).toHaveClass(/sg-row-draft/);

    const before = await getShots(page);
    await fillDraftRow(page, 1, '5');

    await expect(page.getByTestId('grid-row-1')).not.toHaveClass(/sg-row-draft/);
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 5, 20, 30]);
    const calls = await getCreateShotCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ afterShotId: before[0]!.id, beforeShotId: null });
  });

  test('multiple inserted drafts leave shot edits on the correct shots', async ({ page }) => {
    // Gutter-select rows 0-1, insert 2 drafts above them.
    await page.getByTestId('grid-gutter-0').click();
    await page.getByTestId('grid-gutter-1').click({ modifiers: ['Shift'] });
    await clickGutterMenuEntry(page, 1, 'Insert 2 shots before');

    // Rows 0-1 are drafts; the shots now sit at rows 2-4, end draft at 5.
    await expect(page.getByTestId('grid-row-0')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-row-1')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-row-5')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-cell-2-length')).toHaveText('10.0');

    // Regression guard: editing the shot at grid row 3 (the SECOND shot) must
    // not land on a neighbour shifted by the drafts above it.
    await fillCell(page, 3, 'length', '99');
    await expect(page.getByTestId('grid-cell-3-length')).toHaveText('99.0');
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 99, 30]);
  });

  test('Delete discards a gutter-selected draft without confirmation', async ({ page }) => {
    await clickGutterMenuEntry(page, 0, 'Insert shot before');
    await expect(page.getByTestId('grid-row-0')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-row-4')).toBeVisible();

    // Gutter-select the draft row, then Delete: the draft vanishes, no dialog.
    await page.getByTestId('grid-gutter-0').click();
    await page.keyboard.press('Delete');
    await expect(page.getByTestId('grid-row-4')).toHaveCount(0);
    await expect(page.getByTestId('grid-row-0')).not.toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 20, 30]);
  });

  test('Delete on the end draft resets it instead of removing it', async ({ page }) => {
    await fillCell(page, 3, 'length', '5');
    await expect(page.getByTestId('grid-cell-3-depth_in')).toHaveClass(/sg-cell-missing/);

    await page.getByTestId('grid-gutter-3').click();
    await page.keyboard.press('Delete');

    // Still there, but emptied - the partial value and the red marks are gone.
    await expect(page.getByTestId('grid-row-3')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-cell-3-length')).toHaveText('');
    await expect(page.getByTestId('grid-cell-3-depth_in')).not.toHaveClass(/sg-cell-missing/);
    await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);
    expect(await getCreateShotCalls(page)).toHaveLength(0);
  });

  test('clearing a filled required draft cell drops it from the model (no stale commit)', async ({ page }) => {
    // Regression: filling then clearing a required draft cell used to leave the
    // value in the draft model - only the grid cell was shown as "missing" - so
    // completing the OTHER required cells auto-committed a shot with the value the
    // UI showed as blank. Clearing must drop the field so nothing resurrects it.
    await fillCell(page, 3, 'length', '5');
    await expect(page.getByTestId('grid-cell-3-length')).toHaveText('5.0');

    // Clear length (select + Delete): the field is DROPPED from the draft, not
    // hidden behind a stale value. Length was the only thing this draft held, so
    // it is back to being an untouched trailing draft - and an untouched draft
    // carries no missing marks, the same as before anything was typed.
    await page.getByTestId('grid-cell-3-length').click();
    await page.keyboard.press('Delete');
    await expect(page.getByTestId('grid-cell-3-length')).toHaveText('');
    await expect(page.getByTestId('grid-cell-3-length')).not.toHaveClass(/sg-cell-missing/);

    // Fill the remaining required cells. The draft must stay incomplete - without
    // the fix the stale 5 completes it and commits a shot here.
    await fillCell(page, 3, '_azimuth_avg', '100');
    await fillCell(page, 3, 'depth_in', '1');
    await fillCell(page, 3, 'depth_out', '2');

    // Re-enter a NEW length to complete and commit the draft. The created shot
    // must carry 7, not the cleared-then-stale 5 (the race-free discriminator:
    // with the bug, the commit already fired above with length 5 and this edit
    // only updates that shot).
    await fillCell(page, 3, 'length', '7');
    await expect(page.getByTestId('grid-row-4')).toHaveClass(/sg-row-draft/);

    const createdLengths = await page.evaluate(
      () =>
        (((window as unknown as Record<string, unknown>).__MOCK_CREATE_SHOT_CALLS__ as
          | Array<{ shot?: { length?: number } }>
          | undefined) ?? []).map((c) => c?.shot?.length ?? null),
    );
    expect(createdLengths).toEqual([7]);
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 20, 30, 7]);
  });

  test('"Remove draft row" context-menu entry discards the draft', async ({ page }) => {
    // The entry is only offered on draft rows.
    await page.getByTestId('grid-gutter-1').click({ button: 'right' });
    const menu = page.getByTestId('grid-context-menu');
    await expect(menu).toBeVisible();
    await expect(menu.getByRole('button', { name: 'Remove draft row' })).toHaveCount(0);
    await menu.getByRole('button', { name: 'Insert shot before' }).click();

    await expect(page.getByTestId('grid-row-1')).toHaveClass(/sg-row-draft/);
    await clickGutterMenuEntry(page, 1, 'Remove draft row');

    await expect(page.getByTestId('grid-row-1')).not.toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-row-4')).toHaveCount(0);
    await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 20, 30]);
  });

  test('a refused create_shot keeps the typed draft row, and editing it retries (DRAFT-1)', async ({ page }) => {
    await setIpcOverrides(page, {
      create_shot: { __reject: { code: 'FAILED', message: 'Simulated refusal' } satisfies CommandError },
    });
    await fillDraftRow(page, 3, '5');

    // The failure notice is the feedback; what was typed stays in the row.
    await expect(page.getByText('Simulated refusal').first()).toBeVisible();
    await expect(page.getByTestId('grid-row-3')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-cell-3-length')).toHaveText('5.0');
    await expect(page.getByTestId('grid-cell-3-depth_out')).toHaveText('2.0');
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 20, 30]);

    // Editing the row sends it again; this time the backend accepts it.
    await clearIpcOverrideKeys(page, ['create_shot']);
    await fillCell(page, 3, 'length', '6');
    await expect.poll(async () => (await getShots(page)).map((s) => s.length)).toEqual([10, 20, 30, 6]);
    await expect(page.getByTestId('grid-row-4')).toHaveClass(/sg-row-draft/);
    await expect(page.getByTestId('grid-cell-4-length')).toHaveText('');
  });

  test('a value typed while the new row is being saved reaches the created shot, once (#576 D39(f))', async ({
    page,
  }) => {
    // A slow create_shot, as on a large survey: the row stays editable while it is in flight.
    const CREATE_DELAY_MS = 1500;
    await page.evaluate((delay) => {
      type Invoke = (cmd: string, args?: unknown, opts?: unknown) => Promise<unknown>;
      const w = window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke }; __draftCalls__?: string[] };
      const calls: string[] = [];
      w.__draftCalls__ = calls;
      const original = w.__TAURI_INTERNALS__.invoke;
      w.__TAURI_INTERNALS__.invoke = async (cmd, args, opts) => {
        if (cmd === 'create_shot' || cmd === 'update_shot') calls.push(cmd);
        if (cmd === 'create_shot') await new Promise((resolve) => setTimeout(resolve, delay));
        return original(cmd, args, opts);
      };
    }, CREATE_DELAY_MS);

    await fillDraftRow(page, 3, '5'); // the last required field starts create_shot
    await fillCell(page, 3, 'left', '2.5'); // typed while it is on the wire

    const newShotLeft = () =>
      page.evaluate(() => {
        const shots = (window as E2EWindow).__E2E_TEST__.projectStore.getState().project!.sections[0]!.shots;
        return shots.length === 4 ? shots[3]!.left : `shots=${shots.length}`;
      });
    await expect.poll(newShotLeft, { timeout: 5000 }).toBe(2.5);
    await expect(page.getByTestId('grid-cell-3-left')).toHaveText('2.5');
    // settle: proves NO second create_shot of this row is sent - a poll would pass on its
    // first read. Long enough for a second create of the same row to have landed, had one been sent.
    await page.waitForTimeout(CREATE_DELAY_MS + 500);
    expect(await page.evaluate(() => (window as unknown as { __draftCalls__: string[] }).__draftCalls__)).toEqual([
      'create_shot',
      'update_shot',
    ]);
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 20, 30, 5]);
  });
});
