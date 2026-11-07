import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { makeFakeMap } from '../../../../test/factories/fakeMap';
import {
  overlayTouchesRemovedSections,
  overlayPaintDigest,
  InteractOverlayHideController,
  INTERACT_HIDE_FALLBACK_MS,
} from '../interactOverlayHide';

describe('overlayTouchesRemovedSections', () => {
  it('is false when nothing was removed', () => {
    expect(overlayTouchesRemovedSections(['4:400002', '6:1'], [])).toBe(false);
  });

  it('is true when a highlighted shot id belongs to a removed section', () => {
    // "sectionId:shotId" - the prefix is the section id.
    expect(overlayTouchesRemovedSections(['4:400002'], [4])).toBe(true);
  });

  it('is true when a highlighted station id belongs to a removed section', () => {
    // "sectionId:stationIndex" - same prefix rule.
    expect(overlayTouchesRemovedSections(['6:1'], [6])).toBe(true);
  });

  it('is false when no highlighted id is in the removed set', () => {
    expect(overlayTouchesRemovedSections(['4:400002', '6:1'], [7, 8])).toBe(false);
  });

  it('ignores null / undefined / malformed ids', () => {
    expect(overlayTouchesRemovedSections([null, undefined, '', 'nope', ':3'], [3])).toBe(false);
    // ":3" has an empty section prefix -> not section 3.
    expect(overlayTouchesRemovedSections([':3'], [0])).toBe(false);
  });

  it('matches any one id among many (e.g. the spread highlighted-station list)', () => {
    const ids = ['4:400002' /*selected shot*/, null /*hovered shot*/, '9:2', '6:3', '9:5'];
    expect(overlayTouchesRemovedSections(ids, [6])).toBe(true);
    expect(overlayTouchesRemovedSections(ids, [10, 11])).toBe(false);
  });
});

const SOURCE = 'survey-interact';
const LAYERS = ['survey-interact-shots', 'survey-interact-stations', 'survey-stations-highlight'];

describe('InteractOverlayHideController', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('hides every overlay layer and restores on the source-loaded event', () => {
    const f = makeFakeMap({
      omit: ['style'],
      visibility: {
        'survey-interact-shots': 'visible',
        'survey-interact-stations': 'visible',
        'survey-stations-highlight': undefined, // default-visible
      },
    });
    const c = new InteractOverlayHideController(f.map, SOURCE, LAYERS);

    c.hideUntilRetile();
    expect(f.visibility['survey-interact-shots']).toBe('none');
    expect(f.visibility['survey-interact-stations']).toBe('none');
    expect(f.visibility['survey-stations-highlight']).toBe('none');
    expect(f.listenerCount('sourcedata')).toBe(1);

    // Loading progress (isSourceLoaded false) must NOT restore.
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: false });
    expect(f.visibility['survey-interact-shots']).toBe('none');

    // The re-tile completes: restore captured visibility and drop the listener.
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('visible');
    expect(f.visibility['survey-interact-stations']).toBe('visible');
    // The default-visible layer is restored to its captured (undefined) value.
    expect(f.visibility['survey-stations-highlight']).toBe('visible');
    expect(f.listenerCount('sourcedata')).toBe(0);
  });

  it('ignores loaded events from other sources', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots']);
    c.hideUntilRetile();
    f.emit('sourcedata', { sourceId: 'some-other-source', isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('none');
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('visible');
  });

  it('preserves a user-hidden layer (restores to none, never forced visible)', () => {
    const f = makeFakeMap({
      omit: ['style'],
      visibility: {
        'survey-interact-shots': 'visible',
        'survey-interact-stations': 'none', // user hid stations
      },
    });
    const c = new InteractOverlayHideController(f.map, SOURCE, [
      'survey-interact-shots',
      'survey-interact-stations',
    ]);
    c.hideUntilRetile();
    expect(f.visibility['survey-interact-stations']).toBe('none');
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('visible');
    // Was hidden before -> stays hidden after restore.
    expect(f.visibility['survey-interact-stations']).toBe('none');
  });

  // #576 W38-NF-1: the Stations / Shots toggles write these layers too.
  it('restores a visibility set during the hide, not the one it captured', () => {
    const f = makeFakeMap({
      omit: ['style'],
      visibility: { 'survey-interact-shots': 'visible', 'survey-interact-stations': 'none' },
    });
    const c = new InteractOverlayHideController(f.map, SOURCE, [
      'survey-interact-shots',
      'survey-interact-stations',
    ]);
    c.hideUntilRetile();
    c.setLayerVisibility('survey-interact-shots', 'none');
    c.setLayerVisibility('survey-interact-stations', 'visible');
    // #576 W38-NF-2: the hide still holds both until the re-tile.
    expect(f.visibility['survey-interact-stations']).toBe('none');
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('none');
    expect(f.visibility['survey-interact-stations']).toBe('visible');
    // With no hide pending it is a plain write.
    c.setLayerVisibility('survey-interact-shots', 'visible');
    expect(f.visibility['survey-interact-shots']).toBe('visible');
  });

  it('skips layers that do not exist', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, [
      'survey-interact-shots',
      'survey-interact-stations', // absent
    ]);
    c.hideUntilRetile();
    expect(f.visibility['survey-interact-shots']).toBe('none');
    expect(f.visibility['survey-interact-stations']).toBeUndefined();
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('visible');
  });

  it('restores via the fallback timer if the loaded event never arrives', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots']);
    c.hideUntilRetile();
    expect(f.visibility['survey-interact-shots']).toBe('none');
    vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS);
    expect(f.visibility['survey-interact-shots']).toBe('visible');
    expect(f.listenerCount('sourcedata')).toBe(0);
  });

  it('coalesces a second hide while active without clobbering the captured visibility', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots']);
    c.hideUntilRetile();
    // A second removal flush lands while still hidden - must not capture 'none' as
    // the baseline (which would strand the layer hidden after restore).
    c.hideUntilRetile();
    expect(f.listenerCount('sourcedata')).toBe(1);
    f.emit('sourcedata', { sourceId: SOURCE, isSourceLoaded: true });
    expect(f.visibility['survey-interact-shots']).toBe('visible');
  });

  it('dispose restores hidden layers and drops the listener', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots']);
    c.hideUntilRetile();
    c.dispose();
    expect(f.visibility['survey-interact-shots']).toBe('visible');
    expect(f.listenerCount('sourcedata')).toBe(0);
  });

  it('is a no-op to restore when never hidden', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots']);
    c.restore();
    expect(f.visibility['survey-interact-shots']).toBe('visible');
    expect(f.listenerCount('sourcedata')).toBe(0);
  });

  // #576 D16: the map is going away (lightweight unmount) - forget the restore
  // without touching the map, and leave nothing behind to fire later.
  it('drop forgets the pending restore without touching the map', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots']);
    c.hideUntilRetile();
    const setLayout = vi.spyOn(f.map, 'setLayoutProperty');
    c.drop();
    expect(vi.getTimerCount()).toBe(0);
    expect(f.listenerCount('sourcedata')).toBe(0);
    vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS + 1);
    expect(setLayout).not.toHaveBeenCalled();
  });

  // Backstop: a restore that lands once the map has no style (removed, or a lost
  // WebGL context) must not read it - MapLibre's getLayer throws there.
  it('a restore with no live style does not touch the map', () => {
    const f = makeFakeMap({ omit: ['style'], visibility: { 'survey-interact-shots': 'visible' } });
    let live = true;
    const c = new InteractOverlayHideController(f.map, SOURCE, ['survey-interact-shots'], () => live);
    c.hideUntilRetile();
    live = false;
    const getLayer = vi.spyOn(f.map, 'getLayer').mockImplementation(() => {
      throw new TypeError("Cannot read properties of undefined (reading 'getLayer')");
    });
    expect(() => vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS)).not.toThrow();
    expect(getLayer).not.toHaveBeenCalled();
    expect(f.listenerCount('sourcedata')).toBe(0);
  });
});

