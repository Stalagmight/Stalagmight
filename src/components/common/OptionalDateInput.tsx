/**
 * A native date input that can be EMPTY without looking dated.
 *
 * WebKitGTK (the desktop app's webview) paints an empty `<input type="date">`
 * as TODAY's date in ordinary text, so a shot or section with no survey date
 * looked surveyed today. With no date this shows a muted "No date" button
 * instead; a click, Enter or Space on it opens the native input, and leaving
 * the input (or Escape) with nothing picked goes back to "No date". When there
 * is a date, "Clear date" removes it again.
 *
 * Controlled and commit-agnostic: the host owns `value` and decides what a
 * change, a blur or a key means (a draft committed on Enter, a dialog field).
 *
 * Test IDs: `<testId>` is the native input, `<testId>-empty` the "No date"
 * button (or text, when disabled), `<testId>-clear` the clear button.
 */
import { useEffect, useRef, useState, type FocusEvent, type KeyboardEvent, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { CloseIcon } from './icons';

export interface OptionalDateInputProps {
  /** The input's value, ISO `YYYY-MM-DD`; '' for none. */
  value: string;
  onChange: (value: string) => void;
  onBlur?: (event: FocusEvent<HTMLInputElement>) => void;
  /** Runs first; a key it handles (`preventDefault`) is not handled again here. */
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
  /** Remove the date. Offered as "Clear date" whenever there is one. */
  onClear?: () => void;
  disabled?: boolean;
  /** Open on the input, focused: the user already asked to set a date ("Set for all shots"). */
  startEditing?: boolean;
  label: string;
  testId: string;
  inputRef?: RefObject<HTMLInputElement>;
  /** Classes of the input. */
  inputClassName?: string;
}

export function OptionalDateInput({
  value,
  onChange,
  onBlur,
  onKeyDown,
  onClear,
  disabled = false,
  startEditing = false,
  label,
  testId,
  inputRef,
  inputClassName = 'h-9 w-40 px-2',
}: OptionalDateInputProps) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(startEditing);
  const ownInput = useRef<HTMLInputElement>(null);
  const input = inputRef ?? ownInput;
  const emptyRef = useRef<HTMLButtonElement>(null);
  // Where the focus goes after the next render: into the input just opened,
  // or back to "No date" after Escape.
  const focusNext = useRef<'input' | 'empty' | null>(startEditing ? 'input' : null);
  useEffect(() => {
    const target = focusNext.current;
    focusNext.current = null;
    if (target === 'input') input.current?.focus();
    else if (target === 'empty') emptyRef.current?.focus();
  });

  const open = () => {
    focusNext.current = 'input';
    setEditing(true);
  };

  if (value === '' && !editing) {
    if (disabled) {
      return (
        <span className="text-sm text-text-secondary" data-testid={`${testId}-empty`}>
          {t('credit.date.empty')}
        </span>
      );
    }
    return (
      <button
        ref={emptyRef}
        type="button"
        className="h-9 rounded border border-dashed border-border px-2 text-left text-sm text-text-secondary hover:border-accent hover:text-text-primary focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
        aria-label={`${label}: ${t('credit.date.empty')}`}
        onClick={open}
        onKeyDown={(event) => {
          // Before a host that reads Enter as "done" (the table popover).
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            event.stopPropagation();
            open();
          }
        }}
        data-date-empty=""
        data-testid={`${testId}-empty`}
      >
        {t('credit.date.empty')}
      </button>
    );
  }

  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <input
        ref={input}
        type="date"
        value={value}
        // While the input is in use it stays, even when clearing its segments
        // makes the value '' for a moment.
        onFocus={() => setEditing(true)}
        onChange={(event) => {
          setEditing(true);
          onChange(event.target.value);
        }}
        onBlur={(event) => {
          onBlur?.(event);
          // Left with nothing picked: back to "No date".
          if (event.currentTarget.value === '' && !event.currentTarget.validity?.badInput) setEditing(false);
        }}
        onKeyDown={(event) => {
          onKeyDown?.(event);
          if (event.defaultPrevented) return;
          if (event.key === 'Escape' && event.currentTarget.value === '') {
            event.preventDefault();
            event.stopPropagation();
            focusNext.current = 'empty';
            setEditing(false);
          }
        }}
        disabled={disabled}
        aria-label={label}
        className={inputClassName}
        data-testid={testId}
      />
      {onClear && value !== '' && !disabled && (
        <button
          type="button"
          className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded text-text-secondary hover:bg-accent-20 hover:text-text-primary focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
          title={t('credit.date.clear')}
          aria-label={t('credit.date.clear')}
          // Keep the input from blurring first: its blur may commit a draft.
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => {
            focusNext.current = 'empty';
            setEditing(false);
            onClear();
          }}
          data-testid={`${testId}-clear`}
        >
          <CloseIcon size={12} />
        </button>
      )}
    </span>
  );
}
