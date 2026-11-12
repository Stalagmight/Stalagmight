/**
 * The popover that edits one data-table credit cell: a Surveyors or Explorers
 * pill field, or the survey date, of one shot.
 *
 * Spreadsheet semantics: the edit is a draft until it is committed, and the
 * commit is ONE `editShotCredit` for that shot (one undo step), only when the
 * value changed. Enter on the empty field (or the date input) commits and
 * closes; a click outside commits too, including a name still being typed;
 * Escape walks out of the field (list, then typed text) and then cancels.
 *
 * The field owns the keyboard while the popover is open (`useFieldFocus`).
 */
import { useCallback, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { PeopleInput } from '../common/PeopleInput';
import { Button } from '../common/Button';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { usePeopleSuggestions } from '../../hooks/usePeopleSuggestions';
import { rememberPeople } from '../../store/recentPeople';
import { peopleValues, withPerson, withoutPerson } from '../common/SectionMetadataForm';
import { personKey, type PersonLike } from '../../utils/people';
import type { CreditEdit, Person, Shot } from '../../types/ipc.generated';
import type { CreditColumnKey } from './dataTableCreditColumns';

export const CREDIT_EDITOR_TEST_ID = 'credit-cell-editor';
/** The popover's one text or date input. */
export const CREDIT_EDITOR_FIELD = `[data-testid="${CREDIT_EDITOR_TEST_ID}"] input`;

export interface CreditCellEditorProps {
  shot: Shot;
  field: CreditColumnKey;
  /** The cell the popover opens under. */
  anchor: HTMLElement | null;
  /** A character typed on the cell that opened the popover: the field starts with it. */
  seed?: string;
  /** Commit: called once with the edit, only when the value changed. */
  onCommit: (edit: CreditEdit) => void;
  onClose: () => void;
}

/**
 * Below the cell, left-aligned to it, kept inside the window, and following it:
 * opening the popover often coincides with the grid scrolling that cell into
 * view, so a scroll re-places it rather than closing it. `onLost` runs when the
 * cell itself is gone (virtualized away, the table closed), or was never there
 * (a key typed on an active cell scrolled out of view): a popover that cannot be
 * placed would stay invisible, holding the keyboard.
 */
function usePlacement(
  anchor: HTMLElement | null,
  boxRef: React.RefObject<HTMLDivElement | null>,
  onLost: () => void,
) {
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const onLostRef = useRef(onLost);
  onLostRef.current = onLost;
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!box) return undefined;
    if (!anchor) {
      onLostRef.current();
      return undefined;
    }
    const place = () => {
      if (!anchor.isConnected) {
        onLostRef.current();
        return;
      }
      const cell = anchor.getBoundingClientRect();
      const { width, height } = box.getBoundingClientRect();
      const margin = 8;
      const below = cell.bottom + 2;
      const top = below + height + margin > window.innerHeight ? Math.max(margin, cell.top - height - 2) : below;
      const left = Math.max(margin, Math.min(cell.left, window.innerWidth - width - margin));
      setPos((prev) => (prev && prev.top === top && prev.left === left ? prev : { top, left }));
    };
    place();
    window.addEventListener('scroll', place, { capture: true, passive: true });
    window.addEventListener('resize', place);
    return () => {
      window.removeEventListener('scroll', place, { capture: true });
      window.removeEventListener('resize', place);
    };
  }, [anchor, boxRef]);
  return pos;
}

/**
 * Put the keyboard in the field, and keep it there while the popover is open.
 *
 * Not `autoFocus`: React runs that as the box mounts, while it is still
 * `visibility: hidden` waiting for `usePlacement`, and neither WebKit nor
 * Chromium focuses a hidden element. The focus stayed on the grid, which then
 * took the keys: a typed letter went to the shot as a one-letter person, and
 * Escape left the popover open. So focus once the box is placed, again on the
 * next frame (after whatever focus handling the opening gesture still has to
 * run), and take it back when the grid holding the cell grabs it while the
 * popover is open. A press outside is the user leaving (the dismiss commits),
 * and `closingRef` marks a close that hands the focus back on purpose.
 */
function useFieldFocus(
  boxRef: RefObject<HTMLDivElement | null>,
  anchor: HTMLElement | null,
  placed: boolean,
  closingRef: RefObject<boolean>,
) {
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!placed || !box) return undefined;
    const focusField = () => {
      const field = box.querySelector<HTMLInputElement>('input');
      if (!field || closingRef.current || box.contains(document.activeElement)) return;
      field.focus({ preventScroll: true });
      if (field.type === 'text') field.setSelectionRange(field.value.length, field.value.length);
    };
    focusField();
    const frame = requestAnimationFrame(focusField);
    let pressed = false;
    const onPress = () => {
      pressed = true;
    };
    const onRelease = () => {
      pressed = false;
    };
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (pressed || !(target instanceof Element) || box.contains(target)) return;
      if (anchor && (target.contains(anchor) || anchor.contains(target))) focusField();
    };
    const press = ['mousedown', 'pointerdown', 'touchstart'] as const;
    const release = ['mouseup', 'pointerup', 'pointercancel', 'touchend', 'touchcancel'] as const;
    for (const type of press) window.addEventListener(type, onPress, true);
    for (const type of release) window.addEventListener(type, onRelease, true);
    document.addEventListener('focusin', onFocusIn, true);
    return () => {
      cancelAnimationFrame(frame);
      for (const type of press) window.removeEventListener(type, onPress, true);
      for (const type of release) window.removeEventListener(type, onRelease, true);
      document.removeEventListener('focusin', onFocusIn, true);
    };
  }, [boxRef, anchor, placed, closingRef]);
}

