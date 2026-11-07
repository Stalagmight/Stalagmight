/**
 * [#579 review] Settings > General "Reset to Defaults" must stage the track
 * default of THIS BUILD, not the one baked into the generated `defaultConfig`
 * snapshot.
 *
 * The snapshot is produced by `pnpm gen:ipc-types` on a developer machine and
 * therefore always says `"stable"`, while `UpdateTrack::default()` in Rust
 * reads `option_env!("STALAGMIGHT_CHANNEL")` - so on a weekly build the real
 * default is `"weekly"`. The per-ROW reset was fixed for this and the
 * category-wide header button was not, so pressing it on a weekly build
 * silently staged a switch to the stable track.
 */
import { describe, it, expect } from 'vitest';

import { getCategoryDefaults } from './SettingsDialog';
import { defaultConfig } from '../../types/config';
import { buildDefaultUpdateTrack } from '../../utils/updateTrack';

describe('getCategoryDefaults for the general category (#579)', () => {
  it('stages the build-derived update track, not the snapshot one', () => {
    const defaults = getCategoryDefaults('general');

    expect(defaults.general?.update_track).toBe(buildDefaultUpdateTrack());
  });

  /**
   * The whole point of the override: on a build whose channel differs from the
   * snapshot's, the reset must NOT stage the snapshot value. Asserted as an
   * implication so it holds on both kinds of build.
   */
  it('never stages the snapshot track when the build disagrees with it', () => {
    const defaults = getCategoryDefaults('general');

    if (buildDefaultUpdateTrack() !== defaultConfig.general.update_track) {
      expect(defaults.general?.update_track).not.toBe(defaultConfig.general.update_track);
    }
    // Whichever build this is, the row-level reset and the category-level reset
    // must agree - they used to disagree by exactly this field.
    expect(defaults.general?.update_track).toBe(buildDefaultUpdateTrack());
  });

  it('still resets every other general field to the snapshot', () => {
    const defaults = getCategoryDefaults('general');

    expect(defaults.general?.update_check_enabled).toBe(
      defaultConfig.general.update_check_enabled,
    );
    expect(defaults.general?.auto_save).toBe(defaultConfig.general.auto_save);
    expect(defaults.general?.shortcut_feedback).toBe(defaultConfig.general.shortcut_feedback);
  });
});
