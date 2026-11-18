import { describe, it, expect } from 'vitest';
import { deepEqual, divePlanSnapshotOf, divePlanSnapshotsEqual } from '../divePlan';
import type { DivePlanInputs } from '../divePlan';
import { defaultConfig } from '../../types/config';

const baseInputs = (): DivePlanInputs => ({
  origin: { line_id: 1, station_index: 0 },
  endpoint: null,
  waypoints: [{ line_id: 1, station_index: 2 }],
  diver: { rmv_l_per_min: 20, label: null },
  cylinders: [],
  gasConfig: structuredClone(defaultConfig.gas),
  stageDropOverrides: [null, { line_id: 1, station_index: 3 }],
  markedPoints: [{ station: { line_id: 2, station_index: 1 }, label: 'photo' }],
  segments: [
    { speed_m_per_min: 10, on_scooter: false },
    { speed_m_per_min: 30, on_scooter: true },
  ],
});

describe('divePlanSnapshotOf', () => {
  it('maps the camelCase store inputs to the snake_case IPC snapshot', () => {
    const inputs = baseInputs();
    const snap = divePlanSnapshotOf(inputs);
    expect(snap.origin).toEqual({ line_id: 1, station_index: 0 });
    // The nested config + overrides are carried by reference (no copy in the mapper).
    expect(snap.gas_config).toBe(inputs.gasConfig);
    expect(snap.stage_drop_overrides).toBe(inputs.stageDropOverrides);
    expect(snap.waypoints).toBe(inputs.waypoints);
    // Segments are a plain gas input (no free-text label to trim), so unlike marked_points they
    // round-trip by reference.
    expect(snap.segments).toBe(inputs.segments);
    // Marked points are the ONE exception: they are rebuilt so the labels can be trimmed here
    // rather than in the store setter, which runs per keystroke against a controlled input and
    // would eat the space the user just typed. Structural equality, not identity.
    expect(snap.marked_points).toEqual(inputs.markedPoints);
    // Exactly the IPC keys, snake_case. This list is the dirty-tracking contract: a plan input
    // missing here is silently NOT compared, so an edit to it would read as clean and be lost
    // past the close guard.
    expect(Object.keys(snap).sort()).toEqual(
      [
        'cylinders',
        'diver',
        'endpoint',
        'gas_config',
        'marked_points',
        'origin',
        'segments',
        'stage_drop_overrides',
        'waypoints',
      ].sort()
    );
  });
});

describe('deepEqual', () => {
  it('is true for structurally equal objects regardless of key order', () => {
    expect(deepEqual({ a: 1, b: 2 }, { b: 2, a: 1 })).toBe(true);
    expect(deepEqual({ a: { x: [1, 2] } }, { a: { x: [1, 2] } })).toBe(true);
  });

  it('is false for differing primitives, nested values, lengths, and null-vs-value', () => {
    expect(deepEqual({ a: 1 }, { a: 2 })).toBe(false);
    expect(deepEqual({ a: { x: 1 } }, { a: { x: 2 } })).toBe(false);
    expect(deepEqual([1, 2], [1, 2, 3])).toBe(false);
    expect(deepEqual({ a: null }, { a: 1 })).toBe(false);
    expect(deepEqual(null, {})).toBe(false);
    expect(deepEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false);
  });

  it('treats array order as significant (a reordered drop list is a real change)', () => {
    expect(deepEqual([1, 2], [2, 1])).toBe(false);
  });
});

describe('divePlanSnapshotsEqual', () => {
  it('equal inputs compare equal; a diverging nested gas_config differs', () => {
    const a = divePlanSnapshotOf(baseInputs());
    const b = divePlanSnapshotOf(baseInputs());
    expect(divePlanSnapshotsEqual(a, b)).toBe(true);

    const changed = baseInputs();
    changed.gasConfig = { ...changed.gasConfig, swim_speed_m_per_min: 999 };
    expect(divePlanSnapshotsEqual(a, divePlanSnapshotOf(changed))).toBe(false);
  });

  it('a differing stage-drop override slot makes them unequal', () => {
    const a = divePlanSnapshotOf(baseInputs());
    const b = divePlanSnapshotOf({ ...baseInputs(), stageDropOverrides: [null, null] });
    expect(divePlanSnapshotsEqual(a, b)).toBe(false);
  });

  it('a REORDERED way-point list is a real change (order is the route)', () => {
    const inputs = baseInputs();
    inputs.waypoints = [
      { line_id: 1, station_index: 2 },
      { line_id: 3, station_index: 4 },
    ];
    const reordered = { ...inputs, waypoints: [...inputs.waypoints].reverse() };
    expect(
      divePlanSnapshotsEqual(divePlanSnapshotOf(inputs), divePlanSnapshotOf(reordered))
    ).toBe(false);
  });

  it('re-labelling a marked point makes them unequal', () => {
    const a = divePlanSnapshotOf(baseInputs());
    const b = divePlanSnapshotOf({
      ...baseInputs(),
      markedPoints: [{ station: { line_id: 2, station_index: 1 }, label: 'restriction' }],
    });
    expect(divePlanSnapshotsEqual(a, b)).toBe(false);
  });
});
