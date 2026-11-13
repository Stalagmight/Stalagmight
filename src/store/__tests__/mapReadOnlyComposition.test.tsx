/**
 * Hook-composition coverage for `useEditingBlocked()` (src/hooks/useEditingBlocked.ts,
 * #275 canonical-gating sweep) - the shared hook `DataTable.tsx`, `SectionsPanel.tsx`,
 * `WarningResolution.tsx`, `DetailPanel.tsx`,
 * `useProjectMenuItems.tsx`, and `Toolbar.tsx` all now call to derive their local
 * `editingBlocked` gate. `uiStore.isMapReadOnly.test.ts` already covers the
 * appMode/geometryMode truth table for the plain-function form; this covers the SAME
 * predicate wired through the REAL two-hook composition (subscription/re-render
 * semantics differ from a plain function call) plus the speleoReadOnly term.
 *
 * The rerender-across-the-flip test below is the regression guard for the #275
 * blocker: the prior generation of call sites wrote the composition inline as
 * `useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly)`, which
 * short-circuits the second hook call whenever the first term is already `true`.
 * If that term later flips back to `false` on a rerender of the SAME component, the
 * number of hooks called changes between renders and React throws ("Rendered more
 * hooks than during the previous render"). `useEditingBlocked()` calls both store
 * hooks unconditionally on separate lines specifically to make that structurally
 * impossible; this test proves it stays that way.
 *
 * DataTable.tsx itself is not rendered here (its SpreadsheetGrid subtree needs a
 * heavier DOM harness with no existing precedent in this repo) - component-level
 * coverage is impractical per the #275 task scope; this proves the shared hook
 * instead. The tripwire script (`scripts/check-readonly-gates.mjs`) separately
 * guards that call sites keep using it rather than the raw symbols.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useUIStore } from '../uiStore';
import { useSpeleoStore, type SpeleoLinkState } from '../speleoStore';
import { useEditingBlocked, useEditingBlockedReason, editingBlockedReasonNow } from '../../hooks/useEditingBlocked';

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

beforeEach(() => {
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
  // A CONFIRMED-local project. Since #270 the SpeleoDB term fails closed, so
  // `link: null` alone is the UNRESOLVED state (read-only), not the editable one.
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('useEditingBlocked()', () => {
  it('is false in the one fully-editable state: survey + measured + no lock', () => {
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(false);
  });

  it('is true in the closure-adjusted survey view', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(true);
  });

  it('is false in Data + measured + no lock: Data is editable like Survey', () => {
    useUIStore.setState({ appMode: 'data', geometryMode: 'measured' });
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(false);
  });

  it('is true in Data on the closure-adjusted view, and on a locked remote project', () => {
    useUIStore.setState({ appMode: 'data', geometryMode: 'closureAdjusted' });
    const { result, rerender } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(true);
    act(() => {
      useUIStore.setState({ geometryMode: 'measured' });
      useSpeleoStore.setState({ link: READ_ONLY_LINK });
    });
    rerender();
    expect(result.current).toBe(true);
  });

  it('is true in Plan mode even though geometryMode fell back to measured', () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(true);
  });

  it('is true on a locked remote SpeleoDB project, independent of geometry/appMode', () => {
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(true);
  });

  it('is false once the SpeleoDB write lock is held, in the editable survey state', () => {
    useSpeleoStore.setState({ link: { ...READ_ONLY_LINK, editState: 'editing', lockedByOther: false } });
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(false);
  });

  it('does not throw and returns true when a rerender flips appMode into Plan mode (RoH regression guard)', () => {
    // Start fully editable, so a short-circuiting `mapReadOnly || speleoReadOnly`
    // expression would skip the second hook call on THIS render.
    const { result, rerender } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(false);

    // Flip into Plan mode - a short-circuiting expression now returns early on the
    // FIRST term and would call the second hook on this render but not the last
    // (or vice versa depending on evaluation order), changing the hook-call count
    // between renders of the same component instance.
    act(() => {
      useUIStore.setState({ appMode: 'plan' });
    });

    expect(() => rerender()).not.toThrow();
    expect(result.current).toBe(true);
  });

  it('does not throw when a rerender flips isMapReadOnly from true back to false (RoH regression guard)', () => {
    // Start in Plan mode: isMapReadOnly is already true, so a short-circuiting
    // expression skips the speleoReadOnly hook call entirely on this render.
    useUIStore.setState({ appMode: 'plan' });
    const { result, rerender } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(true);

    // Flip back to the fully-editable state - a short-circuiting expression would
    // now call the previously-skipped speleoReadOnly hook, increasing the hook
    // count on this render relative to the last and throwing in real React.
    act(() => {
      useUIStore.setState({ appMode: 'survey' });
    });

    expect(() => rerender()).not.toThrow();
    expect(result.current).toBe(false);
  });
});

describe('useEditingBlockedReason() / editingBlockedReasonNow()', () => {
  const both = () => {
    const { result } = renderHook(() => useEditingBlockedReason());
    return [result.current, editingBlockedReasonNow()];
  };

  it('is null while editing is open', () => {
    expect(both()).toEqual([null, null]);
  });

  it('names the view for the closure-adjusted view and Plan/Export', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    expect(both()).toEqual(['view', 'view']);
    useUIStore.setState({ geometryMode: 'measured', appMode: 'export' });
    expect(both()).toEqual(['view', 'view']);
  });

  it('names the project for a read-only SpeleoDB copy, even inside a read-only view', () => {
    useSpeleoStore.setState({ link: READ_ONLY_LINK, projectLinkKind: 'remote' });
    expect(both()).toEqual(['project', 'project']);
    useUIStore.setState({ appMode: 'plan' });
    expect(both()).toEqual(['project', 'project']);
  });
});
