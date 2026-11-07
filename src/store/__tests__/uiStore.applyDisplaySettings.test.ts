/**
 * applyDisplaySettings restores showShots/showShotComments (#275 B2).
 *
 * Both toggles were always persisted by toggleDisplay via save_display_setting,
 * but the load path never applied them: the Rust DisplaySettings struct omitted
 * them, so the previous project's (or session default) in-memory value silently
 * won over the saved one. These tests pin the restore and the no-bleed behavior.
 */

import { describe, it, expect, beforeEach } from 'vitest';

import { useUIStore } from '../uiStore';
import { DISPLAY_DEFAULTS } from '../displaySelectors';
import type { DisplaySettings } from '../../types/loading';

// Spread the documented single source of truth for toggle defaults instead of
// hand-listing them: a changed default then fails loudly here rather than the
// test silently asserting against a stale copy.
const settingsWith = (overrides: Partial<DisplaySettings>): DisplaySettings => ({
  ...DISPLAY_DEFAULTS,
  geometryMode: 'measured',
  ...overrides,
});

describe('applyDisplaySettings showShots/showShotComments restore', () => {
  beforeEach(() => {
    useUIStore.setState({ showShots: true, showShotComments: false });
  });

  it('applies saved showShots=false and showShotComments=true on load', () => {
    useUIStore
      .getState()
      .applyDisplaySettings(settingsWith({ showShots: false, showShotComments: true }));
    expect(useUIStore.getState().showShots).toBe(false);
    expect(useUIStore.getState().showShotComments).toBe(true);
  });

  it('does not bleed project A values into project B (load is authoritative)', () => {
    // Project A: comments hidden, shots hidden.
    useUIStore
      .getState()
      .applyDisplaySettings(settingsWith({ showShots: false, showShotComments: false }));
    expect(useUIStore.getState().showShots).toBe(false);
    // Project B saved the opposite; its load must win over A's in-memory state.
    useUIStore
      .getState()
      .applyDisplaySettings(settingsWith({ showShots: true, showShotComments: true }));
    expect(useUIStore.getState().showShots).toBe(true);
    expect(useUIStore.getState().showShotComments).toBe(true);
  });

  it('bleed check, true->false direction: a saved false wins over in-memory true', () => {
    useUIStore.setState({ showShotComments: true, showShots: true });
    useUIStore
      .getState()
      .applyDisplaySettings(settingsWith({ showShots: false, showShotComments: false }));
    expect(useUIStore.getState().showShotComments).toBe(false);
    expect(useUIStore.getState().showShots).toBe(false);
  });
});
