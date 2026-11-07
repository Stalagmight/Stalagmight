import { useState, useEffect, useMemo, useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { SpreadsheetGrid, type GridColumn } from '../common/SpreadsheetGrid';
import { ChevronLeftIcon, ChevronRightIcon } from '../common/icons';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { formatNumber } from '../../i18n/format';
import type { RawTableData } from '../../types/ipc/RawTableData';

export const RAW_TABLE_PAGE_SIZE = 1000;
/** Height of the pager footer (px) - subtracted from the grid's share. */
const PAGER_HEIGHT = 28;

const noop = () => {};

interface RawTableViewProps {
  /** Raw sqlite table to browse (must come from get_raw_table_names). */
  tableName: string;
  /** Body height in px (the grid + pager must fit it exactly). */
  height: number;
}

/**
 * Read-only pager over a raw sqlite table of the project database. Values
 * arrive stringified server-side (NULL → "NULL", blobs → "<blob N KB>"), so
 * this always shows the LAST-SAVED state - the host shows an "as of last
 * save" hint while the project is dirty, and a save refetches the page.
 */
export default function RawTableView({ tableName, height }: RawTableViewProps) {
  const { t } = useTranslation();
  const isDirty = useProjectStore((s) => s.isDirty);
  const [data, setData] = useState<RawTableData | null>(null);
  const [page, setPage] = useState(0);
  const [error, setError] = useState<string | null>(null);
  // Monotonic request id: a stale response (slow page fetch overtaken by a
  // newer one) must never clobber the latest page.
  const requestIdRef = useRef(0);

  // Switching tables: reset to page 0 AND drop the previous table's rows so the
  // loading state shows instead of the old columns/rows until the new fetch
  // resolves (the grid remounts on `key={tableName}` but would still get stale
  // `data.rows`).
  useEffect(() => {
    setPage(0);
    setData(null);
    setError(null);
  }, [tableName]);

  const fetchPage = useCallback((table: string, pageToLoad: number) => {
    const requestId = ++requestIdRef.current;
    invokeIpc(
      'get_raw_table',
      { table, limit: RAW_TABLE_PAGE_SIZE, offset: pageToLoad * RAW_TABLE_PAGE_SIZE },
      { purpose: 'browse a raw sqlite table', fields: { table, page: pageToLoad } }
    )
      .then((result) => {
        if (requestId !== requestIdRef.current) return;
        setData(result);
        setError(null);
      })
      .catch((e) => {
        if (requestId !== requestIdRef.current) return;
        setData(null);
        setError(getErrorMessage(e));
      });
  }, []);

  useEffect(() => {
    fetchPage(tableName, page);
  }, [fetchPage, tableName, page]);

  // Refetch when the project just saved (dirty → clean): the view stays
  // literally "as of last save".
  const prevDirtyRef = useRef(isDirty);
  useEffect(() => {
    const wasDirty = prevDirtyRef.current;
    prevDirtyRef.current = isDirty;
    if (wasDirty && !isDirty) fetchPage(tableName, page);
  }, [isDirty, fetchPage, tableName, page]);

  // One read-only text column per sqlite column; rows are pre-stringified.
  const columns = useMemo(
    (): GridColumn<string[]>[] =>
      (data?.columns ?? []).map((name, i) => ({
        key: name,
        header: name,
        getValue: (row) => row[i] ?? '',
      })),
    [data]
  );

  const totalRows = data?.total_rows ?? 0;
  const showPager = totalRows > RAW_TABLE_PAGE_SIZE;
  const lastPage = Math.max(0, Math.ceil(totalRows / RAW_TABLE_PAGE_SIZE) - 1);

  if (error) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-text-secondary" data-testid="raw-table-error">
        {error}
      </div>
    );
  }
  if (!data) {
    return (
      <div className="flex h-full items-center justify-center text-text-secondary" data-testid="raw-table-loading">
        {t('dataTable.rawData.loading')}
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col" data-testid="raw-table-view">
      <SpreadsheetGrid<string[]>
        key={tableName}
        columns={columns}
        rows={data.rows}
        rowKey={(_row, index) => index}
        height={showPager ? height - PAGER_HEIGHT : height}
        onEdit={noop}
        rowLabel={(_row, index) => page * RAW_TABLE_PAGE_SIZE + index + 1}
      />
      {showPager && (
        <div
          className="flex items-center justify-end gap-1 border-t border-border px-3 text-xs text-text-secondary"
          style={{ height: PAGER_HEIGHT }}
          data-testid="raw-table-pager"
        >
          <span className="tabular-nums">
            {t('dataTable.rawData.pager', {
              from: formatNumber(page * RAW_TABLE_PAGE_SIZE + 1),
              to: formatNumber(page * RAW_TABLE_PAGE_SIZE + data.rows.length),
              total: formatNumber(totalRows),
            })}
          </span>
          <button
            className="px-1 hover:text-text disabled:opacity-40"
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={page === 0}
            aria-label={t('common.actions.back')}
            data-testid="raw-table-pager-prev"
          >
            <ChevronLeftIcon size={12} />
          </button>
          <button
            className="px-1 hover:text-text disabled:opacity-40"
            onClick={() => setPage((p) => Math.min(lastPage, p + 1))}
            disabled={page >= lastPage}
            aria-label={t('common.actions.next')}
            data-testid="raw-table-pager-next"
          >
            <ChevronRightIcon size={12} />
          </button>
        </div>
      )}
    </div>
  );
}
