import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import '../../../../i18n';
import { LevelsSlider, type LevelsSliderProps } from '../LevelsSlider';
import { resolveChoice, type LevelsScale } from '../../../../utils/palettes';
import { getMapThemeColors } from '../../../../utils/themeColors';

function setup(scale: LevelsScale | null = null) {
  const onChange = vi.fn<(s: LevelsScale, live: boolean) => void>();
  const props: LevelsSliderProps = {
    domain: { min: 0, max: 100 },
    scale,
    colorScale: { palette: resolveChoice({ id: 'mpl.viridis', reversed: false }, 'depth', getMapThemeColors()), scale },
    histogram: [1, 4, 9, 2, 0, 3],
    unit: 'm',
    precision: 1,
    onChange,
    histogramLabel: 'Histogram of depth values',
    testIdPrefix: 'lv',
  };
  render(<LevelsSlider {...props} />);
  return onChange;
}

describe('LevelsSlider', () => {
  it('shows the auto range and a histogram', () => {
    setup();
    expect(screen.getByTestId('lv-min-input')).toHaveValue('0.0');
    expect(screen.getByTestId('lv-max-input')).toHaveValue('100.0');
    expect(screen.getByTestId('lv-gamma-input')).toHaveValue('1.00');
    expect(screen.getByTestId('lv-histogram').querySelectorAll('rect')).toHaveLength(5); // empty bin skipped
    expect(screen.getByTestId('lv-mid-handle')).toHaveAttribute('aria-valuenow', '50');
  });

  it('nudges a handle with the arrow keys (Shift for bigger steps)', () => {
    const onChange = setup();
    fireEvent.keyDown(screen.getByTestId('lv-min-handle'), { key: 'ArrowRight' });
    expect(onChange).toHaveBeenLastCalledWith({ min: 1, max: 100, gamma: 1 }, false);
    fireEvent.keyDown(screen.getByTestId('lv-max-handle'), { key: 'ArrowLeft', shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, max: 90, gamma: 1 }, false);
  });

  it('turns the midpoint handle into a Levels gamma', () => {
    const onChange = setup();
    // Midpoint moved to 25% of the range -> 0.5^gamma = 0.25 -> gamma 2.
    fireEvent.keyDown(screen.getByTestId('lv-mid-handle'), { key: 'ArrowLeft', shiftKey: true });
    fireEvent.keyDown(screen.getByTestId('lv-mid-handle'), { key: 'ArrowLeft' });
    const [scale] = onChange.mock.calls.at(-1)!;
    expect(scale.gamma).toBeGreaterThan(1);
  });

  it('commits typed values and reverts an impossible range', () => {
    const onChange = setup({ min: 10, max: 60, gamma: 1 });
    const min = screen.getByTestId('lv-min-input');
    fireEvent.change(min, { target: { value: '20' } });
    fireEvent.keyDown(min, { key: 'Enter' });
    expect(onChange).toHaveBeenLastCalledWith({ min: 20, max: 60, gamma: 1 }, false);
    onChange.mockClear();
    fireEvent.change(min, { target: { value: '75' } }); // above max
    fireEvent.blur(min);
    expect(onChange).not.toHaveBeenCalled();
    expect(min).toHaveValue('10.0');
    const gamma = screen.getByTestId('lv-gamma-input');
    fireEvent.change(gamma, { target: { value: '40' } });
    fireEvent.blur(gamma);
    expect(onChange).toHaveBeenLastCalledWith({ min: 10, max: 60, gamma: 9.99 }, false);
  });

  it('reports a drag as live until the pointer is released', () => {
    const onChange = setup();
    const handle = screen.getByTestId('lv-max-handle');
    const track = handle.parentElement!.parentElement!;
    track.getBoundingClientRect = () => ({ left: 0, width: 200, top: 0, height: 10, right: 200, bottom: 10, x: 0, y: 0, toJSON: () => ({}) });
    handle.setPointerCapture = () => {};
    handle.releasePointerCapture = () => {};
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 200 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 150 });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, max: 75, gamma: 1 }, true);
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 140 });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, max: 70, gamma: 1 }, false);
  });
});
