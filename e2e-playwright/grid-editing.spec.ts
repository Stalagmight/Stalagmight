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

/**
 * E2E tests for core SpreadsheetGrid editing in the shots view.
 *
 * Covers type-to-edit, rejected-draft handling (unparseable input is fixed in
 * place; only an emptied cell goes missing-red, then recovers),
 * the Delete clear/restore toggle on a required cell, the clipboard round trip
 * (display TSV out, external TSV in), cut-completes-on-paste as ONE
 * execute_batch (one undo unit), and Ctrl+D fill-down.
 *
 * Clipboard: the main suite grants Chromium clipboard-read/-write so the OS
 * clipboard path is exercised for real; a separate run stubs
 * navigator.clipboard to always reject (the WebKitGTK situation) to pin the
 * grid's internal copy-stash fallback - if that fallback regresses, in-grid
 * copy/paste dies on Linux and the test goes red.
 *
 * Cells are grid-cell-{rowIndex}-{colKey}; the model lives in the project
 * store, and multi-cell commits land in __MOCK_EXECUTE_BATCH_CALLS__ (one
 * entry per execute_batch call, each the snake_case commands array).
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Grid Edit Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1, left: 1.5, right: 2.5 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

interface ShotSnapshot {
  id: number;
  length: number;
  depth_in: number;
  depth_out: number;
  left: number | null;
  right: number | null;
}

async function getShots(page: Page): Promise<ShotSnapshot[]> {
  return page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    const shots = e2e?.projectStore.getState().project?.sections?.[0]?.shots ?? [];
    return shots.map((s) => ({
      id: s.id,
      length: s.length,
      depth_in: s.depth_in,
      depth_out: s.depth_out,
      left: s.left ?? null,
      right: s.right ?? null,
    }));
  });
}

type BatchCommand = { type: string; section_id: number; shot_id: number; updates: Record<string, unknown> };

async function getBatchCalls(page: Page): Promise<BatchCommand[][]> {
  return page.evaluate(() => ((window as E2EWindow).__MOCK_EXECUTE_BATCH_CALLS__ ?? []) as BatchCommand[][]);
}

/** Open the DataTable via keyboard shortcut. */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  // 3 shots + the end draft row (auto-selects first section)
  await expect(page.getByTestId('grid-row-3')).toHaveClass(/sg-row-draft/, { timeout: 5000 });
}

async function seedAndOpen(page: Page): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'Grid Editing Test');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await openDataTable(page);
}

