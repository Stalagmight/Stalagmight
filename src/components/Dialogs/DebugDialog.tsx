import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';

/** Parse a log line into its level, target, message, and optional fields block. */
function parseLine(line: string): { level: string; target: string; message: string; fields: string | null } {
  // Format: "LEVEL target: message {key=val, ...}"
  const spaceIdx = line.indexOf(' ');
  if (spaceIdx === -1) return { level: '', target: '', message: line, fields: null };

  const level = line.slice(0, spaceIdx);
  const rest = line.slice(spaceIdx + 1);

  const colonIdx = rest.indexOf(': ');
  if (colonIdx === -1) return { level, target: '', message: rest, fields: null };

  const target = rest.slice(0, colonIdx);
  let message = rest.slice(colonIdx + 2);
  let fields: string | null = null;

  // Extract trailing {key=val, ...} block
  const braceIdx = message.lastIndexOf(' {');
  if (braceIdx !== -1 && message.endsWith('}')) {
    fields = message.slice(braceIdx + 2, -1);
    message = message.slice(0, braceIdx);
  }

  return { level, target, message, fields };
}

const LEVEL_STYLES: Record<string, { badge: string; text: string }> = {
  ERROR: {
    badge: 'bg-red-600 text-white',
    text: 'text-red-400',
  },
  WARN: {
    badge: 'bg-yellow-600 text-white',
    text: 'text-yellow-300',
  },
  INFO: {
    badge: 'bg-blue-600 text-white',
    text: 'text-blue-300',
  },
  DEBUG: {
    badge: 'bg-neutral-600 text-neutral-200',
    text: 'text-neutral-400',
  },
  TRACE: {
    badge: 'bg-neutral-700 text-neutral-300',
    text: 'text-neutral-500',
  },
};

const DEFAULT_STYLE = { badge: 'bg-neutral-600 text-neutral-200', text: 'text-neutral-400' };

type LevelFilter = 'all' | 'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE';

const LEVEL_RANK: Record<string, number> = { ERROR: 0, WARN: 1, INFO: 2, DEBUG: 3, TRACE: 4 };

function LogLine({ line }: { line: string }) {
  const { t } = useTranslation();
  const { level, target, message, fields } = parseLine(line);
  const style = LEVEL_STYLES[level] ?? DEFAULT_STYLE;
  const isFrontend = target === '[frontend]';

  return (
    <div className="flex items-start gap-2 py-0.5 leading-snug hover:bg-hover px-1 rounded">
      <span
        className={`inline-block w-12 shrink-0 text-center rounded text-[10px] font-bold uppercase px-1 py-px ${style.badge}`}
      >
        {level}
      </span>
      {isFrontend ? (
        <span className="shrink-0 text-[10px] font-semibold rounded px-1 py-px bg-accent text-accent-on">
          {t('debugDialog.frontendBadge')}
        </span>
      ) : (
        <span className="text-text-secondary shrink-0 text-[11px] opacity-70">{target}</span>
      )}
      <span className={`${style.text} flex-1`}>
        {message}
        {fields && (
          <span className="text-text-secondary opacity-60 ml-1">{`{${fields}}`}</span>
        )}
      </span>
    </div>
  );
}

