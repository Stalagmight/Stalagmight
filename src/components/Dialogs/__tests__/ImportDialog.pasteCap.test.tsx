/**
 * The capped-paste toast must fire when a paste runs past MAX_PASTE_ROWS.
 *
 * `applyGridEdits` used to decide the toast from a count assigned INSIDE the
 * drafts updater and read on the very next line - but React can run that updater
 * later than the function returns, so the read saw a stale 0 and the dropped
 * rows vanished with no word. The fix computes the count from the committed
 * drafts BEFORE scheduling, which is reliable because a paste (the only edit
 * that can reach the cap) arrives as one batch against the rows on screen.
 *
 * The grid is stubbed so the test drives `onBatchEdit` directly rather than
 * synthesizing a 5000-row clipboard paste through the real grid.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, act } from '@testing-library/react';

const captured = vi.hoisted(() => ({
  onBatchEdit: null as null | ((edits: { row: number; colKey: string; value: unknown }[]) => void),
}));

// Keep every real export (types, helpers) and only stub the component, so
// transitive imports of the grid barrel still resolve.
vi.mock('../../common/SpreadsheetGrid', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../common/SpreadsheetGrid')>()),
  SpreadsheetGrid: (props: {
    onBatchEdit?: (edits: { row: number; colKey: string; value: unknown }[]) => void;
  }) => {
    captured.onBatchEdit = props.onBatchEdit ?? null;
    return null;
  },
}));

vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import '../../../i18n';
import ImportDialog from '../ImportDialog';
import { MAX_PASTE_ROWS } from '../newSectionRows';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';

let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  captured.onBatchEdit = null;
  showToast = vi.fn();
  useUIStore.setState({
    dialogData: {},
    resolvedTheme: 'dark',
    appMode: 'survey',
    geometryMode: 'measured',
    showToast,
  });
  useProjectStore.setState({ project: null, config: null });
  // useEditingBlocked() fails CLOSED on an unestablished project (#270); a
  // 'local' link makes the grid and its edit funnel writable.
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('ImportDialog capped-paste toast', () => {
  it('fires when a batch edit lands past the row cap', () => {
    render(<ImportDialog />);
    expect(captured.onBatchEdit).not.toBeNull();

    act(() => {
      captured.onBatchEdit!([
        { row: MAX_PASTE_ROWS, colKey: 'length', value: 10 },
        { row: MAX_PASTE_ROWS + 1, colKey: 'length', value: 12 },
      ]);
    });

    expect(showToast).toHaveBeenCalledWith(
      expect.any(String),
      'new-section-paste-cap',
      'error'
    );
  });

  it('does not fire for an edit that stays within the cap', () => {
    render(<ImportDialog />);
    act(() => {
      captured.onBatchEdit!([{ row: 0, colKey: 'length', value: 10 }]);
    });
    expect(showToast).not.toHaveBeenCalled();
  });
});
