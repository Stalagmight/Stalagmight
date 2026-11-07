import i18n from '../../../i18n';

/**
 * MapLibre `error` event boundary filter.
 *
 * Extracted from the (deleted) stock `vectorTileRegistry.ts` so `SurveyLayer`'s
 * `handleMapError` keeps its noise filter without depending on a stock-pipeline
 * module.
 */

/** MapLibre's message when a tile reload targets a worker uid that was already
 *  dropped (a residual reload race). Benign to correctness; console/telemetry
 *  noise only. */
const RELOAD_REMOVED_MESSAGE = /Should not be trying to reload/;

function isReloadRemovedError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error ?? '');
  return RELOAD_REMOVED_MESSAGE.test(message);
}

/** The subset of a MapLibre map `error` event our boundary filter inspects. As
 *  a source tile error bubbles to `map.on('error')` it is merged with the tile
 *  manager's evented-parent data, so the delivered event carries `sourceId`
 *  alongside the underlying `error`. Both are optional/unknown here so a
 *  malformed or non-source error event is classified without throwing. */
export interface MapErrorEventLike {
  error?: unknown;
  sourceId?: unknown;
}

/**
 * Whether a MapLibre map `error` event is a benign worker-uid-gone tile-reload
 * race for one of this survey's `stalx://` sources - the residual noise MapLibre
 * fires from its own `_loadTile` catch for reloadPromises it mints internally
 * (e.g. `reloadTilesForDependencies` marks a tile `'reloading'` and its drain
 * re-issues a load on a tile first seen mid-load). It is benign to correctness
 * (the trailing reconcile re-cuts the errored tile) but is console/telemetry
 * noise.
 *
 * Suppress ONLY when BOTH hold: the error message is the reload-removed
 * signature AND the event's source id belongs to this survey (prefix match).
 * Anything else - a different message, or the same message from a non-survey
 * source - returns `false` and must pass through untouched. Defensive against a
 * missing/non-string `sourceId` or a missing `error`.
 */
export function shouldSuppressMapError(
  evt: MapErrorEventLike | null | undefined,
  surveySourcePrefix: string,
): boolean {
  const sourceId = evt?.sourceId;
  if (typeof sourceId !== 'string' || !sourceId.startsWith(surveySourcePrefix)) return false;
  return isReloadRemovedError(evt?.error);
}

/**
 * User-facing toast copy for a custom-renderer fatal (see
 * `SurveyLayer.handleRenderFatal`). The `reason` carries the failed subsystem so
 * the message stays specific: a font failure loses only the labels and a
 * measure-path fault only the ruler tint, so neither should alarm about the
 * whole survey being gone or the GPU. Anything else (geometry, or an
 * unrecognized reason) is the survey-wide WebGL2/GPU message. There is no stock
 * fallback behind any of these, so surfacing SOMETHING is the point. Localized
 * (`map.renderFatal.*`, #576 RV4-FE-MF-4): it is toast copy, and the lost-context
 * toast shows the survey-wide one too.
 */
export function renderFatalMessage(reason: string): string {
  if (reason.includes('font')) return i18n.t('map.renderFatal.labels');
  if (reason.includes('measure')) return i18n.t('map.renderFatal.measurePath');
  return i18n.t('map.renderFatal.survey');
}
