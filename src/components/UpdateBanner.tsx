import { useTranslation } from 'react-i18next';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useUpdateStore } from '../store/updateStore';
import { useUIStore } from '../store/uiStore';
import { TOAST_KEYS } from '../utils/toastKeys';
import { requestUpdateRestart } from '../utils/quit';
import { downloadAppUpdate } from '../hooks/useAppUpdate';

/**
 * In-app update banner (#579). Rendered alongside the app's other
 * ongoing-condition banners (integrity, history-discarded, autosave), and
 * only ever offers one of three states at a time:
 *  - `available`: version + Download / Release notes / Dismiss.
 *  - `downloading`: progress bar (indeterminate while total is unknown).
 *  - `ready`: Restart, threaded through the exit guards via
 *    `requestUpdateRestart` (docs: see quit.ts / useBeforeUnload.ts). Where
 *    the update installs on quit (#631) the banner says so and the button is
 *    "Restart now"; quitting is enough.
 *
 * Hidden for `idle`/`checking`/`error`, for a download a check started on its
 * own (#631: the banner appears once it is ready, not as a progress bar for an
 * update nobody asked about), and for the version the user already dismissed
 * this session (`dismissedVersion`).
 */
export function UpdateBanner() {
  const { t } = useTranslation();
  const status = useUpdateStore((s) => s.status);
  const info = useUpdateStore((s) => s.info);
  const progress = useUpdateStore((s) => s.progress);
  const backgroundDownload = useUpdateStore((s) => s.backgroundDownload);
  const dismissedVersion = useUpdateStore((s) => s.dismissedVersion);
  const dismiss = useUpdateStore((s) => s.dismiss);
  const setDownloadFailed = useUpdateStore((s) => s.setDownloadFailed);

  if (!info || (status !== 'available' && status !== 'downloading' && status !== 'ready')) {
    return null;
  }
  if (status === 'downloading' && backgroundDownload) {
    return null;
  }
  if (dismissedVersion === info.version) {
    return null;
  }

  // Captured at click time and threaded through both store transitions, so a
  // check that lands mid-download cannot leave this download's completion
  // marking a DIFFERENT version ready (updateStore: `downloadVersion`).
  const downloadingVersion = info.version;

  // The user asked for this one, so unlike a background download its failure
  // is reported.
  const handleDownload = () => {
    void downloadAppUpdate(downloadingVersion, false).then((outcome) => {
      if (outcome !== 'failed') return;
      useUIStore
        .getState()
        .showToast(
          t('updateBanner.toasts.downloadFailed'),
          TOAST_KEYS.APP_UPDATE_DOWNLOAD_FAILED,
          'error',
        );
    });
  };

  // `openUrl`, not `window.open`: the Tauri webview has no browser chrome to
  // open a tab in, and the CSP would not admit the navigation anyway - the same
  // reason SavePermissionDialog and useSpeleoWebLink go through the opener
  // plugin. This hands the URL to the OS default browser.
  const handleOpenNotes = () => {
    void openUrl(info.notesUrl).catch((e: unknown) => {
      console.error('[UpdateBanner] could not open the release notes:', e);
    });
  };

  // The restart arms the backend first and can be refused (nothing downloaded,
  // a newer check superseded it). A refusal must not leave the user staring at
  // a button that silently did nothing.
  const handleRestart = () => {
    void requestUpdateRestart().catch((e: unknown) => {
      console.error('[UpdateBanner] could not start the update restart:', e);
      setDownloadFailed();
      useUIStore
        .getState()
        .showToast(
          t('updateBanner.toasts.installFailed'),
          TOAST_KEYS.APP_UPDATE_INSTALL_FAILED,
          'error',
        );
    });
  };

  const total = progress?.total ?? null;
  const downloaded = progress?.downloaded ?? 0;
  const percent = total && total > 0 ? Math.min(100, (downloaded / total) * 100) : null;

  return (
    <div
      className="bg-accent-10 border-b border-accent-20 px-4 py-2 flex items-center gap-3 text-sm"
      role="status"
      data-testid="update-banner"
    >
      <span className="text-text-primary font-medium flex-1 min-w-0">
        {status === 'ready' && info.installsOnQuit
          ? t('updateBanner.readyOnQuit', { version: info.version })
          : t('updateBanner.available', { version: info.version })}
      </span>

      {(status === 'available' || status === 'ready') && (
        <button
          onClick={handleOpenNotes}
          className="px-2 py-1 text-text-secondary hover:text-text text-xs"
          data-testid="update-banner-notes"
        >
          {t('updateBanner.releaseNotes')}
        </button>
      )}

      {status === 'available' && (
        <button
          onClick={handleDownload}
          className="px-3 py-1 rounded bg-accent text-white text-xs"
          data-testid="update-banner-download"
        >
          {t('updateBanner.download')}
        </button>
      )}

      {status === 'downloading' && (
        <div className="flex items-center gap-2 flex-1 max-w-xs">
          <span className="text-text-secondary text-xs whitespace-nowrap">
            {t('updateBanner.downloading')}
          </span>
          <div className="h-1.5 flex-1 bg-border rounded-full overflow-hidden" data-testid="update-banner-progress">
            {percent === null ? (
              <div
                className="h-full bg-accent-70 animate-pulse-shimmer"
                role="progressbar"
                aria-label={t('updateBanner.downloading')}
              />
            ) : (
              <div
                className="h-full bg-accent transition-all duration-150"
                style={{ width: `${percent}%` }}
                role="progressbar"
                aria-label={t('updateBanner.downloading')}
                aria-valuenow={Math.round(percent)}
                aria-valuemin={0}
                aria-valuemax={100}
              />
            )}
          </div>
        </div>
      )}

      {status === 'ready' && (
        <button
          onClick={handleRestart}
          className="px-3 py-1 rounded bg-accent text-white text-xs"
          data-testid="update-banner-restart"
        >
          {info.installsOnQuit ? t('updateBanner.restartNow') : t('updateBanner.restart')}
        </button>
      )}

      {(status === 'available' || status === 'ready') && (
        <button
          onClick={() => dismiss(info.version)}
          className="px-2 py-1 text-text-secondary hover:text-text text-xs"
          data-testid="update-banner-dismiss"
        >
          {t('updateBanner.dismiss')}
        </button>
      )}
    </div>
  );
}
