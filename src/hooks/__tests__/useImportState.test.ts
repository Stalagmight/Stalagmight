import { describe, it, expect } from 'vitest';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { DmpShot } from '../../types/commands';
import type { ImportState, LineConfig } from '../useImportState';
import { convertToEditableShots, importReducer } from '../useImportState';

const baseShot: EditableShot = {
  id: 0,
  azimuth_in: 10,
  azimuth_out: 10,
  length: 1,
  depth_in: 0,
  depth_out: 0,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal' as const,
};

const baseConfig: LineConfig = {
  lineIndex: 0,
  name: 'Test Line',
  surveyors: [],
  explorers: [],
  date: null,
  color: '#ffffff',
  direction: 'in',
  quality: 1.0,
  shots: [baseShot],
  placement: { type: 'origin' },
};

const secondShot: EditableShot = {
  ...baseShot,
  id: 1,
  azimuth_in: 90,
  azimuth_out: 90,
  length: 2,
};

const secondConfig: LineConfig = {
  lineIndex: 1,
  name: 'Second Line',
  surveyors: [],
  explorers: [],
  date: null,
  color: '#ff0000',
  direction: 'out',
  quality: 1.0,
  shots: [secondShot],
  placement: { type: 'origin' },
};

function buildState(overrides?: Partial<ImportState>): ImportState {
  return {
    lineConfigs: new Map([[0, { ...baseConfig, shots: [{ ...baseShot }] }]]),
    currentLineIndex: 0,
    nextTempId: 1,
    placedLines: new Set(),
    excludedLines: new Set(),
    undoStacks: new Map(),
    redoStacks: new Map(),
    originalValues: new Map([[0, baseConfig]]),
    ...overrides,
  };
}

function buildMultiLineState(): ImportState {
  return buildState({
    lineConfigs: new Map([
      [0, { ...baseConfig, shots: [{ ...baseShot }] }],
      [1, { ...secondConfig, shots: [{ ...secondShot }] }],
    ]),
    originalValues: new Map([
      [0, baseConfig],
      [1, secondConfig],
    ]),
  });
}

function getShots(state: ImportState, line = 0): EditableShot[] {
  return state.lineConfigs.get(line)?.shots ?? [];
}

function makeDmpShot(overrides?: Partial<DmpShot>): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 0,
    heading_out: 0,
    length: 2,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
    ...overrides,
  };
}

