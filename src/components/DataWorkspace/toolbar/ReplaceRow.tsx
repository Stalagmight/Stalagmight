/**
 * The replace box under the find box (same width): replacement text with
 * Replace (this match, then the next), Skip and Replace all; Replace all asks
 * first, in a line under the box that states exactly what will happen.
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
import type { ReplaceAllPreview } from './useReplace';

interface ReplaceRowProps {
  inputRef: RefObject<HTMLInputElement>;
  editingBlocked: boolean;
  hasMatches: boolean;
  busy: boolean;
  onReplaceOne: () => void;
  onSkip: () => void;
  previewAll: () => ReplaceAllPreview | null;
  onReplaceAll: () => void;
  /** Find was closed with Escape from here: hand focus back to the grid. */
  onClosed: () => void;
}

const BOX_BUTTON =
  'h-6 shrink-0 whitespace-nowrap rounded px-1.5 text-xs text-text-primary hover:bg-accent-10 disabled:cursor-not-allowed disabled:opacity-40';

export function ReplaceRow({
  inputRef,
  editingBlocked,
  hasMatches,
  busy,
  onReplaceOne,
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
  const [confirm, setConfirm] = useState<ReplaceAllPreview | null>(null);
  const okRef = useRef<HTMLButtonElement | null>(null);

  // Any change to what would be replaced invalidates the confirmation.
  useEffect(() => setConfirm(null), [replacement, query, regex, hasMatches]);
  useEffect(() => {
    if (confirm) okRef.current?.focus();
  }, [confirm]);

  const disabled = editingBlocked || busy || !hasMatches;

  const askAll = () => {
    const preview = previewAll();
    if (preview) setConfirm(preview);
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
            if (e.key === 'Enter') {
              e.preventDefault();
              if (e.ctrlKey || e.metaKey) askAll();
              else onReplaceOne();
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
              onClick={onReplaceOne}
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
          aria-label={t('dataToolbar.replace.all')}
          aria-describedby="data-replace-confirm-text"
          data-testid="data-replace-confirm"
        >
          <span id="data-replace-confirm-text" className="min-w-0 flex-1" data-testid="data-replace-confirm-text">
            {t('dataToolbar.replace.confirm', {
              matches: tCount(t, 'dataToolbar.replace.matches', confirm.matches),
              cells: tCount(t, 'dataToolbar.replace.cells', confirm.cells),
            })}
            {confirm.lockedMatches > 0 && ` ${tCount(t, 'dataToolbar.replace.confirmLocked', confirm.lockedMatches)}`}
            {confirm.invalidCells > 0 && ` ${tCount(t, 'dataToolbar.replace.confirmInvalid', confirm.invalidCells)}`}
            {confirm.limited && ` ${t('dataToolbar.replace.confirmLimit', { n: formatNumber(MAX_SEARCH_MATCHES) })}`}
          </span>
          <span className="flex shrink-0 gap-1">
            <button
              ref={okRef}
              type="button"
              className="rounded bg-accent px-2 py-0.5 text-accent-on hover:bg-accent-90"
              onClick={() => {
                setConfirm(null);
                onReplaceAll();
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
              {t('dataToolbar.replace.confirmOk')}
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
