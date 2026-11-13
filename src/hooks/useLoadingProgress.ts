/**
 * Hook to listen for Tauri loading-progress events.
 * Updates projectStore loading state based on backend progress events.
 * Includes timeout detection to show "taking longer than expected" message.
 */
import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useProjectStore } from '../store/projectStore';
import type { LoadingProgressPayload } from '../types/loading';

const TIMEOUT_MS = 30000; // 30 second timeout

// Track the highest seen backend loadingId globally to filter truly stale events
// when store.loadingId is 0 (awaiting first event from new load)
let lastSeenBackendLoadingId = 0n;

export function useLoadingProgress() {
  const setLoadingProgress = useProjectStore((s) => s.setLoadingProgress);
  const loading = useProjectStore((s) => s.loading);
  const timeoutRef = useRef<number | null>(null);
  // Capture the clientOpId at effect start to detect if a new load started
  const clientOpIdRef = useRef(loading.clientOpId);

  useEffect(() => {
    // Only listen when loading is active and no error yet
    if (!loading.isLoading || loading.error) return;

    // Update clientOpId ref when effect runs (new load started)
    clientOpIdRef.current = loading.clientOpId;

    // Track whether effect has been cleaned up (for async listener setup race)
    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    const checkTimeout = () => {
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
      timeoutRef.current = window.setTimeout(() => {
        const current = useProjectStore.getState().loading;
        if (current.isLoading && !current.error) {
          // Show timeout warning without replacing the phase
          setLoadingProgress({ phase: 'loading_taking_long' });
        }
      }, TIMEOUT_MS);
    };

    const setupListener = async () => {
      const listener = await listen<LoadingProgressPayload>('loading-progress', (event) => {
        const currentState = useProjectStore.getState().loading;
        // event.payload comes from a Tauri `listen` JSON event, so
        // `loadingId` arrives as a plain JS number even though the ts-rs
        // type says `bigint`. Only the binary result path (loadProjectBinary)
        // actually delivers a JS bigint - a JSON `invoke()` result (e.g.
        // CreateProjectFullResult) lies about the type the same way this
        // event does. Normalize here so every comparison and store write
        // below uses bigint consistently - see issue #486.
        const rawLoadingId = event.payload.loadingId;
        const eventLoadingId =
          typeof rawLoadingId === 'bigint' ? rawLoadingId : BigInt(rawLoadingId);

        // Check if a newer load started (clientOpId changed)
        if (currentState.clientOpId !== clientOpIdRef.current) {
          if (import.meta.env.DEV) {
            console.debug('[Loading] Ignoring event - new load started');
          }
          return;
        }

        // Filter stale events:
        // - If store.loadingId is 0, only accept events with ID > lastSeenBackendLoadingId
        // - If store.loadingId is set, only accept events with matching ID
        if (currentState.loadingId === 0n) {
          // Awaiting first event - only accept if it's newer than any we've seen
          if (eventLoadingId <= lastSeenBackendLoadingId) {
            if (import.meta.env.DEV) {
              console.debug(
                '[Loading] Ignoring stale event (awaiting handshake)',
                eventLoadingId,
                '<=',
                lastSeenBackendLoadingId
              );
            }
            return;
          }
        } else if (eventLoadingId !== currentState.loadingId) {
          if (import.meta.env.DEV) {
            console.debug(
              '[Loading] Ignoring stale event',
              eventLoadingId,
              '!=',
              currentState.loadingId
            );
          }
          return;
        }

        // Filter out export-related events - the Export panel's ExportControls handles those
        // Export phases use a separate flow and should not update project loading state
        const { phase } = event.payload;
        if (phase.startsWith('export_')) {
          if (import.meta.env.DEV) {
            console.debug('[Loading] Ignoring export event in project loading hook', phase);
          }
          return;
        }

        // Update global tracker
        if (eventLoadingId > lastSeenBackendLoadingId) {
          lastSeenBackendLoadingId = eventLoadingId;
        }

        // Reset timeout on each progress event
        checkTimeout();

        const { phaseIndex, phaseCount, current, total, overallPercent, isIndeterminate } =
          event.payload;

        setLoadingProgress({
          loadingId: eventLoadingId, // Set/update backend loadingId
          phase,
          phaseIndex,
          phaseCount,
          phaseProgress: isIndeterminate
            ? undefined
            : current !== null && total !== null && total > 0
              ? (current / total) * 100
              : 0,
          phaseCurrent: current ?? undefined,
          phaseTotal: total ?? undefined,
          overallProgress: overallPercent,
        });

        if (import.meta.env.DEV) {
          console.debug(
            '[Loading]',
            phase,
            phaseIndex + 1,
            '/',
            phaseCount,
            current,
            '/',
            total,
            `(${overallPercent.toFixed(1)}%)`
          );
        }
      });

      // If cleanup ran while we were setting up, immediately unlisten
      if (disposed) {
        listener();
      } else {
        unlisten = listener;
      }
    };

    // Start initial timeout
    checkTimeout();
    setupListener();

    return () => {
      disposed = true;
      // Unlisten if setup completed
      if (unlisten) {
        unlisten();
      }
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
    };
  }, [loading.isLoading, loading.error, loading.clientOpId, setLoadingProgress]);
}
