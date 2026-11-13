/**
 * OptionalDateInput: an empty date is "No date", never an empty native date
 * input (which WebKitGTK paints as today's date). Click / Enter / Space opens
 * the input; leaving it or Escape with nothing picked goes back; a date can be
 * cleared again.
 */
import '../../../i18n';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { OptionalDateInput } from '../OptionalDateInput';

function Harness({ initial = '', disabled = false, onClear }: { initial?: string; disabled?: boolean; onClear?: () => void }) {
  const [value, setValue] = useState(initial);
  return (
    <OptionalDateInput
      value={value}
      onChange={setValue}
      onClear={() => {
        onClear?.();
        setValue('');
      }}
      disabled={disabled}
      label="Survey date"
      testId="date"
    />
  );
}

const dateInputs = () => document.querySelectorAll('input[type="date"]');

describe('OptionalDateInput', () => {
  it('shows "No date", and no date input at all, when there is no date', () => {
    render(<Harness />);
    expect(screen.getByTestId('date-empty')).toHaveTextContent('No date');
    expect(dateInputs()).toHaveLength(0);
    expect(screen.queryByTestId('date-clear')).toBeNull();
  });

  it('opens the native input on click, focused, and goes back when left with nothing picked', () => {
    render(<Harness />);
    fireEvent.click(screen.getByTestId('date-empty'));
    const input = screen.getByTestId('date') as HTMLInputElement;
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe('');
    fireEvent.blur(input);
    expect(screen.getByTestId('date-empty')).toHaveTextContent('No date');
    expect(dateInputs()).toHaveLength(0);
  });

  it('opens from the keyboard and Escape returns to "No date" with the focus on it', () => {
    render(<Harness />);
    fireEvent.keyDown(screen.getByTestId('date-empty'), { key: 'Enter' });
    const input = screen.getByTestId('date');
    expect(document.activeElement).toBe(input);
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(document.activeElement).toBe(screen.getByTestId('date-empty'));
    fireEvent.keyDown(screen.getByTestId('date-empty'), { key: ' ' });
    expect(document.activeElement).toBe(screen.getByTestId('date'));
  });

  it('keeps a picked date in the input, with "Clear date" to remove it', () => {
    const onClear = vi.fn();
    render(<Harness onClear={onClear} />);
    fireEvent.click(screen.getByTestId('date-empty'));
    fireEvent.change(screen.getByTestId('date'), { target: { value: '2026-03-02' } });
    fireEvent.blur(screen.getByTestId('date'));
    expect((screen.getByTestId('date') as HTMLInputElement).value).toBe('2026-03-02');
    fireEvent.click(screen.getByTestId('date-clear'));
    expect(onClear).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('date-empty')).toHaveTextContent('No date');
  });

  it('stays an input while its segments are being cleared', () => {
    render(<Harness initial="2026-03-02" />);
    const input = screen.getByTestId('date');
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: '' } });
    expect(screen.getByTestId('date')).toBe(input);
  });

  it('is plain "No date" text when disabled', () => {
    render(<Harness disabled />);
    const empty = screen.getByTestId('date-empty');
    expect(empty.tagName).toBe('SPAN');
    expect(empty).toHaveTextContent('No date');
    expect(dateInputs()).toHaveLength(0);
  });
});