export default function DebugDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const showToast = useUIStore((s) => s.showToast);

  const handleClose = useCallback(() => {
    trackDialogComplete('debug');
    closeDialog();
  }, [closeDialog]);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<LevelFilter>('all');
  const scrollRef = useRef<HTMLDivElement>(null);

  const fetchLogs = useCallback(async (): Promise<boolean> => {
    try {
      const result = await invokeIpc('get_recent_logs', undefined, { purpose: 'fetch recent debug logs' });
      setLogs(result);
      setError(null);
      return true;
    } catch (e) {
      setError(getErrorMessage(e));
      return false;
    }
  }, []);

  // Explicit Refresh button: re-fetch, then confirm with a transient toast (#49).
  // The on-mount fetch in the effect below stays silent - only a user-initiated
  // refresh surfaces feedback. On failure the inline error already reports it, so
  // the success toast is skipped.
  const handleRefresh = useCallback(async () => {
    if (await fetchLogs()) showToast(t('debugDialog.toasts.refreshed'), TOAST_KEYS.DEBUG_REFRESH, 'success');
  }, [fetchLogs, showToast]);

  useEffect(() => {
    fetchLogs();
  }, [fetchLogs]);

  const filteredLogs = useMemo(() => {
    if (filter === 'all') return logs;
    const minRank = LEVEL_RANK[filter] ?? 2;
    return logs.filter((line) => {
      const spaceIdx = line.indexOf(' ');
      const level = spaceIdx > 0 ? line.slice(0, spaceIdx) : '';
      return (LEVEL_RANK[level] ?? 99) <= minRank;
    });
  }, [logs, filter]);

  // Auto-scroll to bottom when logs change
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [filteredLogs]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(filteredLogs.join('\n'));
      showToast(t('debugDialog.toasts.copied'), TOAST_KEYS.DEBUG_COPY, 'success');
    } catch {
      // Surface the failure rather than swallowing it (e.g. clipboard blocked /
      // unavailable on WebKitGTK). Distinct key so it isn't throttled by a prior
      // successful copy.
      showToast(t('debugDialog.toasts.copyFailed'), TOAST_KEYS.DEBUG_COPY_FAILED, 'error');
    }
  }, [filteredLogs, showToast]);

  // Count by level
  const counts = useMemo(() => {
    const c: Record<string, number> = { ERROR: 0, WARN: 0, INFO: 0, DEBUG: 0, TRACE: 0 };
    for (const line of logs) {
      const spaceIdx = line.indexOf(' ');
      const level = spaceIdx > 0 ? line.slice(0, spaceIdx) : '';
      if (level in c) c[level] = (c[level] ?? 0) + 1;
    }
    return c;
  }, [logs]);

  return (
    <DialogWrapper
      title={t('debugDialog.title')}
      onClose={handleClose}
      size="5xl"
      overflow="hidden"
      data-testid="debug-dialog"
    >
      <div className="flex flex-col h-[70vh]">
        {/* Toolbar */}
        <div className="flex items-center gap-2 mb-2 flex-wrap">
          <Button size="sm" onClick={handleRefresh} data-testid="debug-refresh-button">
            {t('debugDialog.refresh')}
          </Button>
          <Button size="sm" onClick={handleCopy} data-testid="debug-copy-button">
            {t('common.actions.copy')}
          </Button>

          <div className="w-px h-5 bg-border mx-1" />

          {/* Level filter pills */}
          {(['all', 'ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'] as const).map((lvl) => {
            const active = filter === lvl;
            const count = lvl === 'all' ? logs.length : counts[lvl];
            return (
              <button
                key={lvl}
                onClick={() => setFilter(lvl)}
                className={`text-[11px] px-2 py-0.5 rounded font-medium transition-colors ${
                  active
                    ? 'bg-accent text-white'
                    : 'bg-transparent text-text-secondary hover:bg-hover'
                }`}
                data-testid={`debug-filter-${lvl.toLowerCase()}`}
              >
                {lvl === 'all' ? t('debugDialog.filterAll') : lvl}
                {count != null && <span className="ml-1 opacity-60">({count})</span>}
              </button>
            );
          })}

          <span className="text-xs text-text-secondary ml-auto">
            {t('debugDialog.entries', { shown: filteredLogs.length, total: logs.length })}
          </span>
        </div>

        {error && (
          <div className="text-red-500 text-sm mb-2">{error}</div>
        )}

        {/* Log output */}
        <div
          ref={scrollRef}
          className="flex-1 overflow-auto bg-background rounded p-2 text-xs font-mono border border-border"
          data-testid="debug-log-output"
        >
          {filteredLogs.length > 0 ? (
            filteredLogs.map((line, i) => <LogLine key={i} line={line} />)
          ) : (
            <div className="text-text-secondary p-4 text-center">
              {logs.length === 0 ? t('debugDialog.empty') : t('debugDialog.noMatches')}
            </div>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
