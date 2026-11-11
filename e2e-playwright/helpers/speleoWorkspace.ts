import { expect, type Page } from '@playwright/test';

/**
 * Open the left panel's SpeleoDB tab - the SpeleoDB entry point since the toolbar
 * SpeleoDB button was removed - by clicking the tab itself.
 */
export async function openSpeleoTab(page: Page): Promise<void> {
  const tab = page.getByTestId('workspace-tab-speleodb');
  await tab.click();
  await expect(tab).toHaveAttribute('aria-selected', 'true');
}

/**
 * Open the SpeleoDB tab where the left panel is folded away (closed, hover mode,
 * or squeezed out by a narrow window): reveal the panel with its opener tab on
 * the left edge, then click the SpeleoDB tab - the route a user takes, since the
 * hamburger has no SpeleoDB row.
 */
export async function revealSpeleoTab(page: Page): Promise<void> {
  const bar = await page.getByTestId('sections-panel-bar').boundingBox();
  if (!bar || bar.width < 100) await page.getByTestId('panel-tab-sections').click();
  await openSpeleoTab(page);
}
