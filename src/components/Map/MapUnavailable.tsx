import { useTranslation } from 'react-i18next';

/** Stands in for a map that could not start because WebGL2 is unavailable
 *  (see `createMap.ts`). An ongoing condition, so a status in place of the map
 *  rather than a toast (docs/notifications.md). */
export function MapUnavailable({ testId }: { testId: string }) {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      data-testid={testId}
      className="absolute inset-0 z-10 flex items-center justify-center bg-background p-4 text-center"
    >
      <div className="max-w-sm">
        <p className="text-sm font-medium text-text-primary">{t('map.webgl2Unavailable.title')}</p>
        <p className="mt-1 text-xs text-text-secondary">{t('map.webgl2Unavailable.body')}</p>
      </div>
    </div>
  );
}
