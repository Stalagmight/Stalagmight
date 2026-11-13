/**
 * The replace row under the find box: replacement text, Replace (this match,
 * then the next), Skip, and Replace all behind an inline confirmation that
 * states exactly what will happen. Disabled, with the reason, while the
 * workspace is read-only.
 */
import { useEffect, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { MAX_SEARCH_MATCHES } from '../model/search';
import { formatNumber } from '../../../i18n/format';
import { LockIcon } from '../../common/icons';
import { tCount } from './labels';
import { toolbarButtonClass } from './ToolbarMenu';
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
}

export function ReplaceRow({
  inputRef,
  editingBlocked,
  hasMatches,
  busy,
  onReplaceOne,
  onSkip,
  previewAll,
  onReplaceAll,
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

  const disabled = editingBlocked || busy;

  const askAll = () => {
    const preview = previewAll();
    if (preview) setConfirm(preview);
  };

  return (
    <div className="flex flex-wrap items-center gap-2 px-3 pb-1.5" data-testid="data-replace-row">
      <div className="flex min-w-[18rem] max-w-[40rem] flex-[1_1_22rem] items-center rounded border border-border bg-background px-1.5 focus-within:border-accent">
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
              else closeSearch();
            }
          }}
          placeholder={t('dataToolbar.replace.placeholder')}
          aria-label={t('dataToolbar.replace.aria')}
          title={regex ? t('dataToolbar.replace.regexHint') : undefined}
          spellCheck={false}
          autoComplete="off"
          className="min-w-0 flex-1 bg-transparent px-1 py-1 text-xs text-text-primary outline-none placeholder:text-text-secondary disabled:cursor-not-allowed"
          data-testid="data-replace-input"
        />
      </div>
      {editingBlocked ? (
        <span className="inline-flex items-center gap-1 text-xs text-text-secondary" data-testid="data-replace-blocked">
          <LockIcon size={12} />
          {t('dataToolbar.replace.blocked')}
        </span>
      ) : confirm ? (
        <div
          className="flex flex-wrap items-center gap-2 rounded border border-accent-20 bg-accent-10 px-2 py-0.5 text-xs"
          role="alertdialog"
          aria-label={t('dataToolbar.replace.all')}
          data-testid="data-replace-confirm"
        >
          <span data-testid="data-replace-confirm-text">
            {t('dataToolbar.replace.confirm', {
              matches: tCount(t, 'dataToolbar.replace.matches', confirm.matches),
              cells: tCount(t, 'dataToolbar.replace.cells', confirm.cells),
            })}
            {confirm.lockedMatches > 0 && ` ${tCount(t, 'dataToolbar.replace.confirmLocked', confirm.lockedMatches)}`}
            {confirm.invalidCells > 0 && ` ${tCount(t, 'dataToolbar.replace.confirmInvalid', confirm.invalidCells)}`}
            {confirm.limited && ` ${t('dataToolbar.replace.confirmLimit', { n: formatNumber(MAX_SEARCH_MATCHES) })}`}
          </span>
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
                setConfirm(null);
                inputRef.current?.focus();
              }
            }}
            data-testid="data-replace-confirm-ok"
          >
            {t('dataToolbar.replace.confirmOk')}
          </button>
          <button
            type="button"
            className="rounded px-2 py-0.5 hover:bg-accent-20"
            onClick={() => {
              setConfirm(null);
              inputRef.current?.focus();
            }}
            data-testid="data-replace-confirm-cancel"
          >
            {t('dataToolbar.replace.cancel')}
          </button>
        </div>
      ) : (
        <>
          <button
            type="button"
            className={toolbarButtonClass({ disabled })}
            disabled={disabled || !hasMatches}
            title={t('dataToolbar.replace.oneTitle')}
            onClick={onReplaceOne}
            data-testid="data-replace-one"
          >
            {t('dataToolbar.replace.one')}
          </button>
          <button
            type="button"
            className={toolbarButtonClass({ disabled })}
            disabled={!hasMatches}
            title={t('dataToolbar.replace.skipTitle')}
            onClick={onSkip}
            data-testid="data-replace-skip"
          >
            {t('dataToolbar.replace.skip')}
          </button>
          <button
            type="button"
            className={toolbarButtonClass({ disabled })}
            disabled={disabled || !hasMatches}
            title={t('dataToolbar.replace.allTitle')}
            onClick={askAll}
            data-testid="data-replace-all"
          >
            {t('dataToolbar.replace.all')}
          </button>
        </>
      )}
    </div>
  );
}
