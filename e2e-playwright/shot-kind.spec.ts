import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

/**
 * E2E coverage for the 3-way shot kind (#27).
 *
 * The old reference dot had no test-id and nothing exercised it, so clicking or
 * typing in that column was entirely uncovered. These tests pin the cycling
 * cell the issue asked for and the display toggles that replaced the single
 * "reference shots" switch.
 */

const lineSeeds: LineSeed[] = [
  {
    name: 'Kind Line',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

/** The kind the store holds for the Nth shot of the only line. */
async function storedKind(page: Page, shotIndex: number): Promise<string | null> {
  return page.evaluate((i) => {
    const state = (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => unknown } } })
      .__E2E_TEST__.projectStore.getState() as {
      project: { lines: Array<{ shots: Array<{ kind?: string }> }> } | null;
    };
    return state.project?.lines[0]?.shots[i]?.kind ?? null;
  }, shotIndex);
}

/** Seed a project and open the DataTable via its keyboard shortcut. */
async function openDataTable(page: Page): Promise<void> {
  await createProject(page, 'Shot Kind');
  await seedProjectWithLines(page, lineSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId('grid-header-kind')).toBeVisible({ timeout: 5000 });
}

test.describe('shot kind cell (#27)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('clicking the cell cycles blank -> J -> R -> blank', async ({ page }) => {
    await openDataTable(page);

    const cell = page.getByTestId('grid-cell-0-kind').getByTestId('shot-kind-cell');
    // A permanent shot renders blank so virtual shots stand out.
    await expect(cell).toHaveAttribute('data-kind', 'Normal');
    expect(await storedKind(page, 0)).toBe('Normal');

    await cell.click();
    await expect(cell).toHaveAttribute('data-kind', 'Jump');
    await expect.poll(() => storedKind(page, 0)).toBe('Jump');
    await expect(cell).toHaveText('J');

    await cell.click();
    await expect(cell).toHaveAttribute('data-kind', 'Reference');
    await expect.poll(() => storedKind(page, 0)).toBe('Reference');
    await expect(cell).toHaveText('R');

    // Three clicks return to exactly where we started, so the ring closes.
    await cell.click();
    await expect(cell).toHaveAttribute('data-kind', 'Normal');
    await expect.poll(() => storedKind(page, 0)).toBe('Normal');
  });

  test('shift-click walks the ring backwards', async ({ page }) => {
    await openDataTable(page);

    const cell = page.getByTestId('grid-cell-0-kind').getByTestId('shot-kind-cell');
    // From Normal, one shift-click reaches Reference instead of two forward
    // clicks - the whole point of the reverse direction.
    await cell.click({ modifiers: ['Shift'] });
    await expect(cell).toHaveAttribute('data-kind', 'Reference');
    await expect.poll(() => storedKind(page, 0)).toBe('Reference');

    await cell.click({ modifiers: ['Shift'] });
    await expect(cell).toHaveAttribute('data-kind', 'Jump');
    await expect.poll(() => storedKind(page, 0)).toBe('Jump');
  });

  test('never opens a cell editor - the cell is a button, not a text field', async ({ page }) => {
    await openDataTable(page);
    const cell = page.getByTestId('grid-cell-0-kind').getByTestId('shot-kind-cell');

    // Double-click cycles once (the click lands) and opens no editor. A text box
    // over a one-glyph cell was a second, worse way to do what a click does.
    await cell.dblclick();
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
    await expect.poll(() => storedKind(page, 0)).not.toBe('Normal');

    // Nor F2, nor type-to-edit, once the cell is merely selected.
    await page.getByTestId('grid-cell-0-kind').click();
    await page.keyboard.press('F2');
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
    await page.keyboard.press('R');
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
  });

  test('still takes a fill-down, which is the bulk reclassify path', async ({ page }) => {
    await openDataTable(page);

    // Closing the editor must not close the BULK paths: stamping one kind down a
    // run is why the column is writable at all.
    //
    // The range is extended with the keyboard rather than shift-click, because
    // every click in a kind cell cycles it - that is the point of the column, so
    // clicking to select would move the very value being stamped.
    await page.getByTestId('grid-cell-0-kind').getByTestId('shot-kind-cell').click();
    await expect.poll(() => storedKind(page, 0)).toBe('Jump');

    await page.keyboard.press('Shift+ArrowDown');
    await page.keyboard.press('Shift+ArrowDown');
    await page.keyboard.press('Control+d');

    await expect.poll(() => storedKind(page, 1)).toBe('Jump');
    await expect.poll(() => storedKind(page, 2)).toBe('Jump');
  });

  test('cycling one cell leaves its neighbours alone', async ({ page }) => {
    await openDataTable(page);

    await page.getByTestId('grid-cell-1-kind').getByTestId('shot-kind-cell').click();
    await expect.poll(() => storedKind(page, 1)).toBe('Jump');
    expect(await storedKind(page, 0)).toBe('Normal');
    expect(await storedKind(page, 2)).toBe('Normal');
  });

  // NOTE: undo semantics (a 3-way kind must restore the CAPTURED previous kind
  // rather than toggling to Normal, #27 q12) are deliberately NOT tested here.
  // The mock IPC's `undo` is a no-op stub that records the call and returns
  // success without touching project state, so any assertion here would be
  // testing the mock rather than the app. That behaviour is covered for real by
  // the Rust test `test_set_shot_kind_undo_restores_previous_kind`.
});

