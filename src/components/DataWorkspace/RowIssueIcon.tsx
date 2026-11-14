/**
 * A row's survey issue at the right end of its gutter name, in the DataTable
 * style: the error / warning icon with the count of unresolved warnings, the
 * lock of a frozen shot (alone, or after the icon). On a shot with an open
 * warning a click resolves it - Survey opens on the shot with the warning's
 * resolver - unless the data is locked. Stations show the warnings that name
 * them (an over-anchored zero datum). A find match in the issue text
 * (ROW_ISSUES_KEY) outlines it.
 */
import { useTranslation } from 'react-i18next';
import type { CellMarks } from '../common/SpreadsheetGrid/types';
import { AlertCircleIcon, AlertTriangleIcon, LockIcon } from '../common/icons';
import type { DataEntityRow, DataModelCtx, ShotRow } from './types';
import { isShotFrozen, rowWarnings } from './model/columns';
import { dataRowPart } from './dataTestIds';
import { matchClass } from './StationLinkChips';

const stop = (e: { stopPropagation(): void }) => e.stopPropagation();

export function RowIssueIcon({
  row,
  ctx,
  marks,
  onResolve,
}: {
  row: DataEntityRow;
  ctx: DataModelCtx;
  /** Find matches in the row's issue text. */
  marks?: CellMarks | undefined;
  onResolve: (row: ShotRow) => void;
}) {
  const { t } = useTranslation();
  const frozen = row.kind === 'shot' && isShotFrozen(ctx, row);
  const open = rowWarnings(ctx, row).filter((w) => !w.isResolved);
  if (open.length === 0 && !frozen) return null;
  const hasError = open.some((w) => w.severity === 'error');
  const tooltip = [...(frozen ? [t('dataTable.frozen')] : []), ...open.map((w) => w.message)].join('\n');
  const colorClass = hasError ? 'text-error' : frozen && open.length === 0 ? 'text-text-secondary' : 'text-warning';
  const icon =
    frozen && open.length === 0 ? <LockIcon size={13} /> : hasError ? <AlertCircleIcon size={14} /> : <AlertTriangleIcon size={14} />;
  const part = dataRowPart(row);
  const cls = `${colorClass} mr-1 inline-flex h-5 shrink-0 items-center rounded px-0.5${matchClass(marks !== undefined, marks?.current !== undefined)}`;
  const content = (
    <>
      {icon}
      {open.length > 1 && <span className="ml-0.5 text-[11px] tabular-nums">{open.length}</span>}
      {frozen && open.length > 0 && <LockIcon size={11} />}
    </>
  );
  if (open.length > 0 && row.kind === 'shot' && !ctx.editingBlocked) {
    return (
      <button
        type="button"
        tabIndex={-1}
        className={`${cls} cursor-pointer hover:opacity-80 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset`}
        title={tooltip}
        aria-label={t('detailPanel.warnings.resolve')}
        onMouseDown={stop}
        onClick={(e) => {
          e.stopPropagation();
          onResolve(row);
        }}
        data-testid={`data-issues-${part}`}
      >
        {content}
      </button>
    );
  }
  return (
    <span className={cls} title={tooltip} data-testid={`data-issues-${part}`}>
      {content}
    </span>
  );
}
