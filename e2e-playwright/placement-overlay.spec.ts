import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * This spec's own page globals: the flag the placement's cancel callback sets, and
 * the keydown `addEventListener` counter the listener test installs before load.
 */
type PlacementWindow = E2EWindow & { __PLACEMENT_CANCELLED__?: boolean; __keydownAdds__: number };

test.describe('Placement Overlay Coverage', () => {
  // #576 NF-W13FE-CSS-D1 (#605): the docked panels dim to gray WITHOUT a CSS filter (on
  // WebKitGTK a filtered subtree repaints through a GPU promise image that a memory release
  // can deadlock). Their tokens, inherited text and section swatches are gray instead.
  test('dims the docked panels to gray with no CSS filter', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Placement Dim');
    await seedProjectWithSections(page, [
      { name: 'SectionA', direction: 'in', color: '#ff6b6b', shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 }] },
    ]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await page.locator('[data-testid^="section-item-"]').first().click();
    await expect(page.getByTestId('detail-panel')).toContainText('SectionA');
    await page.evaluate(() => {
      (window as E2EWindow).__E2E_TEST__.uiStore.getState().startPlacementMode(
        {
          name: 'Placement Section', surveyor: null, date: null, color: '#ff6b6b', direction: 'in', quality: 1,
          shots: [{
            azimuth_in: 45, azimuth_out: 225, length: 12, depth_in: -2, depth_out: -3, pitch_in: null, pitch_out: null,
            left: null, right: null, up: null, down: null, comment: null, kind: 'Normal', marker_index: null, temperature: null,
          }],
        },
        0,
        undefined,
        () => {},
      );
    });

    const paint = await page.evaluate(() => {
      const gray = (c: string) => {
        const m = /^rgba?\(([\d.]+)[, ]+([\d.]+)[, ]+([\d.]+)/.exec(c.trim());
        return m !== null && Math.abs(Number(m[1]) - Number(m[2])) < 1 && Math.abs(Number(m[2]) - Number(m[3])) < 1;
      };
      return ['sections-panel', 'detail-panel'].map((id) => {
        const panel = document.querySelector(`[data-testid="${id}"]`) as HTMLElement;
        const filters: string[] = [];
        for (let el: HTMLElement | null = panel; el; el = el.parentElement) {
          if (getComputedStyle(el).filter !== 'none') filters.push(el.className.toString());
        }
        const swatch = panel.querySelector('[style*="background-color"]') as HTMLElement | null;
        return {
          id,
          filters,
          accentGray: gray(getComputedStyle(panel).getPropertyValue('--cc-accent')),
          textGray: gray(getComputedStyle(panel).color),
          swatchGray: swatch ? gray(getComputedStyle(swatch).backgroundColor) : 'no swatch',
          dimmed: getComputedStyle(panel.parentElement as HTMLElement).opacity,
        };
      });
    });
    console.log(`[dim] ${JSON.stringify(paint)}`);
    for (const p of paint) {
      expect(p.filters, `${p.id}: no filter on the panel or its ancestors`).toEqual([]);
      expect(p, p.id).toMatchObject({ accentGray: true, textGray: true, swatchGray: true, dimmed: '0.45' });
    }
  });

  test('shows placement preview and cancels on Escape @smoke', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Placement Overlay');
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    await page.evaluate(() => {
      (window as PlacementWindow).__PLACEMENT_CANCELLED__ = false;
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      ui.startPlacementMode(
        {
          name: 'Placement Section',
          surveyor: null,
          date: null,
          color: '#ff6b6b',
          direction: 'in',
          quality: 1,
          shots: [
            {
              azimuth_in: 45,
              azimuth_out: 225,
              length: 12,
              depth_in: -2,
              depth_out: -3,
              pitch_in: null,
              pitch_out: null,
              left: null,
              right: null,
              up: null,
              down: null,
              comment: null,
              kind: 'Normal',
              marker_index: null,
              temperature: null,
            },
            {
              azimuth_in: 90,
              azimuth_out: 270,
              length: 8,
              depth_in: -3,
              depth_out: -4,
              pitch_in: null,
              pitch_out: null,
              left: null,
              right: null,
              up: null,
              down: null,
              comment: null,
              kind: 'Normal',
              marker_index: null,
              temperature: null,
            },
          ],
        },
        0,
        undefined,
        () => {
          (window as PlacementWindow).__PLACEMENT_CANCELLED__ = true;
        }
      );
    });

    const mapCanvas = page.getByTestId('map-canvas');
    await expect(mapCanvas).toBeVisible();

    const box = await mapCanvas.boundingBox();
    if (!box) throw new Error('Map canvas not measurable');

    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);

    await expect(page.getByTestId('placement-preview')).toBeVisible();
    await expect(page.getByTestId('placement-mode-indicator')).toBeVisible();

    await page.keyboard.press('Escape');

    await expect(page.getByTestId('placement-mode-indicator')).not.toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('placement-preview')).not.toBeVisible({ timeout: 5000 });

    const cancelled = await page.evaluate(() => (window as PlacementWindow).__PLACEMENT_CANCELLED__);
    expect(cancelled).toBe(true);
  });

  // #576 D9: every placement pointer move replaces `placementMode` in the store.
  // Effects that only need "is placement on" used to depend on the object, so the
  // window Escape listener was torn down and re-added on every move (~60x/s).
  test('pointer moves during placement do not re-register the Escape listener', async ({ page }) => {
    await page.addInitScript(() => {
      const w = window as unknown as { __keydownAdds__: number };
      w.__keydownAdds__ = 0;
      const add = window.addEventListener.bind(window);
      window.addEventListener = ((type: string, ...rest: unknown[]) => {
        if (type === 'keydown') w.__keydownAdds__++;
        return (add as (...args: unknown[]) => void)(type, ...rest);
      }) as typeof window.addEventListener;
    });
    await setupMockedPage(page);
    await createProject(page, 'Placement Listener');
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    await page.evaluate(() => {
      (window as PlacementWindow).__PLACEMENT_CANCELLED__ = false;
      (window as E2EWindow).__E2E_TEST__.uiStore.getState().startPlacementMode(
        {
          name: 'Placement Section',
          surveyor: null,
          date: null,
          color: '#ff6b6b',
          direction: 'in',
          quality: 1,
          shots: [
            {
              azimuth_in: 45, azimuth_out: 225, length: 12, depth_in: -2, depth_out: -3,
              pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
              comment: null, kind: 'Normal', marker_index: null, temperature: null,
            },
          ],
        },
        0,
        undefined,
        () => {
          (window as PlacementWindow).__PLACEMENT_CANCELLED__ = true;
        }
      );
    });

    const box = await page.getByTestId('map-canvas').boundingBox();
    if (!box) throw new Error('Map canvas not measurable');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await expect(page.getByTestId('placement-preview')).toBeVisible();

    const addsBefore = await page.evaluate(() => (window as PlacementWindow).__keydownAdds__);
    for (let i = 1; i <= 30; i++) {
      await page.mouse.move(box.x + box.width / 2 + i * 3, box.y + box.height / 2);
    }
    await expect(page.getByTestId('placement-preview')).toBeVisible();
    const addsAfter = await page.evaluate(() => (window as PlacementWindow).__keydownAdds__);
    expect(addsAfter - addsBefore).toBe(0);

    // The one listener still cancels.
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('placement-mode-indicator')).not.toBeVisible({ timeout: 5000 });
    expect(await page.evaluate(() => (window as PlacementWindow).__PLACEMENT_CANCELLED__)).toBe(true);
  });
});