const sameList = (a: readonly Person[], b: readonly Person[]) =>
  a.length === b.length && a.every((p, i) => personKey(p) === personKey(b[i]!) && p.name === b[i]!.name);

export function CreditCellEditor({ shot, field, anchor, seed = '', onCommit, onClose }: CreditCellEditorProps) {
  const { t } = useTranslation();
  const boxRef = useRef<HTMLDivElement>(null);
  const role = field === 'survey_date' ? null : field;
  const original = role ? (shot.credit?.[role] ?? []) : [];
  // A ref beside the state: the blur a commit triggers adds a typed name
  // synchronously, before React re-renders.
  const [list, setListState] = useState<Person[]>(original);
  const listRef = useRef<Person[]>(original);
  const setList = useCallback((next: (prev: Person[]) => Person[]) => {
    listRef.current = next(listRef.current);
    setListState(listRef.current);
  }, []);
  const [date, setDate] = useState(shot.credit?.date ?? '');
  const dateRef = useRef<HTMLInputElement>(null);
  const typingRef = useRef(seed.trim() !== '');
  const doneRef = useRef(false);
  const suggestions = usePeopleSuggestions();

  const commit = useCallback(() => {
    if (doneRef.current) return;
    if (role && typingRef.current && document.activeElement instanceof HTMLElement) document.activeElement.blur();
    doneRef.current = true;
    if (role) {
      if (!sameList(listRef.current, original)) onCommit({ [`set_${role}`]: listRef.current });
    } else if (!dateRef.current?.validity?.badInput) {
      // A half-typed date reads as '' with badInput set: that is no edit, not
      // "clear the date".
      const next = date === '' ? null : date;
      if (next !== (shot.credit?.date ?? null)) onCommit({ set_date: next });
    }
    onClose();
  }, [role, original, date, shot.credit?.date, onCommit, onClose]);

  const cancel = useCallback(() => {
    doneRef.current = true;
    onClose();
  }, [onClose]);

  // The field's suggestion list is portaled outside the box: picking from it is inside.
  const suggestionsRef = useMemo<RefObject<HTMLElement | null>>(
    () => ({
      get current() {
        return document.querySelector<HTMLElement>('[data-testid="credit-cell-editor-people-suggestions"]');
      },
    }),
    [],
  );
  const pos = usePlacement(anchor, boxRef, commit);
  useFieldFocus(boxRef, anchor, pos !== null, doneRef);
  // Outside mousedowns commit; scrolls only move the popover (see usePlacement),
  // so the only scroll the shared layer reacts to is the page's own.
  useDismissableLayer({ open: true, onDismiss: commit, contains: [boxRef, suggestionsRef], survivesUnrelatedScroll: true });

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // Keys the field did not keep (PeopleInput stops the ones it uses).
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      cancel();
    } else if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      commit();
    } else {
      // Never let a key reach the grid underneath (Delete would clear cells).
      event.stopPropagation();
    }
  };

  const label = role ? t(`credit.${role}`) : t('credit.dateLabel');
  return createPortal(
    <div
      ref={boxRef}
      role="dialog"
      aria-label={label}
      className="w-80 rounded border border-border bg-panel p-2 shadow-lg"
      style={{ position: 'fixed', zIndex: 60, top: pos?.top ?? 0, left: pos?.left ?? 0, visibility: pos ? 'visible' : 'hidden' }}
      onKeyDown={onKeyDown}
      data-testid={CREDIT_EDITOR_TEST_ID}
      data-field={field}
    >
      {role ? (
        <PeopleInput
          label={label}
          value={peopleValues(list)}
          onAdd={(person: PersonLike) => {
            rememberPeople([person]);
            setList((prev) => withPerson(prev, person));
          }}
          onRemove={(person) => setList((prev) => withoutPerson(prev, person))}
          suggestions={suggestions}
          placeholder={t(`credit.placeholders.${role}`)}
          testId="credit-cell-editor-people"
          defaultText={seed}
          onCommitPending={(has) => {
            typingRef.current = has;
          }}
        />
      ) : (
        <label className="block text-sm text-text-secondary">
          <span className="mb-1 block">{label}</span>
          <input
            ref={dateRef}
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            className="w-full"
            data-testid="credit-cell-editor-date"
          />
        </label>
      )}
      <div className="mt-2 flex items-center justify-between gap-2">
        <span className="text-xs text-text-secondary">{t('credit.cellEditor.hint')}</span>
        <div className="flex gap-1">
          <Button size="sm" onClick={cancel} data-testid="credit-cell-editor-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button size="sm" variant="primary" onClick={commit} data-testid="credit-cell-editor-done">
            {t('credit.cellEditor.done')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
