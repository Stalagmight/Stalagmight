import { useEffect, useRef, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { renderFatalMessage } from './surveyLayer/mapErrors';

/**
 * How long a lost WebGL context gets to come back before the user is told
 * (#576 D38-LOST). A browser that restores does so within a frame or two of
 * the GPU coming back; one that has given up never fires the restore at all.
 */
export const CONTEXT_RESTORE_GRACE_MS = 5000;

export interface ContextRestoreWatch {
  graceMs: number;
  /** The context was lost and the map is not back within `graceMs` of visible time. */
  onUnrecovered: () => void;
  /** It came back after `onUnrecovered` ran (or the watch ended while it was reported). */
  onRecovered: () => void;
}

/**
 * Watch `map` for a WebGL context that is lost and NOT restored. The custom
 * layers come back by themselves when it is restored (glContextRecovery.ts), so
 * a loss alone is nothing to report; a restore that never arrives leaves the
 * survey blank for good, and that the user has to hear about. Returns the detach,
 * which also takes back a report still standing.
 *
 * The map is back only once the style MapLibre rebuilds on the restore has
 * loaded (`style.load`, one frame after `webglcontextrestored`), not at the
 * restore itself: a second loss in that frame leaves MapLibre nothing to rebuild
 * from (an unloaded style serializes to nothing), so the restore that follows
 * brings back no style at all, and that is as unrecovered as a restore that
 * never came (#576 RV18-MAP-2).
 *
 * Only time the document is visible counts towards `graceMs`. MapLibre loads
 * that style in a requestAnimationFrame (`Style.loadJSON` -> `browser.frameAsync`),
 * which a hidden or minimized window does not run while its timers only slow
 * down: a restore that arrived while hidden was reported although all it waited
 * for was the first visible frame (#576 RV19b-M2).
 */
export function watchContextRestore(map: MaplibreMap, opts: ContextRestoreWatch): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** The document a loss being timed follows the visibility of (null: none being timed). */
  let timedDoc: Document | null = null;
  /** Grace time the loss being timed has left, and when its running timer was armed. */
  let remaining = 0;
  let armedAt = 0;
  let reported = false;
  /** A restore came in; its rebuilt style's `style.load` settles the loss. */
  let awaitingLoad = false;
  const arm = (): void => {
    armedAt = Date.now();
    timer = setTimeout(() => {
      timer = null;
      stopTiming();
      reported = true;
      opts.onUnrecovered();
    }, remaining);
  };
  const onVisibility = (): void => {
    if (timedDoc?.visibilityState !== 'hidden') {
      if (timer === null) arm();
    } else if (timer !== null) {
      clearTimeout(timer);
      timer = null;
      remaining = Math.max(0, remaining - (Date.now() - armedAt));
    }
  };
  const stopTiming = (): void => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    timedDoc?.removeEventListener('visibilitychange', onVisibility);
    timedDoc = null;
  };
  const onLost = (): void => {
    if (timer !== null || timedDoc !== null || reported) return;
    remaining = opts.graceMs;
    // No document to follow (node, in the unit tests): time it as if visible.
    if (typeof document === 'undefined') {
      arm();
      return;
    }
    timedDoc = document;
    timedDoc.addEventListener('visibilitychange', onVisibility);
    if (timedDoc.visibilityState !== 'hidden') arm();
  };
  const settle = (): void => {
    stopTiming();
    if (!reported) return;
    reported = false;
    opts.onRecovered();
  };
  const onStyleLoad = (): void => {
    awaitingLoad = false;
    settle();
  };
  const onRestored = (): void => {
    if (awaitingLoad) return;
    awaitingLoad = true;
    map.once('style.load', onStyleLoad);
  };
  map.on('webglcontextlost', onLost);
  map.on('webglcontextrestored', onRestored);
  return () => {
    map.off('webglcontextlost', onLost);
    map.off('webglcontextrestored', onRestored);
    if (awaitingLoad) map.off('style.load', onStyleLoad);
    awaitingLoad = false;
    settle();
  };
}

/**
 * The lost-context notice for one map (survey map, Global View): after
 * {@link CONTEXT_RESTORE_GRACE_MS} (visible) without a restore whose style has loaded, an
 * error toast with a "Reload map" action that rebuilds the map on a new WebGL
 * context (`onReload`, which remounts the map). The toast stays until the map
 * comes back by itself, the map is rebuilt or left, or the user dismisses it.
 */
export function useContextLossToast(
  map: RefObject<MaplibreMap | null>,
  mapLoaded: boolean,
  onReload: () => void,
): void {
  const { t } = useTranslation();
  const onReloadRef = useRef(onReload);
  useEffect(() => {
    onReloadRef.current = onReload;
  }, [onReload]);

  useEffect(() => {
    const current = map.current;
    if (!current || !mapLoaded) return undefined;
    return watchContextRestore(current, {
      graceMs: CONTEXT_RESTORE_GRACE_MS,
      onUnrecovered: () =>
        useUIStore.getState().showToast(renderFatalMessage('webgl-context-lost'), TOAST_KEYS.RENDER_CONTEXT_LOST, 'error', {
          persistent: true,
          action: {
            label: t('map.contextLost.reload'),
            onAction: () => onReloadRef.current(),
            testId: 'map-context-lost-reload',
          },
        }),
      onRecovered: () => useUIStore.getState().dismissToastByKey(TOAST_KEYS.RENDER_CONTEXT_LOST),
    });
  }, [map, mapLoaded, t]);
}
