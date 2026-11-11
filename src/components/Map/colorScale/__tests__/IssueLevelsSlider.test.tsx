import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import '../../../../i18n';
import { IssueLevelsSlider, type IssueLevelsSliderProps } from '../IssueLevelsSlider';
import type { IssueLevels } from '../../surveyLayer/issueScale';

const TRACK = { left: 0, width: 200, top: 0, height: 10, right: 200, bottom: 10, x: 0, y: 0, toJSON: () => ({}) };

function setup(levels: IssueLevels = { min: 0, warn: 5, max: 15 }, over: Partial<IssueLevelsSliderProps> = {}) {
  const onChange = vi.fn<(l: IssueLevels, live: boolean) => void>();
  const props: IssueLevelsSliderProps = {
    levels,
    domainMax: 20,
    histogram: [40, 9, 3, 0, 1],
    colorAt: () => '#808080',
    unit: '°',
    precision: 1,
    disabled: false,
    onChange,
    histogramLabel: 'Histogram of azimuth values',
    testIdPrefix: 'il',
    ...over,
  };
  const view = render(<IssueLevelsSlider {...props} />);
  // The track is the handles row's grandparent.
  const track = screen.getByTestId('il-min-handle').parentElement!.parentElement!;
  track.getBoundingClientRect = () => TRACK as DOMRect;
  return { onChange, view, props };
}

describe('IssueLevelsSlider', () => {
  it('shows Min / Warn / Max and a histogram of the values', () => {
    setup();
    expect(screen.getByTestId('il-min-input')).toHaveValue('0.0');
    expect(screen.getByTestId('il-warn-input')).toHaveValue('5.0');
    expect(screen.getByTestId('il-max-input')).toHaveValue('15.0');
    expect(screen.getByTestId('il-histogram').querySelectorAll('rect')).toHaveLength(4); // empty bin skipped
    expect(screen.getByTestId('il-warn-handle')).toHaveAttribute('aria-valuetext', '5.0°');
  });

  it('a drag is live until release, and keeps Min < Warn < Max', () => {
    const { onChange } = setup();
    const warn = screen.getByTestId('il-warn-handle');
    fireEvent.pointerDown(warn, { pointerId: 1, clientX: 50 });
    fireEvent.pointerMove(warn, { pointerId: 1, clientX: 20 }); // 2 deg
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 2, max: 15 }, true);
    fireEvent.pointerMove(warn, { pointerId: 1, clientX: 190 }); // 19 deg, beyond Max
    const [past] = onChange.mock.calls[onChange.mock.calls.length - 1]!;
    expect(past.warn).toBeLessThan(15);
    fireEvent.pointerUp(warn, { pointerId: 1, clientX: 40 });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 4, max: 15 }, false);
  });

  it('Escape mid-drag puts the levels back, committed', () => {
    const { onChange } = setup();
    const max = screen.getByTestId('il-max-handle');
    fireEvent.pointerDown(max, { pointerId: 1, clientX: 150 });
    fireEvent.pointerMove(max, { pointerId: 1, clientX: 100 });
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 5, max: 15 }, false);
  });

  it('unmounting mid-drag ends the preview on the starting levels', () => {
    const { onChange, view } = setup();
    const min = screen.getByTestId('il-min-handle');
    fireEvent.pointerDown(min, { pointerId: 1, clientX: 0 });
    fireEvent.pointerMove(min, { pointerId: 1, clientX: 30 });
    view.unmount();
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 5, max: 15 }, false);
  });

  it('a press without a move changes nothing', () => {
    const { onChange } = setup();
    const warn = screen.getByTestId('il-warn-handle');
    fireEvent.pointerDown(warn, { pointerId: 1, clientX: 50 });
    fireEvent.pointerUp(warn, { pointerId: 1, clientX: 50 });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('commits typed levels in order and reverts out-of-order or negative ones', () => {
    const { onChange } = setup();
    const warn = screen.getByTestId('il-warn-input');
    fireEvent.change(warn, { target: { value: '8' } });
    fireEvent.keyDown(warn, { key: 'Enter' });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 8, max: 15 }, false);
    onChange.mockClear();
    fireEvent.change(warn, { target: { value: '20' } }); // above Max
    fireEvent.blur(warn);
    const min = screen.getByTestId('il-min-input');
    fireEvent.change(min, { target: { value: '-1' } });
    fireEvent.blur(min);
    expect(onChange).not.toHaveBeenCalled();
    expect(warn).toHaveValue('5.0');
    expect(min).toHaveValue('0.0');
    // A larger Max beyond the slider's span is fine.
    const max = screen.getByTestId('il-max-input');
    fireEvent.change(max, { target: { value: '90' } });
    fireEvent.keyDown(max, { key: 'Enter' });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 5, max: 90 }, false);
  });

  it('arrow keys nudge a handle', () => {
    const { onChange } = setup();
    fireEvent.keyDown(screen.getByTestId('il-max-handle'), { key: 'ArrowRight', shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith({ min: 0, warn: 5, max: 17 }, false);
  });

  it('switched off: greyed, no histogram, no edits', () => {
    const { onChange } = setup(undefined, { disabled: true });
    expect(screen.queryByTestId('il-histogram')).toBeNull();
    expect(screen.getByTestId('il-histogram-off')).toBeInTheDocument();
    expect(screen.getByTestId('il-warn-input')).toBeDisabled();
    const warn = screen.getByTestId('il-warn-handle');
    expect(warn).toHaveAttribute('tabindex', '-1');
    fireEvent.pointerDown(warn, { pointerId: 1, clientX: 50 });
    fireEvent.pointerMove(warn, { pointerId: 1, clientX: 20 });
    fireEvent.keyDown(warn, { key: 'ArrowRight' });
    expect(onChange).not.toHaveBeenCalled();
  });
});
