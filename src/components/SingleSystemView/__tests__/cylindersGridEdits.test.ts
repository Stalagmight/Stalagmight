import { describe, it, expect } from 'vitest';
import {
  applyCylinderEdits,
  clampUsableBar,
  CYL_COL,
  isUsableEditable,
  parseCylRole,
  type CylinderEditContext,
} from '../cylindersGridEdits';
import { pressureToDisplay } from '../../../utils/formatGas';
import type { Cylinder, CylRole } from '../../../types/ipc.generated';

/** The shipped English labels (`gasPanel.roles.*`), which is what a diver would
 *  type or paste into the role column. */
const ROLE_LABELS: Record<CylRole, string> = {
  back_gas: 'Back gas',
  stage: 'Stage',
  dil_out: 'Dil-out',
  bailout: 'Bailout',
  deco: 'Deco',
};

const cyl = (over: Partial<Cylinder> = {}): Cylinder => ({
  water_volume_l: 11.1,
  rated_pressure_bar: 207,
  fill_bar: 207,
  material: 'aluminium',
  role: 'back_gas',
  label: 'AL80',
  enabled: true,
  usable_gas_bar: 0,
  ...over,
});

const ctx = (over: Partial<CylinderEditContext> = {}): CylinderEditContext => ({
  units: 'metric',
  isFree: true,
  conservative: true,
  roleLabels: ROLE_LABELS,
  ...over,
});

describe('parseCylRole', () => {
  it('accepts the role id in any casing or separator shape', () => {
    expect(parseCylRole('dil_out', 'back_gas', ROLE_LABELS)).toBe('dil_out');
    expect(parseCylRole(' DIL OUT ', 'back_gas', ROLE_LABELS)).toBe('dil_out');
    expect(parseCylRole('back-gas', 'deco', ROLE_LABELS)).toBe('back_gas');
  });

  it('accepts the localized label as displayed', () => {
    expect(parseCylRole('Back gas', 'deco', ROLE_LABELS)).toBe('back_gas');
    expect(parseCylRole('Dil-out', 'deco', ROLE_LABELS)).toBe('dil_out');
    expect(parseCylRole('bailout', 'deco', ROLE_LABELS)).toBe('bailout');
  });

  it('accepts an unambiguous prefix of an id or a label', () => {
    // Only "Stage" starts with s across both ids and labels.
    expect(parseCylRole('s', 'deco', ROLE_LABELS)).toBe('stage');
    expect(parseCylRole('bac', 'deco', ROLE_LABELS)).toBe('back_gas');
    expect(parseCylRole('de', 'stage', ROLE_LABELS)).toBe('deco');
    expect(parseCylRole('di', 'stage', ROLE_LABELS)).toBe('dil_out');
  });

  it('skips an ambiguous prefix rather than guessing', () => {
    // "Back gas" and "Bailout" both start with b, and both with "ba".
    expect(parseCylRole('b', 'deco', ROLE_LABELS)).toBeUndefined();
    expect(parseCylRole('ba', 'deco', ROLE_LABELS)).toBeUndefined();
    // "Deco" and "Dil-out" both start with d.
    expect(parseCylRole('d', 'stage', ROLE_LABELS)).toBeUndefined();
  });

  it('treats an empty cell as the current role, and skips nonsense', () => {
    expect(parseCylRole('', 'stage', ROLE_LABELS)).toBe('stage');
    expect(parseCylRole('   ', 'deco', ROLE_LABELS)).toBe('deco');
    expect(parseCylRole('sidemount', 'deco', ROLE_LABELS)).toBeUndefined();
  });
});

describe('isUsableEditable', () => {
  it("is the diver's to set only for a carried budget tank under Free", () => {
    expect(isUsableEditable(cyl(), true)).toBe(true);
    expect(isUsableEditable(cyl(), false)).toBe(false);
    expect(isUsableEditable(cyl({ enabled: false }), true)).toBe(false);
    expect(isUsableEditable(cyl({ role: 'deco' }), true)).toBe(false);
    expect(isUsableEditable(cyl({ role: 'stage' }), true)).toBe(true);
  });
});

describe('clampUsableBar', () => {
  it('caps an overfill to the rated pressure while conservative gas is on', () => {
    const overfilled = cyl({ fill_bar: 240, rated_pressure_bar: 207 });
    expect(clampUsableBar(250, overfilled, true)).toBe(207);
    expect(clampUsableBar(250, overfilled, false)).toBe(240);
    expect(clampUsableBar(150, overfilled, true)).toBe(150);
  });

  it('allows zero and never goes negative', () => {
    expect(clampUsableBar(0, cyl(), true)).toBe(0);
    expect(clampUsableBar(-40, cyl(), true)).toBe(0);
  });
});

