/**
 * Pill input for a list of people (surveyors, explorers): one text field that
 * holds a pill per person followed by an inline text input, with an
 * autocomplete list under it.
 *
 * Keyboard:
 * - Typing filters `suggestions` (see `rankSuggestions`). Up / Down move the
 *   highlight (and open the list when it is closed), Enter or ';' commits.
 * - The first suggestion is highlighted by default only when the query is a
 *   prefix of its name, so Enter on a fresh name creates that name instead of
 *   picking a loose match.
 * - Tab with typed text commits it the way Enter would, then lets focus move.
 * - Escape closes the list, then clears the typed text, and only then
 *   propagates (so the host dialog closes on the third press, not the first).
 * - Backspace in an empty input selects the last pill; a second Backspace
 *   removes it. Any other key deselects it.
 *
 * Identity is the (name, email) pair from `utils/people`: adding someone who
 * is already listed only flashes their pill.
 */
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import {
  disambiguationLabels,
  normalizeName,
  personKey,
  rankSuggestions,
  splitTypedNames,
  typedNames,
  type PersonLike,
  type RankedSuggestion,
  type Suggestion,
  type TextRange,
} from '../../utils/people';
import { CloseIcon, PlusIcon } from './icons';
import { Tooltip } from './Tooltip';

export interface PeopleInputValue {
  person: PersonLike;
  /** Shots carrying this person, when the field aggregates several shots. */
  count?: number;
  /** Shots the field aggregates. `count < total` renders a partial pill. */
  total?: number;
}

export interface PeopleInputProps {
  /** Visible label, e.g. "Surveyors" (visually hidden under `compact`, still announced). */
  label: string;
  value: PeopleInputValue[];
  /**
   * Add one person. A pasted list calls it once per new person within one
   * event, so the parent should update its state functionally.
   */
  onAdd: (person: PersonLike) => void;
  onRemove: (person: PersonLike) => void;
  /** Clicking the body of a partial pill: put this person on every shot. */
  onPromote?: (person: PersonLike) => void;
  suggestions: Suggestion[];
  placeholder?: string;
  /** Read-only: pills (and their tooltips) only, no remove buttons, no input. */
  disabled?: boolean;
  /**
   * Root test ID. Children: `-input`, `-field`, `-pill-<i>`, `-pill-<i>-label`, `-pill-<i>-remove`,
   * `-pill-<i>-promote`, `-pill-<i>-tooltip`, `-suggestions`, `-suggestion-<i>`,
   * `-suggestion-add` (the "Add <typed>" row) and `-status` (the live region).
   */
  testId: string;
  /** Id of the text input (for an external `<label htmlFor>`). */
  id?: string;
  autoFocus?: boolean;
  /** Dense variant for table cells: no box of its own, hidden label, smaller pills. */
  compact?: boolean;
  /**
   * With `compact`: wrap the pills onto as many lines as they need, each name
   * whole (a long one wraps between words), instead of one truncating line. For
   * cells whose row may grow (a read-only preview table), not the fixed-height
   * data grid.
   */
  wrap?: boolean;
  /** Fires when typed-but-uncommitted text appears (true) or goes away (false). */
  onCommitPending?: (hasPending: boolean) => void;
  /** Text the input starts with (a table cell opened by typing a letter), list open. */
  defaultText?: string;
}

type Option =
  | { kind: 'suggestion'; suggestion: RankedSuggestion }
  | { kind: 'create'; name: string };

/** How long a pill flashes when its person is added a second time. */
const FLASH_MS = 700;
/** Above every dialog (dialogStack ranks them from 51) and below tooltips (10000), like `Select`. */
const LIST_Z_INDEX = 9999;
const LIST_MAX_HEIGHT = 260;
/** So the list stays usable under a narrow table cell. */
const MIN_LIST_WIDTH = 220;
/** Smallest list worth showing when the window is very short. */
const MIN_LIST_HEIGHT = 64;
/** Space kept between the list and the field, and between the list and the window's (or app chrome's) edges. */
const LIST_GAP = 2;
const EDGE_MARGIN = 8;
const SEPARATOR = /[;\r\n]/;
/** The keyboard-only focus ring other inputs get from `focus:ring-1` (see index.css's input-modality gate). */
const KEYBOARD_RING =
  '[html[data-input-modality=keyboard]_&:focus-within]:ring-1 [html[data-input-modality=keyboard]_&:focus-within]:ring-accent';

