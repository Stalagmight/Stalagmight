import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage, selectDropdownOption } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';
import type { MockReply } from './mocks/ipc/types';

/**
 * First mocked-tier coverage for the Ariane (.tml/.tmlu) import journey (#411).
 *
 * Drives the whole route the way a user does - unified picker (Ctrl+I) ->
 * routeOpenedFile -> parse_tml -> new-project-for-import -> TmlImportDialog ->
 * execute_tml_import -> import receipt - against the synthetic TML the shared
 * IPC mock serves (no .tml fixture is read anywhere).
 *
 * The mocked file is 2 sections / 5 shots, one of them excluded, one virtual,
 * with 1 loop closure, so the dialog's optional controls and the preview's
 * counts are all observable.
 *
 * [#576 E3] The import itself is canned: the mock lands the file as the default
 * options would, whatever was sent, and answers its receipt. What each option
 * changes about the landed sections, kinds, merges, closures and receipt counts is
 * the backend's (parsers tml tests, core command_import and mock_narrowing_tests).
 * This spec asserts what the dialog shows, what it sends, and that the reply is
 * rendered.
 */

const TML_PATH = '/mock/synthetic-cavern.tml';
const IMPERIAL_TML_PATH = '/mock/synthetic-imperial-cavern.tmlu';

async function setImportPath(page: Page, path: string): Promise<void> {
  await page.evaluate((selectedPath) => {
    const overrides = (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as
      | Record<string, unknown>
      | undefined;
    const next = overrides ?? {};
    next['plugin:dialog|open'] = selectedPath;
    (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = next;
  }, path);
}

/** Picker -> new-project-for-import -> TmlImportDialog, ready to confirm. */
async function openTmlImportDialog(page: Page, path: string, projectName: string): Promise<void> {
  await setImportPath(page, path);
  await page.keyboard.press('Control+i');

  await expect(page.getByTestId('new-project-for-import-dialog')).toBeVisible({ timeout: 10000 });
  await page.getByTestId('project-name-input').fill(projectName);
  await page.getByTestId('create-and-continue-button').click();

  await expect(page.getByTestId('tml-import-dialog')).toBeVisible({ timeout: 10000 });
  // Import is enabled as soon as the preview has something to import: the
  // file-hash round trip is provenance, not a gate (#411 C4 removed the
  // unreachable duplicate check the button used to wait on).
  await expect(page.getByTestId('tml-import-confirm')).toBeEnabled();
}

async function getTmlImportCalls(page: Page): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    () =>
      (window as unknown as { __MOCK_GET_TML_IMPORT_CALLS__?: () => Array<Record<string, unknown>> })
        .__MOCK_GET_TML_IMPORT_CALLS__?.() ?? []
  );
}

async function getImportedSections(
  page: Page
): Promise<Array<{ name: string; kinds: string[] }>> {
  return page.evaluate(() => {
    const state = (
      window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { sections: Array<{ name: string; shots: Array<{ kind: string }> }> } | null;
            };
          };
        };
      }
    ).__E2E_TEST__?.projectStore.getState();
    return (state?.project?.sections ?? []).map((section) => ({
      name: section.name,
      kinds: section.shots.map((shot) => shot.kind),
    }));
  });
}

