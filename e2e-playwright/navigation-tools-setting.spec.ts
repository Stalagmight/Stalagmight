import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  getMapCenter,
  openSettingsFromMenu,
  waitForActiveTool,
  waitForMapReady,
  waitForMapIdle,
  waitForNoDialogs,
} from './helpers/app';

/**
 * #409 - `map.controls.show_navigation_tools`. The Pan and Zoom tools are hidden
 * by default (the map pans on a plain drag and zooms on the wheel under every
 * tool), and this setting puts them back for anyone who cannot do either.
 *
 * No `seedNavigationTools` here: the setting is what is under test, so it has to
 * be driven through the real Settings dialog and the real save round-trip.
 */

async function getSavedConfig(page: Page): Promise<Record<string, unknown> | null> {
  return page.evaluate(() => {
    const get = (window as unknown as {
      __MOCK_GET_SAVED_CONFIG__?: () => Record<string, unknown> | null;
    }).__MOCK_GET_SAVED_CONFIG__;
    return get?.() ?? null;
  });
}

/** Open Settings from the in-project hamburger and land on the Map panel. */
async function openMapSettings(page: Page): Promise<void> {
  await openSettingsFromMenu(page);
  await page.getByTestId('settings-tab-map').click();
  await expect(page.getByTestId('settings-panel-map')).toBeVisible();
}

/** Flip the navigation-tools toggle to `on`, save, and wait for the dialog to close. */
async function setNavigationTools(page: Page, on: boolean): Promise<void> {
  await openMapSettings(page);
  const toggle = page.getByTestId('toggle-show-navigation-tools');
  await expect(toggle).toHaveAttribute('aria-checked', String(!on));
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-checked', String(on));

  const save = page.getByTestId('settings-save-button');
  await expect(save).toBeEnabled();
  await save.click();
  await expect(page.getByTestId('settings-dialog')).toBeHidden({ timeout: 5000 });
}

test.describe('Navigation tools setting (#409)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Navigation Tools Setting');
    await waitForMapReady(page);
    await waitForNoDialogs(page);
  });

  test('the Map settings row exists and is off by default', async ({ page }) => {
    await openMapSettings(page);

    const row = page.getByTestId('setting-show-navigation-tools');
    await expect(row).toBeVisible();
    await expect(page.getByTestId('toggle-show-navigation-tools')).toHaveAttribute(
      'aria-checked',
      'false'
    );

    // ... and the toolbar agrees with it.
    await page.getByTestId('settings-cancel-button').click();
    await expect(page.getByTestId('settings-dialog')).toBeHidden({ timeout: 5000 });
    await expect(page.getByTestId('tool-group-navigate')).toHaveCount(0);
  });

  test('turning it on restores the navigate group, the P shortcut, and the pan drag', async ({ page }) => {
    await expect(page.getByTestId('tool-group-navigate')).toHaveCount(0);

    await setNavigationTools(page, true);

    // The group is back on the toolbar.
    await expect(page.getByTestId('tool-group-navigate')).toBeVisible();

    // ... and so is its shortcut, which was inert a moment ago.
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');

    // The pan tool still pans (it never lost the left button - the gesture rule
    // only takes drag away from the zoom tool, placement, and a move ghost).
    const canvas = page.getByTestId('map-canvas');
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();
    const centerBefore = await getMapCenter(page);
    await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
    await page.mouse.down();
    await page.mouse.move(box!.x + box!.width / 2 + 120, box!.y + box!.height / 2 + 80, { steps: 8 });
    await page.mouse.up();
    await waitForMapIdle(page);

    const centerAfter = await getMapCenter(page);
    expect(
      Math.abs(centerAfter.lng - centerBefore.lng) + Math.abs(centerAfter.lat - centerBefore.lat)
    ).toBeGreaterThan(0.00001);
  });

  test('the saved config carries map.controls.show_navigation_tools', async ({ page }) => {
    expect(await getSavedConfig(page), 'nothing saved before the test acts').toBeNull();

    await setNavigationTools(page, true);

    const saved = await getSavedConfig(page);
    expect(saved).not.toBeNull();
    const map = saved!.map as { controls: { show_navigation_tools: boolean } };
    expect(map.controls.show_navigation_tools).toBe(true);
  });

  test('turning it back off while Pan is active force-resets the tool to select', async ({ page }) => {
    await setNavigationTools(page, true);
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');

    // Off again, with Pan still held. The active tool is persisted UI state, so
    // without the reset the user would be left holding a tool whose button has
    // just been taken off the toolbar.
    await setNavigationTools(page, false);

    await waitForActiveTool(page, 'select');
    await expect(page.getByTestId('tool-group-navigate')).toHaveCount(0);

    // And the shortcut is inert again.
    await page.keyboard.press('p');
    // settle: proves the tool does NOT change back, which a poll cannot show.
    await page.waitForTimeout(200);
    await expect(page.getByTestId('map-canvas')).toHaveAttribute('data-active-tool', 'select');
  });
});