// #301: a dim-set change must hide the overlay across its ~250ms re-tile ONLY
// when what is actually painted changes. setAnchoredSections /
// setImmovableFeatures fire on every tool switch, usually with empty sets, so an
// unconditional hide would blank the highlight and add flicker where there is none.
describe('overlayPaintDigest', () => {
  const shot = (props: Record<string, unknown>): GeoJSON.Feature => ({
    type: 'Feature',
    geometry: { type: 'LineString', coordinates: [[0, 0], [1, 1]] },
    properties: props,
  });

  it('is equal for an unchanged build, so no hide is triggered', () => {
    const a = [shot({ kind: 'shot', opacity: 1, color: '#fff' })];
    const b = [shot({ kind: 'shot', opacity: 1, color: '#fff' })];
    expect(overlayPaintDigest(a)).toBe(overlayPaintDigest(b));
  });

  it('differs when a feature dims', () => {
    expect(overlayPaintDigest([shot({ kind: 'shot', opacity: 1 })])).not.toBe(
      overlayPaintDigest([shot({ kind: 'shot', opacity: 0.25 })]),
    );
  });

  it('differs when a feature is dropped entirely (hover branch gated out)', () => {
    expect(overlayPaintDigest([shot({ kind: 'shot', opacity: 1 })])).not.toBe(
      overlayPaintDigest([]),
    );
  });

  it('is empty for an empty build', () => {
    expect(overlayPaintDigest([])).toBe('');
  });

  // The properties are what the paint expressions read, so two features that
  // differ only in geometry (a drag offset) are not a dim change.
  it('ignores geometry, which no dim rule depends on', () => {
    const a: GeoJSON.Feature = {
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: [[0, 0], [1, 1]] },
      properties: { kind: 'shot', opacity: 1 },
    };
    const b: GeoJSON.Feature = {
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: [[9, 9], [8, 8]] },
      properties: { kind: 'shot', opacity: 1 },
    };
    expect(overlayPaintDigest([a])).toBe(overlayPaintDigest([b]));
  });

  it('distinguishes order, so a swapped emission is a real change', () => {
    const a = shot({ kind: 'shot', opacity: 1 });
    const b = shot({ kind: 'station', opacity: 0.25 });
    expect(overlayPaintDigest([a, b])).not.toBe(overlayPaintDigest([b, a]));
  });
});
