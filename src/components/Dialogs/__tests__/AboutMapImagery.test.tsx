/**
 * Component tests for the About dialog's imagery credit.
 *
 * This is the other half of the Esri split. "Powered by Esri" is allowed to live
 * "in a window that is accessible from a menu or button" rather than on the map,
 * and About is that window - so this block is what makes leaving the notice off
 * the map legitimate rather than merely convenient. If it stops rendering, the
 * on-map overlay alone no longer covers the requirement.
 * https://developers.arcgis.com/documentation/esri-and-data-attribution/interactive-maps/
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../../utils/ipc');

import '../../../i18n';
import { AboutMapImagery } from '../AboutMapImagery';
import { resetTileAttributionCache } from '../../../hooks/useTileAttribution';
import { invokeIpcMock as invokeIpc } from '../../../test/factories/ipc';

const SOURCES = 'Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community';

beforeEach(() => {
  resetTileAttributionCache();
  invokeIpc.mockReset();
  invokeIpc.mockResolvedValue({
    provider: 'esri',
    text: SOURCES,
    notice: 'Powered by Esri',
    notice_url: 'https://www.esri.com',
    live: false,
  });
});

describe('AboutMapImagery', () => {
  it('credits the provider notice with a link to it', async () => {
    render(<AboutMapImagery />);
    const link = await screen.findByTestId('about-imagery-notice-link');
    expect(link).toHaveTextContent('Powered by Esri');
    expect(link).toHaveAttribute('href', 'https://www.esri.com');
  });

  it('opens the provider link externally rather than navigating the app', async () => {
    // The webview would otherwise replace the app itself with esri.com, with no
    // way back. Mirrors the website link directly above it in About.
    const open = vi.fn();
    vi.stubGlobal('open', open);
    render(<AboutMapImagery />);

    fireEvent.click(await screen.findByTestId('about-imagery-notice-link'));
    expect(open).toHaveBeenCalledWith('https://www.esri.com', '_blank');
    vi.unstubAllGlobals();
  });

  it('repeats the source names so they are discoverable off the map', async () => {
    render(<AboutMapImagery />);
    // Verbatim: the provider's string is not ours to reword or reframe.
    expect(await screen.findByTestId('about-imagery-sources')).toHaveTextContent(SOURCES);
  });

  it('renders nothing for a provider with nothing to credit', async () => {
    invokeIpc.mockResolvedValue({
      provider: 'nothing',
      text: '',
      notice: null,
      notice_url: null,
      live: false,
    });
    render(<AboutMapImagery provider="nothing" />);
    await waitFor(() => expect(invokeIpc).toHaveBeenCalled());
    expect(screen.queryByTestId('about-map-imagery')).not.toBeInTheDocument();
  });
});
