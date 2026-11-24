import { test, expect, type Page, type Locator } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithLines, waitForMapReady, waitForNoDialogs, type LineSeed } from './helpers/app';

/**
 * Prose pills keep the line pitch (RS-PROSE). A sentence that names
 * stations and lines draws each name as an inline prose pill; the paragraph's
 * lines must stand exactly as far apart as when the same sentence is plain
 * text. Measured in the browser: the paragraph is narrowed so it wraps onto
 * several lines, the top of each line is read from its text's client rects
 * (`Range.getClientRects`, clustered into lines), and the gaps between
 * consecutive lines are compared, with pills (Full highlighting) and as plain
 * text (highlighting off), on the Bulk Delete dialog's summary and on a
 * Warnings tab message holding a two-tone shot (RS-OWNERSHIP: an end of
 * another line). Every gap must equal the line-height within 0.5 px.
 */

const LONG_LINE = 'Main Passage of the Northern Sump System';
const lineSeeds: LineSeed[] = [
  {
    name: LONG_LINE,
    direction: 'in',
    color: '#ffe600',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: 0, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -2, depth_out: -4 },
      { azimuth_in: 135, azimuth_out: 315, length: 8, depth_in: -4, depth_out: -6 },
      { azimuth_in: 180, azimuth_out: 0, length: 15, depth_in: -6, depth_out: -8 },
    ],
  },
  {
    name: 'Side Passage',
    direction: 'in',
    color: '#1f4fff',
    shots: [{ azimuth_in: 10, azimuth_out: 190, length: 6, depth_in: 0, depth_out: -1 }],
  },
];

/** The gaps between consecutive lines of `el`'s text, and its line-height. */
async function linePitch(el: Locator): Promise<{ pitch: number[]; lineHeight: number; lines: number }> {
  return el.evaluate((root) => {
    const cs = getComputedStyle(root);
    const fs = parseFloat(cs.fontSize);
    const lineHeight = parseFloat(cs.lineHeight);
    const rects: Array<{ top: number; main: boolean }> = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const parent = n.parentElement;
      // The sr-only words sit in a 1 px box; the superscript is raised on purpose.
      if (!parent || parent.closest('.sr-only') || parent.closest('.sh-ppill-sid')) continue;
      if (!(n.textContent ?? '').trim()) continue;
      const main = Math.abs(parseFloat(getComputedStyle(parent).fontSize) - fs) < 0.01;
      const range = document.createRange();
      range.selectNodeContents(n);
      for (const r of range.getClientRects()) if (r.width > 0.5) rects.push({ top: r.top, main });
    }
    rects.sort((a, b) => a.top - b.top);
    const lines: Array<{ first: number; main: number | null }> = [];
    for (const r of rects) {
      const last = lines[lines.length - 1];
      if (!last || r.top > last.first + lineHeight / 2) lines.push({ first: r.top, main: r.main ? r.top : null });
      else if (r.main && (last.main === null || r.top < last.main)) last.main = r.top;
    }
    const pitch: number[] = [];
    for (let i = 1; i < lines.length; i++) {
      const a = lines[i - 1]!.main;
      const b = lines[i]!.main;
      if (a !== null && b !== null) pitch.push(b - a);
    }
    return { pitch, lineHeight, lines: lines.length };
  });
}

async function setHighlighting(page: Page, level: 'full' | 'off'): Promise<void> {
  await page.evaluate((lvl) => {
    (window as unknown as { __E2E_TEST__: { uiStore: { setState: (s: unknown) => void } } }).__E2E_TEST__.uiStore.setState({
      syntacticLevel: lvl,
    });
  }, level);
}

/** Measures `el` with pills, then as plain text, and checks both against its line-height. */
async function expectSamePitch(page: Page, el: Locator): Promise<void> {
  await setHighlighting(page, 'full');
  await expect(el.locator('.sh-ppill').first()).toBeVisible();
  const withPills = await linePitch(el);
  await setHighlighting(page, 'off');
  await expect(el.locator('.sh-ppill')).toHaveCount(0);
  const plain = await linePitch(el);
  await setHighlighting(page, 'full');

  // Wrapped onto several lines both ways, so there are gaps to compare.
  expect(withPills.lines).toBeGreaterThanOrEqual(3);
  expect(plain.lines).toBeGreaterThanOrEqual(3);
  expect(withPills.pitch.length).toBeGreaterThanOrEqual(2);
  expect(plain.pitch.length).toBeGreaterThanOrEqual(2);
  expect(withPills.lineHeight).toBe(plain.lineHeight);
  for (const gap of [...withPills.pitch, ...plain.pitch]) {
    expect(Math.abs(gap - withPills.lineHeight)).toBeLessThanOrEqual(0.5);
  }
  const mean = (xs: number[]) => xs.reduce((s, x) => s + x, 0) / xs.length;
  expect(Math.abs(mean(withPills.pitch) - mean(plain.pitch))).toBeLessThanOrEqual(0.5);
}

