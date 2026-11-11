import type { MouseEvent } from 'react';
import { RemoteUpdatedTime } from './RemoteUpdatedTime';
import type { SpeleoCachedProject, SpeleoProjectSummary } from '../../types/ipc.generated';
import { SpeleoProjectInfo } from './SpeleoProjectInfo';
import {
  recentAccent,
  recentAccentStyle,
  withCachedCopyEdits,
  type SpeleoLockInfo,
} from './SpeleoLockBadge';

interface RemoteSystemCardProps {
  summary: SpeleoProjectSummary;
  /** Lock state for the badge, resolved from the scan for this project id (#218). */
  lock: SpeleoLockInfo;
  /** This computer's cached copy, when the session may see one. */
  cached?: SpeleoCachedProject | undefined;
  /** Pull + open this remote project (shared `useOpenRemoteProject` flow). */
  onOpen: (summary: SpeleoProjectSummary) => void;
  /** Right-click: the SpeleoDB project context menu. */
  onContextMenu?: (e: MouseEvent) => void;
  /** Disable actions while an open/adopt is in flight. */
  busy?: boolean;
}

/**
 * Card for a SpeleoDB project, whether or not it has been downloaded before. It
 * carries no local file/stats, so it shows the SpeleoDB name + lock badge. The card
 * itself is the Open action (click, or Enter/Space when focused), which pulls the
 * project like the browser dialog does; there is no separate Open button.
 */
export default function RemoteSystemCard({
  summary,
  lock,
  cached,
  onOpen,
  onContextMenu,
  busy = false,
}: RemoteSystemCardProps) {
  // The same leading-edge accent as the table row, un-uploaded work included.
  const displayLock = withCachedCopyEdits(lock, cached);
  const testId = `system-card-remote-${summary.id}`;
  // Gate every open on `busy`: a click (or Enter/Space) while a pull is in flight
  // must not fire a concurrent open.
  const open = () => {
    if (!busy) onOpen(summary);
  };

  return (
    <div
      className={`bg-panel border border-border rounded-lg p-4 transition-colors ${
        busy ? 'opacity-60 cursor-default' : 'cursor-pointer hover:border-accent'
      }`}
      style={recentAccentStyle(displayLock)}
      data-testid={testId}
      data-remote="true"
      data-recent-accent={recentAccent(displayLock)}
      role="button"
      aria-disabled={busy || undefined}
      tabIndex={0}
      onClick={open}
      onContextMenu={(e) => {
        e.preventDefault();
        if (!busy) onContextMenu?.(e);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          e.stopPropagation();
          open();
        }
      }}
    >
      <h3 className="text-lg font-medium mb-1">{summary.name}</h3>
      <div className="mb-3" data-testid={`${testId}-path`}>
        <SpeleoProjectInfo lock={displayLock} summary={summary} cached={cached} testIdPrefix={`${testId}-speleo`} />
      </div>

      {/* The footer held the Open button too; with only the time left, draw it
          only when there is a time to show rather than leave an empty strip. */}
      {summary.modified_date && Number.isFinite(Date.parse(summary.modified_date)) && (
        <div className="mt-3 pt-3 border-t border-border flex items-center" data-testid={`${testId}-footer`}>
          <RemoteUpdatedTime date={summary.modified_date} />
        </div>
      )}
    </div>
  );
}
