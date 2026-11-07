/**
 * #576 NF-W13FE-CSS-D1 (#605): the docked panels' placement-mode dim is drawn without a CSS
 * filter. The theme tokens are re-declared gray on the dimmed wrapper (useGrayscaleTokens);
 * a colour that is no token - a section's own colour on its swatch - grays itself through
 * `useGrayscalePaint` when its panel is dimmed (`GrayscalePaintContext`), and only then.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';

vi.mock('../../../utils/ipc');
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

import SectionsPanel from '../SectionsPanel';
import { GrayscalePaintContext, useGrayscalePaint } from '../../../hooks/useGrayscalePaint';
import { grayscaleColor } from '../../../utils/grayscaleTokens';
import { useUIStore } from '../../../store/uiStore';
import { makeSection } from '../../../test/factories/survey';
import { invokeIpcMock } from '../../../test/factories/ipc';

const SECTIONS = [
  makeSection({ id: 1, name: 'SectionA', color: '#ff6b6b' }),
  makeSection({ id: 2, name: 'SectionB', color: '#22c55e' }),
];

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  useUIStore.setState({ selection: null, multiSelection: [], hoveredSectionId: null });
  // jsdom lays nothing out: give the virtualized list a size so its rows render.
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(30);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(300);
});

afterEach(() => vi.restoreAllMocks());

/** The swatch of each section row: the row's first span, painted the section's colour. */
function swatchColors(container: HTMLElement): string[] {
  return SECTIONS.map((s) => {
    const swatch = container.querySelector<HTMLElement>(`[data-testid="section-item-${s.id}"] > span`);
    return swatch?.style.backgroundColor ?? 'missing';
  });
}

const dimmed = ({ children }: { children: ReactNode }) => (
  <GrayscalePaintContext.Provider value={true}>{children}</GrayscalePaintContext.Provider>
);

describe('the docked panels gray their non-token colours under the placement dim (#576 NF-W13FE-CSS-D1)', () => {
  it('useGrayscalePaint is the identity outside a dimmed panel and grayscale(1) inside one', () => {
    expect(renderHook(() => useGrayscalePaint()).result.current('#ff6b6b')).toBe('#ff6b6b');
    const paint = renderHook(() => useGrayscalePaint(), { wrapper: dimmed }).result.current;
    expect(paint('#ff6b6b')).toBe(grayscaleColor('#ff6b6b'));
    expect(paint('#ff6b6b')).toMatch(/^rgb\((\d+(?:\.\d+)?) \1 \1\)$/);
    // Not a plain colour: left as it is.
    expect(paint('var(--cc-accent)')).toBe('var(--cc-accent)');
  });

  it("the Sections panel paints each section's swatch in its colour, and gray while dimmed", () => {
    const plain = render(<SectionsPanel sections={SECTIONS} />);
    const colours = swatchColors(plain.container);
    plain.unmount();
    const gray = render(<SectionsPanel sections={SECTIONS} />, { wrapper: dimmed });
    const grays = swatchColors(gray.container);

    // jsdom serialises the colours as rgb(); the dimmed ones have r = g = b.
    expect(colours).toEqual(['rgb(255, 107, 107)', 'rgb(34, 197, 94)']);
    for (const g of grays) expect(g).toMatch(/^rgb\((\d+(?:\.\d+)?),? (\1),? (\1)\)$/);
    expect(grays[0]).not.toBe(grays[1]);
  });
});
