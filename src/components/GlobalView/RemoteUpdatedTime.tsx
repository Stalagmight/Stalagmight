import { formatDate, formatRelativeTime } from '../../i18n/format';

/** Display the same server timestamp used by the recent-project sort. */
export function RemoteUpdatedTime({ date }: { date: string | undefined }) {
  if (!date || !Number.isFinite(Date.parse(date))) return null;
  const parsed = new Date(date);
  return <time dateTime={date} title={formatDate(parsed, undefined, {
    year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
  })} className="text-xs text-text-secondary">{formatRelativeTime(parsed)}</time>;
}
