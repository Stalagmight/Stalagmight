import { useEffect, useCallback } from 'react';
import { useLatest } from './useLatest';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { IPC_EVENTS, listenIpc, type IpcEventName } from '../utils/ipcEvents';
import { invokeIpc } from '../utils/ipc';
import { useNetworkStore } from '../store/networkStore';

/**
 * Event-driven network status orchestrator.
 *
 * Uses tile fetch outcomes as the primary connectivity signal:
 * - Tauri `tile-transport-error` event → offline
 * - Tauri `tile-transport-success` event → online
 *
 * Only polls the ESRI server when offline (to detect recovery).
 * On offline→online transition, triggers tile reload for stale tiles.
 *
 * Mount once at app root. Consumers read from `useNetworkStore`.
 */
export function useNetworkStatus() {
  const isOnline = useNetworkStore((s) => s.isOnline);
  const setOnline = useNetworkStore((s) => s.setOnline);
  const triggerTileReload = useNetworkStore((s) => s.triggerTileReload);

  // Ref to avoid stale closures in event handlers
  const isOnlineRef = useLatest(isOnline);

  const goOnline = useCallback(() => {
    if (!isOnlineRef.current) {
      setOnline(true);
      triggerTileReload();
      console.info('[useNetworkStatus] Online - tiles reloading');
    }
  }, [setOnline, triggerTileReload, isOnlineRef]);

  const goOffline = useCallback(() => {
    if (isOnlineRef.current) {
      setOnline(false);
      console.info('[useNetworkStatus] Offline');
    }
  }, [setOnline, isOnlineRef]);

  // Listen for Tauri tile transport events + browser online/offline
  useEffect(() => {
    // Registered the way useBeforeUnload / useLaunchWithFile do it: await listen()
    // inside try/catch, so a rejected registration is handled where it fails instead
    // of escaping as an unhandled rejection, and release a listener that resolves
    // after cleanup as soon as it arrives.
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    // The real unlisten is async (it awaits an IPC round trip) and nothing awaits
    // it, so a failed release is reported here like a failed registration.
    const release = (event: string, unlisten: UnlistenFn) => {
      void new Promise<void>((resolve) => resolve(unlisten())).catch((e: unknown) => {
        console.warn(`[useNetworkStatus] ${event} listener release failed:`, e);
      });
    };
    const register = async (event: IpcEventName, handler: () => void) => {
      try {
        const unlisten = await listenIpc(event, handler);
        if (disposed) release(event, unlisten);
        else unlisteners.push(() => release(event, unlisten));
      } catch (e) {
        // Tauri event API not available - the browser online/offline events below still apply
        console.warn(`[useNetworkStatus] ${event} listener not available:`, e);
      }
    };

    void register(IPC_EVENTS.TILE_TRANSPORT_ERROR, () => goOffline());
    void register(IPC_EVENTS.TILE_TRANSPORT_SUCCESS, () => goOnline());

    const handleBrowserOnline = () => {
      // Browser reports online - verify with backend
      invokeIpc('get_network_status', undefined, { purpose: 'check network status' })
        .then((online) => { if (online) goOnline(); })
        .catch(() => { /* ignore - tile events will confirm */ });
    };
    const handleBrowserOffline = () => goOffline();

    window.addEventListener('online', handleBrowserOnline);
    window.addEventListener('offline', handleBrowserOffline);

    return () => {
      disposed = true;
      unlisteners.forEach((fn) => fn());
      window.removeEventListener('online', handleBrowserOnline);
      window.removeEventListener('offline', handleBrowserOffline);
    };
  }, [goOnline, goOffline]);

  // Poll get_network_status only when offline (to detect recovery)
  useEffect(() => {
    if (isOnline) return;

    const poll = async () => {
      try {
        const online = await invokeIpc('get_network_status', undefined, { purpose: 'check network status' });
        if (online) goOnline();
      } catch {
        // Still offline
      }
    };

    poll();
    const intervalId = setInterval(poll, 10000);

    return () => clearInterval(intervalId);
  }, [isOnline, goOnline]);
}
