/**
 * The Links cell's chip row: whole chips that fit, then "+N" listing them all.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import '../../../i18n';
import { fitChips, LinkChipRow, type LinkChip } from '../LinkChipRow';

describe('fitChips', () => {
  it('keeps every chip when they all fit', () => {
    expect(fitChips([100, 80], 184)).toBe(2);
    expect(fitChips([], 10)).toBe(0);
  });

  it('keeps the chips that fit beside the +N pill', () => {
    // 100 + 4 + 100 = 204, + 4 + 32 = 240 <= 250; a third would need 344.
    expect(fitChips([100, 100, 100], 250)).toBe(2);
    expect(fitChips([100, 100, 100], 239)).toBe(1);
  });

  it('always keeps the first chip, even when it alone overflows', () => {
    expect(fitChips([300, 50], 120)).toBe(1);
  });
});

describe('<LinkChipRow>', () => {
  afterEach(() => vi.restoreAllMocks());

  const chips: LinkChip[] = ['Closes to Main:3 · Δ 0.7 m · 1.2 %', '⇄ Branch:0', '⇄ Side:4'].map((label, i) => ({
    key: `c${i}`,
    label,
    node: (
      <button type="button" className="dw-chip shrink-0" data-testid={`chip-${i}`}>
        {label}
      </button>
    ),
  }));

  it('shows the chips that fit and a +N pill listing every link', () => {
    vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid?.startsWith('chip-') ? 100 : 0;
    });
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid === 'row' ? 250 : 0;
    });
    vi.useFakeTimers();
    render(<LinkChipRow chips={chips} testId="row" moreTestId="more" />);
    expect(screen.getByTestId('chip-0')).toBeInTheDocument();
    expect(screen.getByTestId('chip-1')).toBeInTheDocument();
    expect(screen.queryByTestId('chip-2')).toBeNull();
    const more = screen.getByTestId('more');
    expect(more).toHaveTextContent('+1');
    expect(more).toHaveAttribute('aria-label', '1 more link');
    act(() => {
      fireEvent.mouseEnter(more.parentElement!);
    });
    expect(screen.getByTestId('more-list').textContent).toBe(chips.map((c) => c.label).join('\n'));
    vi.useRealTimers();
  });

  it('lets a lone first chip shrink beside the pill instead of overflowing', () => {
    vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid?.startsWith('chip-') ? 300 : 0;
    });
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid === 'row' ? 120 : 0;
    });
    render(<LinkChipRow chips={chips} testId="row" moreTestId="more" />);
    expect(screen.getByTestId('chip-0')).toHaveClass('shrink');
    expect(screen.getByTestId('chip-0')).not.toHaveClass('shrink-0');
    expect(screen.getByTestId('more')).toHaveTextContent('+2');
  });

  it('shows everything when nothing has been laid out (no measurements)', () => {
    render(<LinkChipRow chips={chips} testId="row" moreTestId="more" />);
    expect(screen.getByTestId('chip-2')).toBeInTheDocument();
    expect(screen.queryByTestId('more')).toBeNull();
  });
});
