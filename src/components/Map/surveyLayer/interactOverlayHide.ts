// Transient hide of the interact-overlay layers across a removal flush's re-tile
// (#customRender delete highlight straggler).
//
// The hover / selection highlight (individual hovered / selected shot + station,
// snap ring) renders from the `survey-interact` GeoJSON source. When a section is
// deleted under the custom renderer, SurveyLayer's flush prune calls
// `refreshInteractOverlay()`, whose `setData` drops the removed feature from the
// overlay DATA in the same frame the custom geometry vanishes. But a GeoJSON
// source paints from its resident tile, and `setData` re-tiles asynchronously on
// MapLibre's worker; under the delete flush's own worker load that re-tile lands
// ~250ms later (measured live). So the stale tile keeps painting the deleted
// feature's highlight for a split second, hanging over empty space after the
// geometry is already gone.
//
// setFilter / setData on this source do NOT close the gap: both need the same
// contended worker re-tile and were measured just as slow. `visibility: none` is
// a pure render-skip (no worker, no re-tile), so it drops the stale highlight in
// the very next paint. Visibility is restored on the interact source's next
// loaded event - i.e. once the re-tiled, correct overlay is ready - so the valid
// highlights (features NOT removed) come back with no stale frame in between.
//
// This module holds the pure "did the overlay paint a removed feature" decision
// plus the small hide/restore controller so both can be unit-tested without a
// live MapLibre instance.

import type * as maplibregl from 'maplibre-gl';

/** Section id encoded in a composite overlay id (`"<sectionId>:<...>"`), or null
 *  when the id is null or not in that shape. Shots use `"sectionId:shotId"`,
 *  stations use `"sectionId:stationIndex"` - the prefix is the section id in both. */
function sectionOfOverlayId(id: string | null | undefined): number | null {
  if (!id) return null;
  const colon = id.indexOf(':');
  if (colon <= 0) return null;
  const n = Number(id.slice(0, colon));
  return Number.isNaN(n) ? null : n;
}

/**
 * Whether any currently-highlighted overlay id belongs to a section this flush
 * removed - i.e. the interact overlay was painting a feature that just got
 * deleted, so its stale tile must be hidden until the overlay re-tiles. Pure.
 */
export function overlayTouchesRemovedSections(
  highlightedIds: readonly (string | null | undefined)[],
  removedSectionIds: readonly number[],
): boolean {
  if (removedSectionIds.length === 0) return false;
  const removed = new Set(removedSectionIds);
  for (const id of highlightedIds) {
    const section = sectionOfOverlayId(id);
    if (section !== null && removed.has(section)) return true;
  }
  return false;
}

/**
 * Digest of what an interact-overlay build will actually paint: every emitted
 * feature's visual properties, in emission order.
 *
 * Compare one taken BEFORE a dim-set change with one taken AFTER the rebuild. An
 * unequal pair means the overlay the user is looking at really does change, so its
 * stale tile must be hidden across the re-tile. An equal pair - the common case,
 * e.g. a tool switch that sets empty dim sets - means hiding would only introduce
 * flicker.
 *
 * Deliberately derived from the BUILT features rather than from the dim sets plus
 * a copy of the emission rules. The rules are asymmetric in ways that are easy to
 * get wrong (hover branches gate on anchored AND immovable, selected branches on
 * neither; shot and station feature ids share a numeric space, so `1:100000` and
 * `2:0` both encode to 200000), and a re-derived rule table silently drifts from
 * the builder. Reading the output cannot drift. Pure.
 */
export function overlayPaintDigest(features: readonly GeoJSON.Feature[]): string {
  return features.map((f) => JSON.stringify(f.properties ?? null)).join('|');
}

/** Narrow map surface the controller needs (visibility get/set + source-load
 *  events + layer existence). Keeps the controller unit-testable with a stub. */
export type InteractHideMap = Pick<
  maplibregl.Map,
  'getLayer' | 'getLayoutProperty' | 'setLayoutProperty' | 'on' | 'off'
>;

/** Safety net: restore visibility even if the source-loaded event never arrives
 *  (it always does in practice) so the overlay can never be stranded hidden. */
export const INTERACT_HIDE_FALLBACK_MS = 2000;

/**
 * Hide the interact-overlay layers for the brief window a removal flush's
 * `setData(empty)` takes to re-tile, then restore them once the interact source
 * finishes reloading. See the module header for why `visibility` (not a filter or
 * setData) is the mechanism.
 *
 * Each hidden layer's prior visibility is captured and restored, so a layer the
 * user had hidden (stations / shots visibility toggle) is never forced back on;
 * a toggle flipped while the hide is pending goes through
 * {@link setLayerVisibility}, which updates what the restore puts back.
 */
