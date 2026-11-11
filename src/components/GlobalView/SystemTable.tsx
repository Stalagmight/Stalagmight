import { RemoteUpdatedTime } from './RemoteUpdatedTime';
import { SpeleoProjectInfo } from './SpeleoProjectInfo';
import { type MouseEvent } from 'react';
import { useSpeleoStore } from '../../store/speleoStore';
import { useTranslation } from 'react-i18next';
import {
  formatDate as formatDateIntl,
  formatNumber,
  formatRelativeTime,
} from '../../i18n/format';
import { ElidedPath, Tooltip, ChevronUpIcon, ChevronDownIcon } from '../common';
import { useProjectStatFormatters } from './useProjectStatFormatters';
import type { RecentListItem } from './recentListItems';
import type { RecentSortField, RecentSortDirection } from './useRecentSort';
import {
  SpeleoRecentInfo,
  resolveSpeleoLock,
  recentAccent,
  recentAccentStyle,
  withCachedCopyEdits,
} from './SpeleoLockBadge';
import type { SpeleoProjectSummary } from '../../types/ipc.generated';
import type { SpeleoLocalStateMap } from '../../utils/speleoLocalState';
import { missingSystemRowTestId, systemRowTestId, testIdPart } from '../../utils/testId';

interface SystemTableProps {
  items: RecentListItem[];
  selectedProjectId: string | null;
  multiSelectedIds: Set<string>;
  sortField: RecentSortField;
  sortDirection: RecentSortDirection;
  /** SpeleoDB lock state per project id, from the login/app-open scan (#201). */
  remoteLocks: Record<string, string | null>;
  /** SpeleoDB project name per project id, from the same scan (#201). */
  remoteProjects: Record<string, string>;
  /** Current user's email, to tell our own held lock from another's (#201). */
  myEmail: string | null;
  /** #270: per-project LOCAL state (un-pushed edits), fetched once for the whole
      list. Empty when the fetch failed - which simply means no row is accented. */
  localStates?: SpeleoLocalStateMap;
  /** Toggle/apply sorting when a sortable column header is clicked. */
  onSort: (field: RecentSortField) => void;
  onSelect: (filePath: string, ctrlKey: boolean) => void;
  onOpen: (filePath: string) => void;
  onContextMenu: (e: MouseEvent, filePath: string) => void;
  onRemoveMissing: (filePath: string) => void;
  /** Pull + open a purely-remote SpeleoDB project row (#218). */
  onOpenRemote: (summary: SpeleoProjectSummary) => void;
  /** Right-click on a SpeleoDB row: the SpeleoDB project context menu. */
  onContextMenuRemote?: (e: MouseEvent, item: Extract<RecentListItem, { kind: 'remote' }>) => void;
  /** True while a remote open/adopt is in flight - gates the remote rows so a second
      click can't issue a concurrent open (mirrors RemoteSystemCard's `busy`). */
  busy?: boolean;
}

interface ColumnDef {
  key: string;
  /** Sort field this column maps to, or undefined if it isn't sortable. */
  field?: RecentSortField;
  align: 'left' | 'right';
}

const COLUMNS: ColumnDef[] = [
  { key: 'name', field: 'name', align: 'left' },
  { key: 'path', align: 'left' },
  { key: 'sections', field: 'sections', align: 'right' },
  { key: 'length', field: 'length', align: 'right' },
  { key: 'edited', field: 'last_edited', align: 'left' },
];

function relative(dateStr: string): string {
  try {
    const date = new Date(dateStr);
    if (Number.isNaN(date.getTime())) return dateStr;
    return formatRelativeTime(date);
  } catch {
    return dateStr;
  }
}

function absolute(dateStr: string): string {
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
}

/**
 * Compact, single-row-per-project view of the recent-projects list. Mirrors the
 * card grid's interactions (click to select, Ctrl/Cmd-click to multi-select,
 * double-click to open, right-click for the context menu) and renders missing
 * projects as de-emphasized rows with a remove action.
 */