test.describe('Prose pills keep the line pitch', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Prose Line Pitch');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    // Narrow the measured paragraphs so they wrap onto several lines; a rule,
    // not an inline style, so it survives the re-render between the two levels.
    await page.addStyleTag({
      content:
        '[data-testid="bulk-delete-summary"] { width: 170px; }' +
        ' [data-testid="warnings-content"] .text-sm { display: inline-block; width: 190px; }',
    });
  });

  test('the Bulk Delete summary has the same line pitch with pills as plain text', async ({ page }) => {
    const ids = await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { project: { lines: Array<{ id: number; name: string; shots: Array<{ id: number }> }> } } } };
      }).__E2E_TEST__;
      const line = e2e.projectStore.getState().project.lines[0]!;
      return { lineId: line.id, start: line.shots[1]!.id, end: line.shots[2]!.id, name: line.name };
    });
    await page.evaluate((d) => {
      (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { openDialog: (n: string, x: unknown) => void } } } }).__E2E_TEST__.uiStore
        .getState()
        .openDialog('bulk-delete', {
          lineId: d.lineId,
          startShotId: d.start,
          endShotId: d.end,
          shotCount: 2,
          kind: 'interior',
          lineName: d.name,
          startStationIndex: 1,
          endStationIndex: 3,
        });
    }, ids);
    await expect(page.getByTestId('bulk-delete-dialog')).toBeVisible();
    await expectSamePitch(page, page.getByTestId('bulk-delete-summary'));
  });

  test('a Warnings tab message has the same line pitch with pills as plain text', async ({ page }) => {
    await page.evaluate((lineName) => {
      const e2e = (window as unknown as {
        __E2E_TEST__: {
          projectStore: {
            getState: () => { project: { lines: Array<{ id: number; name: string; shots: Array<{ id: number }> }> } };
            setState: (s: unknown) => void;
          };
        };
      }).__E2E_TEST__;
      const line = e2e.projectStore.getState().project.lines[0]!;
      const side = e2e.projectStore.getState().project.lines[1]!;
      const station = (index: number, name: string) => ({
        kind: 'station',
        text: `${lineName} / ${name}`,
        line_id: line.id,
        context: lineName,
        name,
        station_id: 100 + index,
        slot: { line_id: line.id, station_index: index },
      });
      const parts = [
        { kind: 'text', text: 'The depth readings at ' },
        station(2, '2'),
        { kind: 'text', text: ', where 2 lines meet, differ by 4.0 m; the shots from ' },
        station(1, '1'),
        { kind: 'text', text: ' and ' },
        station(3, '3'),
        { kind: 'text', text: ' disagree, and the shot ' },
        {
          kind: 'shot',
          text: `${lineName}: 1 (${side.name}) \u2192 2`,
          line_id: line.id,
          context: lineName,
          from: { name: '1', home_line: side.id },
          to: { name: '2' },
        },
        { kind: 'text', text: ' is one of them. Check the depths of the shots that meet here.' },
      ];
      const message = parts.map((p) => p.text).join('');
      e2e.projectStore.setState({
        warnings: [
          {
            lineId: line.id,
            shotId: line.shots[1]!.id,
            shotIndex: 1,
            code: 'JUNCTION_DEPTH_SPREAD',
            message,
            messageParts: parts,
            severity: 'warning',
            isResolved: false,
          },
        ],
      });
    }, LONG_LINE);
    await page.getByTestId('detail-tab-warnings').click();
    const message = page.getByTestId('warnings-content').locator('.text-sm').filter({ hasText: 'The depth readings at' });
    await expect(message).toHaveCount(1);
    // The shot is two-tone: its from end belongs to Side Passage.
    await expect(message.locator('.sh-ppill-vis--split')).toHaveCount(1);
    await expect(message.locator('.sh-ppill-seg')).toHaveCount(3);
    await expectSamePitch(page, message);
  });
});
