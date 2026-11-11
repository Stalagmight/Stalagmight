/**
 * E2E: the Closures Computed View read-only pill offers "Resume editing", which
 * switches back to the Raw Data View (the same write the Display menu toggle
 * makes). The SpeleoDB pill's Resume editing is covered in
 * speleodb-auto-lock.spec.ts, next to the lock flow it drives.
 *
 * Also the pills' layout at several map widths and UI zooms: the lock glyph stays
 * on the first line of the words, the action stays beside the words (never a row
 * of its own), and nothing runs under the map's zoom control.
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */
import { test, expect, type Locator, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithSections, waitForMapIdle } from './helpers/app';
import { MAP_MIN_WIDTH } from '../src/components/SingleSystemView/layoutConstants';

type Stores = {
  __E2E_TEST__: {
    projectStore: { getState: () => { project: { sections: Array<{ id: number }>; loop_closures: unknown[] } | null }; setState: (s: unknown) => void };
    uiStore: { getState: () => { geometryMode: string; setGeometryMode: (m: string, source?: string) => void; setAppMode: (m: string) => void } };
  };
};

/** One closure the solve would actually move: without it MapCanvas reverts the view to measured. */
async function enterClosureView(page: Page): Promise<void> {
  await page.evaluate(() => {
    const { projectStore, uiStore } = (window as unknown as Stores).__E2E_TEST__;
    const project = projectStore.getState().project!;
    const secId = project.sections[0]!.id;
    projectStore.setState({ project: { ...project, loop_closures: [...project.loop_closures, {
      id: 1, station_a: { section_id: secId, station_index: 0 }, station_b: { section_id: secId, station_index: 2 },
      path_length: 22, misclosure_distance: 0.3, error_percentage: 1.4, horizontal_misclosure: 0.25,
      vertical_misclosure: 0.15, strategy: 'Batch', is_aligned: false,
    }] } });
    uiStore.getState().setGeometryMode('closureAdjusted', 'e2e');
  });
}

const geometryMode = (page: Page) => page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.uiStore.getState().geometryMode);

/**
 * A narrow window with a wide pinned Sections panel: the panel yields until the map
 * column is exactly MAP_MIN_WIDTH.
 */
async function squeezeMapToMinimum(page: Page): Promise<void> {
  await page.setViewportSize({ width: 806, height: 551 });
  await page.evaluate(() => {
    (window as unknown as { __E2E_TEST__: { uiStore: { setState: (s: unknown) => void } } }).__E2E_TEST__.uiStore
      .setState({ sectionsPanelMode: 'pin', sectionsPanelOpen: true, sectionsPanelWidth: 700 });
  });
  await expect.poll(async () => Math.round((await page.getByTestId('map-canvas').boundingBox())!.width))
    .toBe(MAP_MIN_WIDTH);
}

type Box = { x: number; y: number; width: number; height: number };
const intersects = (a: Box, b: Box) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

