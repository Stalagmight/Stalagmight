/**
 * Component tests for the on-map Esri attribution.
 *
 * The defect these guard is the one that shipped: three MapLibre instances built
 * with `attributionControl: false`, so imagery drew with no attribution at all.
 * The obligations under test come from
 * https://developers.arcgis.com/documentation/esri-and-data-attribution/interactive-maps/
 *
 *   - the source names are visible for as long as the imagery is drawn
 *   - they may collapse behind an expandable affordance when they do not fit
 *   - they render offline, because the fallback is what the field sees
 *   - "Powered by Esri" is NOT here (it belongs in About, which is allowed)
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');

import '../../../i18n';
import { MapAttribution, primarySourceName } from '../MapAttribution';
import { resetTileAttributionCache } from '../../../hooks/useTileAttribution';
import { invokeIpcMock as invokeIpc } from '../../../test/factories/ipc';

const FALLBACK = 'Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community';
const LIVE = 'Source: Esri, Somebody New, and the GIS User Community';

/**
 * Force the auto variant one way or the other. `MapAttribution` measures the
 * element it is positioned over, and jsdom reports 0 for every dimension, which
 * would silently pin every test to the compact branch.
 */
function setHostWidth(width: number) {
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get() {
      return width;
    },
  });
}

beforeEach(() => {
  resetTileAttributionCache();
  invokeIpc.mockReset();
  invokeIpc.mockResolvedValue({
    provider: 'esri',
    text: LIVE,
    notice: 'Powered by Esri',
    notice_url: 'https://www.esri.com',
    live: true,
  });
  setHostWidth(1200);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('MapAttribution', () => {
  it('renders the source names while imagery is drawn', async () => {
    render(<MapAttribution visible />);
    expect(await screen.findByTestId('map-attribution-text')).toHaveTextContent(LIVE);
  });

  it('renders nothing when the imagery is not drawn', () => {
    // The obligation tracks the layer, not the map: satellite off means there is
    // nothing to attribute, and a stale credit over a blank map is wrong.
    render(<MapAttribution visible={false} />);
    expect(screen.queryByTestId('map-attribution')).not.toBeInTheDocument();
  });

  it('does not contact the provider when the imagery is not drawn', () => {
    // The fetch reaches Esri's servers. Opening a project with satellite off
    // must not phone a third party for a credit that will never be shown - this
    // app tells its users it does not touch the network unasked.
    render(<MapAttribution visible={false} />);
    expect(invokeIpc).not.toHaveBeenCalled();
  });

  it('shares one call between surfaces mounted together', async () => {
    // The only thing the in-flight map is for: the survey map, Global View and
    // a Section Preview opening at once must not each pay for the same string.
    render(
      <>
        <MapAttribution visible />
        <MapAttribution visible />
        <MapAttribution visible />
      </>,
    );
    await waitFor(() => expect(screen.getAllByTestId('map-attribution-text')).toHaveLength(3));
    expect(invokeIpc).toHaveBeenCalledTimes(1);
  });

  it('asks again on a later mount, so the backend stays in charge of caching', async () => {
    // Nothing settled is held here. The backend is the only side that knows
    // whether asking costs a network call - it holds a live string for 24h and
    // will not retry a failure for 5 minutes. Keeping settled answers in the
    // renderer put both clocks out of reach: a fallback pinned the compiled-in
    // names for the session, and a live one pinned a day-old string in any
    // session outliving the backend's TTL. Which is the drift this feature
    // exists to prevent - the built-in list said "Maxar" long after that
    // company was renamed.
    const first = render(<MapAttribution visible />);
    expect(await screen.findByTestId('map-attribution-text')).toHaveTextContent(LIVE);
    expect(invokeIpc).toHaveBeenCalledTimes(1);
    first.unmount();

    render(<MapAttribution visible />);
    expect(await screen.findByTestId('map-attribution-text')).toHaveTextContent(LIVE);
    expect(invokeIpc).toHaveBeenCalledTimes(2);
  });

  it('asks again after a fallback too', async () => {
    // The same rule, on the branch that used to be special-cased. `live: false`
    // is what the backend answers when its OWN fetch failed, and that is a
    // RESOLVED promise - so this is the case a naive memo would have pinned.
    invokeIpc.mockResolvedValueOnce({
      provider: 'esri',
      text: FALLBACK,
      notice: null,
      notice_url: null,
      live: false,
    });

    const first = render(<MapAttribution visible />);
    await waitFor(() => expect(invokeIpc).toHaveBeenCalledTimes(1));
    first.unmount();

    render(<MapAttribution visible />);
    expect(await screen.findByTestId('map-attribution-text')).toHaveTextContent(LIVE);
    expect(invokeIpc).toHaveBeenCalledTimes(2);
  });

  it('contacts the provider once the imagery is switched on', async () => {
    const { rerender } = render(<MapAttribution visible={false} />);
    expect(invokeIpc).not.toHaveBeenCalled();

    rerender(<MapAttribution visible />);
    expect(await screen.findByTestId('map-attribution-text')).toHaveTextContent(LIVE);
    expect(invokeIpc).toHaveBeenCalledWith(
      'get_tile_attribution',
      { provider: 'esri' },
      expect.anything(),
    );
  });

  it('shows the compiled-in names before the live value arrives', () => {
    let resolve!: (v: unknown) => void;
    invokeIpc.mockReturnValue(new Promise((r) => { resolve = r; }));

    render(<MapAttribution visible />);

    // Imagery is already on screen at this point, so "attribution pending" is
    // not an acceptable state to render.
    expect(screen.getByTestId('map-attribution-text')).toHaveTextContent(FALLBACK);
    act(() => resolve({ provider: 'esri', text: LIVE, notice: null, notice_url: null, live: true }));
  });

  it('keeps the compiled-in names when the backend call fails', async () => {
    // The offline case, which is the normal case in a cave.
    invokeIpc.mockRejectedValue(new Error('offline'));
    render(<MapAttribution visible />);
    await waitFor(() =>
      expect(screen.getByTestId('map-attribution-text')).toHaveTextContent(FALLBACK),
    );
  });

  it('keeps the compiled-in names when the backend answers with an empty string', async () => {
    invokeIpc.mockResolvedValue({
      provider: 'esri',
      text: '',
      notice: null,
      notice_url: null,
      live: true,
    });
    render(<MapAttribution visible />);
    await waitFor(() =>
      expect(screen.getByTestId('map-attribution-text')).toHaveTextContent(FALLBACK),
    );
  });

  it('does not put the "Powered by Esri" notice on the map', async () => {
    render(<MapAttribution visible />);
    await screen.findByTestId('map-attribution-text');
    // Esri allows that notice behind a menu or button, and the About dialog
    // carries it. Map space goes to what the licence actually requires there.
    expect(screen.queryByText(/Powered by Esri/)).not.toBeInTheDocument();
  });

  describe('chrome', () => {
    it('takes its colours from the theme rather than a fixed palette', async () => {
      render(<MapAttribution visible />);
      const chip = await screen.findByTestId('map-attribution');
      // This shipped with Esri's recommended #323232-on-white applied inline,
      // which punched a white card into the dark theme. The durable guard is
      // that no colour is hard-coded at all: the tokens resolve per theme.
      expect(chip.style.color).toBe('');
      expect(chip.style.backgroundColor).toBe('');
      expect(chip.className).toContain('bg-panel-90');
      expect(chip.className).toContain('text-text-secondary');
    });

    it('rounds only its top-left corner', async () => {
      // It is a bottom-flush bar butted against the map edge and the Data dock
      // tab, not a floating card - a fully rounded chip reads as detached, and
      // top-left is the sole corner not meeting something.
      render(<MapAttribution visible />);
      const chip = await screen.findByTestId('map-attribution');
      // Token-wise, not substring: `rounded-tl` contains `rounded`.
      const rounding = [...chip.classList].filter((c) => c.startsWith('rounded'));
      expect(rounding).toEqual(['rounded-tl']);
    });
  });

  describe('compact affordance', () => {
    it('collapses when the map is too narrow for the names', async () => {
      setHostWidth(300);
      render(<MapAttribution visible />);
      expect(await screen.findByTestId('map-attribution-toggle')).toBeInTheDocument();
      expect(screen.queryByTestId('map-attribution-text')).not.toBeInTheDocument();
    });

    it('collapses for a provider with no compiled-in fallback, once its names arrive', async () => {
      // Only `esri` is in the compiled-in table, so any other provider renders
      // nothing until the IPC answers. The sizing observer is installed by an
      // effect that early-returns while there is no element to measure, so it
      // has to re-run when the names land - otherwise the wide bar renders on a
      // host far too narrow for it.
      setHostWidth(300);
      invokeIpc.mockResolvedValue({
        provider: 'other',
        text: 'Source: Somebody Else',
        notice: null,
        notice_url: null,
        live: true,
      });
      render(<MapAttribution visible provider="other" />);

      expect(await screen.findByTestId('map-attribution-toggle')).toBeInTheDocument();
      expect(screen.queryByTestId('map-attribution-text')).not.toBeInTheDocument();
    });

    it('is forced regardless of width for small surfaces', async () => {
      // The Section Preview passes variant="compact": it is a few hundred pixels
      // square and exists to show the geometry underneath.
      setHostWidth(1200);
      render(<MapAttribution visible variant="compact" />);
      expect(await screen.findByTestId('map-attribution-toggle')).toBeInTheDocument();
    });

    it('opens the names only after the pointer dwells', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      render(<MapAttribution visible variant="compact" />);
      const toggle = await screen.findByTestId('map-attribution-toggle');

      fireEvent.mouseEnter(toggle.parentElement as HTMLElement);
      // A cursor crossing the corner of a map must not flash the panel open.
      expect(screen.queryByTestId('map-attribution-panel')).not.toBeInTheDocument();

      act(() => { vi.advanceTimersByTime(400); });
      expect(screen.getByTestId('map-attribution-panel')).toHaveTextContent(LIVE);
    });

    it('pins open on click, for pointers that never hover', async () => {
      // `userEvent`, not `fireEvent`: a browser focuses a button on mouse-down,
      // so the real sequence is focus THEN click. A bare `fireEvent.click`
      // dispatches no focus and so cannot see a focus/click handler pair that
      // cancel each other out - which is exactly the bug this once hid, leaving
      // the names unreachable by tap.
      const user = userEvent.setup();
      render(<MapAttribution visible variant="compact" />);
      const toggle = await screen.findByTestId('map-attribution-toggle');

      await user.click(toggle);
      expect(screen.getByTestId('map-attribution-panel')).toHaveTextContent(LIVE);
      expect(toggle).toHaveAttribute('aria-expanded', 'true');

      await user.click(toggle);
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      // Unhover before asserting the panel is gone: the pointer is still parked
      // on the toggle after a click, and hover opens it again on a 350ms REAL
      // timer. Asserting absence without this races that timer on a loaded box.
      await user.unhover(toggle);
      expect(screen.queryByTestId('map-attribution-panel')).not.toBeInTheDocument();
    });

    it('closes a pinned panel when the pointer goes elsewhere', async () => {
      // Dropping the button's `onFocus` (which broke click-to-open) took its
      // `onBlur` with it, and that was the only outside-dismiss path - a pinned
      // panel then sat over the map until someone found Escape.
      const user = userEvent.setup();
      render(
        <div>
          <MapAttribution visible variant="compact" />
          <button type="button" data-testid="elsewhere">elsewhere</button>
        </div>,
      );
      const toggle = await screen.findByTestId('map-attribution-toggle');

      await user.click(toggle);
      expect(toggle).toHaveAttribute('aria-expanded', 'true');

      await user.click(screen.getByTestId('elsewhere'));
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      expect(screen.queryByTestId('map-attribution-panel')).not.toBeInTheDocument();
    });

    it('reaches the names from the keyboard', async () => {
      // Tab to it, press Enter. A <button> turns Enter/Space into a click, so
      // the keyboard route is the click route - no separate focus handling, and
      // nothing for a click to fight with.
      const user = userEvent.setup();
      render(<MapAttribution visible variant="compact" />);
      const toggle = await screen.findByTestId('map-attribution-toggle');

      await user.tab();
      expect(toggle).toHaveFocus();
      // Focus alone must NOT open it; that is what used to cancel the click.
      expect(screen.queryByTestId('map-attribution-panel')).not.toBeInTheDocument();

      await user.keyboard('{Enter}');
      expect(screen.getByTestId('map-attribution-panel')).toHaveTextContent(LIVE);
    });

    it('closes a pinned panel on Escape', async () => {
      const user = userEvent.setup();
      render(<MapAttribution visible variant="compact" />);
      const toggle = await screen.findByTestId('map-attribution-toggle');

      await user.click(toggle);
      expect(screen.getByTestId('map-attribution-panel')).toBeInTheDocument();

      fireEvent.keyDown(window, { key: 'Escape' });
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      await user.unhover(toggle);
      expect(screen.queryByTestId('map-attribution-panel')).not.toBeInTheDocument();
    });

    it('names the primary source rather than showing a bare i', async () => {
      // A lone `i` credits nobody. The collapsed chip stays an attribution by
      // naming the first source, with the rest of the list in the panel.
      render(<MapAttribution visible variant="compact" />);
      expect(await screen.findByTestId('map-attribution-label')).toHaveTextContent('by Esri');
    });

    it('falls back to the bare i when no source name can be read', async () => {
      invokeIpc.mockResolvedValue({
        provider: 'esri',
        text: 'Sources:',
        notice: null,
        notice_url: null,
        live: true,
      });
      render(<MapAttribution visible variant="compact" />);
      // "Sources:" parses to nothing; inventing a name would be worse than the
      // fallback list, which is what actually renders here.
      const toggle = await screen.findByTestId('map-attribution-toggle');
      expect(toggle).toBeInTheDocument();
    });

    it('carries the names on the button itself, so they survive a missed hover', async () => {
      render(<MapAttribution visible variant="compact" />);
      const toggle = await screen.findByTestId('map-attribution-toggle');
      expect(toggle).toHaveAttribute('title', LIVE);
      expect(toggle).toHaveAccessibleName();
    });
  });

  describe('primarySourceName', () => {
    it('reads the first source out of the live Esri string', () => {
      expect(primarySourceName(LIVE)).toBe('Esri');
      expect(primarySourceName(FALLBACK)).toBe('Esri');
    });

    it('tolerates either prefix spelling and none at all', () => {
      // Both appear across ArcGIS services, and a service may use neither.
      expect(primarySourceName('Sources: Esri, TomTom, Garmin')).toBe('Esri');
      expect(primarySourceName('source:Esri, TomTom')).toBe('Esri');
      expect(primarySourceName('Esri, TomTom')).toBe('Esri');
    });

    it('yields nothing rather than a stray fragment', () => {
      expect(primarySourceName('')).toBe('');
      expect(primarySourceName('Source:')).toBe('');
      expect(primarySourceName('   ')).toBe('');
    });
  });

  it('asks the backend once no matter how many surfaces are mounted', async () => {
    // All three map surfaces can be on screen together; that must not mean three
    // round trips for one string.
    render(
      <>
        <MapAttribution visible />
        <MapAttribution visible />
        <MapAttribution visible variant="compact" />
      </>,
    );
    await screen.findAllByTestId('map-attribution');
    expect(invokeIpc).toHaveBeenCalledTimes(1);
    expect(invokeIpc).toHaveBeenCalledWith(
      'get_tile_attribution',
      { provider: 'esri' },
      expect.anything(),
    );
  });
});
