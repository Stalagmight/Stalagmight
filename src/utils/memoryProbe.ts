/**
 * Memory-probe instrumentation helpers, read by the Memory Stats panel
 * (`components/MemoryProbe`).
 *
 * 1. The custom WebGL renderer's buffer sizes: each controller publishes the
 *    bytes of the vertex arrays it built last (`recordRendererBuild`) - the
 *    arrays it uploads to the GPU. They replaced per-source GeoJSON feature
 *    counts, which nothing had written since the custom renderer replaced the
 *    MapLibre sources (#576 D19-D2).
 *
 * 2. Rough retained-byte estimates for the categories the WebProcess
 *    accumulates over a session. The point is to surface *which bucket grew*,
 *    not produce a byte-accurate accountant.
 *
 * 3. Recent perf timings, and the freeze detector that feeds them while the
 *    panel is visible (`startLongTaskObserver`).
 *
 * Recording is a couple of property writes per build, cheap enough to leave in
 * release; nothing here changes app behaviour.
 */
import type { SurveyGeometry } from '../types/ipc/SurveyGeometry';

/** The custom renderer's buffer sets: the survey map's geometry (shot lines and
 *  station discs) and its labels, and the Global View's geometry. */
export type RendererBufferKind = 'survey-geometry' | 'survey-labels' | 'global-geometry';

export interface RendererBufferStat {
  /** Bytes of the arrays the last build produced; 0 once its controller let them go. */
  bytes: number;
  /** Builds since the app started - surfaces rebuild churn. */
  builds: number;
}

export const RENDERER_BUFFER_KINDS: readonly RendererBufferKind[] = [
  'survey-geometry',
  'survey-labels',
  'global-geometry',
];

const rendererBuffers: Record<RendererBufferKind, RendererBufferStat> = {
  'survey-geometry': { bytes: 0, builds: 0 },
  'survey-labels': { bytes: 0, builds: 0 },
  'global-geometry': { bytes: 0, builds: 0 },
};

/** A controller built `bytes` of vertex data for `kind` (replaces the last build's size). */
export function recordRendererBuild(kind: RendererBufferKind, bytes: number): void {
  const stat = rendererBuffers[kind];
  stat.bytes = bytes;
  stat.builds += 1;
}

/** The controller for `kind` released its buffers (unmount or teardown). */
export function clearRendererBuffers(kind: RendererBufferKind): void {
  rendererBuffers[kind].bytes = 0;
}

/** A copy of every buffer set's stat, so consumers cannot mutate the record. */
export function getRendererBuffers(): Record<RendererBufferKind, RendererBufferStat> {
  const out = {} as Record<RendererBufferKind, RendererBufferStat>;
  for (const kind of RENDERER_BUFFER_KINDS) out[kind] = { ...rendererBuffers[kind] };
  return out;
}

/** The last builds' bytes summed over every buffer set. */
export function totalRendererBufferBytes(): number {
  return RENDERER_BUFFER_KINDS.reduce((sum, kind) => sum + rendererBuffers[kind].bytes, 0);
}

/**
 * Recent perf timings - surfaced in the MemoryProbe panel so the user can
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
 * Freeze detector for the Memory Stats panel. Two sources run while it is on: a
 * `longtask` PerformanceObserver (any task over 50 ms that blocks the main
 * thread) where the engine supports that entry type - WebKitGTK does not - and a
 * requestAnimationFrame frame-gap detector everywhere (an inter-frame interval
 * over 200 ms is a freeze). The back-button "frozen then snap" symptom shows up
 * as one dominant entry.
 *
 * Runs ONLY while the panel is visible (#576 D19): the panel starts it from its
 * visibility effect and calls the returned disposer when hidden. The frame-gap
 * chain re-arms on every frame, so left running it is a 60 Hz wakeup that keeps
 * the WebView from ever going idle - for a list nobody reads in a normal session.
 * The disposer cancels the pending frame and disconnects the observer.
 */
let longTaskCounter = 0;
export function startLongTaskObserver(): () => void {
  let observer: PerformanceObserver | null = null;
  if (typeof PerformanceObserver !== 'undefined') {
    try {
      const supported = (
        // Some WebKits don't expose supportedEntryTypes; treat as unknown.
        (PerformanceObserver as unknown as { supportedEntryTypes?: readonly string[] }).supportedEntryTypes ?? []
      ).includes('longtask');
      if (supported) {
        observer = new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) {
            if (entry.duration < 50) continue;
            longTaskCounter += 1;
            recordPerfTiming(`longtask #${longTaskCounter}`, entry.duration);
          }
        });
        observer.observe({ entryTypes: ['longtask'] });
      }
    } catch {
      // The frame-gap detector below still runs.
      observer = null;
    }
  }

  // Frame-gap detector: only records the *worst* freeze seen in each
  // ~1 s window so the panel doesn't fill with `freeze #N` entries from
  // every minor stall. Threshold 200 ms - short stalls are noise; we only
  // care about user-perceptible ones.
  let stopped = false;
  let frame: number | null = null;
  let last = performance.now();
  let worstInWindow = 0;
  let windowStart = last;
  const FREEZE_THRESHOLD_MS = 200;
  const WINDOW_MS = 1000;
  const tick = () => {
    if (stopped) return;
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
    frame = requestAnimationFrame(tick);
  };
  frame = requestAnimationFrame(tick);

  return () => {
    stopped = true;
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    observer?.disconnect();
    observer = null;
  };
}

/**
 * Estimate retained bytes for a `SurveyGeometry` payload - used to compute
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
