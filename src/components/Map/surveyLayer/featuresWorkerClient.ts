/**
 * Main-thread client for the survey-features Worker.
 *
 * Lazily creates a single Worker (re-used across calls) and routes
 * `build` requests through it, returning a Promise. Falls back to
 * synchronous main-thread build if the Worker can't be constructed
 * (e.g. in test runners that don't support Vite's `?worker` syntax).
 *
 * Tested rough costs on a 75k-shot / 89k-station project:
 *   structured-clone of inputs (Section[] + Station[]):  ~50 ms
 *   buildSurveyFeatures in worker (off main thread):    ~400 ms (parallel)
 *   structured-clone of features back:                   ~80 ms
 * vs synchronous main-thread build of ~450 ms blocking the UI.
 * Net: ~270 ms of main-thread time given back to React + rendering.
 */
import {
  buildSurveyFeatures,
  type BuildSurveyFeaturesOptions,
  type DepthRange,
  type SurveyFeatureSets,
} from './features';
import type { LoopClosure, Marker } from '../../../types/project';
import type { SurveyData } from '../../../types/survey-data';
import type { GpsOrigin, DecimalPrecision } from '../../../types/config';
import type { MapThemeColors } from '../../../utils/themeColors';
import { recordPerfTiming } from '../../../utils/memoryProbe';

interface BuildArgs {
  /** Struct-of-Arrays survey data. Posted to the worker via structured
   *  clone; the underlying typed arrays clone efficiently as a single
   *  large block per array (vs the legacy ~75k JS objects). */
  surveyData: SurveyData;
  loopClosures: LoopClosure[] | undefined;
  markers: Marker[] | undefined;
  gpsOrigin: GpsOrigin | null;
  showDepthGradient: boolean;
  depthRange: DepthRange;
  themeColors: MapThemeColors;
  precision: DecimalPrecision | undefined;
  options: BuildSurveyFeaturesOptions;
}

/** Worker singleton; created lazily on first use. */
let workerInstance: Worker | null = null;
/** Whether we've already tried (and possibly failed) to create the worker.
 *  Once we fall back to inline builds we don't keep retrying. */
let workerCreationAttempted = false;
/** Pending build requests keyed by id. Resolves when worker posts back. */
const pending = new Map<number, { resolve: (s: SurveyFeatureSets) => void; reject: (e: Error) => void }>();
let nextRequestId = 0;

function ensureWorker(): Worker | null {
  if (workerInstance) return workerInstance;
  if (workerCreationAttempted) return null;
  workerCreationAttempted = true;

  try {
    // Vite/Webpack `new Worker(new URL(..., import.meta.url), { type: 'module' })`
    // is the recommended pattern; the bundler emits the worker as a separate
    // chunk and rewrites the URL.
    const w = new Worker(new URL('./featuresWorker.ts', import.meta.url), { type: 'module' });
    w.onmessage = (e: MessageEvent<{ kind: 'result' | 'error'; requestId: number; features?: SurveyFeatureSets; message?: string }>) => {
      const { kind, requestId } = e.data;
      const slot = pending.get(requestId);
      if (!slot) return;
      pending.delete(requestId);
      if (kind === 'result' && e.data.features) {
        slot.resolve(e.data.features);
      } else {
        slot.reject(new Error(e.data.message ?? 'unknown worker error'));
      }
    };
    w.onerror = (e) => {
      // Worker-level error (uncaught exception, etc.) — fail all pending.
      const err = new Error(`featuresWorker: ${e.message}`);
      for (const slot of pending.values()) slot.reject(err);
      pending.clear();
    };
    workerInstance = w;
    return w;
  } catch (e) {
    // Vite-without-worker, jsdom test env, etc. Fall back to inline.
    console.warn('[featuresWorkerClient] failed to construct Worker, falling back to inline build:', e);
    return null;
  }
}

/**
 * Build survey features off the main thread when possible. Falls back to
 * synchronous in-line build if the Worker isn't available (test envs).
 *
 * The promise resolves with the same `SurveyFeatureSets` shape that the
 * synchronous `buildSurveyFeatures` returns; the caller can swap in/out
 * without changing the downstream `setData` calls.
 */
export function buildSurveyFeaturesAsync(args: BuildArgs): Promise<SurveyFeatureSets> {
  const w = ensureWorker();
  if (!w) {
    // Synchronous fallback. Wrap in a resolved promise to keep the call
    // site uniform; the (small) microtask shift is harmless.
    const t0 = performance.now();
    const features = buildSurveyFeatures(
      args.surveyData,
      args.loopClosures,
      args.gpsOrigin,
      args.showDepthGradient,
      args.depthRange,
      args.themeColors,
      args.precision,
      args.options,
      args.markers,
    );
    recordPerfTiming('buildFeatures (inline fallback)', performance.now() - t0);
    return Promise.resolve(features);
  }

  return new Promise<SurveyFeatureSets>((resolve, reject) => {
    const requestId = nextRequestId++;
    pending.set(requestId, { resolve, reject });
    const tPost = performance.now();
    w.postMessage({ kind: 'build', requestId, args });
    // Note: we don't await postMessage — its return is synchronous after the
    // browser schedules the structured-clone, which dominates the cost here.
    recordPerfTiming('buildFeatures postMessage', performance.now() - tPost);
  });
}

/** Tear down the worker. Currently only used from tests. */
export function terminateFeaturesWorker(): void {
  if (workerInstance) {
    workerInstance.terminate();
    workerInstance = null;
  }
  workerCreationAttempted = false;
  for (const slot of pending.values()) slot.reject(new Error('worker terminated'));
  pending.clear();
}
