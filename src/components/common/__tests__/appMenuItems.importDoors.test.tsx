/**
 * The two hamburger doors, pinned by the picker each one opens (#508).
 *
 * "Open / Import" and "Import" are one word apart and route through the same
 * `routeOpenedFile`, so nothing downstream distinguishes them - the ONLY thing
 * that does is the filter set each hands to the picker. That makes these two
 * assertions the whole of the behavior: Open offers everything the app can open
 * or import, Import offers `.dmp` and nothing else (the one format that is
 * additive to an open project; every other one can only close it and start a
 * new project, which is Open's job).
 *
 * Pinned here rather than in an e2e because the mock picker returns a fixed
 * path regardless of the filters it is handed - an e2e click would pass just as
 * happily with the filters regrown.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const pickAndRouteMock = vi.fn();
vi.mock('../../../utils/openOrImport', () => ({
  pickAndRoute: (...args: unknown[]) => pickAndRouteMock(...args),
}));

import { importMenuItem, leadingMenuItems } from '../appMenuItems';
import { openOrImportFilters, inProjectImportFilters } from '../../../utils/fileDialogs';

const t = ((key: string) => key) as unknown as Parameters<typeof importMenuItem>[0];

beforeEach(() => {
  pickAndRouteMock.mockClear();
});

describe('hamburger import doors (#508)', () => {
  it('the in-project Import row picks .dmp/.stalx and flags itself as an import door', () => {
    const item = importMenuItem(t);
    expect(item.submenu).toBeUndefined();
    (item.onClick as () => void)();
    expect(pickAndRouteMock).toHaveBeenCalledWith(inProjectImportFilters(), 'Import Failed', { importDoor: true });
    // Pin the extension set itself, not just "equals the helper" - a helper
    // that quietly regrew .tml entries would still satisfy the line
    // above while re-offering formats Import cannot apply to an open project.
    const [filters] = pickAndRouteMock.mock.calls[0]!;
    expect(
      new Set((filters as Array<{ extensions: string[] }>).flatMap((f) => f.extensions)),
    ).toEqual(new Set(['dmp', 'stalx']));
  });

  it('the Open / Import row keeps the full unified picker', () => {
    const open = leadingMenuItems(t, { onNewProject: () => {} }, 'hamburger').find(
      (i) => i.id === 'global-open-project-button',
    );
    (open?.onClick as () => void)();
    expect(pickAndRouteMock).toHaveBeenCalledWith(openOrImportFilters(), 'Open Failed');
    // Literal set, for the same reason the Import row pins one: comparing
    // against the helper passes vacuously if the helper SHRINKS, which is the
    // regression that would quietly make a format unopenable app-wide.
    const [filters] = pickAndRouteMock.mock.calls[0]!;
    const exts = new Set((filters as Array<{ extensions: string[] }>).flatMap((f) => f.extensions));
    for (const ext of ['stalx', 'dmp', 'tml', 'tmlu', 'mak', 'dat', 'wpj', 'srv', 'gpx']) {
      expect(exts).toContain(ext);
    }
  });
});
