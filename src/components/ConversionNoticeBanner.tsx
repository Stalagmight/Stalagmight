import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ConversionNoticeView } from '../types/ipc/ConversionNoticeView';
import { isNamePart, type ProsePart } from '../utils/prose';
import { BackendProse, ProseParts } from './common/Prose';

/** The banner's sentence on saving, by the file's state (`ConversionFile`). */
const SAVE_SENTENCE = {
  not_yet_saved: 'banners.conversion.notYetSaved',
  saved: 'banners.conversion.saved',
  saved_as_new_file: 'banners.conversion.savedAsNewFile',
  downloaded: 'banners.conversion.downloaded',
  read_only: 'banners.conversion.readOnly',
} as const;

/** The banner's lead, by which older file was converted (`ConversionSource`). */
const LEAD = {
  old_format: 'banners.conversion.lead',
  earlier_version_2: 'banners.conversion.upgradedLead',
} as const;

/** `changes` joined into one sentence, its first letter capitalised. */
function changesSentence(changes: readonly string[]): string {
  const text = changes.join('; ');
  return `${text.charAt(0).toUpperCase()}${text.slice(1)}.`;
}

/**
 * {@link changesSentence} as prose parts (RS-PROSE): each clause by its parts
 * when the backend sends some (`change_parts`, index-aligned), else its words,
 * joined by '; ' and ended with '.'. The first letter is capitalised when the
 * sentence opens with words; a sentence that opens with a name keeps the name
 * as it is. `null` when no clause names anything (the plain sentence then).
 */
function changesSentenceParts(
  changes: readonly string[],
  parts: readonly (readonly ProsePart[])[] | undefined,
): ProsePart[] | null {
  if (!parts?.some((clause) => clause.some(isNamePart))) return null;
  const out: ProsePart[] = [];
  changes.forEach((clause, i) => {
    if (i > 0) out.push({ kind: 'text', text: '; ' });
    const named = parts[i];
    if (named?.some(isNamePart)) out.push(...named);
    else out.push({ kind: 'text', text: clause });
  });
  out.push({ kind: 'text', text: '.' });
  const first = out[0];
  if (first?.kind === 'text') out[0] = { kind: 'text', text: `${first.text.charAt(0).toUpperCase()}${first.text.slice(1)}` };
  return out;
}

/**
 * The conversion notice (plan 7.2.10, demo issue I-5; kept stations, plan
 * 2026-11-23 RS 8.2 and 8.3): an old-format (SQLite) file, or a version 2
 * file an earlier build wrote, whose conversion changed something a line
 * shows. One short lead for the kind of file, then only what applies (the
 * backend words each change with its count), then, when a station is now
 * shown under another station's name or a comment is hidden, that Detach
 * Line from Station or Remove Closure brings each back, then ONE sentence on
 * saving that is true for the file now: before the converting save it says
 * the next save writes the new format; after it (the store follows
 * `CommandResult.legacy_converted`) it says it was saved; a file open
 * read-only is never saved, so it says the file stays as it is. A long list sits
 * behind Details, grouped by line. Stays until dismissed. A station or a line a
 * clause names is drawn as a prose pill (RS-PROSE: `change_parts`,
 * `detail_parts`); a screen reader reads the clause's plain words.
 */
export function ConversionNoticeBanner({
  notice,
  onDismiss,
}: {
  notice: ConversionNoticeView;
  onDismiss: () => void;
}) {
  const { t } = useTranslation();
  const [showDetails, setShowDetails] = useState(false);
  const hasDetails = notice.details.length > 0;
  const sentence = changesSentenceParts(notice.changes, notice.change_parts);
  return (
    <div
      className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-start gap-3 text-sm"
      role="status"
      data-testid="conversion-notice-banner"
    >
      <div className="flex-1 min-w-0">
        <span className="text-warning font-medium" data-testid="conversion-notice-lead">
          {t(LEAD[notice.source])}
        </span>{' '}
        <span className="text-text-secondary" data-testid="conversion-notice-text">
          {sentence ? <ProseParts parts={sentence} /> : changesSentence(notice.changes)}
        </span>{' '}
        {notice.brings_back && (
          <>
            <span className="text-text-secondary" data-testid="conversion-notice-brings-back">
              {t('banners.conversion.bringsBack')}
            </span>{' '}
          </>
        )}
        <span className="text-text-secondary" data-testid="conversion-notice-save">
          {t(SAVE_SENTENCE[notice.file])}
        </span>
        {hasDetails && showDetails && (
          <ul className="mt-1 max-h-32 overflow-y-auto text-xs text-text-secondary list-disc pl-5" data-testid="conversion-notice-details">
            {notice.details.map((line, i) => (
              <li key={`${i}:${line}`}>
                <BackendProse text={line} parts={notice.detail_parts?.[i]} />
              </li>
            ))}
          </ul>
        )}
      </div>
      {hasDetails && (
        <button
          onClick={() => setShowDetails((open) => !open)}
          className="px-2 py-1 text-text-secondary hover:text-text text-xs whitespace-nowrap"
          aria-expanded={showDetails}
          data-testid="conversion-notice-details-button"
        >
          {t(showDetails ? 'banners.conversion.hideDetails' : 'banners.conversion.details')}
        </button>
      )}
      <button
        onClick={onDismiss}
        className="px-2 py-1 text-text-secondary hover:text-text text-xs"
        data-testid="conversion-notice-dismiss-button"
      >
        {t('banners.conversion.dismiss')}
      </button>
    </div>
  );
}
