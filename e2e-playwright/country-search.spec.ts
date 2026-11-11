import { test, expect } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';

test('country search filters by name and code and supports keyboard selection', async ({ page }) => {
  await setupMockedPage(page);
  await page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__: { uiStore: { getState: () => { openDialog: (name: string) => void } } };
    }).__E2E_TEST__;
    stores.uiStore.getState().openDialog('publish-to-speleodb');
  });
  const country = page.getByTestId('publish-to-speleodb-dialog-country');
  await country.click();
  const search = page.getByTestId('publish-to-speleodb-dialog-country-search');
  await expect(search).toBeFocused();
  await search.fill('united states of america');
  await expect(page.getByRole('option')).toHaveCount(1);
  await search.press('ArrowDown');
  await search.press('Enter');
  await expect(country).toContainText('United States of America');
  await expect(country).toBeFocused();
  await country.click();
  await expect(search).toHaveValue('');
  await search.fill('zzzzzz');
  await expect(page.getByRole('option')).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: 'No countries' })).toBeVisible();
  await search.fill('mx');
  await page.getByRole('option', { name: 'Mexico', exact: true }).click();
  await expect(country).toContainText('Mexico');
  await country.click();
  await search.press('Escape');
  await expect(search).toHaveCount(0);
  await expect(page.getByTestId('publish-to-speleodb-dialog')).toBeVisible();
});
