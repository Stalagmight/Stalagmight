/**
 * E2E tests for the generic Select component (portaled dropdown).
 *
 * Subject: the Number Format select in Settings > General
 * (`select-number-format`, 4 options). The DataTable section dropdown that
 * these tests previously used no longer exists - the table header was
 * condensed and sections are now selected app-wide.
 *
 * Behaviors under test are the Select component's own: open/close, portal
 * z-order, keyboard navigation, and the various close paths.
 */
import { test, expect, Page, Locator } from './fixtures/coverage';
import { setupMockedPage, openSettingsFromMenu } from './helpers/app';

/** Open the Settings dialog from the GlobalView header (via hamburger menu). */
async function openSettingsDialog(page: Page): Promise<void> {
  await openSettingsFromMenu(page);
}

/** Navigate to the General tab (hosts the number-format select). */
async function navigateToGeneralTab(page: Page): Promise<void> {
  const generalTab = page.getByTestId('settings-tab-general');
  await expect(generalTab).toBeVisible();
  await generalTab.click();
  await expect(page.getByTestId('settings-panel-general')).toBeVisible({ timeout: 3000 });
}

/** Get the trigger button of the subject Select. */
function getSelectTrigger(page: Page): Locator {
  return page.getByTestId('select-number-format');
}

async function pressUntilActiveDescendant(
  page: Page,
  trigger: Locator,
  key: string,
  expectedId: string,
  maxAttempts = 5,
): Promise<void> {
  // Retry the press itself (a keystroke can land before the listbox handler is
  // attached), but wait on the attribute rather than sleeping between tries.
  let attempts = 0;
  await expect(async () => {
    attempts += 1;
    expect(attempts, `${key} never moved aria-activedescendant to ${expectedId}`).toBeLessThanOrEqual(maxAttempts);
    await trigger.focus();
    await expect(trigger).toBeFocused();
    await page.keyboard.press(key);
    await expect(trigger).toHaveAttribute('aria-activedescendant', expectedId, { timeout: 250 });
  }).toPass({ timeout: 5000 });
}

/**
 * Get the portaled Select listbox from a trigger button using aria-controls.
 * Waits for the attribute to be set (React state update) before reading it.
 * This avoids ambiguity with other listbox elements on the page.
 */
async function getSelectListbox(page: Page, trigger: Locator): Promise<Locator> {
  // Wait for aria-controls to be set (React may need a render cycle after open)
  await expect(trigger).toHaveAttribute('aria-controls', /.+/);
  const listboxId = await trigger.getAttribute('aria-controls');
  // Use attribute selector to avoid CSS escaping issues with React useId() colons
  return page.locator(`[id="${listboxId}"]`);
}

