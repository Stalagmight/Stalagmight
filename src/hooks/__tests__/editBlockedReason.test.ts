/**
 * #576 D39(a): the reason a disabled survey-mutating control gives - the term of
 * the canonical editing gate that applies, the one the user cannot lift from the
 * current screen first - and null exactly when the gate is open.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import { editBlockedReasonNow, isEditingBlockedNow } from '../useEditingBlocked';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';

function setState({
  appMode = 'survey',
  geometryMode = 'measured',
  remoteReadOnly = false,
  newerFile = false,
}: {
  appMode?: 'survey' | 'plan' | 'export';
  geometryMode?: 'measured' | 'closureAdjusted';
  remoteReadOnly?: boolean;
  newerFile?: boolean;
}) {
  useUIStore.setState({ appMode, geometryMode });
  useSpeleoStore.setState({ projectLinkKind: remoteReadOnly ? 'remote' : 'local', link: null });
  useProjectStore.setState({ newerSchemaVersion: newerFile ? 99 : null });
}

beforeEach(() => setState({}));

describe('editBlockedReasonNow (D39(a))', () => {
  it('is null when editing is allowed', () => {
    expect(editBlockedReasonNow()).toBeNull();
    expect(isEditingBlockedNow()).toBe(false);
  });

  it.each([
    [{ appMode: 'plan' as const }, 'workspace'],
    [{ appMode: 'export' as const }, 'workspace'],
    [{ geometryMode: 'closureAdjusted' as const }, 'closures'],
    [{ remoteReadOnly: true }, 'lock'],
    [{ newerFile: true }, 'newerFile'],
    // The term the current screen cannot lift wins.
    [{ remoteReadOnly: true, appMode: 'plan' as const }, 'lock'],
    [{ newerFile: true, remoteReadOnly: true, appMode: 'plan' as const }, 'newerFile'],
    [{ appMode: 'plan' as const, geometryMode: 'closureAdjusted' as const }, 'workspace'],
  ])('%o -> %s, and the gate agrees it is blocked', (state, reason) => {
    setState(state);
    expect(editBlockedReasonNow()).toBe(reason);
    expect(isEditingBlockedNow()).toBe(true);
  });
});
