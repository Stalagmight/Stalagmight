/**
 * Import item gating for useProjectMenuItems: the in-project "Import" hamburger
 * row is NEVER disabled, in any read-only state. The pre-#407
 * `disabled: editingBlocked` gate (#275) was picker-side and in the wrong
 * layer - the additive DMP-into-open-project case it meant to stop is rejected
 * by routeOpenedFile's read-only guard, with a cause-specific message and (for
 * the causes one click can fix) an offer to switch and continue, none of which
 * a greyed-out row can say (pinned in openOrImport.test.ts). Narrowing the row
 * to `.dmp` (#508) does not revive the gate: the guard still owns that call.
 * These pins keep a picker-side disable from quietly coming back.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useProjectMenuItems } from '../useProjectMenuItems';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';

const READ_ONLY_LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: 'someone-else',
  lockedByOther: true,
  hasUnpushedEdits: false,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

function importItem(): { disabled?: boolean; onClick?: unknown } | undefined {
  const { result } = renderHook(() =>
    useProjectMenuItems({ onSave: async () => ({ success: true }) }),
  );
  return result.current.find(
    (it) => (it as { id?: string }).id === 'global-import-menu-button',
  ) as { disabled?: boolean; onClick?: unknown } | undefined;
}

beforeEach(() => {
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
  // Confirmed-local project - the editable baseline (#270 fail-closed gate).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', session: null });
});

// Asserts PRESENCE (the row is rendered and clickable) as well as no `disabled`
// flag in every state: `importItem()?.disabled` alone would also pass if a
// regression conditionally OMITTED the row in blocked states - the same
// picker-side gate class these pins exist to block, just in a stronger form.
function expectImportItemAvailable(): void {
  const item = importItem();
  expect(typeof item?.onClick).toBe('function');
  expect(item?.disabled).toBeUndefined();
}

describe('useProjectMenuItems Import item - never picker-disabled (#407)', () => {
  it('is present with its row in the fully-editable baseline', () => {
    expectImportItemAvailable();
  });

  it('stays available in the closure-adjusted view (router guards the DMP case)', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    expectImportItemAvailable();
  });

  it('stays available in Plan mode even when geometryMode falls back to measured', () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    expectImportItemAvailable();
  });

  it('stays available on a locked remote SpeleoDB project', () => {
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    expectImportItemAvailable();
  });

  it('stays available with the SpeleoDB write lock held', () => {
    useSpeleoStore.setState({ link: { ...READ_ONLY_LINK, editState: 'editing', lockedByOther: false } });
    expectImportItemAvailable();
  });
});
