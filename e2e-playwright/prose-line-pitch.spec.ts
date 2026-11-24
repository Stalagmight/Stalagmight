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
 * another line) and a station of another line. Every gap must equal the
 * line-height within 0.5 px. Every pill leads with its line's dot
 * (RS-LEADDOT), and a foreign station or shot end has its superscript home
 * dot after its name (RS-LEADDOT), measured with them; the home dot takes no
 * room, stays with its name and clears the Station ID after it.
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

/** Shows a Warnings tab message naming stations, a two-tone shot and a one-colour shot, and returns it. */
async function showWarning(page: Page): Promise<Locator> {
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
      { kind: 'text', text: ' (shown as ' },
      // A station of Side Passage shown on the long line: Side's colour and
      // name (RS-OWNERSHIP), its home dot before its Station ID (RS-LEADDOT).
      {
        kind: 'station',
        text: `${lineName} / 0 (${side.name})`,
        line_id: line.id,
        context: side.name,
        name: '0',
        home_line: side.id,
        station_id: 900,
        slot: { line_id: line.id, station_index: 0 },
      },
      { kind: 'text', text: ')' },
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
      { kind: 'text', text: ' is one of them. Check the depths of the shots that meet here, as on ' },
      {
        kind: 'shot',
        text: `${lineName}: 2 \u2192 3`,
        line_id: line.id,
        context: lineName,
        from: { name: '2' },
        to: { name: '3' },
      },
      { kind: 'text', text: '.' },
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
  return message;
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
    const message = await showWarning(page);
    // One shot is two-tone: its from end belongs to Side Passage. Its head,
    // from half, arrow and to half are four segments (RS-ARROW).
    await expect(message.locator('.sh-ppill-vis--split')).toHaveCount(1);
    await expect(message.locator('.sh-ppill-seg')).toHaveCount(4);
    await expectSamePitch(page, message);
  });

  test('a wrap never separates a shot\'s arrow from either end (RS-ARROW)', async ({ page }) => {
    const message = await showWarning(page);
    // Narrow the message step by step: the arrow of each shot pill (the
    // two-tone one and the one-colour one) stays on the line of the from
    // end's last word and of the to end's first word.
    const broken = await message.evaluate((root) => {
      const lineOf = (node: Text, i: number) => {
        const r = document.createRange();
        r.setStart(node, i);
        r.setEnd(node, i + 1);
        const box = r.getClientRects()[0]!;
        return (box.top + box.bottom) / 2;
      };
      const textNodes = (el: Element) => {
        const out: Text[] = [];
        const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        for (let n = w.nextNode(); n; n = w.nextNode()) if ((n as Text).data.trim()) out.push(n as Text);
        return out;
      };
      const joins = [...root.querySelectorAll('.sh-ppill-join')];
      const bad: string[] = [];
      for (let width = 60; width <= 420; width += 4) {
        (root as HTMLElement).style.width = `${width}px`;
        for (const join of joins) {
          const nodes = textNodes(join);
          const arrowNode = nodes.find((n) => n.data.includes('\u2192'))!;
          const at = nodes.indexOf(arrowNode);
          const before = nodes[at - 1]!;
          const after = nodes[at + 1]!;
          const a = lineOf(arrowNode, arrowNode.data.indexOf('\u2192'));
          if (Math.abs(lineOf(before, before.data.length - 1) - a) > 2 || Math.abs(lineOf(after, 0) - a) > 2) {
            bad.push(`${width}px: ${join.textContent}`);
          }
        }
      }
      return { joins: joins.length, bad };
    });
    expect(broken.joins).toBe(2);
    expect(broken.bad).toEqual([]);
  });

  // The dot is an empty inline box whose only text is a zero-width joiner.
  // Injection: without the joiner Chromium left the dot at the end of the
  // line before its word whenever the line broke right before the pill.
  test('the leading dot stays with its first word, inside the glow at its start (RS-LEADDOT)', async ({ page }) => {
    const message = await showWarning(page);
    const report = await message.evaluate((root) => {
      const pills = [...root.querySelectorAll('.sh-ppill')];
      const mid = (r: DOMRect) => (r.top + r.bottom) / 2;
      const bad: string[] = [];
      let dots = 0;
      let breaksBefore = 0;
      for (let width = 60; width <= 420; width += 8) {
        (root as HTMLElement).style.width = `${width}px`;
        for (const pill of pills) {
          const found = pill.querySelectorAll('[data-prose-dot]');
          dots = Math.max(dots, found.length);
          const dot = found[0] as HTMLElement | undefined;
          if (!dot || found.length !== 1) {
            bad.push(`${width}px: ${pill.textContent} has ${found.length} dots`);
            continue;
          }
          const d = dot.getBoundingClientRect();
          // The first letter after the dot (the context's or the name's) is on its line.
          const w = document.createTreeWalker(pill.querySelector('.sh-ppill-vis')!, NodeFilter.SHOW_TEXT);
          let first: Text | null = null;
          for (let n = w.nextNode(); n; n = w.nextNode()) {
            if (dot.compareDocumentPosition(n) & Node.DOCUMENT_POSITION_FOLLOWING && /\S/.test((n as Text).data)) {
              first = n as Text;
              break;
            }
          }
          const r = document.createRange();
          const at = first!.data.search(/\S/);
          r.setStart(first!, at);
          r.setEnd(first!, at + 1);
          const letter = r.getClientRects()[0]!;
          if (Math.abs(mid(letter) - mid(d)) > letter.height / 2) bad.push(`${width}px: the dot of ${pill.textContent} left its first word`);
          // A line may still break before the pill: the dot then starts a line, the words before it end the one above.
          const prev = pill.previousSibling;
          if (prev && prev.nodeType === Node.TEXT_NODE && /\S\s*$/.test((prev as Text).data)) {
            const t = prev as Text;
            const i = t.data.search(/\S\s*$/);
            const before = document.createRange();
            before.setStart(t, i);
            before.setEnd(t, i + 1);
            if (mid(before.getClientRects()[0]!) < d.top) breaksBefore += 1;
          }
          if (!(d.right <= letter.left + 0.5)) bad.push(`${width}px: the dot of ${pill.textContent} is not before its first word`);
          // Inside the glow's core at its start, centred on it: the one-colour
          // pill's first painted fragment, or the two-colour pill's first box.
          const core = (pill.querySelector('.sh-ppill-vis--split .sh-ppill-w') ?? pill.querySelector('.sh-ppill-vis'))!;
          const box = core.getClientRects()[0]!;
          const cs = getComputedStyle(core);
          const split = core.classList.contains('sh-ppill-w');
          // A word box keeps the halo's room above and below (spread + 1.5 blur, em); the core is inside it.
          const ps = getComputedStyle(pill);
          const em = (name: string) => parseFloat(ps.getPropertyValue(name));
          const reach = split ? (em('--glow-spread') + 1.5 * em('--glow-blur')) * parseFloat(ps.fontSize) : 0;
          if (split && !(reach > 0)) bad.push('no halo reach read');
          const top = box.top + (split ? reach : 0);
          const bottom = box.bottom - (split ? reach : 0);
          if (!(d.top >= top - 0.5 && d.bottom <= bottom + 0.5)) bad.push(`${width}px: the dot of ${pill.textContent} sticks out of the core`);
          if (Math.abs(mid(d) - (top + bottom) / 2) > 1.5) bad.push(`${width}px: the dot of ${pill.textContent} is off the core's middle`);
          if (!(d.left >= box.left + (split ? parseFloat(cs.paddingLeft) - 0.5 : 0))) bad.push(`${width}px: the dot of ${pill.textContent} starts before the core`);
        }
      }
      return { pills: pills.length, dots, bad, breaksBefore };
    });
    expect(report.pills).toBe(6);
    expect(report.dots).toBe(1);
    expect(report.breaksBefore).toBeGreaterThan(0);
    expect(report.bad.slice(0, 10)).toEqual([]);
  });

  // RS-LEADDOT: "a small color dot in superscript right after the station
  // name, it doesnt need to take any space horizontally". Injections: a home
  // dot with its advance (no negative margin) fails "takes no room"; one
  // without the Station ID's offset fails "clears the Station ID".
  test('a foreign station\'s home dot is superscript right after its name, takes no room and clears the Station ID (RS-LEADDOT)', async ({ page }) => {
    const message = await showWarning(page);
    const report = await message.evaluate((root) => {
      const homes = () => [...root.querySelectorAll('[data-prose-home-dot]')] as HTMLElement[];
      const mid = (r: { top: number; bottom: number }) => (r.top + r.bottom) / 2;
      /** The painted circle: centred in the box's padding, which is its radius and 1.25px of room for its ring. */
      const circle = (dot: HTMLElement) => {
        const box = dot.getBoundingClientRect();
        const cs = getComputedStyle(dot);
        const fs = parseFloat(cs.fontSize);
        const r = parseFloat(cs.paddingLeft) - 1.25;
        const cx = box.left + parseFloat(cs.paddingLeft);
        const cy = mid(box);
        return { left: cx - r, right: cx + r, top: cy - r, bottom: cy + r, fs };
      };
      /** The last visible letter before the dot, and its baseline (a zero-size probe after it). */
      const before = (dot: HTMLElement) => {
        const pill = dot.closest('.sh-ppill')!;
        const w = document.createTreeWalker(pill, NodeFilter.SHOW_TEXT);
        let last: Text | null = null;
        for (let n = w.nextNode(); n; n = w.nextNode()) {
          if (dot.compareDocumentPosition(n) & Node.DOCUMENT_POSITION_FOLLOWING) break;
          if ((n as Text).parentElement!.closest('.sr-only')) continue;
          if (/\S/.test((n as Text).data)) last = n as Text;
        }
        const r = document.createRange();
        r.setStart(last!, last!.data.length - 1);
        r.setEnd(last!, last!.data.length);
        const probe = document.createElement('span');
        probe.style.cssText = 'display:inline-block;width:0;height:0';
        last!.parentNode!.insertBefore(probe, last!.nextSibling);
        const baseline = probe.getBoundingClientRect().bottom;
        probe.remove();
        return { glyph: r.getClientRects()[0]!, baseline };
      };
      const glyphs = () => {
        const out: number[] = [];
        const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let n = w.nextNode(); n; n = w.nextNode()) {
          if (!(n as Text).data.trim()) continue;
          const r = document.createRange();
          r.selectNodeContents(n);
          for (const x of r.getClientRects()) out.push(x.left, x.top);
        }
        return out;
      };
      const bad: string[] = [];
      let count = 0;
      let sids = 0;
      let moved = 0;
      for (let width = 60; width <= 420; width += 8) {
        (root as HTMLElement).style.width = `${width}px`;
        const dots = homes();
        count = Math.max(count, dots.length);
        for (const dot of dots) {
          const c = circle(dot);
          const { glyph, baseline } = before(dot);
          const where = `${width}px ${dot.closest('.sh-ppill')!.textContent}`;
          const size = (c.right - c.left) / c.fs;
          // "More subtle" (RS-LEADDOT): 0.26em.
          if (size < 0.24 || size > 0.3) bad.push(`${where}: ${size.toFixed(2)}em`);
          // Right after the name, on its line.
          const gap = c.left - glyph.right;
          if (gap < -0.5 || gap > 0.1 * c.fs) bad.push(`${where}: ${gap.toFixed(2)}px after the name`);
          if (Math.abs(mid(glyph) - mid(c)) > glyph.height / 2) bad.push(`${where}: off its name's line`);
          // Superscript: its middle well above the baseline, its bottom too.
          const up = (baseline - mid(c)) / c.fs;
          if (up < 0.5 || up > 0.75) bad.push(`${where}: middle ${up.toFixed(2)}em up`);
          // Clear of the Station ID after it.
          const next = dot.nextElementSibling;
          if (next?.hasAttribute('data-prose-sid')) {
            sids += 1;
            const t = next.firstChild as Text;
            const r = document.createRange();
            r.setStart(t, 0);
            r.setEnd(t, 1);
            const hash = r.getClientRects()[0]!;
            if (!(hash.left - c.right >= 0.12 * c.fs)) bad.push(`${where}: ${(hash.left - c.right).toFixed(2)}px to the Station ID`);
          }
        }
        // Takes no room: hiding every home dot moves no glyph.
        const shown = glyphs();
        for (const dot of dots) dot.style.display = 'none';
        const hidden = glyphs();
        for (const dot of dots) dot.style.display = '';
        if (shown.length !== hidden.length || shown.some((v, i) => Math.abs(v - hidden[i]!) > 0.1)) {
          moved += 1;
          bad.push(`${width}px: hiding the home dots moved text`);
        }
      }
      return { count, sids, moved, bad };
    });
    // The foreign station and the two-tone shot's foreign from end.
    expect(report.count).toBe(2);
    expect(report.sids).toBeGreaterThan(0);
    expect(report.moved).toBe(0);
    expect(report.bad.slice(0, 10)).toEqual([]);
  });
});
