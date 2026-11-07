import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, createSectionViaStore } from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';
import { setIpcOverrides } from './helpers/ipcOverrides';
import { getIpcCalls, recordIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';

/**
 * E2E for the structural-integrity warn + repair banner (#17). The mock's
 * check_project_integrity returns issues seeded via __MOCK_SET_INTEGRITY_ISSUES__;
 * repair_project_integrity clears them and returns the count.
 */

async function seedAndCheck(page: Page, issues: string[]): Promise<void> {
  await page.evaluate((seed) => {
    (window as unknown as { __MOCK_SET_INTEGRITY_ISSUES__: (i: string[]) => void })
      .__MOCK_SET_INTEGRITY_ISSUES__(seed);
  }, issues);
  await page.evaluate(async () => {
    await (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { checkProjectIntegrity: () => Promise<string[]> } } };
    }).__E2E_TEST__.projectStore.getState().checkProjectIntegrity();
  });
}

/**
 * Press Tab `count` times, then Shift+Tab `count` times, and return every press
 * after which focus was NOT in the integrity dialog (empty when it held focus).
 */
async function focusEscapes(page: Page, count: number): Promise<string[]> {
  const escapes: string[] = [];
  for (const key of ['Tab', 'Shift+Tab']) {
    for (let i = 1; i <= count; i++) {
      await page.keyboard.press(key);
      const spot = await page.evaluate(() => {
        const active = document.activeElement;
        const dialog = document.querySelector('[data-testid="integrity-dialog"]');
        return {
          id: active?.getAttribute('data-testid') ?? active?.tagName.toLowerCase() ?? 'none',
          inDialog: dialog !== null && active !== null && dialog.contains(active),
        };
      });
      if (!spot.inDialog) escapes.push(`${key} #${i} -> ${spot.id}`);
    }
  }
  return escapes;
}

