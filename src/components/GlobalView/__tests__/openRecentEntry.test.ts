/**
 * #450: recents opens (map, list/table, keyboard shortcut) must attribute the
 * telemetry source BEFORE routing, and must route with `allow: 'project-only'`
 * so a persisted recents entry - untrusted input, could carry any extension -
 * can never reach the import parsers (#450 security review).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const routeOpenedFileMock = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock('../../../utils/openOrImport', () => ({
  routeOpenedFile: routeOpenedFileMock,
}));

const setProjectActionSourceMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryStubs({
    setProjectActionSource: setProjectActionSourceMock,
  }),
);

import { openRecentEntry } from '../openRecentEntry';

beforeEach(() => {
  routeOpenedFileMock.mockClear();
  setProjectActionSourceMock.mockClear();
});

describe('openRecentEntry', () => {
  it('attributes the source before routing, with the project-only gate', async () => {
    const callOrder: string[] = [];
    setProjectActionSourceMock.mockImplementation(() => callOrder.push('attribute'));
    routeOpenedFileMock.mockImplementation(() => {
      callOrder.push('route');
      return Promise.resolve();
    });

    await openRecentEntry('/projects/cave-alpha.stalx', 'recent-list');

    expect(setProjectActionSourceMock).toHaveBeenCalledWith('recent-list');
    expect(routeOpenedFileMock).toHaveBeenCalledWith(
      '/projects/cave-alpha.stalx',
      'Open Failed',
      { allow: 'project-only' },
    );
    expect(callOrder).toEqual(['attribute', 'route']);
  });

  it('tags the map source distinctly from the list source', async () => {
    await openRecentEntry('/projects/cave-alpha.stalx', 'recent-map');
    expect(setProjectActionSourceMock).toHaveBeenCalledWith('recent-map');
  });
});