export class InteractOverlayHideController {
  private active = false;
  private timeout: ReturnType<typeof setTimeout> | null = null;
  private onSourceData: ((e: maplibregl.MapSourceDataEvent) => void) | null = null;
  /** Per-layer visibility captured at hide time, restored on settle. */
  private saved = new Map<string, maplibregl.VisibilitySpecification | undefined>();

  /**
   * `isLive` reports whether the map still has a style to restore into: false
   * once the map is removed, and while a lost WebGL context has it torn down.
   * There a restore writes nothing - `setLayoutProperty` reads the style
   * without `?.` and would throw - and there is nothing to restore into anyway.
   */
  constructor(
    private readonly map: InteractHideMap,
    private readonly sourceId: string,
    private readonly layerIds: readonly string[],
    private readonly isLive: () => boolean = () => true,
  ) {}

  /**
   * Hide the overlay layers until the interact source finishes its next re-tile.
   * A second removal that lands while already hidden just extends the fallback
   * window (the pending source-loaded listener still restores on the real settle).
   */
  hideUntilRetile(): void {
    if (this.active) {
      this.armFallback();
      return;
    }
    this.active = true;
    this.capture();
    this.setVisible(false);
    const handler = (e: maplibregl.MapSourceDataEvent): void => {
      // The re-tile driven by the prune's setData completes when the source next
      // reports fully loaded; earlier events for this source are loading:false.
      if (e.sourceId !== this.sourceId || !e.isSourceLoaded) return;
      this.restore();
    };
    this.onSourceData = handler;
    this.map.on('sourcedata', handler);
    this.armFallback();
  }

  /** Restore the captured visibility and drop the listener/timer. Safe to call
   *  when inactive (no-op). */
  restore(): void {
    if (!this.active) return;
    // A worker restart or context restore puts a rebuilt style in place that is
    // empty until its `style.load` one frame later, and that copy was serialized
    // with these layers hidden. A restore landing in that frame (the fallback
    // timer) finds none of them: keep the capture, still listening, for the
    // restore that follows the load (SurveyLayer's `beforeLoss`).
    // `isLive` first: on a torn-down style (the map removed, a lost context)
    // `getLayer` finds none of the layers either (it returns undefined without
    // a style), but that is not the loading frame: fall through and drop the
    // capture. The write below needs the style - `setLayoutProperty` reads it
    // without `?.` - and there is nothing to restore into anyway.
    if (
      this.isLive() &&
      this.saved.size > 0 &&
      [...this.saved.keys()].every((id) => !this.map.getLayer(id))
    ) {
      return;
    }
    const saved = [...this.saved];
    this.drop();
    if (!this.isLive()) return;
    for (const [id, vis] of saved) {
      if (this.map.getLayer(id)) this.map.setLayoutProperty(id, 'visibility', vis ?? 'visible');
    }
  }

  /**
   * Set an overlay layer's visibility as the user's toggles now want it (the
   * Stations / Shots toggles own three of the layers). A hide holding the layer
   * restores this value rather than the one it captured, so a toggle flipped
   * during the hide is not undone when the hide ends, and the layer stays hidden
   * until then: writing it now would end the hide early for this layer (a
   * context restore's replay re-applies the toggles right after its own flush
   * may have started a hide).
   */
  setLayerVisibility(id: string, vis: maplibregl.VisibilitySpecification): void {
    if (this.active && this.saved.has(id)) {
      this.saved.set(id, vis);
      return;
    }
    this.map.setLayoutProperty(id, 'visibility', vis);
  }

  /** Teardown: restore any hidden layers and drop the timer/listener. */
  dispose(): void {
    this.restore();
  }

  /**
   * Map-free teardown for when the map itself is going away: forget the pending
   * restore (timer + listener) WITHOUT restoring visibility. The fallback timer
   * would otherwise fire after the map is removed and pin this controller - and
   * the SurveyLayer behind it - until it does. Idempotent.
   */
  drop(): void {
    this.active = false;
    if (this.timeout) {
      clearTimeout(this.timeout);
      this.timeout = null;
    }
    if (this.onSourceData) {
      this.map.off('sourcedata', this.onSourceData);
      this.onSourceData = null;
    }
    this.saved.clear();
  }

  private capture(): void {
    this.saved.clear();
    for (const id of this.layerIds) {
      if (!this.map.getLayer(id)) continue;
      this.saved.set(id, this.map.getLayoutProperty(id, 'visibility') as maplibregl.VisibilitySpecification | undefined);
    }
  }

  private setVisible(visible: boolean): void {
    const value = visible ? 'visible' : 'none';
    for (const id of this.saved.keys()) {
      if (this.map.getLayer(id)) this.map.setLayoutProperty(id, 'visibility', value);
    }
  }

  private armFallback(): void {
    if (this.timeout) clearTimeout(this.timeout);
    this.timeout = setTimeout(() => this.restore(), INTERACT_HIDE_FALLBACK_MS);
  }
}