describe('useImportState reducer', () => {
  // -----------------------------------------------------------------------
  // ADD_SHOT
  // -----------------------------------------------------------------------

  it('adds shots with unique temp IDs and blank defaults', () => {
    const state1 = importReducer(buildState(), { type: 'ADD_SHOT' });
    const state2 = importReducer(state1, { type: 'ADD_SHOT' });

    const shots = state2.lineConfigs.get(0)?.shots ?? [];
    const firstNew = shots[1];
    const secondNew = shots[2];

    expect(firstNew?.tempId).toBeDefined();
    expect(secondNew?.tempId).toBeDefined();
    expect(firstNew?.tempId).not.toBe(secondNew?.tempId);
    expect(firstNew?.id).not.toBe(secondNew?.id);
    expect(firstNew?.length).toBe(0);
    expect(firstNew?.azimuth_in).toBe(0);
    expect(firstNew?.azimuth_out).toBe(0);
    expect(firstNew?.depth_in).toBe(0);
    expect(firstNew?.depth_out).toBe(0);
    expect(state2.nextTempId).toBe(3);
  });

  // -----------------------------------------------------------------------
  // NAVIGATE
  // -----------------------------------------------------------------------

  it('NAVIGATE changes currentLineIndex', () => {
    const state = buildMultiLineState();
    const next = importReducer(state, { type: 'NAVIGATE', lineIndex: 1 });
    expect(next.currentLineIndex).toBe(1);
  });

  // -----------------------------------------------------------------------
  // UPDATE_FIELD
  // -----------------------------------------------------------------------

  it('UPDATE_FIELD changes a line metadata field', () => {
    const state = buildState();
    const next = importReducer(state, {
      type: 'UPDATE_FIELD',
      field: 'name',
      value: 'Renamed',
    });
    expect(next.lineConfigs.get(0)?.name).toBe('Renamed');
  });

  it('UPDATE_FIELD pushes undo entry and clears redo', () => {
    const state = buildState();
    const next = importReducer(state, {
      type: 'UPDATE_FIELD',
      field: 'name',
      value: 'Renamed',
    });
    expect(next.undoStacks.get(0)?.length).toBe(1);
    expect(next.redoStacks.get(0)?.length).toBe(0);
  });

  // -----------------------------------------------------------------------
  // UPDATE_SHOT
  // -----------------------------------------------------------------------

  it('UPDATE_SHOT changes a shot field', () => {
    const state = buildState();
    const next = importReducer(state, {
      type: 'UPDATE_SHOT',
      shotIndex: 0,
      field: 'length',
      value: 42,
    });
    expect(getShots(next)[0]?.length).toBe(42);
  });

  it('UPDATE_SHOT is no-op for out-of-bounds index', () => {
    const state = buildState();
    const next = importReducer(state, {
      type: 'UPDATE_SHOT',
      shotIndex: 99,
      field: 'length',
      value: 42,
    });
    expect(getShots(next)).toEqual(getShots(state));
  });

  // -----------------------------------------------------------------------
  // DELETE_SHOT
  // -----------------------------------------------------------------------

  it('DELETE_SHOT marks shot as deleted', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'DELETE_SHOT', index: 0 });
    expect(getShots(next)[0]?.isDeleted).toBe(true);
  });

  it('DELETE_SHOT is no-op for out-of-bounds index', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'DELETE_SHOT', index: 99 });
    expect(getShots(next)[0]?.isDeleted).toBeUndefined();
  });

  // -----------------------------------------------------------------------
  // INSERT_SHOT_BEFORE / INSERT_SHOT_AFTER
  // -----------------------------------------------------------------------

  it('INSERT_SHOT_BEFORE inserts blank shot at given index', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'INSERT_SHOT_BEFORE', index: 0 });
    const shots = getShots(next);
    expect(shots).toHaveLength(2);
    expect(shots[0]?.isNew).toBe(true);
    expect(shots[0]?.length).toBe(0);
    expect(shots[1]?.id).toBe(baseShot.id);
  });

  it('INSERT_SHOT_AFTER inserts blank shot after given index', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'INSERT_SHOT_AFTER', index: 0 });
    const shots = getShots(next);
    expect(shots).toHaveLength(2);
    expect(shots[0]?.id).toBe(baseShot.id);
    expect(shots[1]?.isNew).toBe(true);
  });

  // -----------------------------------------------------------------------
  // SET_PLACEMENT
  // -----------------------------------------------------------------------

  it('SET_PLACEMENT updates placement without marking as placed', () => {
    const state = buildState();
    const next = importReducer(state, {
      type: 'SET_PLACEMENT',
      placement: { type: 'zeroDatum', coordinates: { latitude: 1, longitude: 2, depth_reference: 0 }, stationIndex: 0 },
    });
    expect(next.lineConfigs.get(0)?.placement.type).toBe('zeroDatum');
    expect(next.placedLines.has(0)).toBe(false);
  });

  // -----------------------------------------------------------------------
  // MARK_PLACED / UNMARK_PLACED
  // -----------------------------------------------------------------------

  it('MARK_PLACED adds line to placedLines', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'MARK_PLACED', lineIndex: 0 });
    expect(next.placedLines.has(0)).toBe(true);
  });

  it('UNMARK_PLACED removes line from placedLines', () => {
    const state = buildState({ placedLines: new Set([0]) });
    const next = importReducer(state, { type: 'UNMARK_PLACED', lineIndex: 0 });
    expect(next.placedLines.has(0)).toBe(false);
  });

  // -----------------------------------------------------------------------
  // EXCLUDE_LINE / INCLUDE_LINE
  // -----------------------------------------------------------------------

  it('EXCLUDE_LINE adds to excluded and removes from placed', () => {
    const state = buildState({ placedLines: new Set([0]) });
    const next = importReducer(state, { type: 'EXCLUDE_LINE', lineIndex: 0 });
    expect(next.excludedLines.has(0)).toBe(true);
    expect(next.placedLines.has(0)).toBe(false);
  });

  it('INCLUDE_LINE removes from excluded', () => {
    const state = buildState({ excludedLines: new Set([0]) });
    const next = importReducer(state, { type: 'INCLUDE_LINE', lineIndex: 0 });
    expect(next.excludedLines.has(0)).toBe(false);
  });

  // -----------------------------------------------------------------------
  // UNDO / REDO
  // -----------------------------------------------------------------------

  it('UNDO restores previous field value', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed' });
    expect(state.lineConfigs.get(0)?.name).toBe('Changed');

    state = importReducer(state, { type: 'UNDO' });
    expect(state.lineConfigs.get(0)?.name).toBe('Test Line');
  });

  it('UNDO pushes to redo stack', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed' });
    state = importReducer(state, { type: 'UNDO' });
    expect(state.redoStacks.get(0)?.length).toBe(1);
    expect(state.undoStacks.get(0)?.length).toBe(0);
  });

  it('REDO restores undone change', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed' });
    state = importReducer(state, { type: 'UNDO' });
    expect(state.lineConfigs.get(0)?.name).toBe('Test Line');

    state = importReducer(state, { type: 'REDO' });
    expect(state.lineConfigs.get(0)?.name).toBe('Changed');
  });

  it('REDO is no-op when redo stack is empty', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'REDO' });
    expect(next).toBe(state);
  });

  it('UNDO is no-op when undo stack is empty', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'UNDO' });
    expect(next).toBe(state);
  });

  it('new action clears redo stack', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'First' });
    state = importReducer(state, { type: 'UNDO' });
    expect(state.redoStacks.get(0)?.length).toBe(1);

    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Second' });
    expect(state.redoStacks.get(0)?.length).toBe(0);
  });

  it('UNDO/REDO round-trips shot edits', () => {
    let state = buildState();
    const originalLength = getShots(state)[0]?.length;

    state = importReducer(state, { type: 'UPDATE_SHOT', shotIndex: 0, field: 'length', value: 99 });
    expect(getShots(state)[0]?.length).toBe(99);

    state = importReducer(state, { type: 'UNDO' });
    expect(getShots(state)[0]?.length).toBe(originalLength);

    state = importReducer(state, { type: 'REDO' });
    expect(getShots(state)[0]?.length).toBe(99);
  });

  // -----------------------------------------------------------------------
  // RESET_LINE
  // -----------------------------------------------------------------------

  it('RESET_LINE restores original values and clears undo/redo', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed' });
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'color', value: '#000000' });
    state = importReducer(state, { type: 'MARK_PLACED', lineIndex: 0 });

    state = importReducer(state, { type: 'RESET_LINE' });
    expect(state.lineConfigs.get(0)?.name).toBe('Test Line');
    expect(state.lineConfigs.get(0)?.color).toBe('#ffffff');
    expect(state.lineConfigs.get(0)?.placement.type).toBe('origin');
    expect(state.placedLines.has(0)).toBe(false);
    expect(state.undoStacks.get(0)?.length).toBe(0);
    expect(state.redoStacks.get(0)?.length).toBe(0);
  });

  // -----------------------------------------------------------------------
  // RESTORE_STATE
  // -----------------------------------------------------------------------

  it('RESTORE_STATE replaces configs and placement sets', () => {
    const state = buildState();
    const newConfigs = new Map([[1, secondConfig]]);
    const next = importReducer(state, {
      type: 'RESTORE_STATE',
      lineConfigs: newConfigs,
      placedLines: new Set([1]),
      excludedLines: new Set(),
      currentLineIndex: 1,
    });
    expect(next.currentLineIndex).toBe(1);
    expect(next.lineConfigs.has(1)).toBe(true);
    expect(next.placedLines.has(1)).toBe(true);
  });

  // -----------------------------------------------------------------------
  // Undo stack is per-line
  // -----------------------------------------------------------------------

  it('undo stacks are independent per line', () => {
    let state = buildMultiLineState();

    // Edit line 0
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed 0' });
    // Navigate to line 1
    state = importReducer(state, { type: 'NAVIGATE', lineIndex: 1 });
    // Edit line 1
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed 1' });

    // Undo on line 1
    state = importReducer(state, { type: 'UNDO' });
    expect(state.lineConfigs.get(1)?.name).toBe('Second Line');
    // Line 0 should still be changed
    expect(state.lineConfigs.get(0)?.name).toBe('Changed 0');
  });
});

describe('convertToEditableShots', () => {
  it('excludes standard shots with length 1 by default', () => {
    const editableShots = convertToEditableShots([
      makeDmpShot({ length: 1 }),
      makeDmpShot({ length: 2.5 }),
    ]);

    expect(editableShots).toHaveLength(1);
    expect(editableShots[0]?.length).toBe(2.5);
    expect(editableShots[0]?.id).toBe(0);
  });

  it('keeps non-length-1 standard shots and reindexes IDs', () => {
    const editableShots = convertToEditableShots([
      makeDmpShot({ length: 3 }),
      makeDmpShot({ length: 1 }),
      makeDmpShot({ length: 4 }),
    ]);

    expect(editableShots).toHaveLength(2);
    expect(editableShots[0]?.id).toBe(0);
    expect(editableShots[1]?.id).toBe(1);
    expect(editableShots[0]?.length).toBe(3);
    expect(editableShots[1]?.length).toBe(4);
  });
});