/**
 * Where the mouse pointer last was, page-wide. Read when a suggestion list
 * opens, so a list that appears UNDER a resting pointer can tell the browser's
 * "the content moved under the cursor" events from the user moving the mouse.
 */
let lastPointer: { x: number; y: number } | null = null;
let trackingPointer = false;
function trackPointer(): void {
  if (trackingPointer || typeof window === 'undefined') return;
  trackingPointer = true;
  window.addEventListener(
    'mousemove',
    (event) => {
      lastPointer = { x: event.clientX, y: event.clientY };
    },
    { capture: true, passive: true },
  );
}

function isPartial(entry: PeopleInputValue): boolean {
  return typeof entry.count === 'number' && typeof entry.total === 'number' && entry.count < entry.total;
}

/**
 * Place the portaled list under the field, left-aligned and at least as wide
 * as it. It opens ABOVE only when it does not fit below and there is more
 * room above: unlike `useAnchoredFixedMenu` (which always takes the roomier
 * side), an autocomplete reads top-down from the field, and flipping it over
 * a dialog's title whenever the field sits low in the dialog is jarring.
 * Measure-then-place like that hook: a hidden first pass gives the natural
 * height, and the app chrome (`--toolbar-h` / `--statusbar-h`) bounds it.
 */
function useListPlacement(
  open: boolean,
  anchorRef: RefObject<HTMLElement | null>,
  listRef: RefObject<HTMLElement | null>,
  remeasureKey: string,
): CSSProperties {
  const [placement, setPlacement] = useState<{ top: number; left: number; width: number; maxHeight: number } | null>(null);
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    const list = listRef.current;
    if (!open || !anchor || !list) {
      setPlacement(null);
      return;
    }
    const rect = anchor.getBoundingClientRect();
    const styles = getComputedStyle(anchor);
    const topBound = (parseFloat(styles.getPropertyValue('--toolbar-h')) || 0) + EDGE_MARGIN;
    const bottomBound = window.innerHeight - (parseFloat(styles.getPropertyValue('--statusbar-h')) || 0) - EDGE_MARGIN;
    const natural = Math.min(list.scrollHeight, LIST_MAX_HEIGHT);
    const below = bottomBound - rect.bottom - LIST_GAP;
    const above = rect.top - topBound - LIST_GAP;
    const up = natural > below && above > below;
    const maxHeight = Math.max(Math.min(natural, up ? above : below), MIN_LIST_HEIGHT);
    const width = Math.max(rect.width, MIN_LIST_WIDTH);
    const left = Math.max(EDGE_MARGIN, Math.min(rect.left, window.innerWidth - width - EDGE_MARGIN));
    setPlacement({ top: up ? rect.top - LIST_GAP - maxHeight : rect.bottom + LIST_GAP, left, width, maxHeight });
  }, [open, anchorRef, listRef, remeasureKey]);

  if (!placement) {
    return { position: 'fixed', top: 0, left: 0, zIndex: LIST_Z_INDEX, visibility: 'hidden', pointerEvents: 'none' };
  }
  return { position: 'fixed', ...placement, zIndex: LIST_Z_INDEX, visibility: 'visible' };
}

function highlightRange(text: string, range: TextRange | null): ReactNode {
  if (!range) return text;
  const [start, end] = range;
  return (
    <>
      {text.slice(0, start)}
      <strong className="font-semibold">{text.slice(start, end)}</strong>
      {text.slice(end)}
    </>
  );
}

