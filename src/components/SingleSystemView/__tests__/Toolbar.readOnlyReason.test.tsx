/**
 * Demo finding V-5: in a file made with a newer Stalagmight the toolbar's
 * Import, Undo and Redo were plain `disabled` buttons with no title and no
 * tooltip, while the menu rows and the shortcuts said why. They now say why
 * the way the menu rows do (`DisabledReason`, the same component): the
 * button is `aria-disabled`, the reason shows on hover, and a click shows it
 * as the read-only notice, never the action. In an editable file nothing
 * changes. Injection: drop `reason` from the Undo dropdown, or the
 * `ImportReason` wrapper, and the first case fails.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: vi.fn(async () => []),
}));

import '../../../i18n';
import i18n from '../../../i18n';
import Toolbar from '../Toolbar';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { DISABLED_REASON_DELAY_MS } from '../../common/DisabledReason';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const onImport = vi.fn();
const onUndo = vi.fn();
const onRedo = vi.fn();

function renderToolbar() {
  render(
    <Toolbar
      menuItems={[]}
      onImport={onImport}
      onUndo={onUndo}
      onRedo={onRedo}
      canUndo
      canRedo={false}
      placementModeActive={false}
      dataTableFullScreen={false}
    />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  useUIStore.getState().clearToast();
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', session: null } as never);
  useProjectStore.setState({ fileReadOnlyReason: null } as never);
});

describe('toolbar Import, Undo and Redo in a read-only file (V-5)', () => {
  it('say why on hover, and a click shows the notice instead of the action', () => {
    vi.useFakeTimers();
    try {
      useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' } as never);
      renderToolbar();
      const reason = i18n.t('editGate.readOnlyFileNotice');
      for (const id of ['toolbar-import-button', 'toolbar-undo-button', 'toolbar-redo-button']) {
        const button = screen.getByTestId(id);
        expect(button.getAttribute('aria-disabled'), id).toBe('true');
        expect(button.hasAttribute('disabled'), id).toBe(false);
        fireEvent.mouseEnter(button.parentElement!);
        act(() => {
          vi.advanceTimersByTime(DISABLED_REASON_DELAY_MS + 50);
        });
        expect(screen.getByTestId(`${id}-reason`), id).toHaveTextContent(reason);
        fireEvent.mouseLeave(button.parentElement!);
        act(() => {
          vi.advanceTimersByTime(50);
        });
        fireEvent.click(button);
      }
      expect(onImport).not.toHaveBeenCalled();
      expect(onUndo).not.toHaveBeenCalled();
      expect(onRedo).not.toHaveBeenCalled();
      const notices = useUIStore.getState().toasts.filter((toast) => toast.key === 'edit-disabled');
      expect(notices).toHaveLength(1);
      expect(notices[0]?.message).toBe(reason);
    } finally {
      vi.useRealTimers();
    }
  });

  it('stay as they were in an editable file', () => {
    renderToolbar();
    const importButton = screen.getByTestId('toolbar-import-button');
    expect(importButton.hasAttribute('aria-disabled')).toBe(false);
    fireEvent.click(importButton);
    expect(onImport).toHaveBeenCalledOnce();
    // Redo has nothing to redo: plainly disabled, no reason.
    expect(screen.getByTestId('toolbar-redo-button').hasAttribute('disabled')).toBe(true);
    expect(screen.queryByTestId('toolbar-redo-button-reason')).toBeNull();
  });
});
