import { type MouseEvent } from 'react';
import { useTranslation } from 'react-i18next';
import {
  formatDate as formatDateIntl,
  formatNumber,
  formatRelativeTime,
} from '../../i18n/format';
import { ElidedPath, Tooltip } from '../common';
import { useProjectStatFormatters } from './useProjectStatFormatters';
import {
  SpeleoRecentInfo,
  recentAccent,
  recentCardStyle,
  type SpeleoLockInfo,
} from './SpeleoLockBadge';
import { systemCardTestId } from '../../utils/testId';

interface SystemCardProps {
  name: string;
  location: string | null;
  filePath: string;
  modifiedAt: string;
  sectionCount: number;
  totalLength: number;
  isSelected?: boolean;
  /** SpeleoDB lock state for this entry (#201); a local project renders no badge. */
  speleoLock?: SpeleoLockInfo;
  /** Called when the card is clicked. Receives true if Ctrl/Cmd was held. */
  onSelect?: (ctrlKey: boolean) => void;
  /** Called on right-click to show context menu */
  onContextMenu?: (e: MouseEvent) => void;
  /**
   * Called on double-click to open the project. Wired to GlobalView's
   * `handleOpenFromList` (matching `RemoteSystemCard`'s `onOpen`), which sets
   * the 'recent-list' telemetry source and routes through the shared
   * open/import router's `allow: 'project-only'` gate - so this component owns
   * no routing or attribution of its own.
   */
  onOpen?: (filePath: string) => void;
}

export default function SystemCard({
  name,
  location,
  filePath,
  modifiedAt,
  sectionCount,
  totalLength,
  isSelected,
  speleoLock,
  onSelect,
  onContextMenu,
  onOpen,
}: SystemCardProps) {
  const { t } = useTranslation();
  const { formatLength } = useProjectStatFormatters();
  const testId = systemCardTestId(filePath);

  const handleClick = (e: MouseEvent) => {
    // Single click selects the card
    // Pass ctrlKey (or metaKey for Mac) to support multi-select
    if (onSelect) {
      onSelect(e.ctrlKey || e.metaKey);
    }
  };

  const handleDoubleClick = () => {
    // Delegates to GlobalView's handleOpenFromList (via onOpen), which owns
    // the telemetry attribution and the shared open/import router - see the
    // onOpen prop doc above for why routing doesn't live in this component.
    onOpen?.(filePath);
  };

  const handleContextMenu = (e: MouseEvent) => {
    if (onContextMenu) {
      onContextMenu(e);
    }
  };

  // Relative "edited" label (e.g. "2 hours ago"); the exact timestamp is shown
  // in the tooltip below. Falls back to the raw string if the date is invalid.
  const relativeModified = (dateStr: string) => {
    try {
      const date = new Date(dateStr);
      if (Number.isNaN(date.getTime())) return dateStr;
      return formatRelativeTime(date);
    } catch {
      return dateStr;
    }
  };

  const absoluteModified = (dateStr: string) => {
    try {
      const date = new Date(dateStr);
      if (Number.isNaN(date.getTime())) return dateStr;
      return formatDateIntl(date, undefined, {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return dateStr;
    }
  };

  return (
    <div
      className={`
        bg-panel border rounded-lg p-4
        cursor-pointer transition-colors
        ${isSelected
          ? 'border-accent ring-1 ring-accent'
          : 'border-border hover:border-accent'
        }
      `}
      // A bar down the leading edge: yellow for work that is only on this computer
      // (#270), bright neutral when you hold the SpeleoDB write lock, blue when
      // someone else does. Accent only - no badge here (the recents list is a
      // chooser, and un-uploaded work is not an error).
      //
      // Via recentCardStyle, not recentAccentStyle: `ring-1` above compiles to a
      // box-shadow, which an inline one would replace outright, dropping the
      // selection ring off every accented card.
      style={recentCardStyle(speleoLock, isSelected === true)}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
      onContextMenu={handleContextMenu}
      data-testid={testId}
      data-local-edits={speleoLock?.hasLocalEdits === true ? 'true' : 'false'}
      data-recent-accent={recentAccent(speleoLock)}
      role="button"
      tabIndex={0}
      aria-pressed={isSelected}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          handleClick(e as unknown as MouseEvent);
        }
      }}
    >
      <h3 className="text-lg font-medium mb-1">{name}</h3>
      {location && (
        <p className="text-sm text-text-secondary mb-1">{location}</p>
      )}
      {/* SpeleoDB projects show the mark + lock state instead of the local path
          (irrelevant for a linked project, #201); local ones show the path. */}
      {speleoLock?.isRemote ? (
        <div className="mb-3" data-testid={`${testId}-path`}>
          <SpeleoRecentInfo info={speleoLock} name={name} testIdPrefix={`${testId}-speleo`} />
        </div>
      ) : (
        <ElidedPath
          path={filePath}
          className="text-xs text-text-tertiary mb-3"
          data-testid={`${testId}-path`}
        />
      )}

      <div className="grid grid-cols-2 gap-2 text-sm">
        <div>
          <span className="text-text-secondary">{t('globalView.systemCard.sections')}:</span>{' '}
          <span>{formatNumber(sectionCount)}</span>
        </div>
        <div>
          <span className="text-text-secondary">{t('globalView.systemCard.length')}:</span>{' '}
          <span>{formatLength(totalLength)}</span>
        </div>
      </div>

      <div className="mt-3 pt-3 border-t border-border text-xs text-text-secondary">
        <Tooltip content={absoluteModified(modifiedAt)}>
          <span data-testid={`${testId}-modified`}>
            {t('globalView.systemCard.modified', { relative: relativeModified(modifiedAt) })}
          </span>
        </Tooltip>
      </div>
    </div>
  );
}
