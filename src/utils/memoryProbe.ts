/**
 * Memory-probe instrumentation helpers.
 *
 * Two roles:
 *
 * 1. Track per-source feature counts whenever map layers push GeoJSON
 *    FeatureCollections to MapLibre. We can't read back from MapLibre's
 *    worker memory, so we publish the counts ourselves at the call site.
 *
 * 2. Estimate retained bytes for the categories the WebProcess accumulates
 *    over a session. The estimates are deliberately rough heuristics — the
 *    point is to surface *which bucket grew*, not produce a byte-accurate
 *    accountant.
 *
 * Everything in this module is no-ops in terms of behavior; it only writes
 * to a singleton state read by the MemoryProbe panel. Calls are cheap enough
 * to leave in release (single object-property write per setData call).
 */
import type { SurveyGeometry } from '../types/ipc/SurveyGeometry';

export type FeatureSourceKind =
  | 'survey-shots'
  | 'survey-shot-gradients'
  | 'survey-stations'
  | 'survey-shot-labels'
  | 'survey-station-labels'
  | 'survey-section-labels'
  | 'survey-loop-closures'
  | 'survey-markers'
  | 'survey-ghost-shots'
  | 'survey-ghost-stations'
  | 'survey-ghost-merge'
  | 'global-survey';

export interface FeatureCountSnapshot {
  /** Number of features published to MapLibre per source. */
  perSource: Record<FeatureSourceKind, number>;
  /** Total cumulative setData calls per source — surfaces churn rate. */
  setDataCalls: Record<FeatureSourceKind, number>;
}

const EMPTY_COUNTS = (): FeatureCountSnapshot => ({
  perSource: {
    'survey-shots': 0,
    'survey-shot-gradients': 0,
    'survey-stations': 0,
    'survey-shot-labels': 0,
    'survey-station-labels': 0,
    'survey-section-labels': 0,
    'survey-loop-closures': 0,
    'survey-markers': 0,
    'survey-ghost-shots': 0,
    'survey-ghost-stations': 0,
    'survey-ghost-merge': 0,
    'global-survey': 0,
  },
  setDataCalls: {
    'survey-shots': 0,
    'survey-shot-gradients': 0,
    'survey-stations': 0,
    'survey-shot-labels': 0,
    'survey-station-labels': 0,
    'survey-section-labels': 0,
    'survey-loop-closures': 0,
    'survey-markers': 0,
    'survey-ghost-shots': 0,
    'survey-ghost-stations': 0,
    'survey-ghost-merge': 0,
    'global-survey': 0,
  },
});

const counts: FeatureCountSnapshot = EMPTY_COUNTS();

/**
 * Record that `count` features were just pushed to a MapLibre source.
 * Call this every time a layer calls `setData`. Overwrites previous count
 * (we want the current live size, not cumulative).
 */
export function recordFeatureCount(source: FeatureSourceKind, count: number): void {
  counts.perSource[source] = count;
  counts.setDataCalls[source] = (counts.setDataCalls[source] ?? 0) + 1;
}

export function getFeatureCounts(): FeatureCountSnapshot {
  // Shallow clone so consumers can't mutate.
  return {
    perSource: { ...counts.perSource },
    setDataCalls: { ...counts.setDataCalls },
  };
}

export function resetFeatureCounts(): void {
  Object.assign(counts, EMPTY_COUNTS());
}

/**
 * Zero only the per-source feature counts for a given prefix, leaving the
 * cumulative setData call counter intact. Used by SurveyLayer/GlobalSurveyLayer
 * `destroy()` so the panel reflects "no map is alive anymore" rather than
 * showing stale counts from the most recent setData call.
 */
export function clearFeatureCountsForPrefix(prefix: string): void {
  for (const key of Object.keys(counts.perSource) as FeatureSourceKind[]) {
    if (key.startsWith(prefix)) {
      counts.perSource[key] = 0;
    }
  }
}

/**
 * Recent perf timings — surfaced in the MemoryProbe panel so the user can
 * see freeze costs without opening devtools. Keyed by a short label; the
 * most recent entry per label wins.
 */
const perfTimings: Record<string, { ms: number; ts: number }> = {};

export function recordPerfTiming(label: string, ms: number): void {
  perfTimings[label] = { ms: Math.round(ms), ts: Date.now() };
}

export function getPerfTimings(): Record<string, { ms: number; ts: number }> {
  return { ...perfTimings };
}