test.describe('Grid editing', () => {
  test.beforeEach(async ({ context, page }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await seedAndOpen(page);
  });

  test('typing on a selected cell opens the editor seeded with the keystroke', async ({ page }) => {
    await page.getByTestId('grid-cell-1-length').click();
    await page.keyboard.press('4');

    // Type-to-edit: the first printable key opens the editor holding just it
    // (not appended to the existing "20").
    const editor = page.getByTestId('grid-cell-editor');
    await expect(editor).toBeVisible();
    await expect(editor).toHaveValue('4');

    await page.keyboard.type('2');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);

    await expect(page.getByTestId('grid-cell-1-length')).toHaveText('42.0');
    // Enter commits and advances down.
    await expect(page.getByTestId('grid-cell-2-length')).toHaveAttribute('data-active-cell', 'true');
    expect((await getShots(page)).map((s) => s.length)).toEqual([10, 42, 30]);
  });

  test('an unparseable draft is rejected in place instead of clearing the cell', async ({ page }) => {
    const cell = page.getByTestId('grid-cell-0-length');
    await cell.dblclick();
    const editor = page.getByTestId('grid-cell-editor');
    await editor.fill('abc');
    await page.keyboard.press('Enter');

    // A non-empty draft the column cannot parse is a typo to fix, NOT a request
    // to empty the cell: the editor stays open on the draft and the model is
    // untouched. Lumping this in with the clear let a host acting on the clear
    // signal write its unset sentinel over the measurement the user was only
    // mistyping a correction to.
    await expect(editor).toHaveValue('abc');
    await expect(cell).not.toHaveClass(/sg-cell-missing/);
    expect((await getShots(page))[0]!.length).toBe(10);

    // Tab must not walk the selection off the cell still being fixed.
    await page.keyboard.press('Tab');
    await expect(editor).toHaveValue('abc');
    await expect(cell).toHaveAttribute('data-active-cell', 'true');

    // Escape discards the bad draft outright: nothing committed, nothing marked,
    // and the committed value is still there once the editor is out of the way
    // (a cell renders empty only while its own editor is open).
    await page.keyboard.press('Escape');
    await expect(editor).toHaveCount(0);
    await expect(cell).not.toHaveClass(/sg-cell-missing/);
    await expect(cell).toHaveText('10.0');
    expect((await getShots(page))[0]!.length).toBe(10);

    // Fixing a rejected draft in place commits it and closes the editor.
    await cell.dblclick();
    await editor.fill('1o5');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveValue('1o5');
    await editor.fill('12.5');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);
    await expect(cell).toHaveText('12.5');
    expect((await getShots(page))[0]!.length).toBe(12.5);

    // Nullable columns take the same rule: garbage in an LRUD cell is rejected
    // in place, never silently discarded and never written as null.
    const lrud = page.getByTestId('grid-cell-0-left');
    await lrud.dblclick();
    await editor.fill('xyz');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveValue('xyz');
    await expect(lrud).not.toHaveClass(/sg-cell-missing/);
    expect((await getShots(page))[0]!.left).toBe(1.5);

    await page.keyboard.press('Escape');
    await expect(editor).toHaveCount(0);
  });

  // Kept separate from the rejection test above so a regression in one branch
  // cannot mask the other: emptying is the ONE draft that still clears.
  test('an emptied required cell goes missing-red and recovers on a valid commit', async ({ page }) => {
    const cell = page.getByTestId('grid-cell-0-length');
    const editor = page.getByTestId('grid-cell-editor');

    // An EMPTIED draft is a real clear: the cell goes red-empty while the model
    // keeps its value (a survey never stores 0 for "not measured").
    await cell.dblclick();
    await editor.fill('');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);
    await expect(cell).toHaveClass(/sg-cell-missing/);
    await expect(cell).toHaveText('');
    expect((await getShots(page))[0]!.length).toBe(10);

    // Re-edit: a missing cell seeds an EMPTY editor (not the hidden old value),
    // and a valid commit clears the mark.
    await cell.dblclick();
    await expect(editor).toHaveValue('');
    await editor.fill('7.5');
    await page.keyboard.press('Enter');
    await expect(cell).not.toHaveClass(/sg-cell-missing/);
    await expect(cell).toHaveText('7.5');
    expect((await getShots(page))[0]!.length).toBe(7.5);
  });

  test('Delete on a required cell toggles missing-red without touching the model', async ({ page }) => {
    const cell = page.getByTestId('grid-cell-0-length');
    await cell.click();
    await page.keyboard.press('Delete');

    // Required number: never write 0 - display empties, model keeps the value.
    await expect(cell).toHaveClass(/sg-cell-missing/);
    await expect(cell).toHaveText('');
    expect((await getShots(page))[0]!.length).toBe(10);

    // Delete again restores: the model still holds 10, so unmarking re-reveals it.
    await page.keyboard.press('Delete');
    await expect(cell).not.toHaveClass(/sg-cell-missing/);
    await expect(cell).toHaveText('10.0');
    expect((await getShots(page))[0]!.length).toBe(10);
    expect(await getBatchCalls(page)).toHaveLength(0);
  });

  test('copy writes display TSV to the OS clipboard; external TSV pastes as one batch', async ({ page }) => {
    await page.getByTestId('grid-cell-0-depth_in').click();
    await page.getByTestId('grid-cell-0-depth_out').click({ modifiers: ['Shift'] });
    await page.keyboard.press('Control+c');
    await expect(page.getByTestId('grid-copy-marquee')).toBeVisible();

    // The OS clipboard got the display-formatted TSV of the selection.
    await expect
      .poll(() => page.evaluate(() => navigator.clipboard.readText()))
      .toBe('0.0\t-1.0');

    // Paste EXTERNAL data (overwrite the clipboard so the internal stash can't
    // match): both cells must re-parse and land on the shot under the cursor.
    await page.evaluate(() => navigator.clipboard.writeText('-7.5\t-8.5'));
    await page.getByTestId('grid-cell-1-depth_in').click();
    await page.keyboard.press('Control+v');

    await expect(page.getByTestId('grid-cell-1-depth_in')).toHaveText('-7.5');
    await expect(page.getByTestId('grid-cell-1-depth_out')).toHaveText('-8.5');
    const shots = await getShots(page);
    expect(shots[1]).toMatchObject({ depth_in: -7.5, depth_out: -8.5 });

    // One execute_batch, one update_shot carrying both cells (one undo unit).
    const calls = await getBatchCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toHaveLength(1);
    expect(calls[0]![0]).toMatchObject({
      type: 'update_shot',
      shot_id: shots[1]!.id,
      updates: { depth_in: -7.5, depth_out: -8.5 },
    });
  });

  test('cut clears the source cells when pasted, all in a single batch', async ({ page }) => {
    await page.getByTestId('grid-cell-0-left').click();
    await page.getByTestId('grid-cell-0-right').click({ modifiers: ['Shift'] });
    await page.keyboard.press('Control+x');
    // A pending cut marks its source; nothing is cleared until the paste lands.
    await expect(page.getByTestId('grid-cut-marquee')).toBeVisible();
    expect((await getShots(page))[0]).toMatchObject({ left: 1.5, right: 2.5 });

    await page.getByTestId('grid-cell-1-left').click();
    await page.keyboard.press('Control+v');

    await expect(page.getByTestId('grid-cell-1-left')).toHaveText('1.5');
    await expect(page.getByTestId('grid-cell-1-right')).toHaveText('2.5');
    await expect(page.getByTestId('grid-cell-0-left')).toHaveText('');
    const shots = await getShots(page);
    expect(shots[0]).toMatchObject({ left: null, right: null });
    expect(shots[1]).toMatchObject({ left: 1.5, right: 2.5 });

    // Destination writes AND source clears in ONE execute_batch (one undo unit),
    // grouped into one update_shot per touched shot.
    const calls = await getBatchCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toHaveLength(2);
    const byShot = new Map(calls[0]!.map((c) => [c.shot_id, c]));
    expect(byShot.get(shots[1]!.id)).toMatchObject({ type: 'update_shot', updates: { left: 1.5, right: 2.5 } });
    expect(byShot.get(shots[0]!.id)).toMatchObject({ type: 'update_shot', updates: { left: null, right: null } });
  });

  test('keyboard navigation scrolls the active column into view horizontally', async ({ page }) => {
    // Narrow enough that the column set overflows the scroller, wide enough
    // that the 240px comment column still fits the cell area (over-wide
    // columns deliberately show their left edge instead).
    await page.setViewportSize({ width: 1100, height: 720 });
    await page.getByTestId('grid-cell-0-length').click();
    const scroller = page
      .locator('[data-testid="spreadsheet-grid"]')
      .locator('.overflow-auto')
      .first();
    await expect.poll(() => scroller.evaluate((el) => el.scrollLeft)).toBe(0);
    // End jumps to the rightmost column - the scroller must follow.
    await page.keyboard.press('End');
    await expect.poll(() => scroller.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
    // The active cell must sit inside the viewport, clear of the sticky gutter.
    const within = await page.evaluate(() => {
      const el = document.querySelector('[data-testid="spreadsheet-grid"] .overflow-auto')!;
      const cell = el.querySelector('[data-testid^="grid-cell-0-"][aria-selected="true"]');
      if (!cell) return 'no-active-cell';
      const c = cell.getBoundingClientRect();
      const s = el.getBoundingClientRect();
      const gutter = el.querySelector('.sg-gutter')!.getBoundingClientRect();
      return c.right <= s.right + 1 && c.left >= gutter.right - 1;
    });
    expect(within).toBe(true);
    // Home returns and the scroller follows back.
    await page.keyboard.press('Home');
    await expect.poll(() => scroller.evaluate((el) => el.scrollLeft)).toBe(0);
  });

  test('Ctrl+D fills the selection down as one batch', async ({ page }) => {
    await page.getByTestId('grid-cell-0-length').click();
    await page.getByTestId('grid-cell-2-length').click({ modifiers: ['Shift'] });
    await page.keyboard.press('Control+d');

    await expect(page.getByTestId('grid-cell-1-length')).toHaveText('10.0');
    await expect(page.getByTestId('grid-cell-2-length')).toHaveText('10.0');
    const shots = await getShots(page);
    expect(shots.map((s) => s.length)).toEqual([10, 10, 10]);

    const calls = await getBatchCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toHaveLength(2);
    const byShot = new Map(calls[0]!.map((c) => [c.shot_id, c]));
    expect(byShot.get(shots[1]!.id)).toMatchObject({ updates: { length: 10 } });
    expect(byShot.get(shots[2]!.id)).toMatchObject({ updates: { length: 10 } });
  });
});

