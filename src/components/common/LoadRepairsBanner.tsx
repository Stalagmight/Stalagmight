import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import type { LoadRepairs } from '../../utils/loadProjectBinary';

/** Item keys under `banners.loadRepairs.items`, in the order the notice lists them. */
const KINDS: ReadonlyArray<keyof LoadRepairs> = ['shots', 'sectionOffsets', 'stationOffsets', 'gpsPoints'];

/**
 * [#576 B35(h)] One notice after an open that reset or dropped stored values it could
 * not use (non-finite shot measurements, section and station moves, GPS tie points),
 * with a count per kind.
 *
 * The backend already neutralized these at the load boundary - the right call, the
 * values would poison every coordinate - but it did so silently, and the next save
 * makes the change permanent: exactly the "silent state change" docs/notifications.md
 * forbids. Banner with a Dismiss rather than a toast, like the discarded-history
 * banner (#473) that reports the same class of load-time loss: the changed values
 * stay in the open project, and a toast that times out during the first map render
 * would leave no trace of it. No action button: there is nothing to undo, only
 * something to know (details are in the log).
 */
export function LoadRepairsBanner() {
  const { t } = useTranslation();
  const loadRepairs = useProjectStore((s) => s.loadRepairs);
  const dismissed = useProjectStore((s) => s.loadRepairsDismissed);
  const dismiss = useProjectStore((s) => s.dismissLoadRepairs);
  const hasProject = useProjectStore((s) => s.project !== null);
  const isLoading = useProjectStore((s) => s.loading.isLoading);

  if (!loadRepairs || dismissed || !hasProject || isLoading) return null;

  const items = KINDS.filter((kind) => loadRepairs[kind] > 0)
    .map((kind) => t(`banners.loadRepairs.items.${kind}`, { count: loadRepairs[kind] }))
    .join(', ');

  return (
    <div
      className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
      role="status"
      data-testid="load-repairs-banner"
    >
      <span className="text-warning font-medium flex-1 min-w-0">
        {t('banners.loadRepairs.lead', { items })}{' '}
        <span className="text-text-secondary font-normal">{t('banners.loadRepairs.detail')}</span>
      </span>
      <button
        onClick={dismiss}
        className="px-2 py-1 text-text-secondary hover:text-text text-xs"
        data-testid="load-repairs-dismiss-button"
      >
        {t('banners.loadRepairs.dismiss')}
      </button>
    </div>
  );
}
