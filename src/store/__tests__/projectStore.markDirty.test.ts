import { afterEach, describe, expect, it } from 'vitest';

import { useProjectStore } from '../projectStore';

/**
 * #110: the DMP import wizard applies lines via direct `import_lines` IPC
 * calls that bypass `executeCommand`, so they never set `isDirty` themselves. The
 * wizard now calls `markDirty()` after each successful batch so the imported
 * lines register as unsaved - powering autosave + the project-close guard.
 * (Window-close during the wizard is guarded separately, by `isImportInProgress`.)
 * This guards that primitive.
 */
describe('projectStore.markDirty (#110)', () => {
  afterEach(() => {
    useProjectStore.setState({ isDirty: false });
  });

  it('flips isDirty so autosave + the project-close guard engage', () => {
    useProjectStore.setState({ isDirty: false });

    useProjectStore.getState().markDirty();

    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('is idempotent - staying dirty across repeated batches', () => {
    useProjectStore.setState({ isDirty: false });

    useProjectStore.getState().markDirty();
    useProjectStore.getState().markDirty();

    expect(useProjectStore.getState().isDirty).toBe(true);
  });
});
