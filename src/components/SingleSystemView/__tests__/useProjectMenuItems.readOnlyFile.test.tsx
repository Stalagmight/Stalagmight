/**
 * Demo issue N-1: in a file made with a newer Stalagmight (open read-only),
 * the menu's Save, Save As, Save and Close, Import and Import from MNemo...
 * all looked enabled; Save and Save and Close then raised the "Save Failed ...
 * report it as a bug" dialog, and Save and Close left the project open.
 *
 * This version can write such a file nowhere (Save As included: the backend
 * refuses every destination, because its encoder would drop what the newer
 * version stored) and add nothing to it, so those rows are off, with the
 * read-only notice as their reason (shown on hover, and as the toast a click
 * gives: N-12). Open, Export and Close Project stay. Save and Close, should
 * it run anyway, closes without saving: there is nothing of the user's to
 * save (every edit, ignore, import and undo of the file is refused, and a
 * display toggle is a local-only save), and the save could only be refused.
 *
 * Demo finding V-4: Save and Close's reason talked about changing the file,
 * while the user wanted to close it; it now points to Close Project.
 *
 * Injection: dropping `readOnlyOff` from a row fails the first case; dropping
 * the read-only branch of `saveAndClose` (it saves first) fails the third;
 * giving Save and Close the shared read-only reason again fails the first.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useProjectMenuItems } from '../useProjectMenuItems';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { AppMenuItem } from '../../common/AppMenu';
import i18n from '../../../i18n';

const OFF_ROWS = [
  'toolbar-save-button',
  'project-menu-save-as',
  'project-menu-save-and-close',
  'global-import-menu-button',
  'app-menu-import-from-mnemo',
];
const KEPT_ROWS = ['global-open-project-button', 'toolbar-export-button', 'back-button'];

const onSave = vi.fn(async () => ({ success: true }));
const closeProject = vi.fn(async () => ({ success: true }));

function rows(): AppMenuItem[] {
  const { result } = renderHook(() => useProjectMenuItems({ onSave }));
  return result.current;
}
const row = (items: AppMenuItem[], id: string) => items.find((item) => item.id === id);

beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({
    project: { file_path: '/synthetic/future.stalx', created_at: 't0' } as never,
    isDirty: false,
    fileReadOnlyReason: null,
    closeProject,
  } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', session: null });
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', activeDialog: null });
});

describe('the project menu in a file made with a newer Stalagmight', () => {
  it('turns off every row that would save or add, and says why', () => {
    useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' });
    const items = rows();
    for (const id of OFF_ROWS) {
      const item = row(items, id);
      expect(item, id).toBeDefined();
      expect(item?.disabled, id).toBe(true);
      expect(item?.disabledReason, id).toBe(
        id === 'project-menu-save-and-close'
          ? 'Nothing to save: use Close Project.'
          : i18n.t('editGate.readOnlyFileNotice'),
      );
    }
    for (const id of KEPT_ROWS) {
      expect(row(items, id)?.disabled, id).toBeUndefined();
    }
  });

  it('leaves those rows on in an editable file', () => {
    const items = rows();
    for (const id of [...OFF_ROWS, ...KEPT_ROWS]) {
      expect(row(items, id)?.disabled, id).toBeUndefined();
    }
  });

  it('Save and Close closes without saving, and asks nothing', async () => {
    useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' });
    row(rows(), 'project-menu-save-and-close')?.onClick?.();
    await waitFor(() => expect(closeProject).toHaveBeenCalledTimes(1));
    expect(onSave).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).not.toBe('unsaved-changes');
  });

  it('Save and Close in an editable file still saves first', async () => {
    row(rows(), 'project-menu-save-and-close')?.onClick?.();
    await waitFor(() => expect(closeProject).toHaveBeenCalledTimes(1));
    expect(onSave).toHaveBeenCalledWith({ closeAfterSave: true });
  });
});
