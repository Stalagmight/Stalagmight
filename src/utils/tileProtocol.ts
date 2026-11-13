/**
 * Tile protocol utilities.
 *
 * NOTE: The actual tile fetching is handled by Tauri's native protocol handler
 * registered in src-tauri/src/lib.rs via register_asynchronous_uri_scheme_protocol.
 * This returns binary data directly without JSON serialization overhead.
 *
 * Stats are tracked in Rust via atomic counters and exposed via the
 * `get_tile_fetch_stats` IPC command.
 */

import { invoke } from '@tauri-apps/api/core';
import * as maplibregl from 'maplibre-gl';
import { isAndroid } from './platform';

/**
 * Tile fetch statistics from the Rust native protocol handler.
 */
export interface TileFetchStats {
  /** Number of tile requests currently in flight */
  inFlight: number;
  /** Average latency of completed tile fetches in ms */
  avgLatencyMs: number;
  /** Last tile fetch latency in ms */
  lastLatencyMs: number;
  /** Total tiles fetched since app start */
  totalFetched: number;
  /** Total fetch errors (after retries exhausted) */
  totalErrors: number;
}

/**
 * Get current tile fetch statistics from the Rust backend.
 * Returns real-time metrics from the native tile protocol handler.
 */
export async function getTileFetchStats(): Promise<TileFetchStats> {
  const stats = await invoke<{
    in_flight: number;
    avg_latency_ms: number;
    last_latency_ms: number;
    total_fetched: number;
    total_errors: number;
  }>('get_tile_fetch_stats');
  return {
    inFlight: stats.in_flight,
    avgLatencyMs: stats.avg_latency_ms,
    lastLatencyMs: stats.last_latency_ms,
    totalFetched: stats.total_fetched,
    totalErrors: stats.total_errors,
  };
}

/**
 * Register the tile protocol for MapLibre.
 *
 * On desktop, this is a no-op - tiles go directly through Tauri's native
 * protocol handler (register_asynchronous_uri_scheme_protocol).
 *
 * On Android, the native protocol handler serializes requests through a single
 * WebView IO thread (shouldInterceptRequest), limiting concurrency to 1 tile
 * at a time. Instead, we register a JS-side MapLibre protocol that fetches
 * tiles via Tauri IPC commands, which run concurrently on the async runtime.
 */
export function registerTileProtocol(): void {
  if (!isAndroid) return;

  maplibregl.addProtocol('stalagmight-tiles', async (params: { url: string }) => {
    // Parse provider/z/y/x from URL: stalagmight-tiles://esri/10/500/250
    const match = params.url.match(/stalagmight-tiles:\/\/([^/]+)\/(\d+)\/(\d+)\/(\d+)/);
    if (!match) throw new Error(`Invalid tile URL: ${params.url}`);
    const [, provider, z, y, x] = match;
    const data = await invoke<number[]>('fetch_tile', {
      provider,
      z: Number(z),
      y: Number(y),
      x: Number(x),
    });
    return { data: new Uint8Array(data) };
  });
}

/**
 * Unregister the tile protocol.
 */
export function unregisterTileProtocol(): void {
  if (!isAndroid) return;
  maplibregl.removeProtocol('stalagmight-tiles');
}