test.describe('shot kind context menu (#27)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('offers the two other kinds, and Unmark for the current one', async ({ page }) => {
    await openDataTable(page);

    const openMenu = async () =>
      page.evaluate(() => {
        const proj = (
          window as unknown as { __E2E_TEST__: { projectStore: { getState: () => unknown } } }
        ).__E2E_TEST__.projectStore.getState() as {
          project: { lines: Array<{ id: number; shots: Array<{ id: number }> }> };
        };
        const sec = proj.project.lines[0]!;
        (
          window as unknown as {
            __E2E_TEST__: { uiStore: { getState: () => { openDialog: (t: string, p: unknown) => void } } };
          }
        ).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
          target: { type: 'shot', lineId: sec.id, shotId: sec.shots[0]!.id },
          position: { x: 300, y: 260 },
        });
      });

    // A permanent shot: both virtual kinds on offer, nothing to unmark.
    await openMenu();
    await expect(page.getByRole('button', { name: 'Mark as Jump' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Mark as Reference' })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Unmark/ })).toHaveCount(0);

    // Make it a Jump: the permanent entry becomes "Unmark Jump", and the icon
    // carries the struck-through letter badge.
    await page.getByRole('button', { name: 'Mark as Jump' }).click();
    await expect.poll(() => storedKind(page, 0)).toBe('Jump');
    await openMenu();
    await expect(page.getByRole('button', { name: 'Unmark Jump' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Mark as Reference' })).toBeVisible();
    await expect(page.locator('svg .kind-strike')).toHaveCount(1);

    // Unmark returns it to permanent.
    await page.getByRole('button', { name: 'Unmark Jump' }).click();
    await expect.poll(() => storedKind(page, 0)).toBe('Normal');
  });
});

test.describe('jump / reference display toggles (#27)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('jumps and references toggle independently', async ({ page }) => {
    await createProject(page, 'Kind Toggles');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    await page.getByTestId('display-menu-button').click();

    const jumps = page.getByTestId('display-toggle-showJumps');
    const references = page.getByTestId('display-toggle-showReferenceShots');
    await expect(jumps).toBeVisible();
    await expect(references).toBeVisible();

    // Both default on, matching the single pre-#27 toggle.
    await expect(jumps).toHaveAttribute('aria-pressed', 'true');
    await expect(references).toHaveAttribute('aria-pressed', 'true');

    // Turning one off must not disturb the other - that independence is the
    // whole reason the single toggle was split.
    await jumps.click();
    await expect(jumps).toHaveAttribute('aria-pressed', 'false');
    await expect(references).toHaveAttribute('aria-pressed', 'true');

    await references.click();
    await expect(jumps).toHaveAttribute('aria-pressed', 'false');
    await expect(references).toHaveAttribute('aria-pressed', 'false');

    await jumps.click();
    await expect(jumps).toHaveAttribute('aria-pressed', 'true');
    await expect(references).toHaveAttribute('aria-pressed', 'false');
  });
});