test.describe('Select Dropdown (Portal)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);
  });

  test('dropdown options render topmost above dialog content (z-order proof)', async ({ page }) => {
    // Open the number-format dropdown inside the settings dialog
    const trigger = getSelectTrigger(page);
    await expect(trigger).toBeVisible();
    await trigger.click();

    // Verify the listbox is visible via aria-controls
    const listbox = await getSelectListbox(page, trigger);
    await expect(listbox).toBeVisible();

    // Get the bounding box of an option in the dropdown
    const firstOption = listbox.locator('[role="option"]').first();
    await expect(firstOption).toBeVisible();
    const optionBox = await firstOption.boundingBox();
    expect(optionBox).not.toBeNull();

    // Use elementFromPoint at the center of the option to verify it's the topmost
    // element, not hidden behind the dialog content (the listbox is portaled to
    // document.body and must escape the dialog's stacking context)
    const isOptionTopmost = await page.evaluate(({ x, y }) => {
      const el = document.elementFromPoint(x, y);
      if (!el) return false;
      return el.closest('[role="listbox"]') !== null || el.closest('[role="option"]') !== null;
    }, {
      x: optionBox!.x + optionBox!.width / 2,
      y: optionBox!.y + optionBox!.height / 2,
    });

    expect(isOptionTopmost).toBe(true);
  });

  test('dropdown closes on external scroll', async ({ page }) => {
    // Open the number-format dropdown
    const trigger = getSelectTrigger(page);
    await trigger.click();

    const listbox = await getSelectListbox(page, trigger);
    await expect(listbox).toBeVisible();

    // Dispatch scroll on window (Select listens via window.addEventListener('scroll', ..., { capture: true }))
    await page.evaluate(() => {
      window.dispatchEvent(new Event('scroll'));
    });

    // Dropdown should close
    await expect(listbox).not.toBeVisible({ timeout: 2000 });
  });

  test('keyboard navigation works (ArrowDown/Up, Enter, Escape)', async ({ page }) => {
    // Focus the number-format select trigger
    const trigger = getSelectTrigger(page);
    await trigger.focus();
    await expect(trigger).toBeFocused();

    // Open dropdown via ArrowDown (highlights the current selection)
    await page.keyboard.press('ArrowDown');
    let listbox = await getSelectListbox(page, trigger);
    await expect(listbox).toBeVisible();
    await expect(trigger).toHaveAttribute('aria-expanded', 'true');

    // Verify aria-activedescendant is set on the trigger (wait for React state update)
    await expect(trigger).toHaveAttribute('aria-activedescendant', /.+/);

    const initialActive = await trigger.getAttribute('aria-activedescendant');
    if (!initialActive) {
      throw new Error('Expected aria-activedescendant to be set on the trigger.');
    }

    // Number format has 4 options: locale, period (1,234.56), comma, space
    const options = listbox.locator('[role="option"]');
    await expect(options).toHaveCount(4);
    const periodOption = options.nth(1);
    const periodOptionId = await periodOption.getAttribute('id');
    if (!periodOptionId) {
      throw new Error('Expected the period-format option to have an id for aria-activedescendant.');
    }

    // ArrowDown should move to the period format option
    await pressUntilActiveDescendant(page, trigger, 'ArrowDown', periodOptionId);

    // ArrowUp should return to initial item
    await pressUntilActiveDescendant(page, trigger, 'ArrowUp', initialActive);

    // Move back to the period format option and select with Enter
    await pressUntilActiveDescendant(page, trigger, 'ArrowDown', periodOptionId);
    await page.keyboard.press('Enter');

    // Dropdown should close after selection
    await expect(listbox).not.toBeVisible({ timeout: 2000 });

    // Verify the selected value changed
    await expect(trigger).toContainText('1,234.56');

    // Re-open and close with Escape
    await expect(trigger).toBeFocused();
    await page.keyboard.press('ArrowDown');
    listbox = await getSelectListbox(page, trigger);
    await expect(listbox).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(listbox).not.toBeVisible({ timeout: 2000 });
    await expect(trigger).toHaveAttribute('aria-expanded', 'false');

    // Escape was consumed by the open dropdown - the settings dialog stays open
    await expect(page.getByTestId('settings-dialog')).toBeVisible();
  });

  test('internal scroll does not close dropdown', async ({ page }) => {
    // Open the number-format dropdown
    const trigger = getSelectTrigger(page);
    await trigger.click();

    const listbox = await getSelectListbox(page, trigger);
    await expect(listbox).toBeVisible();

    // Dispatch a scroll event originating from inside the listbox.
    // Select.tsx ignores scroll events where event.target is inside dropdownRef
    // (covers user scrolling through options and scrollIntoView from keyboard nav).
    await listbox.evaluate((el) => {
      el.dispatchEvent(new Event('scroll', { bubbles: true }));
    });

    // Dropdown should remain open -- internal scroll is not treated as external
    await expect(listbox).toBeVisible();

    // Verify we can still interact with the dropdown (select an option)
    const firstOption = listbox.locator('[role="option"]').first();
    await expect(firstOption).toBeVisible();
  });

  test('click outside closes dropdown', async ({ page }) => {
    // Open the number-format dropdown
    const trigger = getSelectTrigger(page);
    await trigger.click();

    const listbox = await getSelectListbox(page, trigger);
    await expect(listbox).toBeVisible();

    // Mousedown elsewhere on the page (outside the dropdown and trigger).
    // Use dispatchEvent on the settings sidebar so no click side effects fire.
    await page.getByTestId('settings-tab-general').dispatchEvent('mousedown');

    // Dropdown should close
    await expect(listbox).not.toBeVisible({ timeout: 2000 });
  });
});
