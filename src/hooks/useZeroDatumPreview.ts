import { useEffect, useRef, useState } from 'react';
import { useProjectStore } from '../store/projectStore';
import { getErrorMessage } from '../utils/ipc';
import { projectIdentityKey } from '../utils/projectIdentity';
import { previewZeroDatum } from '../utils/zeroDatumPreview';
import type { GpsCoordinates, StationRef } from '../types/project';
import type { ZeroDatumPreview } from '../types/ipc/ZeroDatumPreview';

/** Quiet time after the last GPS keystroke before asking the backend. */
export const ZERO_DATUM_PREVIEW_DEBOUNCE_MS = 300;

/**
 * Live `preview_zero_datum` answer for the Edit Station dialog (#741): what a
 * zero datum with `coordinates` on `stationRef` would do. `null` while disabled,
 * while waiting, or when the backend has no answer.
 *
 * Debounced, because the GPS field changes on every keystroke. A reply paints
 * only while it is still the latest request AND the project is still the one it
 * was asked about (`projectIdentityKey`, as the ruler's along-line query does):
 * an edit, an undo or another project in between invalidates it, and the effect
 * re-asks for the new state.
 */
export function useZeroDatumPreview(
  stationRef: StationRef | undefined,
  coordinates: GpsCoordinates | null,
  enabled: boolean
): ZeroDatumPreview | null {
  const projectKey = useProjectStore((s) => projectIdentityKey(s.project));
  const [preview, setPreview] = useState<ZeroDatumPreview | null>(null);
  const requestSeq = useRef(0);

  const sectionId = stationRef?.section_id;
  const stationIndex = stationRef?.station_index;
  const latitude = coordinates?.latitude;
  const longitude = coordinates?.longitude;
  const depthReference = coordinates?.depth_reference;

  useEffect(() => {
    const seq = ++requestSeq.current;
    // Whatever was shown answered other inputs: never leave it up.
    setPreview(null);
    if (!enabled || sectionId === undefined || stationIndex === undefined) return;
    if (latitude === undefined || longitude === undefined) return;

    const timer = setTimeout(() => {
      const ref = { section_id: sectionId, station_index: stationIndex };
      const gps = { latitude, longitude, depth_reference: depthReference ?? 0 };
      previewZeroDatum(ref, gps)
        .then((answer) => {
          if (seq !== requestSeq.current) return;
          if (projectIdentityKey(useProjectStore.getState().project) !== projectKey) return;
          setPreview(answer);
        })
        .catch((e: unknown) => {
          // Advisory only: the dialog still saves without it.
          if (seq === requestSeq.current) {
            console.warn('Zero datum preview failed:', getErrorMessage(e, 'unknown error'));
          }
        });
    }, ZERO_DATUM_PREVIEW_DEBOUNCE_MS);

    return () => {
      clearTimeout(timer);
      // Unmount or new inputs: a reply still in flight is stale.
      requestSeq.current++;
    };
  }, [enabled, sectionId, stationIndex, latitude, longitude, depthReference, projectKey]);

  return preview;
}
