/**
 * The find box: query, match case / whole word / regex toggles, the match
 * count, previous / next, the replace toggle and the search options.
 *
 * The input is instant; the query reaches the view pipeline debounced
 * (`QUERY_DEBOUNCE_MS`). Enter / Shift+Enter go to the next / previous match
 * (Enter right after typing just applies the query, which already selects the
 * first match from the active row). Escape clears the query, then closes find.
 * Alt+C / Alt+W / Alt+R toggle the options while the box has focus.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { compileSearch } from '../model/search';
import type { DataView } from '../types';
import { formatNumber } from '../../../i18n/format';
import { ChevronDownIcon, ChevronRightIcon, ChevronUpIcon, CloseIcon } from '../../common/icons';
import { SearchIcon } from './toolbarIcons';
import { FIELD_BOX_CLASS, FIELD_INPUT_CLASS, iconButtonClass } from './ToolbarMenu';
import { FindOptionsMenu } from './FindOptionsMenu';
import type { FindNavigation } from './useFindNavigation';
import { replaceShortcutKey } from './useFindShortcuts';

export const QUERY_DEBOUNCE_MS = 150;

type Toggle = 'matchCase' | 'wholeWord' | 'regex';

/** Alt+<key> (by physical key, so macOS's Option characters don't matter). */
const ALT_TOGGLES: Record<string, Toggle> = { KeyC: 'matchCase', KeyW: 'wholeWord', KeyR: 'regex' };

interface FindBarProps {
  view: DataView;
  nav: FindNavigation;
  inputRef: RefObject<HTMLInputElement>;
  /** The workspace is read-only (the read-only count is then noise). */
  editingBlocked: boolean;
  /** Find was closed with Escape: hand focus back to the grid. */
  onClosed: () => void;
}