describe('applyCylinderEdits - fill', () => {
  it('converts a psi display value to canonical bar', () => {
    const rows = [cyl({ fill_bar: 200 })];
    const next = applyCylinderEdits(
      rows,
      [{ row: 0, colKey: CYL_COL.fill, value: 3000 }],
      ctx({ units: 'imperial' })
    );
    expect(next[0]?.fill_bar).toBeCloseTo(206.8427, 3);
  });

  it('round-trips a metric display value unchanged', () => {
    const next = applyCylinderEdits(
      [cyl({ fill_bar: 200 })],
      [{ row: 0, colKey: CYL_COL.fill, value: pressureToDisplay(232, 'metric') }],
      ctx()
    );
    expect(next[0]?.fill_bar).toBe(232);
  });

  it('skips a non-positive fill - the backend requires one above zero', () => {
    const rows = [cyl({ fill_bar: 200 })];
    expect(applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.fill, value: 0 }], ctx())).toBe(rows);
    expect(applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.fill, value: -5 }], ctx())).toBe(
      rows
    );
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.fill, value: Number.NaN }], ctx())
    ).toBe(rows);
  });
});

describe('applyCylinderEdits - usable gas', () => {
  it('clamps to the conservative ceiling, in canonical bar', () => {
    const rows = [cyl({ fill_bar: 240, rated_pressure_bar: 207, usable_gas_bar: 100 })];
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.usable, value: 250 }], ctx())[0]
        ?.usable_gas_bar
    ).toBe(207);
    expect(
      applyCylinderEdits(
        rows,
        [{ row: 0, colKey: CYL_COL.usable, value: 250 }],
        ctx({ conservative: false })
      )[0]?.usable_gas_bar
    ).toBe(240);
  });

  it('clamps a psi edit after converting it', () => {
    const rows = [cyl({ fill_bar: 200, rated_pressure_bar: 207, usable_gas_bar: 100 })];
    const next = applyCylinderEdits(
      rows,
      [{ row: 0, colKey: CYL_COL.usable, value: 5000 }],
      ctx({ units: 'imperial' })
    );
    expect(next[0]?.usable_gas_bar).toBe(200);
  });

  it('allows zero and floors a negative', () => {
    const rows = [cyl({ usable_gas_bar: 100 })];
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.usable, value: 0 }], ctx())[0]
        ?.usable_gas_bar
    ).toBe(0);
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.usable, value: -20 }], ctx())[0]
        ?.usable_gas_bar
    ).toBe(0);
  });

  it('refuses to write a row whose usable gas the engine computes', () => {
    const rows = [cyl({ usable_gas_bar: 100 })];
    expect(
      applyCylinderEdits(
        rows,
        [{ row: 0, colKey: CYL_COL.usable, value: 50 }],
        ctx({ isFree: false })
      )
    ).toBe(rows);
    const decoRows = [cyl({ role: 'deco', usable_gas_bar: 100 })];
    expect(
      applyCylinderEdits(decoRows, [{ row: 0, colKey: CYL_COL.usable, value: 50 }], ctx())
    ).toBe(decoRows);
  });
});

describe('applyCylinderEdits - enabled and role', () => {
  it('takes a boolean straight, and a pasted string through the token table', () => {
    const rows = [cyl()];
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.enabled, value: false }], ctx())[0]
        ?.enabled
    ).toBe(false);
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.enabled, value: 'no' }], ctx())[0]?.enabled
    ).toBe(false);
    // A cleared cell is a no-op, so Delete over the table cannot switch a tank off.
    expect(applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.enabled, value: '' }], ctx())).toBe(
      rows
    );
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.enabled, value: 'maybe' }], ctx())
    ).toBe(rows);
  });

  it('takes a CylRole straight through, and parses a typed label', () => {
    const rows = [cyl()];
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.role, value: 'stage' }], ctx())[0]?.role
    ).toBe('stage');
    expect(
      applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.role, value: 'Dil-out' }], ctx())[0]?.role
    ).toBe('dil_out');
    expect(applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.role, value: 'b' }], ctx())).toBe(
      rows
    );
    expect(applyCylinderEdits(rows, [{ row: 0, colKey: CYL_COL.role, value: 42 }], ctx())).toBe(
      rows
    );
  });
});

describe('applyCylinderEdits - batches', () => {
  it('applies edits in order, so usable gas clamps against the new fill', () => {
    const rows = [cyl({ fill_bar: 100, rated_pressure_bar: 300, usable_gas_bar: 0 })];
    const next = applyCylinderEdits(
      rows,
      [
        { row: 0, colKey: CYL_COL.fill, value: 250 },
        { row: 0, colKey: CYL_COL.usable, value: 240 },
      ],
      ctx()
    );
    expect(next[0]?.fill_bar).toBe(250);
    expect(next[0]?.usable_gas_bar).toBe(240);
  });

  it('returns the identical array when no edit changed anything', () => {
    const rows = [cyl({ fill_bar: 207, usable_gas_bar: 50 }), cyl({ role: 'stage' })];
    const next = applyCylinderEdits(
      rows,
      [
        { row: 0, colKey: CYL_COL.fill, value: 207 },
        { row: 0, colKey: CYL_COL.usable, value: 50 },
        { row: 1, colKey: CYL_COL.role, value: 'Stage' },
        { row: 1, colKey: CYL_COL.enabled, value: true },
        { row: 7, colKey: CYL_COL.fill, value: 100 },
        { row: 0, colKey: CYL_COL.tank, value: 'nonsense' },
      ],
      ctx()
    );
    expect(next).toBe(rows);
  });
});