/** The pill does not overlap the +/- control, and its action is whole, inside the map and on top. */
async function expectClearOfZoomControl(page: Page, pill: Locator, action: Locator): Promise<void> {
  const zoom = (await page.getByTestId('map-canvas').locator('.maplibregl-ctrl-top-right .maplibregl-ctrl-group').boundingBox())!;
  const map = (await page.getByTestId('map-canvas').boundingBox())!;
  const pillBox = (await pill.boundingBox())!;
  const actionBox = (await action.boundingBox())!;
  expect(intersects(pillBox, zoom), `pill ${JSON.stringify(pillBox)} vs zoom ${JSON.stringify(zoom)}`).toBe(false);
  expect(actionBox.x).toBeGreaterThanOrEqual(map.x);
  expect(actionBox.x + actionBox.width).toBeLessThanOrEqual(zoom.x);
  // Whole: as wide as its own content, not clipped by the pill.
  expect(await action.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  // On top at its centre (nothing covers it), so a click lands on it.
  const hit = await page.evaluate(([x, y]) => document.elementFromPoint(x!, y!)?.closest('button')?.dataset.testid ?? null,
    [actionBox.x + actionBox.width / 2, actionBox.y + actionBox.height / 2]);
  expect(hit).toBe(await action.getAttribute('data-testid'));
}

test.describe('Closures Computed View pill', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic closure pill');
    await seedProjectWithSections(page, [{
      name: 'Synthetic loop', direction: 'in',
      shots: [
        { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
        { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      ],
    }]);
    await waitForMapIdle(page);
  });

  test('Resume editing switches back to the Raw Data View', async ({ page }) => {
    await enterClosureView(page);
    const pill = page.getByTestId('geometry-readonly-banner');
    await expect(pill).toBeVisible();
    const resume = pill.getByTestId('geometry-readonly-resume');
    await expect(resume).toHaveText('Resume editing');
    await resume.click();
    await expect.poll(() => geometryMode(page)).toBe('measured');
    await expect(pill).toHaveCount(0);
  });

  test('at the minimum map width the pills stay clear of the zoom control, actions whole', async ({ page }) => {
    // The map floor is in CSS px, so this one layout stands for every UI zoom: a
    // larger zoom only shrinks the CSS viewport, and the side panels yield until the
    // map is at MAP_MIN_WIDTH (what 1024x700 at 127% or 1280x720 at 150% produced).
    await squeezeMapToMinimum(page);
    await enterClosureView(page);
    const closure = page.getByTestId('geometry-readonly-banner');
    await expect(closure).toBeVisible();
    await expectClearOfZoomControl(page, closure, closure.getByTestId('geometry-readonly-resume'));

    // The SpeleoDB pill (signed out: the longest wording, with Connect) in the same row.
    await page.evaluate(() => {
      const w = window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } };
      w.__E2E_TEST__.speleoStore.setState({ session: null, link: {
        permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null, lockedByOther: false,
        hasUnpushedEdits: false, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
        noAccess: false, serverError: null,
      } });
    });
    const speleo = page.getByTestId('speleodb-readonly-banner');
    await expect(speleo).toBeVisible();
    await expectClearOfZoomControl(page, speleo, speleo.getByTestId('speleodb-readonly-login'));

    // Back to the closure pill: its action is not just visible but the thing clicked.
    await page.evaluate(() => {
      const w = window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } };
      w.__E2E_TEST__.speleoStore.setState({ link: null, projectLinkKind: 'local' });
    });
    await closure.getByTestId('geometry-readonly-resume').click();
    await expect.poll(() => geometryMode(page)).toBe('measured');
  });

  test('no Resume where the computed view is forced (Plan)', async ({ page }) => {
    await enterClosureView(page);
    await expect(page.getByTestId('geometry-readonly-resume')).toBeVisible();
    await page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.uiStore.getState().setAppMode('plan'));
    await expect(page.getByTestId('geometry-readonly-banner')).toHaveCount(0);
    await expect(page.getByTestId('geometry-readonly-resume')).toHaveCount(0);
  });
});

/** Seed the SpeleoDB pill: read-only, signed out (Connect) or signed in and able to resume (Resume editing). */
async function showSpeleoPill(page: Page, signedIn: boolean): Promise<Locator> {
  await page.evaluate((signedIn) => {
    const w = window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } };
    w.__E2E_TEST__.speleoStore.setState({
      session: signedIn ? { loggedIn: true, userEmail: 'me@example.com', endpoint: 'https://example.com' } : null,
      link: {
        permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null, lockedByOther: false,
        hasUnpushedEdits: false, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
        noAccess: false, serverError: null,
      },
    });
  }, signedIn);
  const pill = page.getByTestId('speleodb-readonly-banner');
  await expect(pill.getByTestId(signedIn ? 'speleodb-readonly-resume' : 'speleodb-readonly-login')).toBeVisible();
  return pill;
}

async function clearSpeleoPill(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } };
    w.__E2E_TEST__.speleoStore.setState({ link: null, projectLinkKind: 'local' });
  });
}

type Rect = { left: number; right: number; top: number; bottom: number };
/** Where a pill's glyph, the first line of its words, the words and its action are drawn. */
async function pillLayout(pill: Locator): Promise<{ glyph: Rect; firstLine: Rect; words: Rect; lines: number; action: Rect | null }> {
  return pill.evaluate((el) => {
    const rect = (r: DOMRect) => ({ left: r.left, right: r.right, top: r.top, bottom: r.bottom });
    const id = el.dataset.testid!;
    const words = el.querySelector<HTMLElement>(`[data-testid="${id}-body"] > span`)!;
    const range = document.createRange();
    range.selectNodeContents(words);
    const lineRects = [...range.getClientRects()].filter((r) => r.width > 0);
    const action = el.querySelector('button');
    return {
      glyph: rect(el.querySelector(`[data-testid="${id}-glyph"]`)!.getBoundingClientRect()),
      firstLine: rect(lineRects[0]!),
      words: rect(words.getBoundingClientRect()),
      lines: new Set(lineRects.map((r) => Math.round(r.top))).size,
      action: action ? rect(action.getBoundingClientRect()) : null,
    };
  });
}