/**
 * Long-task observer: any task >50ms that blocks the main thread shows up
 * here. The back-button "frozen then snap" symptom should produce one
 * dominant long task spanning React commit + paint + GC; this surfaces it.
 *
 * Registered once per module load; observer survives HMR.
 *
 * WebKitGTK does NOT implement `longtask` entryType, so we fall back to a
 * rAF-chain frame-gap detector (any inter-frame interval >50ms is a freeze).
 */
let longTaskObserver: PerformanceObserver | null = null;
let frameGapDetectorRunning = false;
let longTaskCounter = 0;
export function startLongTaskObserver(): void {
  if (typeof PerformanceObserver !== 'undefined' && !longTaskObserver) {
    try {
      const supported = (
        // Some WebKits don't expose supportedEntryTypes; treat as unknown.
        (PerformanceObserver as unknown as { supportedEntryTypes?: readonly string[] }).supportedEntryTypes ?? []
      ).includes('longtask');
      if (supported) {
        longTaskObserver = new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) {
            if (entry.duration < 50) continue;
            longTaskCounter += 1;
            recordPerfTiming(`longtask #${longTaskCounter}`, entry.duration);
          }
        });
        longTaskObserver.observe({ entryTypes: ['longtask'] });
      }
    } catch {
      // Fall through to the frame-gap detector below.
    }
  }

  // Frame-gap detector: only records the *worst* freeze seen in each
  // ~1 s window so the panel doesn't fill with `freeze #N` entries from
  // every minor stall. Threshold raised to 200 ms — short stalls are
  // noise; we only care about user-perceptible ones.
  if (!frameGapDetectorRunning) {
    frameGapDetectorRunning = true;
    let last = performance.now();
    let worstInWindow = 0;
    let windowStart = last;
    const FREEZE_THRESHOLD_MS = 200;
    const WINDOW_MS = 1000;
    const tick = () => {
      const now = performance.now();
      const gap = now - last;
      if (gap > FREEZE_THRESHOLD_MS && gap > worstInWindow) {
        worstInWindow = gap;
      }
      if (now - windowStart >= WINDOW_MS) {
        if (worstInWindow > 0) {
          longTaskCounter += 1;
          recordPerfTiming(`freeze #${longTaskCounter}`, worstInWindow);
        }
        windowStart = now;
        worstInWindow = 0;
      }
      last = now;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }
}

/**
 * Estimate retained bytes for a `SurveyGeometry` payload — used to compute
 * how much memory the GlobalView geometryData cache holds in aggregate.
 *
 * Heuristic per shot: 2 boxed [lng,lat] arrays (~64B each in JSC including
 * the outer Array header) + the 4 named properties on a small inline JS
 * object (~96B). Floor of ~225B/shot, plus the SurveyGeometry header.
 */
export function estimateGeometryBytes(geometry: SurveyGeometry): number {
  // Header + origin array + shots-array header
  const header = 48 + 48 + 32;
  return header + geometry.shots.length * 225;
}

/**
 * Estimate retained bytes for a single GeoJSON Feature with the rough
 * property shape we use (LineString + ~9 props, or Point + ~7 props).
 * Calibrated empirically — see docs/performance.md.
 */
export function estimateFeatureBytes(kind: FeatureSourceKind): number {
  switch (kind) {
    case 'survey-shots':
    case 'survey-shot-gradients':
    case 'survey-loop-closures':
    case 'survey-ghost-shots':
      // LineString feature (~2-vertex) + ~9 properties
      return 350;
    case 'survey-stations':
    case 'survey-station-labels':
    case 'survey-shot-labels':
    case 'survey-ghost-stations':
    case 'survey-ghost-merge':
    case 'survey-section-labels':
    case 'survey-markers':
      // Point feature + ~7 properties (label string included)
      return 320;
    case 'global-survey':
      // Mixed -- assume average of line + point shapes
      return 320;
  }
}

/** Sum estimated bytes across all tracked sources. */
export function estimateMaplibreFeatureBytes(): number {
  let total = 0;
  for (const key of Object.keys(counts.perSource) as FeatureSourceKind[]) {
    total += counts.perSource[key] * estimateFeatureBytes(key);
  }
  // MapLibre clones into the worker — double on the main thread side
  // accounting (we observe both copies in the WebProcess).
  return total * 2;
}
