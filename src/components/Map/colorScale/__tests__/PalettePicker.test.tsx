import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import '../../../../i18n';
import { PalettePicker, type PalettePickerProps } from '../PalettePicker';

function setup(over: Partial<PalettePickerProps> = {}) {
  const props: PalettePickerProps = {
    kind: 'depth',
    value: { id: 'theme', reversed: false },
    pinned: [],
    onPreview: vi.fn(),
    onSelect: vi.fn(),
    onTogglePin: vi.fn(),
    testIdPrefix: 'p',
    ...over,
  };
  render(<PalettePicker {...props} />);
  fireEvent.click(screen.getByTestId('p-trigger'));
  return props;
}

describe('PalettePicker', () => {
  it('opens with the theme and current folders expanded and others collapsed', () => {
    setup({ value: { id: 'mpl.magma', reversed: false } });
    expect(screen.getByTestId('p-option-theme')).toBeInTheDocument();
    expect(screen.getByTestId('p-option-mpl.magma')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('p-group-mpl-diverging')).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('p-option-mpl.coolwarm')).toBeNull();
    fireEvent.click(screen.getByTestId('p-group-mpl-diverging'));
    expect(screen.getByTestId('p-option-mpl.coolwarm')).toBeInTheDocument();
  });

  it('searches across every folder and previews what is hovered', () => {
    const props = setup();
    fireEvent.change(screen.getByTestId('p-search'), { target: { value: 'viri' } });
    const row = screen.getByTestId('p-option-mpl.viridis');
    expect(screen.queryByTestId('p-option-mpl.magma')).toBeNull();
    fireEvent.mouseEnter(row);
    expect(props.onPreview).toHaveBeenLastCalledWith({ id: 'mpl.viridis', reversed: false });
    fireEvent.click(row);
    expect(props.onSelect).toHaveBeenCalledWith({ id: 'mpl.viridis', reversed: false });
    expect(props.onPreview).toHaveBeenLastCalledWith(null);
    expect(screen.queryByTestId('p-panel')).toBeNull();
  });

  it('matches group names too, and says when nothing matches', () => {
    setup();
    fireEvent.change(screen.getByTestId('p-search'), { target: { value: 'metals' } });
    expect(screen.getByTestId('p-option-grad.chrome')).toBeInTheDocument();
    fireEvent.change(screen.getByTestId('p-search'), { target: { value: 'zzzz' } });
    expect(screen.getByTestId('p-no-results')).toBeInTheDocument();
  });

  it('reads a matplotlib _r suffix as the reversed palette', () => {
    const props = setup();
    const search = screen.getByTestId('p-search');
    fireEvent.change(search, { target: { value: 'viridis_r' } });
    fireEvent.keyDown(search, { key: 'Enter' });
    expect(props.onSelect).toHaveBeenCalledWith({ id: 'mpl.viridis', reversed: true });
  });

  it('arrows through the visible palettes with a live preview; Escape reverts and closes', () => {
    const props = setup();
    const search = screen.getByTestId('p-search');
    fireEvent.change(search, { target: { value: 'plasma' } });
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    expect(props.onPreview).toHaveBeenLastCalledWith({ id: 'mpl.plasma', reversed: false });
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(props.onPreview).toHaveBeenLastCalledWith(null);
    expect(props.onSelect).not.toHaveBeenCalled();
    expect(screen.queryByTestId('p-panel')).toBeNull();
  });

  it('pins without picking, and lists pins first', () => {
    const props = setup({ pinned: ['grad.cenote'] });
    const pinnedRow = screen.getByTestId('p-pinned-grad.cenote');
    const firstOption = within(screen.getByRole('listbox')).getAllByRole('option')[0];
    expect(firstOption).toBe(pinnedRow);
    fireEvent.click(screen.getByTestId('p-pinned-pin-grad.cenote'));
    expect(props.onTogglePin).toHaveBeenCalledWith('grad.cenote');
    expect(props.onSelect).not.toHaveBeenCalled();
    expect(screen.getByTestId('p-pinned-pin-grad.cenote')).toHaveAttribute('aria-pressed', 'true');
  });

  it('closes on an outside click, reverting the preview', () => {
    const props = setup();
    act(() => {
      document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });
    expect(screen.queryByTestId('p-panel')).toBeNull();
    expect(props.onPreview).toHaveBeenLastCalledWith(null);
  });
});