export function FindBar({ view, nav, inputRef, editingBlocked, onClosed }: FindBarProps) {
  const { t } = useTranslation();
  const search = useDataWorkspaceStore((s) => s.search);
  const currentMatch = useDataWorkspaceStore((s) => s.currentMatch);
  const setSearch = useDataWorkspaceStore((s) => s.setSearch);
  const closeSearch = useDataWorkspaceStore((s) => s.closeSearch);
  const [draft, setDraft] = useState(search.query);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // The query can change from outside (Clear all, project reset): follow it
  // unless the user is mid-typing.
  useEffect(() => {
    if (timer.current === null) setDraft(search.query);
  }, [search.query]);
  useEffect(() => () => {
    if (timer.current !== null) clearTimeout(timer.current);
  }, []);

  const cancelPending = () => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  };
  const commit = useCallback(
    (query: string) => {
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = null;
      const s = useDataWorkspaceStore.getState().search;
      // Re-committing the same query would rebuild the whole view for nothing.
      if (s.query !== query || !s.open) setSearch({ query, open: true });
    },
    [setSearch],
  );

  const onChange = (value: string) => {
    setDraft(value);
    if (!useDataWorkspaceStore.getState().search.open) setSearch({ open: true });
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => commit(value), QUERY_DEBOUNCE_MS);
  };

  const toggle = (key: Toggle) => {
    const s = useDataWorkspaceStore.getState().search;
    setSearch({ [key]: !s[key], open: true });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    // Enter that confirms an IME composition is not a command.
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter') {
      e.preventDefault();
      if (timer.current !== null) commit(draft);
      else if (e.shiftKey) nav.prev();
      else nav.next();
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (draft !== '') {
        setDraft('');
        commit('');
      } else {
        // A pending commit (type, delete, Escape within the debounce) must not reopen find.
        cancelPending();
        closeSearch();
        onClosed();
      }
      return;
    }
    const alt = e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey ? ALT_TOGGLES[e.code] : undefined;
    if (alt) {
      e.preventDefault();
      e.stopPropagation();
      toggle(alt);
    }
  };

  const compiled = useMemo(
    () => compileSearch({ query: search.query, matchCase: search.matchCase, wholeWord: search.wholeWord, regex: search.regex }),
    [search.query, search.matchCase, search.wholeWord, search.regex],
  );
  const lockedCount = useMemo(() => view.matches.reduce((n, m) => n + (m.lockReason !== null ? 1 : 0), 0), [view.matches]);
  const total = view.matches.length;

  let count: { text: string; tone: 'normal' | 'error'; title?: string } | null = null;
  if (compiled.error) {
    count = { text: t('dataToolbar.find.regexError'), tone: 'error', title: compiled.error };
  } else if (search.open && search.query !== '') {
    if (total === 0) count = { text: t('dataToolbar.find.noResults'), tone: 'error' };
    else if (currentMatch < 0) {
      const n = `${formatNumber(total)}${view.matchLimitReached ? '+' : ''}`;
      count = { text: t('dataToolbar.find.noCurrent', { count: total, n }), tone: 'normal' };
    }
    else {
      const key = view.matchLimitReached ? 'dataToolbar.find.countLimit' : 'dataToolbar.find.count';
      count = { text: t(key, { current: formatNumber(currentMatch + 1), total: formatNumber(total) }), tone: 'normal' };
    }
  }
  const showLocked = count !== null && count.tone === 'normal' && search.replaceOpen && !editingBlocked && lockedCount > 0;

  const toggleButton = (key: Toggle, glyph: ReactNode, title: string, testId: string) => (
    <button
      type="button"
      className={`${iconButtonClass(search[key])} font-mono`}
      aria-pressed={search[key]}
      title={title}
      aria-label={title}
      onClick={() => toggle(key)}
      onMouseDown={(e) => e.preventDefault() /* keep focus in the input */}
      data-testid={testId}
    >
      {glyph}
    </button>
  );

  return (
    <div
      className={`${FIELD_BOX_CLASS} w-full`}
      role="search"
      data-testid="data-find"
    >
      <span className="shrink-0 text-text-secondary">
        <SearchIcon size={14} />
      </span>
      <input
        ref={inputRef}
        type="text"
        value={draft}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onFocus={() => {
          if (!useDataWorkspaceStore.getState().search.open) setSearch({ open: true });
        }}
        placeholder={t('dataToolbar.find.placeholder')}
        aria-label={t('dataToolbar.find.aria')}
        aria-invalid={compiled.error ? true : undefined}
        spellCheck={false}
        autoComplete="off"
        className={FIELD_INPUT_CLASS}
        data-testid="data-find-input"
      />
      {draft !== '' && (
        <button
          type="button"
          className={iconButtonClass(false)}
          title={t('dataToolbar.find.clear')}
          aria-label={t('dataToolbar.find.clear')}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            setDraft('');
            commit('');
            inputRef.current?.focus();
          }}
          data-testid="data-find-clear"
        >
          <CloseIcon size={10} />
        </button>
      )}
      {toggleButton('matchCase', 'Aa', t('dataToolbar.find.matchCase'), 'data-find-match-case')}
      {toggleButton('wholeWord', <span className="underline underline-offset-2">ab</span>, t('dataToolbar.find.wholeWord'), 'data-find-whole-word')}
      {toggleButton('regex', '.*', t('dataToolbar.find.regex'), 'data-find-regex')}
      {/* Room is reserved only while there is a count: an idle find box stays
          narrow enough for the toolbar to keep its labels. */}
      <span
        className={`shrink-0 whitespace-nowrap text-center tabular-nums ${count ? 'ml-1 min-w-[3.5rem]' : ''} ${
          count?.tone === 'error' ? 'text-error' : 'text-text-secondary'
        }`}
        title={count?.title}
        aria-live="polite"
        data-testid={compiled.error ? 'data-find-error' : 'data-find-count'}
      >
        {count?.text ?? ''}
        {showLocked && (
          <span title={t('dataToolbar.find.readOnlyTitle')} data-testid="data-find-locked">
            {' · '}
            {t('dataToolbar.find.readOnly', { n: formatNumber(lockedCount) })}
          </span>
        )}
      </span>
      <button
        type="button"
        className={iconButtonClass(false)}
        title={t('dataToolbar.find.prev')}
        aria-label={t('dataToolbar.find.prev')}
        disabled={total === 0}
        onMouseDown={(e) => e.preventDefault()}
        onClick={nav.prev}
        data-testid="data-find-prev"
      >
        <ChevronUpIcon size={14} />
      </button>
      <button
        type="button"
        className={iconButtonClass(false)}
        title={t('dataToolbar.find.next')}
        aria-label={t('dataToolbar.find.next')}
        disabled={total === 0}
        onMouseDown={(e) => e.preventDefault()}
        onClick={nav.next}
        data-testid="data-find-next"
      >
        <ChevronDownIcon size={14} />
      </button>
      <button
        type="button"
        className={iconButtonClass(search.replaceOpen)}
        title={search.replaceOpen ? t('dataToolbar.find.closeReplace') : t(replaceShortcutKey())}
        aria-label={search.replaceOpen ? t('dataToolbar.find.closeReplace') : t(replaceShortcutKey())}
        aria-expanded={search.replaceOpen}
        onClick={() => setSearch({ replaceOpen: !search.replaceOpen, open: true })}
        data-testid="data-find-replace-toggle"
      >
        {search.replaceOpen ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
      </button>
      <FindOptionsMenu />
    </div>
  );
}