const overlapsVertically = (a: Rect, b: Rect) => a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;

/**
 * The glyph shares the first line of the words (and sits left of them); the action
 * is beside the words (`beside`) or, below the pill's hard minimum, on the next
 * line starting under the words - never a third row of its own.
 */
async function expectPillLayout(pill: Locator, action: 'beside' | 'under'): Promise<void> {
  const l = await pillLayout(pill);
  const at = JSON.stringify(l);
  expect(overlapsVertically(l.glyph, l.firstLine), `glyph on the first line: ${at}`).toBe(true);
  expect(Math.abs((l.glyph.top + l.glyph.bottom) / 2 - (l.firstLine.top + l.firstLine.bottom) / 2), at).toBeLessThanOrEqual(3);
  expect(l.glyph.right, at).toBeLessThanOrEqual(l.words.left);
  if (!l.action) return;
  if (action === 'beside') {
    expect(overlapsVertically(l.action, l.words), `action beside the words: ${at}`).toBe(true);
    expect(l.action.left, at).toBeGreaterThanOrEqual(l.words.right - 0.5);
  } else {
    expect(l.action.top, `action under the words: ${at}`).toBeGreaterThanOrEqual(l.words.bottom - 0.5);
    expect(Math.abs(l.action.left - l.words.left), `action aligned under the words: ${at}`).toBeLessThanOrEqual(1);
  }
}

/** Every pill the map can show, each with its action: SpeleoDB signed out, SpeleoDB able to resume, Closures. */
async function forEachPill(page: Page, check: (pill: Locator, action: Locator, name: string) => Promise<void>): Promise<void> {
  for (const signedIn of [false, true]) {
    const pill = await showSpeleoPill(page, signedIn);
    await check(pill, pill.getByTestId(signedIn ? 'speleodb-readonly-resume' : 'speleodb-readonly-login'), `SpeleoDB ${signedIn ? 'resume' : 'connect'}`);
  }
  await clearSpeleoPill(page);
  await enterClosureView(page);
  const closure = page.getByTestId('geometry-readonly-banner');
  await expect(closure).toBeVisible();
  await check(closure, closure.getByTestId('geometry-readonly-resume'), 'Closures');
}

test.describe('read-only pill layout', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic pill layout');
    await seedProjectWithSections(page, [{
      name: 'Synthetic loop', direction: 'in',
      shots: [
        { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
        { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      ],
    }]);
    await waitForMapIdle(page);
  });

  test('a wide window: glyph, words (one line) and action on one row', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await forEachPill(page, async (pill, action, name) => {
      await expectPillLayout(pill, 'beside');
      expect((await pillLayout(pill)).lines, name).toBe(1);
      await expectClearOfZoomControl(page, pill, action);
    });
  });

  test('the minimum map width: the words wrap in their own column, the action stays beside them', async ({ page }) => {
    await squeezeMapToMinimum(page);
    await forEachPill(page, async (pill, action) => {
      await expectPillLayout(pill, 'beside');
      await expectClearOfZoomControl(page, pill, action);
    });
  });

  test('below the pill\'s hard minimum the action goes under the words, aligned with them', async ({ page }) => {
    await squeezeMapToMinimum(page);
    // Two notices sharing the row can leave a pill this narrow; cap one to stand in.
    await page.addStyleTag({ content: '[data-testid="speleodb-readonly-banner"], [data-testid="geometry-readonly-banner"] { max-width: 150px; }' });
    await forEachPill(page, async (pill, action) => {
      await expectPillLayout(pill, 'under');
      await expectClearOfZoomControl(page, pill, action);
    });
  });
});

test.describe('read-only pill layout at 150% UI zoom', () => {
  // A 1280x720 window at 150% webview zoom: the CSS viewport is the window / 1.5.
  test.use({ viewport: { width: 853, height: 480 }, deviceScaleFactor: 1.5 });

  test('the action stays beside the words and clear of the zoom control', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic pill zoom');
    await seedProjectWithSections(page, [{
      name: 'Synthetic loop', direction: 'in',
      shots: [
        { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
        { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      ],
    }]);
    await waitForMapIdle(page);
    // The side panels yield until the map is at its floor.
    await expect.poll(async () => Math.round((await page.getByTestId('map-canvas').boundingBox())!.width))
      .toBeLessThanOrEqual(MAP_MIN_WIDTH + 40);
    await forEachPill(page, async (pill, action) => {
      await expectPillLayout(pill, 'beside');
      await expectClearOfZoomControl(page, pill, action);
    });
  });
});
