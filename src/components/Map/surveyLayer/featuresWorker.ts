/**
 * Web Worker entry point for off-thread `buildSurveyFeatures`.
 *
 * Main thread → worker: `{ kind: 'build', requestId, args }`
 * Worker → main thread: `{ kind: 'result', requestId, features }` on success,
 *                       `{ kind: 'error', requestId, message }` on failure.
 *
 * The request/response pairing lets us share a single worker instance across
 * concurrent `update()` calls; the client wrapper matches results by id.
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

interface BuildArgs {
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

interface BuildMessage {
  kind: 'build';
  requestId: number;
  args: BuildArgs;
}

interface ResultMessage {
  kind: 'result';
  requestId: number;
  features: SurveyFeatureSets;
}

interface ErrorMessage {
  kind: 'error';
  requestId: number;
  message: string;
}

type IncomingMessage = BuildMessage;
type OutgoingMessage = ResultMessage | ErrorMessage;

self.onmessage = (e: MessageEvent<IncomingMessage>) => {
  const msg = e.data;
  if (msg.kind !== 'build') return;

  try {
    const features = buildSurveyFeatures(
      msg.args.surveyData,
      msg.args.loopClosures,
      msg.args.gpsOrigin,
      msg.args.showDepthGradient,
      msg.args.depthRange,
      msg.args.themeColors,
      msg.args.precision,
      msg.args.options,
      msg.args.markers,
    );
    const out: OutgoingMessage = { kind: 'result', requestId: msg.requestId, features };
    self.postMessage(out);
  } catch (e) {
    const out: OutgoingMessage = {
      kind: 'error',
      requestId: msg.requestId,
      message: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
    };
    self.postMessage(out);
  }
};

// Required for TS to treat this as a module (otherwise `self.onmessage`
// types fall back to the wrong DOM globals).
export {};
