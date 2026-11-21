/**
 * The replace box under the find box (same width): replacement text with
 * Replace (this match, then the next), Skip and Replace all; Replace all asks
 * first, in a line under the box that states exactly what will happen. Replace
 * asks the same way when its cell is a number shown rounded (the hidden
 * decimals go: the value is written as shown).
 * Disabled, with the reason, while the workspace is read-only.
 */
import { useEffect, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { MAX_SEARCH_MATCHES } from '../model/search';
import { formatNumber } from '../../../i18n/format';
import { LockIcon } from '../../common/icons';
import { tCount } from './labels';
import { FIELD_BOX_CLASS, FIELD_INPUT_CLASS } from './ToolbarMenu';
import type { ReplaceAllPreview, RoundedOnePreview } from './useReplace';
import type { SearchMatch } from '../types';

interface ReplaceRowProps {
  inputRef: RefObject<HTMLInputElement>;
  editingBlocked: boolean;
  /** The current matches: any change (a toggle, a scope, a filter, an edit, an
   *  undo) invalidates a pending Replace all confirmation. */
  matches: readonly SearchMatch[];
  busy: boolean;
  onReplaceOne: () => void;
  /** Non-null: Replace must ask first (see useReplace `previewOne`). */
  previewOne: () => RoundedOnePreview | null;
  onSkip: () => void;
  previewAll: () => ReplaceAllPreview | null;
  onReplaceAll: () => void;
  /** Find was closed with Escape from here: hand focus back to the grid. */
  onClosed: () => void;
}

/** The confirmation line under the box: Replace all's counts, or Replace's
 *  rounded number. */
type PendingConfirm = { kind: 'all'; preview: ReplaceAllPreview } | { kind: 'one'; rounded: RoundedOnePreview };

const BOX_BUTTON =
  'h-6 shrink-0 whitespace-nowrap rounded px-1.5 text-xs text-text-primary hover:bg-accent-10 disabled:cursor-not-allowed disabled:opacity-40';

export function ReplaceRow({
  inputRef,
  editingBlocked,
  matches,
  busy,
  onReplaceOne,
  previewOne,
  onSkip,
  previewAll,
  onReplaceAll,
  onClosed,
}: ReplaceRowProps) {
  const { t } = useTranslation();
  const replacement = useDataWorkspaceStore((s) => s.search.replacement);
  const regex = useDataWorkspaceStore((s) => s.search.regex);
  const query = useDataWorkspaceStore((s) => s.search.query);
  const setSearch = useDataWorkspaceStore((s) => s.setSearch);
  const closeSearch = useDataWorkspaceStore((s) => s.closeSearch);
  const currentMatch = useDataWorkspaceStore((s) => s.currentMatch);
  const [confirm, setConfirm] = useState<PendingConfirm | null>(null);
  const okRef = useRef<HTMLButtonElement | null>(null);

  const hasMatches = matches.length > 0;
  // Any change to what would be replaced invalidates the confirmation.
  useEffect(() => setConfirm(null), [replacement, query, regex, matches]);
  // Replace's question is about the current match only.
  useEffect(() => setConfirm((c) => (c?.kind === 'one' ? null : c)), [currentMatch]);
  useEffect(() => {
    if (confirm) okRef.current?.focus();
  }, [confirm]);

  const disabled = editingBlocked || busy || !hasMatches;

  const askAll = () => {
    const preview = previewAll();
    if (preview) setConfirm({ kind: 'all', preview });
  };
  const askOne = () => {
    const rounded = previewOne();
    if (rounded) {
      setConfirm({ kind: 'one', rounded });
      return;
    }
    setConfirm(null);
    onReplaceOne();
  };
  const cancel = () => {
    setConfirm(null);
    inputRef.current?.focus();
  };

  return (
    <div className="flex flex-col gap-1" data-testid="data-replace-row">
      <div className={FIELD_BOX_CLASS}>
        <input
          ref={inputRef}
          type="text"
          value={replacement}
          disabled={editingBlocked}
          onChange={(e) => setSearch({ replacement: e.target.value })}
          onKeyDown={(e) => {
            // Enter that confirms an IME composition is not a command.
            if (e.nativeEvent.isComposing || e.keyCode === 229) return;
            if (e.key === 'Enter') {
              e.preventDefault();
              if (e.ctrlKey || e.metaKey) askAll();
              else askOne();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              e.stopPropagation();
              if (confirm) setConfirm(null);
              else {
                closeSearch();
                onClosed();
              }
            }
          }}
          placeholder={editingBlocked ? t('dataToolbar.replace.blocked') : t('dataToolbar.replace.placeholder')}
          aria-label={t('dataToolbar.replace.aria')}
          title={regex ? t('dataToolbar.replace.regexHint') : undefined}
          spellCheck={false}
          autoComplete="off"
          className={FIELD_INPUT_CLASS}
          data-testid="data-replace-input"
        />
        {editingBlocked ? (
          <span
            className="inline-flex shrink-0 items-center gap-1 px-1 text-text-secondary"
            title={t('dataToolbar.replace.blocked')}
            data-testid="data-replace-blocked"
          >
            <LockIcon size={12} />
            <span className="sr-only">{t('dataToolbar.replace.blocked')}</span>
          </span>
        ) : (
          <>
            <button
              type="button"
              className={BOX_BUTTON}
              disabled={disabled}
              title={t('dataToolbar.replace.oneTitle')}
              onClick={askOne}
              data-testid="data-replace-one"
            >
              {t('dataToolbar.replace.one')}
            </button>
            <button
              type="button"
              className={BOX_BUTTON}
              disabled={!hasMatches}
              title={t('dataToolbar.replace.skipTitle')}
              onClick={onSkip}
              data-testid="data-replace-skip"
            >
              {t('dataToolbar.replace.skip')}
            </button>
            <button
              type="button"
              className={BOX_BUTTON}
              disabled={disabled}
              title={t('dataToolbar.replace.allTitle')}
              onClick={askAll}
              data-testid="data-replace-all"
            >
              {t('dataToolbar.replace.all')}
            </button>
          </>
        )}
      </div>
      {confirm && !editingBlocked && (
        <div
          className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded border border-accent-20 bg-accent-10 px-2 py-1 text-xs"
          role="alertdialog"
          aria-label={confirm.kind === 'all' ? t('dataToolbar.replace.all') : t('dataToolbar.replace.one')}
          aria-describedby="data-replace-confirm-text"
          data-testid="data-replace-confirm"
          data-kind={confirm.kind}
        >
          <span id="data-replace-confirm-text" className="min-w-0 flex-1" data-testid="data-replace-confirm-text">
            {confirm.kind === 'one' ? (
              t('dataToolbar.replace.confirmRoundedOne', { held: confirm.rounded.held, shown: confirm.rounded.shown })
            ) : (
              <>
                {t('dataToolbar.replace.confirm', {
                  matches: tCount(t, 'dataToolbar.replace.matches', confirm.preview.matches),
                  cells: tCount(t, 'dataToolbar.replace.cells', confirm.preview.cells),
                })}
                {confirm.preview.lockedMatches > 0 &&
                  ` ${tCount(t, 'dataToolbar.replace.confirmLocked', confirm.preview.lockedMatches)}`}
                {confirm.preview.invalidCells > 0 &&
                  ` ${tCount(t, 'dataToolbar.replace.confirmInvalid', confirm.preview.invalidCells)}`}
                {confirm.preview.roundedCells > 0 &&
                  ` ${tCount(t, 'dataToolbar.replace.confirmRounded', confirm.preview.roundedCells)}`}
                {confirm.preview.limited &&
                  ` ${t('dataToolbar.replace.confirmLimit', { n: formatNumber(MAX_SEARCH_MATCHES) })}`}
              </>
            )}
          </span>
          <span className="flex shrink-0 gap-1">
            <button
              ref={okRef}
              type="button"
              className="rounded bg-accent px-2 py-0.5 text-accent-on hover:bg-accent-90"
              onClick={() => {
                setConfirm(null);
                if (confirm.kind === 'all') onReplaceAll();
                else onReplaceOne();
              }}
              onKeyDown={(e) => {
                if (e.key === 'Escape') {
                  e.preventDefault();
                  e.stopPropagation();
                  cancel();
                }
              }}
              data-testid="data-replace-confirm-ok"
            >
              {confirm.kind === 'all' ? t('dataToolbar.replace.confirmOk') : t('dataToolbar.replace.one')}
            </button>
            <button
              type="button"
              className="rounded px-2 py-0.5 hover:bg-accent-20"
              onClick={cancel}
              data-testid="data-replace-confirm-cancel"
            >
              {t('dataToolbar.replace.cancel')}
            </button>
          </span>
        </div>
      )}
    </div>
  );
}
