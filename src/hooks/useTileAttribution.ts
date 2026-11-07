import { useEffect, useState } from 'react';
import { invokeIpc } from '../utils/ipc';
import { PROVIDER_ATTRIBUTION } from '../utils/tileConfig';

/**
 * Attribution for one tile provider, as the UI consumes it.
 *
 * `text` is the data-source list that MUST be displayed wherever the imagery is
 * drawn. `notice`/`noticeUrl` are the provider credit ("Powered by Esri"), which
 * Esri permits behind a menu or button - the About dialog carries those.
 */
export interface ResolvedTileAttribution {
  /** Data-source names. Required on (or directly beneath) the map. */
  text: string;
  /** Provider notice, e.g. "Powered by Esri". */
  notice: string | null;
  /** Brand URL for `notice`. */
  noticeUrl: string | null;
  /** `true` once `text` came from the provider's live service metadata. */
  live: boolean;
}

/** The compiled-in answer, used before the IPC resolves and after it fails. */
export function fallbackAttribution(provider: string): ResolvedTileAttribution {
  const known = PROVIDER_ATTRIBUTION[provider];
  return {
    text: known?.attribution ?? '',
    notice: known?.notice ?? null,
    noticeUrl: known?.noticeUrl ?? null,
    live: false,
  };
}

/**
 * The one IN-FLIGHT request per provider, so concurrent mounts share a call.
 *
 * Deliberately not a cache. Three surfaces can display Esri imagery at once -
 * the survey map, Global View and a Section Preview - and mounting them
 * together must not mean three round trips. But that is the whole job: the
 * entry is dropped the moment the request settles, whatever it resolved to.
 *
 * Caching belongs to the backend, which is the only side that knows what it
 * has: a live string good for 24h, or a failed attempt it will not retry for 5
 * minutes (`tile_attribution.rs`). Keeping settled answers here as well put
 * both of those out of reach - a fallback pinned the compiled-in names for the
 * session, and a live one pinned a day-old string in any session outliving the
 * backend's TTL. One cache, in the place that owns the clocks.
 */
const inFlight = new Map<string, Promise<ResolvedTileAttribution>>();

export function loadTileAttribution(provider: string): Promise<ResolvedTileAttribution> {
  const cached = inFlight.get(provider);
  if (cached) return cached;

  const settle = <T>(value: T): T => {
    inFlight.delete(provider);
    return value;
  };

  const request = invokeIpc(
    'get_tile_attribution',
    { provider },
    { purpose: 'display map imagery attribution' },
  )
    .then((result) =>
      settle({
        // An empty live string would leave required attribution blank, so treat
        // it the same as a failure and keep the compiled-in names.
        text: result.text || fallbackAttribution(provider).text,
        notice: result.notice,
        noticeUrl: result.notice_url,
        live: result.live && !!result.text,
      }),
    )
    .catch(() =>
      // Never surfaced to the user: the fallback is a correct attribution, and
      // a toast about a copyright string is noise.
      settle(fallbackAttribution(provider)),
    );

  inFlight.set(provider, request);
  return request;
}

/** Test seam: forget any in-flight request. */
export function resetTileAttributionCache(): void {
  inFlight.clear();
}

/**
 * Resolve a provider's attribution, starting from the compiled-in value so the
 * map never renders imagery with a blank credit while the IPC is in flight.
 *
 * `enabled` gates the network call, and defaults to true. Pass the same
 * condition that decides whether the imagery is drawn: the attribution is only
 * owed where the imagery is, and the fetch reaches the provider's servers, so
 * running it for a hidden credit contacts a third party on behalf of a user who
 * turned that imagery off. Gated here rather than at each call site so a surface
 * added later cannot forget - and because a caller cannot fix it itself, hooks
 * being unconditional by rule.
 */
export function useTileAttribution(provider: string, enabled = true): ResolvedTileAttribution {
  const [attribution, setAttribution] = useState<ResolvedTileAttribution>(() =>
    fallbackAttribution(provider),
  );

  useEffect(() => {
    let active = true;
    // Re-seed synchronously: switching providers must not leave the previous
    // provider's names under the new provider's imagery, even for a frame.
    setAttribution(fallbackAttribution(provider));
    if (!enabled) return;
    loadTileAttribution(provider).then((resolved) => {
      if (active) setAttribution(resolved);
    });
    return () => {
      active = false;
    };
  }, [provider, enabled]);

  return attribution;
}
