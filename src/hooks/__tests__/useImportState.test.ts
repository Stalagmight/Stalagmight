import { describe, it, expect } from 'vitest';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { DmpShot } from '../../types/commands';
import type { ImportState, SectionConfig } from '../useImportState';
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

const baseConfig: SectionConfig = {
  sectionIndex: 0,
  name: 'Test Section',
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

const secondConfig: SectionConfig = {
  sectionIndex: 1,
  name: 'Second Section',
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
    sectionConfigs: new Map([[0, { ...baseConfig, shots: [{ ...baseShot }] }]]),
    currentSectionIndex: 0,
    nextTempId: 1,
    placedSections: new Set(),
    excludedSections: new Set(),
    undoStacks: new Map(),
    redoStacks: new Map(),
    originalValues: new Map([[0, baseConfig]]),
    ...overrides,
  };
}

function buildMultiSectionState(): ImportState {
  return buildState({
    sectionConfigs: new Map([
      [0, { ...baseConfig, shots: [{ ...baseShot }] }],
      [1, { ...secondConfig, shots: [{ ...secondShot }] }],
    ]),
    originalValues: new Map([
      [0, baseConfig],
      [1, secondConfig],
    ]),
  });
}

function getShots(state: ImportState, section = 0): EditableShot[] {
  return state.sectionConfigs.get(section)?.shots ?? [];
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

    const shots = state2.sectionConfigs.get(0)?.shots ?? [];
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

  it('NAVIGATE changes currentSectionIndex', () => {
    const state = buildMultiSectionState();
    const next = importReducer(state, { type: 'NAVIGATE', sectionIndex: 1 });
    expect(next.currentSectionIndex).toBe(1);
  });

  // -----------------------------------------------------------------------
  // UPDATE_FIELD
  // -----------------------------------------------------------------------

  it('UPDATE_FIELD changes a section metadata field', () => {
    const state = buildState();
    const next = importReducer(state, {
      type: 'UPDATE_FIELD',
      field: 'name',
      value: 'Renamed',
    });
    expect(next.sectionConfigs.get(0)?.name).toBe('Renamed');
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
    expect(next.sectionConfigs.get(0)?.placement.type).toBe('zeroDatum');
    expect(next.placedSections.has(0)).toBe(false);
  });

  // -----------------------------------------------------------------------
  // MARK_PLACED / UNMARK_PLACED
  // -----------------------------------------------------------------------

  it('MARK_PLACED adds section to placedSections', () => {
    const state = buildState();
    const next = importReducer(state, { type: 'MARK_PLACED', sectionIndex: 0 });
    expect(next.placedSections.has(0)).toBe(true);
  });

  it('UNMARK_PLACED removes section from placedSections', () => {
    const state = buildState({ placedSections: new Set([0]) });
    const next = importReducer(state, { type: 'UNMARK_PLACED', sectionIndex: 0 });
    expect(next.placedSections.has(0)).toBe(false);
  });

  // -----------------------------------------------------------------------
  // EXCLUDE_SECTION / INCLUDE_SECTION
  // -----------------------------------------------------------------------

  it('EXCLUDE_SECTION adds to excluded and removes from placed', () => {
    const state = buildState({ placedSections: new Set([0]) });
    const next = importReducer(state, { type: 'EXCLUDE_SECTION', sectionIndex: 0 });
    expect(next.excludedSections.has(0)).toBe(true);
    expect(next.placedSections.has(0)).toBe(false);
  });

  it('INCLUDE_SECTION removes from excluded', () => {
    const state = buildState({ excludedSections: new Set([0]) });
    const next = importReducer(state, { type: 'INCLUDE_SECTION', sectionIndex: 0 });
    expect(next.excludedSections.has(0)).toBe(false);
  });

  // -----------------------------------------------------------------------
  // UNDO / REDO
  // -----------------------------------------------------------------------

  it('UNDO restores previous field value', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed' });
    expect(state.sectionConfigs.get(0)?.name).toBe('Changed');

    state = importReducer(state, { type: 'UNDO' });
    expect(state.sectionConfigs.get(0)?.name).toBe('Test Section');
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
    expect(state.sectionConfigs.get(0)?.name).toBe('Test Section');

    state = importReducer(state, { type: 'REDO' });
    expect(state.sectionConfigs.get(0)?.name).toBe('Changed');
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
  // RESET_SECTION
  // -----------------------------------------------------------------------

  it('RESET_SECTION restores original values and clears undo/redo', () => {
    let state = buildState();
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed' });
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'color', value: '#000000' });
    state = importReducer(state, { type: 'MARK_PLACED', sectionIndex: 0 });

    state = importReducer(state, { type: 'RESET_SECTION' });
    expect(state.sectionConfigs.get(0)?.name).toBe('Test Section');
    expect(state.sectionConfigs.get(0)?.color).toBe('#ffffff');
    expect(state.sectionConfigs.get(0)?.placement.type).toBe('origin');
    expect(state.placedSections.has(0)).toBe(false);
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
      sectionConfigs: newConfigs,
      placedSections: new Set([1]),
      excludedSections: new Set(),
      currentSectionIndex: 1,
    });
    expect(next.currentSectionIndex).toBe(1);
    expect(next.sectionConfigs.has(1)).toBe(true);
    expect(next.placedSections.has(1)).toBe(true);
  });

  // -----------------------------------------------------------------------
  // Undo stack is per-section
  // -----------------------------------------------------------------------

  it('undo stacks are independent per section', () => {
    let state = buildMultiSectionState();

    // Edit section 0
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed 0' });
    // Navigate to section 1
    state = importReducer(state, { type: 'NAVIGATE', sectionIndex: 1 });
    // Edit section 1
    state = importReducer(state, { type: 'UPDATE_FIELD', field: 'name', value: 'Changed 1' });

    // Undo on section 1
    state = importReducer(state, { type: 'UNDO' });
    expect(state.sectionConfigs.get(1)?.name).toBe('Second Section');
    // Section 0 should still be changed
    expect(state.sectionConfigs.get(0)?.name).toBe('Changed 0');
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