export function SystemTable({
  items,
  selectedProjectId,
  multiSelectedIds,
  sortField,
  sortDirection,
  remoteLocks,
  remoteProjects,
  myEmail,
  localStates = {},
  onSort,
  onSelect,
  onOpen,
  onContextMenu,
  onRemoveMissing,
  onOpenRemote,
  onContextMenuRemote,
  busy = false,
}: SystemTableProps) {
  const { t } = useTranslation();
  const remoteSummaries = useSpeleoStore((s) => s.remoteSummaries);
  const { formatLength } = useProjectStatFormatters();

  const headerClass = 'text-xs font-medium text-text-secondary px-3 py-2 sticky top-0 bg-background z-10';
  const cellClass = 'px-3 py-2 text-sm';

  const renderHeader = (col: ColumnDef) => {
    const label = t(`globalView.recent.columns.${col.key}`);
    const alignClass = col.align === 'right' ? 'text-right' : 'text-left';
    const isActive = col.field !== undefined && sortField === col.field;
    const ariaSort = isActive ? (sortDirection === 'asc' ? 'ascending' : 'descending') : undefined;

    if (!col.field) {
      return (
        <th key={col.key} className={`${headerClass} ${alignClass}`}>{label}</th>
      );
    }

    const field = col.field;
    return (
      <th key={col.key} className={`${headerClass} ${alignClass}`} aria-sort={ariaSort}>
        <button
          type="button"
          onClick={() => onSort(field)}
          className={`inline-flex items-center gap-1 hover:text-text ${
            isActive ? 'text-text' : ''
          }`}
          aria-label={t('globalView.recent.sortByColumn', { column: label })}
          data-testid={`recent-col-${col.key}`}
        >
          <span>{label}</span>
          {isActive &&
            (sortDirection === 'asc' ? (
              <ChevronUpIcon size={14} />
            ) : (
              <ChevronDownIcon size={14} />
            ))}
        </button>
      </th>
    );
  };

  return (
    <table className="w-full border-collapse" data-testid="globalview-list-table">
      <thead>
        <tr className="border-b border-border">
          {COLUMNS.map(renderHeader)}
        </tr>
      </thead>
      <tbody>
        {items.map((item) => {
          if (item.kind === 'missing') {
            return (
              <tr
                key={item.filePath}
                className="border-b border-border opacity-70"
                data-testid={missingSystemRowTestId(item.filePath)}
                data-missing="true"
              >
                <td className={`${cellClass} text-text-secondary line-through decoration-1`}>{item.name}</td>
                <td className={cellClass}>
                  <ElidedPath path={item.filePath} className="text-xs text-text-tertiary" />
                </td>
                <td className={`${cellClass} text-warning text-right`} colSpan={2}>
                  {t('globalView.recent.missing.notFound')}
                </td>
                <td className={cellClass}>
                  <button
                    type="button"
                    onClick={() => onRemoveMissing(item.filePath)}
                    className="text-xs text-text-secondary hover:text-danger underline"
                    data-testid={`system-row-missing-remove-${testIdPart(item.filePath)}`}
                  >
                    {t('globalView.recent.missing.remove')}
                  </button>
                </td>
              </tr>
            );
          }

          if (item.kind === 'remote') {
            // #218: a purely-remote SpeleoDB project (not in local recents). No local
            // file/stats - show the SpeleoDB name + lock badge, and open (pull) it on
            // row click. Reuses resolveSpeleoLock via a synthetic project id.
            const remoteLock = withCachedCopyEdits(resolveSpeleoLock(
              { is_remote: true, speleodb_project_id: item.projectId },
              remoteLocks,
              myEmail,
              remoteProjects,
              localStates,
              { ...remoteSummaries, [item.projectId]: item.summary },
            ), item.cached);
            return (
              <tr
                key={`remote-${item.projectId}`}
                className={`border-b border-border transition-colors ${
                  busy ? 'opacity-60 cursor-default' : 'cursor-pointer hover:bg-panel'
                }`}
                data-testid={`system-row-remote-${item.projectId}`}
                data-remote="true"
                data-recent-accent={recentAccent(remoteLock)}
                role="button"
                aria-disabled={busy || undefined}
                tabIndex={0}
                onClick={() => {
                  if (!busy) onOpenRemote(item.summary);
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  if (!busy) onContextMenuRemote?.(e, item);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    e.stopPropagation();
                    if (!busy) onOpenRemote(item.summary);
                  }
                }}
              >
                {/* The leading-edge accent from the card grid (#270 yellow, or the
                    lock neutrals). It sits on the first CELL, not the row: under
                    border-collapse a <tr> generates no box, so neither its border nor
                    its box-shadow paints. */}
                <td
                  className={`${cellClass} font-medium`}
                  style={recentAccentStyle(remoteLock)}
                >
                  {item.summary.name}
                </td>
                <td className={cellClass}>
                  <SpeleoProjectInfo
                    lock={remoteLock}
                    summary={item.summary}
                    cached={item.cached}
                    testIdPrefix={`system-row-remote-${item.projectId}-speleo`}
                  />
                </td>
                <td className={`${cellClass} text-text-tertiary text-right`} colSpan={2}>
                  {t('globalView.recent.remote.onSpeleoDb')}
                </td>
                {/* The row itself is the Open action (click, Enter/Space), like the
                    card grid's remote cards and every local row; no separate button. */}
                <td className={cellClass}>
                  <RemoteUpdatedTime date={item.summary.modified_date} />
                </td>
              </tr>
            );
          }

          const { project } = item;
          const lock = resolveSpeleoLock(project, remoteLocks, myEmail, remoteProjects, localStates, remoteSummaries);
          const isSelected =
            multiSelectedIds.has(item.filePath) || selectedProjectId === item.filePath;

          return (
            <tr
              key={item.filePath}
              className={`border-b border-border cursor-pointer transition-colors ${
                isSelected ? 'bg-accent-10' : 'hover:bg-panel'
              }`}
              data-testid={systemRowTestId(item.filePath)}
              data-local-edits={lock.hasLocalEdits ? 'true' : 'false'}
              data-recent-accent={recentAccent(lock)}
              role="button"
              tabIndex={0}
              aria-pressed={isSelected}
              onClick={(e) => onSelect(item.filePath, e.ctrlKey || e.metaKey)}
              onDoubleClick={() => onOpen(item.filePath)}
              onContextMenu={(e) => onContextMenu(e, item.filePath)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  onSelect(item.filePath, e.ctrlKey || e.metaKey);
                }
              }}
            >
              <td className={`${cellClass} font-medium`} style={recentAccentStyle(lock)}>
                <span>{project.name}</span>
                {project.location && (
                  <span className="text-text-secondary"> · {project.location}</span>
                )}
              </td>
              <td className={cellClass}>
                {/* SpeleoDB projects show the mark + lock state instead of the local
                    path (irrelevant for a linked project, #201); local ones show it. */}
                {lock.isRemote ? (
                  <SpeleoRecentInfo
                    info={lock}
                    name={project.name}
                    testIdPrefix={`system-row-${testIdPart(item.filePath)}-speleo`}
                  />
                ) : (
                  <ElidedPath
                    path={item.filePath}
                    className="text-xs text-text-tertiary"
                    data-testid={`system-row-${testIdPart(item.filePath)}-path`}
                  />
                )}
              </td>
              <td className={`${cellClass} text-right tabular-nums`}>{formatNumber(project.section_count)}</td>
              <td className={`${cellClass} text-right tabular-nums`}>{formatLength(project.total_length)}</td>
              <td className={`${cellClass} text-text-secondary`}>
                <Tooltip content={absolute(project.modified_at)}>
                  <span data-testid={`system-row-modified-${testIdPart(item.filePath)}`}>{relative(project.modified_at)}</span>
                </Tooltip>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

export default SystemTable;
