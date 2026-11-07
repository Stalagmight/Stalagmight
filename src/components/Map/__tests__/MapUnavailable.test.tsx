import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import en from '../../../i18n/locales/en.json';

/**
 * MapLibre 6 throws `GPUInitializationError` from the `Map` constructor when no
 * WebGL2 context exists (#606). The map host must show the WebGL2 message in
 * place of the map instead of letting the throw escape its effect.
 */

const h = vi.hoisted(() => {
  class GPUInitializationError extends Error {
    readonly statusMessage: string | null = null;
  }
  const state = { error: new GPUInitializationError('no webgl2') as Error };
  class FakeMap {
    constructor() {
      throw state.error;
    }
  }
  return { GPUInitializationError, FakeMap, state };
});

vi.mock('maplibre-gl', async (importOriginal) => ({
  ...(await importOriginal<typeof import('maplibre-gl')>()),
  Map: h.FakeMap,
  GPUInitializationError: h.GPUInitializationError,
}));

import '../../../i18n';
import SectionPreview from '../../Dialogs/SectionPreview';
import GlobalViewMap from '../../GlobalView/GlobalViewMap';
import MapCanvas from '../MapCanvas';
import type { ProjectMetadata } from '../../../types/project';
import { createMap } from '../createMap';

describe('map without WebGL2', () => {
  beforeEach(() => {
    h.state.error = new h.GPUInitializationError('no webgl2');
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  function expectMessage(testId: string): void {
    const status = screen.getByTestId(testId);
    expect(status).toHaveTextContent(en.map.webgl2Unavailable.title);
    expect(status).toHaveTextContent(en.map.webgl2Unavailable.body);
  }

  it('shows the WebGL2 message in place of the section preview', () => {
    render(<SectionPreview />);
    expectMessage('section-preview-webgl2-unavailable');
    expect(screen.getByTestId('section-preview')).toBeInTheDocument();
  });

  it('shows the WebGL2 message in place of the survey map', () => {
    render(<MapCanvas />);
    expectMessage('map-webgl2-unavailable');
    expect(screen.getByTestId('map-canvas')).toBeInTheDocument();
  });

  it('shows the WebGL2 message in place of the Global View map', () => {
    const project: ProjectMetadata = {
      name: 'a',
      location: null,
      file_path: 'a.stalx',
      modified_at: '2026-01-01T00:00:00Z',
      section_count: 1,
      total_length: 100,
      station_count: 2,
      gps_center: [-87.4, 20.2],
      gps_bounds: null,
      is_remote: false,
    };
    render(
      <GlobalViewMap
        projects={[project]}
        selectedProjectId={null}
        onProjectSelect={() => {}}
        onProjectOpen={() => {}}
      />,
    );
    expectMessage('globalview-webgl2-unavailable');
    expect(screen.getByTestId('globalview-map')).toBeInTheDocument();
  });

  it('createMap returns null only for the WebGL2 error and rethrows anything else', () => {
    const container = document.createElement('div');
    expect(createMap({ container })).toBeNull();
    h.state.error = new Error('boom');
    expect(() => createMap({ container })).toThrow('boom');
  });
});