test.describe('Ariane TML import journey', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await page.evaluate(() =>
      (window as unknown as { __MOCK_RESET_TML_IMPORT_CALLS__?: () => void }).__MOCK_RESET_TML_IMPORT_CALLS__?.()
    );
  });

  test('imports a TML from the unified picker through to the receipt @smoke', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Synthetic Cavern Import');

    const dialog = page.getByTestId('tml-import-dialog');
    await expect(dialog).toContainText('Synthetic Cavern');
    await expect(page.getByTestId('tml-section-count')).toHaveText('2');
    // 4, not the file's 5 raw rows: the preview counts what will actually import
    // under the current toggles, and the excluded shot is out by default (#411 C4).
    await expect(page.getByTestId('tml-shot-count')).toHaveText('4');
    await expect(page.getByTestId('tml-section-row-0')).toContainText('Main Line');
    await expect(page.getByTestId('tml-section-row-1')).toContainText('Side Passage');
    // The branch-split marker rides on the second section.
    await expect(page.getByTestId('tml-section-row-1')).toContainText('branch');
    // A connected file must show NO stranded badge. This is the e2e mirror of
    // the wire-id regression: the mock delivers i64 ids as JSON numbers exactly
    // like production, so a bigint-typed lookup that misses every parent turns
    // this badge on and this line red.
    await expect(page.getByTestId('tml-section-reanchored-1')).toHaveCount(0);
    await expect(page.getByTestId('tml-closure-count')).toContainText('1 loop closure');

    await page.getByTestId('tml-import-confirm').click();
    await expect(dialog).toBeHidden({ timeout: 15000 });

    // Warning-free, unconverted receipt: #411/#420 downgrade it to a toast, not
    // the modal. Assert the toast FIRST (it waits for the receipt to actually
    // resolve) so the modal check after it isn't a same-instant false pass. The
    // exact summary string - 2 of 2 sections, 4 shots (excluded out by
    // default), the file's single loop closure - keeps its e2e contract here,
    // moved from the old modal assertion. `.filter` because the toast testid is
    // per-toast and strict mode errors on a second concurrent toast.
    await expect(
      page.getByTestId('toast').filter({ hasText: 'Ariane TML import' })
    ).toContainText('Ariane TML import: 2 sections - 4 shots - 1 loop reconstructed', {
      timeout: 10000,
    });
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();
    await expect(page.getByTestId('single-system-view')).toBeVisible();

    const sections = await getImportedSections(page);
    expect(sections.map((section) => section.name)).toEqual(['Main Line', 'Side Passage']);

    const calls = await getTmlImportCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.includeExcluded).toBe(false);
    expect(calls[0]!.commentsOnStations).toBe(true);
    expect(calls[0]!.virtualShotMode).toBe('Auto');
  });

  test('the excluded-shot toggle counts the extra shot in the preview and sends includeExcluded', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Excluded Shot Import');

    await page.getByTestId('tml-include-excluded').check();
    // Opting the excluded shot in moves the honest preview count with it.
    await expect(page.getByTestId('tml-shot-count')).toHaveText('5');
    await page.getByTestId('tml-import-confirm').click();

    // Toast, not modal (warning-free receipt, #411/#420). The receipt's counts are
    // the backend's answer (#576 E3: what an opted-in excluded shot adds to it is
    // pinned in Rust); the smoke test above pins how a receipt renders.
    await expect(
      page.getByTestId('toast').filter({ hasText: 'Ariane TML import' })
    ).toBeVisible({ timeout: 15000 });
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();

    const calls = await getTmlImportCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.includeExcluded).toBe(true);
  });

  test('the virtual-shot override sends AllReferences', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Virtual Shot Import');

    await selectDropdownOption(page, 'tml-virtual-shot-mode', 'AllReferences');
    await page.getByTestId('tml-import-confirm').click();

    // Toast, not modal (warning-free receipt, #411/#420).
    await expect(
      page.getByTestId('toast').filter({ hasText: 'Ariane TML import' })
    ).toBeVisible({ timeout: 15000 });
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();

    const calls = await getTmlImportCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.virtualShotMode).toBe('AllReferences');
  });

  test('the all-jumps override sends AllJumps', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'All Jumps Import');

    await selectDropdownOption(page, 'tml-virtual-shot-mode', 'AllJumps');
    await page.getByTestId('tml-import-confirm').click();

    // Toast, not modal (warning-free receipt, #411/#420).
    await expect(
      page.getByTestId('toast').filter({ hasText: 'Ariane TML import' })
    ).toBeVisible({ timeout: 15000 });
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();

    const calls = await getTmlImportCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.virtualShotMode).toBe('AllJumps');
  });

  test('an imperial file reports the unit conversion in the dialog and the receipt', async ({ page }) => {
    await openTmlImportDialog(page, IMPERIAL_TML_PATH, 'Imperial Import');

    await expect(page.getByTestId('tml-import-dialog')).toContainText(
      'Converted from imperial to metric'
    );

    // The backend's receipt for a converted file carries its unit note; the receipt
    // must surface it (#576 E3: the mock answers canned, so the spec seeds the reply).
    await setIpcOverrides(page, {
      execute_tml_import: {
        warnings: [],
        remote_link: null,
        section_ids: [1, 2],
        import_receipt: {
          source_format: 'Ariane TML',
          file_name: 'synthetic-imperial-cavern.tmlu',
          sections_found: 2,
          sections_imported: 2,
          shot_count: 4,
          closures_requested: 1,
          closures_created: 1,
          unit_conversion: 'Lengths converted from feet to meters',
          warnings: [],
          info: [],
          zero_datum_created: null,
        },
      } satisfies MockReply<'execute_tml_import'>,
    });

    await page.getByTestId('tml-import-confirm').click();

    const receipt = page.getByTestId('import-receipt-dialog');
    await expect(receipt).toBeVisible({ timeout: 15000 });
    await expect(page.getByTestId('import-receipt-unit-conversion')).toHaveText(
      'Lengths converted from feet to meters'
    );
    await page.getByTestId('import-receipt-continue').click();
    await expect(receipt).toBeHidden();
  });

  test('cancelling the TML dialog imports nothing into the fresh project', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Cancelled Import');

    await page.getByTestId('tml-import-cancel').click();
    // Cancelling leaves behind the project that was created FOR this import, so
    // the dialog asks what to do with it instead of dropping the user into an
    // empty project unannounced (#411 C2). Keeping it is the non-destructive
    // choice, and still imports nothing.
    await expect(page.getByTestId('tml-import-residue-dialog')).toBeVisible();
    await page.getByTestId('tml-import-residue-keep').click();
    await expect(page.getByTestId('tml-import-dialog')).toBeHidden();
    await expect(page.getByTestId('tml-import-residue-dialog')).toBeHidden();

    expect(await getTmlImportCalls(page)).toHaveLength(0);
    expect(await getImportedSections(page)).toEqual([]);
  });

  // #447: the Cancel/Import footer used to scroll away with the dialog body,
  // starting below the fold at 1024x640 (the issue's cited default zoom).
  //
  // #425 moved this dialog to the mNemo wizard's shape - a `WizardScrollBody`
  // scroller with a map column beside the settings - so the footer is now a
  // flex SIBLING below the scroller rather than sticky inside it. The
  // guarantee is unchanged and still worth pinning structurally: nothing the
  // body scrolls can take the primary action off screen or end up on top of it.
  test('the footer stays visible and clear of the body when the body scrolls', async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 500 });
    await openTmlImportDialog(page, TML_PATH, 'Sticky Footer Import');

    const scroller = page.getByTestId('wizard-body-scroll');
    const confirmButton = page.getByTestId('tml-import-confirm');

    await expect(confirmButton).toBeInViewport();
    await scroller.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await expect(confirmButton).toBeInViewport();

    const scrollerBox = await scroller.boundingBox();
    const footerBox = await confirmButton.boundingBox();
    expect(scrollerBox).not.toBeNull();
    expect(footerBox).not.toBeNull();
    if (scrollerBox && footerBox) {
      expect(scrollerBox.y + scrollerBox.height).toBeLessThanOrEqual(footerBox.y + 1);
    }
  });

  // #425: the per-section filter. Side Passage hangs off Main Line, so
  // unticking Main Line also exercises the re-anchor path end to end.
  test('unticking a section imports only the rest, and says so on the wire', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Filtered Import');

    await expect(page.getByTestId('tml-selected-count')).toContainText('2 of 2');
    await page.getByTestId('tml-section-0').uncheck();

    await expect(page.getByTestId('tml-selected-count')).toContainText('1 of 2');
    // Counts follow the SELECTION, not the file: Side Passage keeps 2 shots
    // (its Closure row is never imported).
    await expect(page.getByTestId('tml-section-count')).toHaveText('1');
    await expect(page.getByTestId('tml-shot-count')).toHaveText('2');

    await page.getByTestId('tml-import-confirm').click();
    await expect(page.getByTestId('tml-import-dialog')).toBeHidden({ timeout: 15000 });

    const calls = await getTmlImportCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.selectedSections).toEqual([1]);
  });

  test('unticking every section disables Import', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Empty Selection Import');

    await page.getByTestId('tml-select-all').uncheck();

    await expect(page.getByTestId('tml-import-empty')).toBeVisible();
    await expect(page.getByTestId('tml-import-confirm')).toBeDisabled();
    expect(await getTmlImportCalls(page)).toHaveLength(0);
  });

  // #425: placement is a stated choice now, not a silent default. The synthetic
  // file carries no georeference, so the "use the file's position" option is
  // not offered and Origin is what the dialog proposes.
  test('states the placement choice on the wire, and holds Import on bad coordinates', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Placement Import');

    await expect(page.getByTestId('tml-placement-step')).toBeVisible();
    await expect(page.getByTestId('tml-placement-file')).toHaveCount(0);
    await expect(page.getByTestId('tml-placement-origin')).toBeChecked();

    // Choosing manual entry with nothing typed is not an answer.
    await page.getByTestId('tml-placement-manual').check();
    await expect(page.getByTestId('tml-import-confirm')).toBeDisabled();
    await page.getByTestId('tml-placement-latitude').fill('91');
    await page.getByTestId('tml-placement-longitude').fill('0');
    await expect(page.getByTestId('tml-placement-error')).toBeVisible();
    await expect(page.getByTestId('tml-import-confirm')).toBeDisabled();

    // Back to the origin, which the backend now hears explicitly.
    await page.getByTestId('tml-placement-origin').check();
    await expect(page.getByTestId('tml-import-confirm')).toBeEnabled();
    await page.getByTestId('tml-import-confirm').click();
    await expect(page.getByTestId('tml-import-dialog')).toBeHidden({ timeout: 15000 });

    const calls = await getTmlImportCalls(page);
    expect(calls[0]?.rootPlacement).toEqual({ type: 'origin' });
  });

  test('discarding the empty project after cancelling returns to the project list', async ({ page }) => {
    await openTmlImportDialog(page, TML_PATH, 'Discarded Import');

    await page.getByTestId('tml-import-cancel').click();
    await expect(page.getByTestId('tml-import-residue-dialog')).toBeVisible();
    await page.getByTestId('tml-import-residue-discard').click();

    await expect(page.getByTestId('tml-import-residue-dialog')).toBeHidden();
    await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 10000 });
    expect(await getTmlImportCalls(page)).toHaveLength(0);
  });
});
