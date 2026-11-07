/**
 * Fakes for the Global View map's collaborators (#576 E15): a `maplibre-gl`
 * module (Map, Marker, Popup, LngLatBounds, the controls) and the WebGL
 * `GlobalSurveyLayer`, both recording into `fakeGlobalView.state`. They let the
 * real GlobalViewMap and globalViewStore run under jsdom. A test mocks both
 * modules with them and drives the map through the state:
 *
 *     vi.mock('maplibre-gl', async () => (await import('<rel>/test/factories/globalViewMap')).maplibreModule);
 *     vi.mock('../GlobalSurveyLayer', async () =>
 *       (await import('<rel>/test/factories/globalViewMap')).globalSurveyLayerModule);
 *     import { fakeGlobalView } from '<rel>/test/factories/globalViewMap';
 *     ...
 *     act(() => fakeGlobalView.state.map?.fire('load'));
 *
 * The state lives as long as the test file (each file gets its own module
 * instance); reset the fields a test reads in `beforeEach`.
 */
type Handler = (e?: unknown) => void;

const state = {
  /** The last map the component constructed. */
  map: null as FakeMap | null,
  /** How many markers have been constructed. */
  markersCreated: 0,
  /** The bounds of every `fitBounds` call, as west/east longitudes. */
  fits: [] as { west: number; east: number }[],
  /** The file paths of the systems each `GlobalSurveyLayer.update` call received. */
  updates: [] as string[][],
};

class FakeMap {
  private handlers = new Map<string, Set<Handler>>();
  private container: HTMLElement;
  dragRotate = { disable() {} };
  touchPitch = { disable() {} };
  touchZoomRotate = { disableRotation() {} };
  keyboard = { disable() {} };
  doubleClickZoom = { disable() {} };
  constructor(opts: { container: HTMLElement }) {
    this.container = opts.container;
    state.map = this;
  }
  addControl() {}
  on(ev: string, fn: Handler) {
    if (!this.handlers.has(ev)) this.handlers.set(ev, new Set());
    this.handlers.get(ev)!.add(fn);
  }
  off(ev: string, fn: Handler) {
    this.handlers.get(ev)?.delete(fn);
  }
  once() {}
  /** Call every handler registered for `ev`. */
  fire(ev: string) {
    for (const fn of [...(this.handlers.get(ev) ?? [])]) fn({});
  }
  getBounds() {
    return { contains: () => true, getCenter: () => ({ lng: 0, lat: 0 }) };
  }
  getCenter() {
    return { lng: 0, lat: 0 };
  }
  getZoom() {
    return 10;
  }
  getContainer() {
    return this.container;
  }
  getCanvas() {
    return document.createElement('canvas');
  }
  getSource() {
    return undefined;
  }
  fitBounds(b: FakeBounds) {
    state.fits.push({ west: b.getSouthWest().lng, east: b.getNorthEast().lng });
  }
  easeTo() {}
  flyTo() {}
  setMaxBounds() {}
  setMinZoom() {}
  triggerRepaint() {}
  remove() {}
}

class FakeMarker {
  private el: HTMLElement;
  constructor(opts: { element: HTMLElement }) {
    this.el = opts.element;
    state.markersCreated += 1;
  }
  setLngLat() {
    return this;
  }
  addTo() {
    document.body.appendChild(this.el);
    return this;
  }
  remove() {
    this.el.remove();
  }
  getElement() {
    return this.el;
  }
}

class FakePopup {
  setLngLat() {
    return this;
  }
  setDOMContent() {
    return this;
  }
  addTo() {
    return this;
  }
  remove() {}
}

class FakeBounds {
  constructor(
    private sw: [number, number],
    private ne: [number, number],
  ) {}
  getSouthWest() {
    return { lng: this.sw[0], lat: this.sw[1] };
  }
  getNorthEast() {
    return { lng: this.ne[0], lat: this.ne[1] };
  }
  getCenter() {
    return { lng: (this.sw[0] + this.ne[0]) / 2, lat: (this.sw[1] + this.ne[1]) / 2 };
  }
}

class Noop {}

class FakeGlobalSurveyLayer {
  update(systems: { metadata: { file_path: string } }[]) {
    state.updates.push(systems.map((s) => s.metadata.file_path));
  }
  setVisible() {}
  setSelectedSystems() {}
  applyThemeColors() {}
  setPrecision() {}
  setSearchFilter() {}
  destroy() {}
}

export const fakeGlobalView = { state, FakeMap, FakeMarker, FakePopup, FakeBounds, Noop };

/** What `vi.mock('maplibre-gl', ...)` resolves to. maplibre-gl 6 is ESM-only
 *  with named exports (`import * as maplibregl`), so there is no `default`. */
export const maplibreModule = {
  Map: FakeMap,
  Marker: FakeMarker,
  Popup: FakePopup,
  LngLatBounds: FakeBounds,
  NavigationControl: Noop,
  ScaleControl: Noop,
};

/** What `vi.mock('<rel>/GlobalSurveyLayer', ...)` resolves to. */
export const globalSurveyLayerModule = {
  default: FakeGlobalSurveyLayer,
  generateSystemColor: () => '#336699',
};
