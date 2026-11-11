import { test, expect } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { systemCardTestId } from './helpers/testIds';

const ALPHA_CARD = systemCardTestId('/projects/cave-alpha.stalx');
const BETA_CARD = systemCardTestId('/projects/cave-beta.stalx');
const LOCAL_CARD = systemCardTestId('/projects/local-survey.stalx');
const ALPHA_MARKER = `system-marker-${encodeURIComponent('/projects/cave-alpha.stalx')}`;

test.describe('GlobalView Metadata Panel', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 10000 });
  });

  test.describe('Panel Visibility', () => {
    test('list view hides the details panel until a project is selected', async ({ page }) => {
      // No selection on load → the details panel is hidden in the list view.
      await expect(page.locator('[data-testid="metadata-panel"]')).toHaveCount(0);
      // Selecting a project reveals it (after the double-click debounce window).
      await page.getByTestId(ALPHA_CARD).click();
      await expect(page.locator('[data-testid="metadata-panel"]')).toBeVisible({ timeout: 5000 });
    });

    test('map view shows the empty state when no project is selected', async ({ page }) => {
      // The empty "Select a project" state lives in the map view, where the panel
      // is always present alongside the map.
      await page.getByTestId('globalview-view-toggle-map').click();
      const emptyState = page.locator('[data-testid="metadata-panel-empty"]');
      await expect(emptyState).toBeVisible();
      await expect(emptyState).toContainText('Select a project to see details');
    });
  });

  test('Open project is the standard primary action (accent), not yellow, in every theme', async ({ page }) => {
    await page.getByTestId(ALPHA_CARD).click();
    const open = page.getByTestId('metadata-open-button');
    await expect(open).toBeVisible({ timeout: 5000 });
    const themes = ['dark', 'light', 'cave', 'dracula', 'one_dark', 'solarized_light',
      'catppuccin_latte', 'nord', 'catppuccin_mocha', 'kaleido_eink'];
    for (const theme of themes) {
      await page.evaluate((name) => { document.documentElement.dataset.theme = name; }, theme);
      // The button animates colour changes (transition-colors), so poll until it
      // settles on the theme's tokens, resolved through an un-animated probe.
      await expect.poll(() => open.evaluate((el) => {
        const probe = document.createElement('div');
        probe.style.backgroundColor = 'var(--cc-accent)';
        probe.style.color = 'var(--cc-accent-on)';
        document.body.appendChild(probe);
        const want = getComputedStyle(probe);
        const got = getComputedStyle(el);
        const same = got.backgroundColor === want.backgroundColor && got.color === want.color;
        probe.remove();
        return same;
      }), { message: theme }).toBe(true);
    }
    // The same Button variant every dialog's main action uses.
    await expect(open).toHaveAttribute('data-primary-action', '');
  });

  test.describe('Project Selection from List', () => {
    test('should show project details when card is clicked', async ({ page }) => {
      // Click on a system card to select it
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();

      // Wait for metadata to appear
      const metadataName = page.locator('[data-testid="metadata-name"]');
      await expect(metadataName).toBeVisible({ timeout: 5000 });
      await expect(metadataName).toContainText('Cave System Alpha');
    });

    test('should display all metadata fields for selected project', async ({ page }) => {
      // Select the first project
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();

      // Check all metadata fields
      await expect(page.locator('[data-testid="metadata-name"]')).toContainText('Cave System Alpha');
      await expect(page.locator('[data-testid="metadata-location"]')).toContainText('Yucatan, Mexico');
      await expect(page.locator('[data-testid="metadata-file-path"]')).toHaveAttribute('data-full-path', '/projects/cave-alpha.stalx');
      await expect(page.locator('[data-testid="metadata-sections"]')).toContainText('5');
      await expect(page.locator('[data-testid="metadata-length"]')).toBeVisible();
      await expect(page.locator('[data-testid="metadata-gps"]')).toBeVisible();
    });

    test('long project name scrolls on hover instead of a tooltip', async ({ page }) => {
      await page.getByTestId(ALPHA_CARD).click();
      await expect(page.locator('[data-testid="metadata-name"]')).toBeVisible();
      // Force the name field narrow so the name overflows its space.
      await page.addStyleTag({ content: '[data-testid="metadata-name"] [data-full-text]{max-width:50px !important}' });
      const marquee = page.locator('[data-testid="metadata-name"] [data-full-text]');
      await marquee.hover();
      const overlay = marquee.locator('[data-elided-overlay]');
      await expect(overlay).toHaveCount(1);
      // Out of flow (won't disturb the row), animating, and no tooltip.
      expect(await overlay.evaluate((el) => getComputedStyle(el).position)).toBe('absolute');
      expect(await overlay.evaluate((el) => getComputedStyle(el).animationName)).toContain('elided-path-scroll');
      await expect(page.locator('[role="tooltip"]')).toHaveCount(0);
    });

    test('should show selected state on card', async ({ page }) => {
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();

      // Card should have ring class indicating selection
      await expect(card).toHaveClass(/ring-accent/);
    });

    test('should update metadata when different card is selected', async ({ page }) => {
      // Select first project
      const alphaCard = page.getByTestId(ALPHA_CARD);
      await alphaCard.click();
      await expect(page.locator('[data-testid="metadata-name"]')).toContainText('Cave System Alpha');

      // Select second project
      const betaCard = page.getByTestId(BETA_CARD);
      await betaCard.click();
      await expect(page.locator('[data-testid="metadata-name"]')).toContainText('Underwater Cave Beta');
      await expect(page.locator('[data-testid="metadata-location"]')).toContainText('Florida, USA');
    });
  });

  test.describe('Project Selection from Map', () => {
    test.beforeEach(async ({ page }) => {
      // Switch to Map tab
      const mapTab = page.locator('[data-testid="globalview-view-toggle-map"]');
      await mapTab.click();
      await expect(page.getByTestId('globalview-map')).toBeVisible({ timeout: 5000 });

      // Switch to markers mode so markers stay visible (survey mode hides
      // markers once geometry loads, which can cause flaky click failures)
      const toggle = page.getByTestId('view-mode-toggle');
      await expect(toggle).toBeVisible({ timeout: 5000 });
      await toggle.click();

      await expect(page.locator('.globalview-system-marker')).toHaveCount(3, { timeout: 5000 });
    });

    test('should show project details when map marker is clicked', async ({ page }) => {
      // Click on a marker (in markers mode, all markers are reliably visible)
      const marker = page.getByTestId(ALPHA_MARKER);
      await marker.click();

      // Wait for metadata to appear
      const metadataName = page.locator('[data-testid="metadata-name"]');
      await expect(metadataName).toBeVisible({ timeout: 5000 });
      await expect(metadataName).toContainText('Cave System Alpha');
    });
  });

  test.describe('Panel Actions', () => {
    test.beforeEach(async ({ page }) => {
      // Select a project first
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();
      await page.waitForSelector('[data-testid="metadata-name"]', { timeout: 5000 });
    });

    test('should have Open Project button', async ({ page }) => {
      const openButton = page.locator('[data-testid="metadata-open-button"]');
      await expect(openButton).toBeVisible();
      await expect(openButton).toContainText('Open project');
    });

    test('should have Show in Folder button', async ({ page }) => {
      const folderButton = page.locator('[data-testid="metadata-show-folder-button"]');
      await expect(folderButton).toBeVisible();
      await expect(folderButton).toContainText('Show in folder');
    });

    test('should have Remove from Recent button', async ({ page }) => {
      const removeButton = page.locator('[data-testid="metadata-remove-button"]');
      await expect(removeButton).toBeVisible();
      await expect(removeButton).toContainText('Remove from recent');
    });

    test('should open project when Open Project button is clicked', async ({ page }) => {
      const openButton = page.locator('[data-testid="metadata-open-button"]');
      await openButton.click();

      // Should transition to SingleSystemView (indicated by canvas being visible)
      await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });
    });

    test('should have correct aria-label on action buttons', async ({ page }) => {
      const openButton = page.locator('[data-testid="metadata-open-button"]');
      await expect(openButton).toHaveAttribute('aria-label', /Open Cave System Alpha/);

      const folderButton = page.locator('[data-testid="metadata-show-folder-button"]');
      await expect(folderButton).toHaveAttribute('aria-label', /Show Cave System Alpha in folder/);

      const removeButton = page.locator('[data-testid="metadata-remove-button"]');
      await expect(removeButton).toHaveAttribute('aria-label', /Remove Cave System Alpha from recent/);
    });
  });

  test.describe('Panel Collapse/Expand', () => {
    test.beforeEach(async ({ page }) => {
      // The list-view panel only appears once a project is selected (past the
      // double-click debounce), so select one before exercising collapse/expand.
      await page.getByTestId(ALPHA_CARD).click();
      await page.waitForSelector('[data-testid="metadata-panel"]', { timeout: 5000 });
    });

    test('should collapse panel when collapse button is clicked', async ({ page }) => {
      // Panel should start expanded
      const panel = page.locator('[data-testid="metadata-panel"]');
      await expect(panel).toBeVisible();

      // Click collapse button
      const collapseButton = page.locator('[data-testid="metadata-panel-collapse-button"]');
      await collapseButton.click();

      // Panel should be collapsed (showing collapsed variant)
      const collapsedPanel = page.locator('[data-testid="metadata-panel-collapsed"]');
      await expect(collapsedPanel).toBeVisible();
      await expect(panel).not.toBeVisible();
    });

    test('should expand panel when expand button is clicked', async ({ page }) => {
      // Collapse first
      const collapseButton = page.locator('[data-testid="metadata-panel-collapse-button"]');
      await collapseButton.click();

      // Click expand button
      const expandButton = page.locator('[data-testid="metadata-panel-expand-button"]');
      await expect(expandButton).toBeVisible();
      await expandButton.click();

      // Panel should be expanded again
      const panel = page.locator('[data-testid="metadata-panel"]');
      await expect(panel).toBeVisible();
    });

    test('should have correct aria-expanded state', async ({ page }) => {
      const collapseButton = page.locator('[data-testid="metadata-panel-collapse-button"]');
      await expect(collapseButton).toHaveAttribute('aria-expanded', 'true');

      await collapseButton.click();

      const expandButton = page.locator('[data-testid="metadata-panel-expand-button"]');
      await expect(expandButton).toHaveAttribute('aria-expanded', 'false');
    });
  });

  test.describe('Panel Accessibility', () => {
    test.beforeEach(async ({ page }) => {
      // List-view panel is selection-gated; select a project so it's present.
      await page.getByTestId(ALPHA_CARD).click();
      await page.waitForSelector('[data-testid="metadata-panel"]', { timeout: 5000 });
    });

    test('should have complementary role', async ({ page }) => {
      const panel = page.locator('[data-testid="metadata-panel"]');
      await expect(panel).toHaveAttribute('role', 'complementary');
    });

    test('should have accessible label', async ({ page }) => {
      const panel = page.locator('[data-testid="metadata-panel"]');
      await expect(panel).toHaveAttribute('aria-label', 'Project metadata panel');
    });

    test('should have heading for the panel', async ({ page }) => {
      const heading = page.locator('#metadata-panel-title');
      await expect(heading).toBeVisible();
      await expect(heading).toContainText('Project details');
    });
  });

  test.describe('GPS Display', () => {
    test('should display GPS coordinates when project has GPS', async ({ page }) => {
      // Select a project with GPS
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();

      const gpsField = page.locator('[data-testid="metadata-gps"]');
      await expect(gpsField).toBeVisible();
      // Should show formatted GPS (not "Not set")
      await expect(gpsField).not.toContainText('Not specified');
    });

    test('should display "Not set" when project has no GPS', async ({ page }) => {
      // Select the project without GPS
      const card = page.getByTestId(LOCAL_CARD);
      await card.click();

      const gpsField = page.locator('[data-testid="metadata-gps"]');
      await expect(gpsField).toBeVisible();
      await expect(gpsField).toContainText('Not specified');
    });
  });

  test.describe('Formatting', () => {
    test('should format length in km for large values', async ({ page }) => {
      // Select a project with large length
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();

      const lengthField = page.locator('[data-testid="metadata-length"]');
      await expect(lengthField).toBeVisible();
      // 1250.5m should be formatted as km
      await expect(lengthField).toContainText('km');
    });
  });

  test.describe('Project Settings button', () => {
    test('has no Project Settings button (settings live in the open project)', async ({ page }) => {
      const card = page.getByTestId(ALPHA_CARD);
      await card.click();
      await expect(page.locator('[data-testid="metadata-name"]')).toBeVisible();

      const open = page.locator('[data-testid="metadata-open-button"]');
      await expect(open).toBeVisible();

      // Per-project settings require opening the project, so the recents panel
      // no longer offers a Project Settings shortcut.
      await expect(page.locator('[data-testid="metadata-project-settings-button"]')).toHaveCount(0);
    });
  });
});
