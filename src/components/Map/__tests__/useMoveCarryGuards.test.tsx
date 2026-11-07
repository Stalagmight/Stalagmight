/**
 * GHOST-2 (#576): the map-side carry guards on their own (useMoveCarryGuards.ts).
 * The registry is pinned by store/__tests__/moveCarry.test.ts and the chokepoint by
 * projectStore.moveCarry.test.ts; this covers what invalidates a carry without any
 * mutation being issued: a survey change, a project close or switch, read-only, a
 * tool switch and entering placement mode - and which of those tell the user.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useUIStore, type PlacementModeState } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { beginMoveCarry, cancelMoveCarry, isMoveCarryInFlight } from '../../../store/moveCarry';
import { useMoveCarryGuards } from '../useMoveCarryGuards';
import type { Project } from '../../../types/project';

const PROJECT_A = { created_at: 'project-a' } as Project;
const PROJECT_B = { created_at: 'project-b' } as Project;
const PLACEMENT = { alignStationIndex: 0, cursorPosition: null, snappedToStation: null } as unknown as PlacementModeState;

describe('useMoveCarryGuards (GHOST-2)', () => {
  let showToast: ReturnType<typeof vi.fn>;
  let cancel: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    showToast = vi.fn();
    cancel = vi.fn();
    // Fully editable: Survey, measured, a CONFIRMED-local project (the SpeleoDB term
    // fails closed on an unresolved link, which would cancel as read-only).
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', activeTool: 'move_station', placementMode: null, showToast });
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
    useProjectStore.setState({ project: PROJECT_A, projectTreeGeneration: 41 });
    renderHook(() => useMoveCarryGuards({ current: null }));
    beginMoveCarry(41, { cancel, restorePreview: vi.fn() });
  });

  afterEach(() => {
    cancelMoveCarry('toolSwitch');
  });

  it('a survey change in the same project cancels the carry and says so', () => {
    useProjectStore.setState({ projectTreeGeneration: 42 });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(isMoveCarryInFlight()).toBe(false);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(expect.any(String), expect.any(String), 'warning');
  });

  it('an unrelated store write that leaves the generation alone keeps the carry', () => {
    useProjectStore.setState({ isDirty: true });
    expect(cancel).not.toHaveBeenCalled();
    expect(isMoveCarryInFlight()).toBe(true);
  });

  it('a project switch cancels the carry silently, even though the generation moved too', () => {
    useProjectStore.setState({ project: PROJECT_B, projectTreeGeneration: 42 });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(isMoveCarryInFlight()).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('a project close cancels the carry silently', () => {
    useProjectStore.setState({ project: null, projectTreeGeneration: 42 });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('entering placement mode cancels the carry silently', () => {
    useUIStore.setState({ placementMode: PLACEMENT });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(isMoveCarryInFlight()).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('switching among the move tools keeps the carry; any other tool cancels it silently', () => {
    useUIStore.setState({ activeTool: 'move_segment' });
    useUIStore.setState({ activeTool: 'move_shot' });
    expect(cancel).not.toHaveBeenCalled();
    useUIStore.setState({ activeTool: 'select' });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('editing becoming blocked cancels the carry and says so', () => {
    useUIStore.setState({ appMode: 'plan' });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledTimes(1);
  });
});