test.describe('Structural-integrity banner', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    // The integrity banner is tied to an OPEN project: checkProjectIntegrity
    // drops its result when no project is loaded (so it can't leak onto the
    // global view). Open a project so the banner can render.
    await createProject(page, 'Integrity Test');
  });

  test('no banner when the project is structurally sound', async ({ page }) => {
    await seedAndCheck(page, []);
    await expect(page.getByTestId('integrity-banner')).toHaveCount(0);
  });

  test('shows issue details in a dialog and repairs from there', async ({ page }) => {
    await seedAndCheck(page, [
      'Merged stations 1:0 and 0:13 are 30.03 m apart but should be the same point',
      'Station merge #0 station_a references missing section 42',
    ]);

    const banner = page.getByTestId('integrity-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('2 integrity issues');

    // Details are shown in a dialog on Review, not on hover.
    await page.getByTestId('integrity-review-button').click();
    const dialog = page.getByTestId('integrity-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('integrity-dialog-list')).toContainText('30.03 m apart');
    await expect(dialog.getByTestId('integrity-dialog-list')).toContainText('missing section 42');

    await page.getByTestId('integrity-dialog-repair').click();

    // Dialog and banner clear; a confirmation toast appears.
    await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
    await expect(page.getByTestId('integrity-banner')).toHaveCount(0);
    // Scoped to the toast: ToastStack renders every message TWICE on purpose - once
    // in an `sr-only` live region that announces it, and once as the visible,
    // `aria-hidden` copy - so an unscoped text match is ambiguous by design. The
    // visible one is what this test is about.
    await expect(
      page.getByTestId('toast').filter({ hasText: /Repaired 2 issues/ }),
    ).toBeVisible();
  });

  test('repair reports issues that still need manual attention', async ({ page }) => {
    await seedAndCheck(page, [
      'Station merge #0 station_a references missing section 42',
      'Loop closure 1 (0:2 ↔ 1:0) is marked closed but its ends are 8.10 m apart - recompute or remove it',
    ]);
    // One of the two issues is detect-only and survives repair.
    await page.evaluate(() => {
      (window as unknown as { __MOCK_SET_INTEGRITY_REMAINING__: (n: number) => void })
        .__MOCK_SET_INTEGRITY_REMAINING__(1);
    });

    await page.getByTestId('integrity-review-button').click();
    await page.getByTestId('integrity-dialog-repair').click();

    // Honest message: fixed one, one needs manual attention. Scoped to the toast
    // for the same reason as above - the live region carries the same text.
    await expect(
      page.getByTestId('toast').filter({ hasText: /Fixed 1 issue; 1 need.* manual attention/ }),
    ).toBeVisible();
    // The banner stays (the residual issue is still present).
    await expect(page.getByTestId('integrity-banner')).toContainText('1 integrity issue');
  });

  test('review dialog can be cancelled without repairing', async ({ page }) => {
    await seedAndCheck(page, ['Zero datum #0 references missing section 7']);
    await expect(page.getByTestId('integrity-banner')).toContainText('1 integrity issue');
    await page.getByTestId('integrity-review-button').click();
    await expect(page.getByTestId('integrity-dialog')).toBeVisible();
    await page.getByTestId('integrity-dialog-cancel').click();
    await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
    // Banner stays - nothing was repaired.
    await expect(page.getByTestId('integrity-banner')).toBeVisible();
  });

  test('Repair is disabled with a reason while the workspace is read-only (#576 D39(a))', async ({ page }) => {
    await seedAndCheck(page, ['Station merge #0 station_a references missing section 42']);
    // Plan is a read-only workspace: the banner and the issue list stay, Repair does not run.
    await page.getByTestId('mode-plan').click();
    const startEntrance = page.getByTestId('gas-start-entrance');
    if (await startEntrance.isVisible().catch(() => false)) await startEntrance.click();

    await page.getByTestId('integrity-review-button').click();
    const dialog = page.getByTestId('integrity-dialog');
    await expect(dialog.getByTestId('integrity-dialog-list')).toContainText('missing section 42');
    await expect(page.getByTestId('integrity-dialog-repair')).toBeDisabled();
    await expect(page.getByTestId('integrity-dialog-repair-blocked')).toHaveText('Switch to Survey to repair');

    // Back in Survey the same dialog repairs.
    await page.getByTestId('integrity-dialog-cancel').click();
    await page.getByTestId('mode-survey').click();
    await page.getByTestId('integrity-review-button').click();
    await expect(page.getByTestId('integrity-dialog-repair')).toBeEnabled();
    await expect(page.getByTestId('integrity-dialog-repair-blocked')).toHaveCount(0);
  });

  // [NF3] The integrity dialog was rendered inside #root, outside the dialog stack:
  // focus stayed on Review behind it and Tab walked on through the app.
  test('the integrity dialog holds keyboard focus and gives it back to Review when it closes', async ({ page }) => {
    await seedAndCheck(page, ['Station merge #0 station_a references missing section 42']);
    const review = page.getByTestId('integrity-review-button');
    await review.click();
    await expect(page.getByTestId('integrity-dialog')).toBeVisible();

    // Focus moves to its first control, and more presses each way than it has
    // controls (Cancel, Repair) never leave it.
    await expect(page.getByTestId('integrity-dialog-cancel')).toBeFocused();
    expect(await focusEscapes(page, 3)).toEqual([]);

    // Cancel closes it, and focus is back on Review.
    await expect(page.getByTestId('integrity-dialog-cancel')).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
    await expect(review).toBeFocused();
  });

  // [NF3] A dialog stacked over it made #root inert, and the integrity dialog with
  // it, while the integrity dialog (z-[1000]) still painted on top of that dialog.
  test('a dialog opened over the integrity dialog is painted above it, and the integrity dialog has the keyboard back when it closes', async ({ page }) => {
    await seedAndCheck(page, ['Station merge #0 station_a references missing section 42']);
    await page.getByTestId('integrity-review-button').click();
    const cancel = page.getByTestId('integrity-dialog-cancel');
    await cancel.focus();
    await expect(cancel).toBeFocused();

    await page.evaluate(() => {
      (window as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog('error', { title: 'Save failed', message: 'The disk is full.' });
    });
    const errorDialog = page.getByTestId('error-dialog');
    await expect(errorDialog).toBeVisible();

    // Paint order: neither #root nor #dialog-root forms a stacking context, so the
    // two fixed layers are ordered by their z-index in the root one.
    const layers = await page.evaluate(() => {
      const z = (el: Element | null | undefined): number => (el ? Number(getComputedStyle(el).zIndex) : NaN);
      return {
        integrity: z(document.querySelector('[data-testid="integrity-dialog"]')),
        error: z(document.querySelector('[data-testid="error-dialog"]')?.parentElement),
      };
    });
    expect(layers.error).toBeGreaterThan(layers.integrity);

    // Closing the dialog above (its OK) hands the keyboard back to the integrity dialog.
    await page.getByTestId('error-dialog-body').getByTestId('error-dialog-close').click();
    await expect(errorDialog).toHaveCount(0);
    await expect(cancel).toBeFocused();
    expect(await focusEscapes(page, 3)).toEqual([]);
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
  });

  test('banner can be dismissed without opening the dialog', async ({ page }) => {
    await seedAndCheck(page, ['Zero datum #0 references missing section 7']);
    await expect(page.getByTestId('integrity-banner')).toContainText('1 integrity issue');
    await page.getByTestId('integrity-dismiss-button').click();
    await expect(page.getByTestId('integrity-banner')).toHaveCount(0);
  });

  // The integrity dialog is on the dialog stack, but it is neither the
  // `activeDialog` slot nor a screen occupant, so the survey shortcuts did not
  // stand down for it: Ctrl+O / Ctrl+I asked for the file picker, `?` opened the
  // shortcuts list, Ctrl+Z undid and Delete asked to delete the selection, all on
  // the project behind it. Each test presses its keys with the dialog open, then
  // closes it and presses them again: they act then, so the first presses were
  // stopped by the dialog, not keys that do nothing here anyway.
  test.describe('shortcuts stand down while the integrity dialog is open', () => {
    const activeDialog = (page: Page) =>
      page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().activeDialog);

    async function openIntegrityDialog(page: Page): Promise<void> {
      await seedAndCheck(page, ['Station merge #0 station_a references missing section 42']);
      await page.getByTestId('integrity-review-button').click();
      await expect(page.getByTestId('integrity-dialog-cancel')).toBeFocused();
    }

    async function closeIntegrityDialog(page: Page): Promise<void> {
      await page.getByTestId('integrity-dialog-cancel').click();
      await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
    }

    test('Ctrl+O and Ctrl+I do not ask for the file picker', async ({ page }) => {
      // The picker answers "cancelled", so the control presses below open nothing more.
      await setIpcOverrides(page, { 'plugin:dialog|open': null });
      await recordIpcCalls(page, ['plugin:dialog|open']);
      await openIntegrityDialog(page);

      await page.keyboard.press('Control+o');
      await page.keyboard.press('Control+i');
      // settle: proves no picker is asked for; an empty call log is what the first
      // read sees whether or not the shortcuts fired.
      await page.waitForTimeout(300);
      expect(await getIpcCalls(page, 'plugin:dialog|open')).toEqual([]);
      await expect(page.getByTestId('integrity-dialog')).toBeVisible();

      // Control: with the dialog closed, Ctrl+O asks for the picker, and Ctrl+I asks
      // for it for a .dmp (a project is open).
      await closeIntegrityDialog(page);
      await page.keyboard.press('Control+o');
      await waitForIpcCall(page, 'plugin:dialog|open', 1);
      await page.keyboard.press('Control+i');
      const calls = await waitForIpcCall(page, 'plugin:dialog|open', 2);
      expect(calls[1]?.args).toMatchObject({ options: { filters: [{ extensions: ['dmp'] }] } });
    });

    test('? does not open the shortcuts list', async ({ page }) => {
      await openIntegrityDialog(page);

      await page.keyboard.press('?');
      // settle: proves the shortcuts list does not open; its absence is what the
      // first read sees whether or not the shortcut fired.
      await page.waitForTimeout(300);
      expect(await activeDialog(page)).toBeNull();
      await expect(page.getByTestId('keyboard-shortcuts-dialog')).toHaveCount(0);

      // Control: with the dialog closed, `?` opens it.
      await closeIntegrityDialog(page);
      await page.keyboard.press('?');
      await expect(page.getByTestId('keyboard-shortcuts-dialog')).toBeVisible();
    });

    test('Ctrl+Z does not undo', async ({ page }) => {
      await recordIpcCalls(page, ['undo']);
      await openIntegrityDialog(page);

      await page.keyboard.press('Control+z');
      // settle: proves no undo is sent; an empty call log is what the first read
      // sees whether or not the shortcut fired.
      await page.waitForTimeout(300);
      expect(await getIpcCalls(page, 'undo')).toEqual([]);

      // Control: with the dialog closed, Ctrl+Z undoes.
      await closeIntegrityDialog(page);
      await page.keyboard.press('Control+z');
      await waitForIpcCall(page, 'undo');
    });

    test('Delete does not ask to delete the selection', async ({ page }) => {
      await createSectionViaStore(page, {
        name: 'Integrity Delete Section',
        shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 1 }],
      });
      await openIntegrityDialog(page);
      await page.evaluate(() => {
        const e2e = (window as E2EWindow).__E2E_TEST__;
        const id = e2e.projectStore.getState().project!.sections[0]!.id;
        e2e.uiStore.getState().setSelection({ type: 'section', sectionId: id });
      });

      await page.keyboard.press('Delete');
      // settle: proves the delete confirmation does not open; its absence is what
      // the first read sees whether or not the shortcut fired.
      await page.waitForTimeout(300);
      expect(await activeDialog(page)).toBeNull();
      await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);

      // Control: with the dialog closed, Delete asks to delete the selected section.
      await closeIntegrityDialog(page);
      await page.keyboard.press('Delete');
      await expect(page.getByTestId('confirm-dialog')).toBeVisible();
      expect(await activeDialog(page)).toBe('confirm-delete');
    });
  });
});