test.describe('Grid clipboard without OS clipboard access', () => {
  test('in-grid copy/paste falls back to the internal stash', async ({ page }) => {
    // Simulate a webview where async clipboard access is denied outright
    // (WebKitGTK): every navigator.clipboard call rejects. No permissions are
    // granted to this context.
    await page.addInitScript(() => {
      const denied = () => Promise.reject(new DOMException('Clipboard access denied', 'NotAllowedError'));
      Object.defineProperty(navigator, 'clipboard', {
        value: { readText: denied, writeText: denied, read: denied, write: denied },
        configurable: true,
      });
    });
    await seedAndOpen(page);

    // Sanity: the OS clipboard really is unreachable, so a successful paste
    // below can only have come from the grid's internal copy stash.
    expect(
      await page.evaluate(async () => {
        try {
          await navigator.clipboard.readText();
          return false;
        } catch {
          return true;
        }
      }),
    ).toBe(true);

    await page.getByTestId('grid-cell-0-depth_in').click();
    await page.getByTestId('grid-cell-0-depth_out').click({ modifiers: ['Shift'] });
    await page.keyboard.press('Control+c');
    // The stash + marquee are set synchronously; the failed OS write is moot.
    await expect(page.getByTestId('grid-copy-marquee')).toBeVisible();

    // Context-menu paste (the path that falls back to the stash when reads fail).
    await page.getByTestId('grid-cell-1-depth_in').click();
    await page.getByTestId('grid-cell-1-depth_in').click({ button: 'right' });
    const menu = page.getByTestId('grid-context-menu');
    await expect(menu).toBeVisible();
    await menu.getByRole('button', { name: /^Paste/ }).click();

    await expect(page.getByTestId('grid-cell-1-depth_in')).toHaveText('0.0');
    await expect(page.getByTestId('grid-cell-1-depth_out')).toHaveText('-1.0');
    const shots = await getShots(page);
    expect(shots[1]).toMatchObject({ depth_in: 0, depth_out: -1 });

    const calls = await getBatchCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]![0]).toMatchObject({
      type: 'update_shot',
      shot_id: shots[1]!.id,
      updates: { depth_in: 0, depth_out: -1 },
    });
  });
});