export function PeopleInput({
  label,
  value,
  onAdd,
  onRemove,
  onPromote,
  suggestions,
  placeholder,
  disabled = false,
  testId,
  id,
  autoFocus = false,
  compact = false,
  wrap = false,
  onCommitPending,
  defaultText = '',
}: PeopleInputProps) {
  const { t } = useTranslation();
  const baseId = useId();
  const inputId = id ?? `${baseId}-input`;
  const labelId = `${baseId}-label`;
  const listboxId = `${baseId}-listbox`;
  const optionId = useCallback((index: number) => `${baseId}-option-${index}`, [baseId]);

  const fieldRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // `textRef` mirrors the input synchronously, so the blur that follows a Tab
  // commit can never commit the same text twice. React 18 re-renders between
  // the two today (discrete events flush), but the field should not depend on it.
  const [text, setTextState] = useState(defaultText);
  const textRef = useRef(defaultText);
  const setText = useCallback((next: string) => {
    textRef.current = next;
    setTextState(next);
  }, []);
  const [listOpen, setListOpen] = useState(defaultText.trim() !== '');
  // Keyboard / hover highlight, valid only for the text it was set on: any edit
  // falls back to the default highlight of the new results.
  const [nav, setNav] = useState<{ text: string; index: number } | null>(null);
  const [armedKey, setArmedKey] = useState<string | null>(null);
  const [flashKey, setFlashKey] = useState<string | null>(null);
  const flashTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [focusedPill, setFocusedPill] = useState<number | null>(null);
  const [announcement, setAnnouncement] = useState('');

  const selected = useMemo(() => value.map((entry) => entry.person), [value]);
  const selectedKeys = useMemo(() => new Set(selected.map(personKey)), [selected]);
  const speleoKeys = useMemo(
    () => new Set(suggestions.filter((s) => s.source === 'speleodb').map((s) => personKey(s.person))),
    [suggestions],
  );
  const labels = useMemo(
    () =>
      disambiguationLabels([...selected, ...suggestions.map((s) => s.person)], {
        noEmail: t('common.peopleInput.noEmail'),
      }),
    [selected, suggestions, t],
  );
  const labelOf = (person: PersonLike) => labels.get(personKey(person)) ?? normalizeName(person.name);

  const query = normalizeName(text);
  const ranked = useMemo(
    () => (disabled ? [] : rankSuggestions(query, suggestions, selected)),
    [disabled, query, suggestions, selected],
  );
  const options = useMemo<Option[]>(() => {
    const rows: Option[] = ranked.map((suggestion) => ({ kind: 'suggestion', suggestion }));
    // "Add <typed>" unless that exact identity (this name, no email) is already
    // listed or selected: a same-named person WITH an email is someone else.
    const typedKey = personKey({ name: query });
    if (query !== '' && !selectedKeys.has(typedKey) && !ranked.some((r) => personKey(r.person) === typedKey)) {
      rows.push({ kind: 'create', name: query });
    }
    return rows;
  }, [ranked, query, selectedKeys]);

  const showList = !disabled && listOpen && options.length > 0;
  useLayoutEffect(() => {
    if (showList) hoverOrigin.current = lastPointer;
  }, [showList]);
  // Two or more different people carry exactly the typed name (two "John
  // Smith"s with different emails): Enter must not quietly pick one of them.
  const ambiguous = useMemo(() => {
    if (query === '') return false;
    const typedKey = personKey({ name: query });
    const same = ranked.filter((r) => personKey({ name: r.person.name }) === typedKey);
    return same.length > 1 && !same.some((r) => personKey(r.person) === typedKey);
  }, [query, ranked]);
  const defaultIndex = ranked[0]?.match === 'name-prefix' && !ambiguous ? 0 : -1;
  const activeIndex = !showList
    ? -1
    : nav && nav.text === text
      ? Math.min(nav.index, options.length - 1)
      : defaultIndex;

  const hasPending = query !== '';
  const onCommitPendingRef = useRef(onCommitPending);
  onCommitPendingRef.current = onCommitPending;
  const reportedPendingRef = useRef(false);
  useEffect(() => {
    if (reportedPendingRef.current === hasPending) return;
    reportedPendingRef.current = hasPending;
    onCommitPendingRef.current?.(hasPending);
  }, [hasPending]);

  useEffect(
    () => () => {
      if (flashTimerRef.current !== null) clearTimeout(flashTimerRef.current);
    },
    [],
  );

  const flash = useCallback((key: string) => {
    if (flashTimerRef.current !== null) clearTimeout(flashTimerRef.current);
    setFlashKey(key);
    flashTimerRef.current = setTimeout(() => {
      flashTimerRef.current = null;
      setFlashKey(null);
    }, FLASH_MS);
  }, []);

  const closeList = useCallback(() => {
    setListOpen(false);
    setNav(null);
  }, []);

  // Hover moves the highlight only after the pointer really MOVED over the
  // list. A list that opens (or scrolls) under a resting pointer gets
  // mouseenter / mouseover, and even mousemove, for the row that slid under
  // it; following those put the highlight on whatever row happened to be
  // there - the "Add <typed>" row, say - so Enter created a new person instead
  // of taking the default-highlighted match.
  useEffect(trackPointer, []);
  const hoverOrigin = useRef<{ x: number; y: number } | null>(null);

  /** Add each person not already listed; a repeat flashes the existing pill instead. */
  const addPeople = (people: PersonLike[]) => {
    const added = new Set<string>();
    const names: string[] = [];
    let repeat: PersonLike | null = null;
    for (const person of people) {
      const key = personKey(person);
      if (selectedKeys.has(key) || added.has(key)) {
        if (selectedKeys.has(key)) repeat = person;
        continue;
      }
      added.add(key);
      names.push(normalizeName(person.name));
      onAdd(person);
    }
    if (repeat) {
      flash(personKey(repeat));
      if (names.length === 0) setAnnouncement(t('common.peopleInput.announce.duplicate', { name: labelOf(repeat) }));
    }
    if (names.length > 0) setAnnouncement(t('common.peopleInput.announce.added', { name: names.join(', ') }));
  };

  /**
   * Typed text that names exactly one known person becomes that person, email
   * included, so a name typed out in full still gets the identity the list
   * would have given it. In order: the ONE SpeleoDB collaborator whose name or
   * email it is; else the known person with exactly that identity (this name,
   * no email); else the one known person of any source whose name or email it
   * is. Several matches, or none, give a new person with just the typed name -
   * never a guess between two same-named people.
   */
  const resolveTyped = (name: string): PersonLike => {
    const matching = (source?: Suggestion['source']) => {
      const seen = new Map<string, PersonLike>();
      for (const s of suggestions) {
        if ((source === undefined || s.source === source) && typedNames(name, s.person)) seen.set(personKey(s.person), s.person);
      }
      return [...seen.values()];
    };
    const collaborators = matching('speleodb');
    if (collaborators.length === 1) return collaborators[0]!;
    const key = personKey({ name });
    const exact = suggestions.find((s) => personKey(s.person) === key);
    if (exact) return exact.person;
    const known = matching();
    return known.length === 1 ? known[0]! : { name };
  };

  const pickOption = (option: Option) => {
    setText('');
    closeList();
    // The "Add <typed>" row is an explicit "someone new": exactly the typed name.
    addPeople([option.kind === 'suggestion' ? option.suggestion.person : { name: option.name }]);
  };

  /** Commit the typed text: the highlighted suggestion if asked and there is one, else the typed names. */
  const commit = (useHighlight: boolean) => {
    const option = useHighlight && activeIndex >= 0 ? options[activeIndex] : undefined;
    if (option) {
      pickOption(option);
      return;
    }
    const names = splitTypedNames(textRef.current);
    setText('');
    closeList();
    if (names.length > 0) addPeople(names.map(resolveTyped));
  };

  const remove = (person: PersonLike) => {
    setArmedKey(null);
    onRemove(person);
    setAnnouncement(t('common.peopleInput.announce.removed', { name: labelOf(person) }));
    // The x button (keyboard path) disappears with its pill; keep focus in the field.
    inputRef.current?.focus();
  };

  const onChange = (next: string) => {
    setArmedKey(null);
    if (SEPARATOR.test(next)) {
      // A separator that arrived without its keydown (autocorrect, an IME,
      // drag and drop): commit what precedes the last one, keep the rest.
      const parts = next.split(SEPARATOR);
      const rest = parts.pop() ?? '';
      setText(rest);
      setNav(null);
      setListOpen(rest.trim() !== '');
      addPeople(splitTypedNames(parts.join(';')).map(resolveTyped));
      return;
    }
    setText(next);
    setListOpen(next.trim() !== '');
  };

  const onPaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const pasted = event.clipboardData.getData('text/plain') || event.clipboardData.getData('text');
    if (!SEPARATOR.test(pasted)) return; // a single name: the normal change path
    event.preventDefault();
    const input = event.currentTarget;
    const start = input.selectionStart ?? input.value.length;
    const end = input.selectionEnd ?? start;
    const combined = input.value.slice(0, start) + pasted + input.value.slice(end);
    setArmedKey(null);
    setText('');
    closeList();
    addPeople(splitTypedNames(combined).map(resolveTyped));
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key !== 'Backspace') setArmedKey(null);
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowUp': {
        if (options.length === 0) return;
        event.preventDefault();
        event.stopPropagation();
        const down = event.key === 'ArrowDown';
        const last = options.length - 1;
        if (!showList) {
          setListOpen(true);
          setNav({ text, index: down ? 0 : last });
          return;
        }
        const next = activeIndex < 0 ? (down ? 0 : last) : (activeIndex + (down ? 1 : -1) + options.length) % options.length;
        setNav({ text, index: next });
        return;
      }
      case 'Enter':
      case ';': {
        // Never type the separator itself.
        if (event.key === ';') event.preventDefault();
        // Modified Enter (Ctrl+Enter to submit a dialog, Shift+Enter) is the host's.
        if (event.key === 'Enter' && (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey)) return;
        // Enter on an empty input is the host's too (a dialog's default action).
        if (activeIndex < 0 && textRef.current.trim() === '') return;
        event.preventDefault();
        event.stopPropagation();
        if (activeIndex < 0 && ambiguous) {
          // Ask which one instead of guessing: show the candidates.
          setListOpen(true);
          setAnnouncement(t('common.peopleInput.announce.ambiguous', { name: query }));
          return;
        }
        commit(true);
        return;
      }
      case 'Tab':
        // Users expect the name they typed to survive tabbing away: commit it
        // like Enter, and do not prevent the focus move.
        if (textRef.current.trim() !== '') commit(true);
        else closeList();
        return;
      case 'Escape':
        if (showList) {
          event.preventDefault();
          event.stopPropagation();
          closeList();
        } else if (text !== '') {
          event.preventDefault();
          event.stopPropagation();
          setText('');
        }
        return;
      case 'Backspace': {
        const lastEntry = value[value.length - 1];
        if (text !== '' || !lastEntry) return;
        event.preventDefault();
        const key = personKey(lastEntry.person);
        if (armedKey === key) {
          remove(lastEntry.person);
        } else {
          setArmedKey(key);
          setAnnouncement(t('common.peopleInput.announce.armed', { name: labelOf(lastEntry.person) }));
        }
        return;
      }
      default:
        return;
    }
  };

  const onBlur = () => {
    // Focus went to another window or app: the input is still the document's
    // active element and the user is coming back to it, so keep the text.
    if (document.activeElement === inputRef.current) return;
    setArmedKey(null);
    // Leaving the field commits typed text rather than dropping it, the same
    // as Tab, but as typed: a loose highlight is never picked by a blur, only
    // an exact known identity is reused (Gmail's recipient field does the
    // same). Hosts that must know about text that has not been committed yet
    // get it from `onCommitPending`.
    commit(false);
  };

  const onFieldMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (disabled || event.target === inputRef.current) return;
    // A click on a pill, its buttons or the empty part of the field keeps the
    // focus (and any typed text) in the input instead of blurring it.
    event.preventDefault();
    inputRef.current?.focus();
  };

  // Re-placed when the rows change (the list's height) or a pill changes the
  // field's height by wrapping onto another line.
  const listStyle = useListPlacement(showList, fieldRef, listRef, `${options.length}:${value.length}`);
  useDismissableLayer({
    open: showList,
    onDismiss: closeList,
    contains: [fieldRef, listRef],
    scrollSafe: [listRef],
    survivesUnrelatedScroll: true,
  });

  useEffect(() => {
    if (activeIndex >= 0) document.getElementById(optionId(activeIndex))?.scrollIntoView?.({ block: 'nearest' });
  }, [activeIndex, optionId]);

  const pillKeys = useMemo(() => {
    const seen = new Map<string, number>();
    return selected.map((person) => {
      const key = personKey(person);
      const n = seen.get(key) ?? 0;
      seen.set(key, n + 1);
      return n === 0 ? key : `${key}#${n}`;
    });
  }, [selected]);

  const renderPill = (entry: PeopleInputValue, index: number) => {
    const { person } = entry;
    const key = personKey(person);
    const pillTestId = `${testId}-pill-${index}`;
    const pillLabel = labelOf(person);
    const email = (person.email ?? '').trim();
    const partial = isPartial(entry);
    const promotable = partial && onPromote !== undefined && !disabled;
    const armed = armedKey === key && index === value.length - 1;
    const isSpeleo = speleoKeys.has(key);
    const onShots = partial
      ? t('common.peopleInput.onShots', { carrying: entry.count, total: entry.total })
      : typeof entry.total === 'number' && entry.total > 1
        ? t('common.peopleInput.onAllShots', { count: entry.total })
        : '';
    const details = [email, isSpeleo ? t('common.peopleInput.speleodbCollaborator') : '', onShots].filter(Boolean);

    const tone = armed
      ? 'border-accent bg-accent-30 ring-1 ring-accent text-text-primary'
      : partial
        ? 'border-dashed border-accent-50 bg-accent-5 text-text-secondary'
        : 'border-accent-30 bg-accent-10 text-text-primary';
    const size = compact ? (wrap ? 'min-h-5 py-px px-1 gap-0.5' : 'h-5 px-1 gap-0.5') : 'h-6 px-1.5 gap-1';
    const body = (
      <>
        <span className={wrap ? 'whitespace-normal' : 'truncate'} data-testid={`${pillTestId}-label`}>
          {pillLabel}
        </span>
        {partial && (
          <span className="shrink-0 text-[10px] tabular-nums text-text-secondary" aria-hidden="true" data-testid={`${pillTestId}-count`}>
            {entry.count}/{entry.total}
          </span>
        )}
      </>
    );

    return (
      <li
        key={pillKeys[index]}
        className="inline-flex min-w-0 max-w-full rounded focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
        data-testid={pillTestId}
        data-partial={partial || undefined}
        data-armed={armed || undefined}
        data-flash={flashKey === key || undefined}
        tabIndex={disabled ? 0 : undefined}
        onFocus={() => setFocusedPill(index)}
        onBlur={(event: FocusEvent<HTMLLIElement>) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocusedPill(null);
        }}
      >
        <Tooltip
          className="max-w-full"
          open={focusedPill === index ? true : undefined}
          tooltipTestId={`${pillTestId}-tooltip`}
          content={
            <span className="flex flex-col gap-0.5 text-xs">
              <span className="font-medium text-text-primary">{normalizeName(person.name)}</span>
              {details.map((line) => (
                <span key={line} className="text-text-secondary">
                  {line}
                </span>
              ))}
              {promotable && <span className="text-text-secondary">{t('common.peopleInput.promoteHint')}</span>}
            </span>
          }
        >
          <span
            className={`inline-flex min-w-0 max-w-full items-center rounded border text-xs transition-shadow ${size} ${tone} ${
              flashKey === key ? 'ring-2 ring-accent' : ''
            }`}
          >
            {promotable ? (
              <button
                type="button"
                className="inline-flex min-w-0 items-center gap-1 rounded-sm hover:text-text-primary"
                onClick={() => onPromote?.(person)}
                data-testid={`${pillTestId}-promote`}
              >
                {body}
                <span className="sr-only">{t('common.peopleInput.promote', { name: pillLabel })}</span>
              </button>
            ) : (
              body
            )}
            {details.length > 0 && <span className="sr-only">{`, ${details.join(', ')}`}</span>}
            {!disabled && (
              <button
                type="button"
                className="-mr-0.5 inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm text-text-secondary hover:bg-accent-30 hover:text-text-primary"
                aria-label={t('common.peopleInput.remove', { name: pillLabel })}
                onClick={() => remove(person)}
                data-testid={`${pillTestId}-remove`}
              >
                <CloseIcon size={10} />
              </button>
            )}
          </span>
        </Tooltip>
      </li>
    );
  };

  /** A mousemove over row `index`: follow it only if the pointer moved since the last one seen. */
  const hoverRow = (event: MouseEvent<HTMLDivElement>, index: number) => {
    const at = { x: event.clientX, y: event.clientY };
    const before = hoverOrigin.current;
    hoverOrigin.current = at;
    if (before === null || (before.x === at.x && before.y === at.y)) return;
    if (!nav || nav.text !== text || nav.index !== index) setNav({ text, index });
  };

  const renderOption = (option: Option, index: number) => {
    const active = index === activeIndex;
    const rowClass = `flex items-center gap-2 cursor-pointer ${compact ? 'px-2 py-0.5' : 'px-2 py-1'} ${
      active ? 'bg-accent-20' : ''
    }`;
    if (option.kind === 'create') {
      return (
        <div
          key="create"
          id={optionId(index)}
          role="option"
          aria-selected={active}
          className={`${rowClass} text-text-secondary ${index > 0 ? 'border-t border-border' : ''}`}
          onMouseMove={(event) => hoverRow(event, index)}
          onClick={() => pickOption(option)}
          data-testid={`${testId}-suggestion-add`}
        >
          <PlusIcon size={12} />
          <span className="truncate">{t('common.peopleInput.addNew', { name: option.name })}</span>
        </div>
      );
    }
    const { person, source, nameRange, emailRange } = option.suggestion;
    const email = (person.email ?? '').trim();
    return (
      <div
        key={personKey(person)}
        id={optionId(index)}
        role="option"
        aria-selected={active}
        className={rowClass}
        onMouseMove={(event) => hoverRow(event, index)}
        onClick={() => pickOption(option)}
        data-testid={`${testId}-suggestion-${index}`}
      >
        <span className="truncate text-text-primary">{highlightRange(normalizeName(person.name), nameRange)}</span>
        {email && <span className="min-w-0 truncate text-xs text-text-secondary">{highlightRange(email, emailRange)}</span>}
        {source === 'speleodb' && (
          <span
            className="ml-auto shrink-0 rounded border border-border px-1 text-[10px] leading-4 text-text-secondary"
            data-testid={`${testId}-suggestion-${index}-badge`}
          >
            {t('common.peopleInput.speleodbBadge')}
          </span>
        )}
      </div>
    );
  };

  // Compact stays on one line (a table row has a fixed height): pills shrink
  // and truncate instead of wrapping, and the tooltip carries the full name.
  const fieldClass = compact
    ? wrap
      ? `flex-wrap min-h-[24px] py-0.5 gap-0.5 bg-transparent`
      : `flex-nowrap overflow-hidden min-h-[24px] h-full px-1 py-0.5 gap-0.5 bg-transparent focus-within:ring-1 focus-within:ring-inset focus-within:ring-accent`
    : `flex-wrap min-h-[32px] px-1.5 py-1 gap-1 rounded border border-border bg-background focus-within:border-accent ${KEYBOARD_RING}`;

  return (
    <div className={`min-w-0 ${compact && !wrap ? 'h-full' : ''}`} data-testid={testId} data-dropdown-open={showList || undefined}>
      <label
        id={labelId}
        htmlFor={disabled ? undefined : inputId}
        className={compact ? 'sr-only' : 'mb-1 block text-sm text-text-secondary'}
      >
        {label}
      </label>
      <div
        ref={fieldRef}
        className={`flex w-full min-w-0 items-center text-sm ${fieldClass} ${disabled ? 'cursor-default' : 'cursor-text'}`}
        onMouseDown={onFieldMouseDown}
        data-testid={`${testId}-field`}
      >
        <ul role="list" aria-labelledby={labelId} className="contents">
          {value.map(renderPill)}
        </ul>
        {disabled ? (
          value.length === 0 && <span className="text-text-secondary">{t('common.states.none')}</span>
        ) : (
          <input
            ref={inputRef}
            id={inputId}
            type="text"
            role="combobox"
            aria-autocomplete="list"
            aria-expanded={showList}
            aria-controls={showList ? listboxId : undefined}
            aria-activedescendant={activeIndex >= 0 ? optionId(activeIndex) : undefined}
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            autoFocus={autoFocus}
            data-autofocus={autoFocus ? '' : undefined}
            value={text}
            placeholder={value.length === 0 ? placeholder : undefined}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            onBlur={onBlur}
            // Grows into whatever the last pill row leaves, and only wraps onto a
            // row of its own when less than ~6 characters remain: a wider minimum
            // left an empty second row under pills that nearly filled the field.
            className={`min-w-[6ch] flex-1 rounded-none border-0 bg-transparent p-0 text-text-primary placeholder:text-text-secondary focus:outline-none focus-visible:outline-none ${
              compact ? 'h-5 text-xs' : 'h-6'
            }`}
            data-testid={`${testId}-input`}
          />
        )}
      </div>
      <span className="sr-only" aria-live="polite" data-testid={`${testId}-status`}>
        {announcement}
      </span>
      {showList &&
        createPortal(
          <div
            ref={listRef}
            id={listboxId}
            role="listbox"
            aria-labelledby={labelId}
            className={`overflow-auto rounded border border-border bg-panel py-1 shadow-lg ${compact ? 'text-xs' : 'text-sm'}`}
            style={listStyle}
            // Keep the focus (and the typed text) in the input while picking.
            onMouseDown={(event) => event.preventDefault()}
            data-testid={`${testId}-suggestions`}
          >
            {options.map(renderOption)}
          </div>,
          document.body,
        )}
    </div>
  );
}

export default PeopleInput;
