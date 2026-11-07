/**
 * [#576 Q-W22S-1-D1] `storedDisplayValue` reads a stored display row the way the
 * backend builds a load's `display_settings` (`core/loading.rs`
 * `DisplaySettings::from_stored`): its default when no row holds it, the older keys
 * a file may hold it under, and a grid spacing / geometry mode it cannot read falls
 * back to 10 / "measured". All data is synthetic.
 */
import { describe, expect, it } from 'vitest';

import { storedDisplayValue } from '../storedDisplaySettings';

describe('storedDisplayValue mirrors DisplaySettings::from_stored', () => {
  it('reads a boolean row as "true" or not, with its default when absent', () => {
    expect(storedDisplayValue({ showGrid: 'true' }, 'showGrid')).toBe(true);
    expect(storedDisplayValue({ showGrid: 'false' }, 'showGrid')).toBe(false);
    expect(storedDisplayValue({ showGrid: 'yes' }, 'showGrid')).toBe(false);
    expect(storedDisplayValue({}, 'showGrid')).toBe(false);
    expect(storedDisplayValue({}, 'showStations')).toBe(true);
    expect(storedDisplayValue({ showStations: 'false' }, 'showStations')).toBe(false);
    expect(storedDisplayValue({}, 'showDepthGradient')).toBe(false);
  });

  it('falls back through the older keys for reference shots and jumps', () => {
    expect(storedDisplayValue({ showVirtualShots: 'false' }, 'showReferenceShots')).toBe(false);
    expect(storedDisplayValue({ showVirtualShots: 'false' }, 'showJumps')).toBe(false);
    expect(storedDisplayValue({ showReferenceShots: 'false' }, 'showJumps')).toBe(false);
    expect(storedDisplayValue({ showJumps: 'true', showReferenceShots: 'false' }, 'showJumps')).toBe(true);
    expect(storedDisplayValue({}, 'showJumps')).toBe(true);
  });

  it('reads the grid spacing as a number, 10 otherwise', () => {
    expect(storedDisplayValue({ gridSpacing: '50' }, 'gridSpacing')).toBe(50);
    expect(storedDisplayValue({ gridSpacing: '2.5' }, 'gridSpacing')).toBe(2.5);
    expect(storedDisplayValue({ gridSpacing: '' }, 'gridSpacing')).toBe(10);
    expect(storedDisplayValue({ gridSpacing: 'wide' }, 'gridSpacing')).toBe(10);
    expect(storedDisplayValue({}, 'gridSpacing')).toBe(10);
  });

  it('reads the geometry mode as one of its two values, "measured" otherwise', () => {
    expect(storedDisplayValue({ geometryMode: 'closureAdjusted' }, 'geometryMode')).toBe('closureAdjusted');
    expect(storedDisplayValue({ geometryMode: 'adjusted' }, 'geometryMode')).toBe('measured');
    expect(storedDisplayValue({}, 'geometryMode')).toBe('measured');
  });

  it('has nothing for a key that is not a stored display setting', () => {
    expect(storedDisplayValue({ showDevStats: 'true' }, 'showDevStats')).toBeUndefined();
  });
});
