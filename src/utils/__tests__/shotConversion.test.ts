import { describe, it, expect } from 'vitest';
import type { DmpShot } from '../../types/commands';
import { blankEditableShot } from '../../components/common/editableShot';
import { convertToEditableShots, editableShotToShotData } from '../shotConversion';
import { validateEditableShot } from '../validation';
import { makeDmpShot as dmpShotOf } from '../../test/factories/dmp';

function makeDmpShot(overrides?: Partial<DmpShot>): DmpShot {
  return dmpShotOf({ heading_in: 0, heading_out: 0, length: 2, ...overrides });
}

describe('convertToEditableShots', () => {
  it('excludes standard shots with length 1 by default', () => {
    const editableShots = convertToEditableShots([
      makeDmpShot({ length: 1 }),
      makeDmpShot({ length: 2.5 }),
    ], 5);

    expect(editableShots).toHaveLength(1);
    expect(editableShots[0]?.length).toBe(2.5);
    expect(editableShots[0]?.id).toBe(0);
  });

  it('keeps non-length-1 standard shots and reindexes IDs', () => {
    const editableShots = convertToEditableShots([
      makeDmpShot({ length: 3 }),
      makeDmpShot({ length: 1 }),
      makeDmpShot({ length: 4 }),
    ], 5);

    expect(editableShots).toHaveLength(2);
    expect(editableShots[0]?.id).toBe(0);
    expect(editableShots[1]?.id).toBe(1);
    expect(editableShots[0]?.length).toBe(3);
    expect(editableShots[1]?.length).toBe(4);
  });
});

// #576 B24: the mNemo records a water temperature (v3+) and a marker index on
// every shot, and the model has a slot for each (`Shot.temperature`,
// `Shot.marker_index`). They ride through the wizard as read-only pass-through
// fields, so a row the user did not add keeps the device's values all the way
// to the `import_sections` payload.
describe('DMP device fields through the wizard conversion', () => {
  it('keeps the device temperature and marker index on an imported row', () => {
    const [row] = convertToEditableShots([makeDmpShot({ temperature: 24.5, marker_index: 3 })], 5);
    const sent = editableShotToShotData(row!);
    expect({ temperature: sent.temperature, marker_index: sent.marker_index }).toEqual({
      temperature: 24.5,
      marker_index: 3,
    });
  });

  it('keeps them across a cell edit, which spreads the row', () => {
    const [row] = convertToEditableShots([makeDmpShot({ temperature: 18, marker_index: 7 })], 4);
    const edited = { ...row!, length: 9.5 };
    const sent = editableShotToShotData(edited);
    expect(sent.length).toBe(9.5);
    expect(sent.temperature).toBe(18);
    expect(sent.marker_index).toBe(7);
  });

  // A PIN, not a B24 regression test: a blank row sent nulls before the B24
  // fix too, so this passes without it (#576 RV2-FE-D5). It keeps the fix
  // from inventing device fields for a row the device never recorded.
  it('pin: sends none for a row added in the wizard', () => {
    const sent = editableShotToShotData({ ...blankEditableShot(-1), length: 3 });
    expect(sent.temperature).toBeNull();
    expect(sent.marker_index).toBeNull();
  });

  it('sends no temperature for a v2 dump, which records none', () => {
    // A v2 record has no temperature bytes; the parser fills 0.0, which is
    // not a reading. The marker index IS in a v2 record.
    const [row] = convertToEditableShots([makeDmpShot({ temperature: 0, marker_index: 2 })], 2);
    const sent = editableShotToShotData(row!);
    expect(sent.temperature).toBeNull();
    expect(sent.marker_index).toBe(2);
  });

  // #576 NF-AN4-3 (decision: v2/v3 LRUD imports as unmeasured): a v2 or v3
  // record has no LRUD bytes (they arrive with v4), and the parser fills 0,
  // which reached the project as a MEASURED zero-width passage on every shot.
  it('sends no LRUD for a v2 or v3 dump, which records none', () => {
    for (const version of [2, 3]) {
      const [row] = convertToEditableShots([makeDmpShot()], version);
      const sent = editableShotToShotData(row!);
      expect({ version, lrud: [sent.left, sent.right, sent.up, sent.down] }).toEqual({
        version,
        lrud: [null, null, null, null],
      });
    }
  });

  it('keeps a v4 LRUD, zeros included, as measured', () => {
    const [row] = convertToEditableShots([makeDmpShot({ left: 1.5, right: 0, up: 2, down: 0.4 })], 4);
    const sent = editableShotToShotData(row!);
    expect([sent.left, sent.right, sent.up, sent.down]).toEqual([1.5, 0, 2, 0.4]);
  });

  // A PIN, not a B24 regression test: the conversion never flipped the pitch,
  // so this passes without the fix (#576 RV2-FE-D5). It guards the frame
  // against a future "correction" that would invert every slope.
  it('pin: carries the pitch in the frame the parser left it, unflipped', () => {
    // `parse_section` already turned the device's +up reading into the
    // model's +down frame; a second flip here would invert every slope.
    const [row] = convertToEditableShots(
      [makeDmpShot({ length: 12.5, depth_in: 14.51, depth_out: 18.15, pitch_in: 16.9, pitch_out: 16.9 })],
      5,
    );
    const sent = editableShotToShotData(row!);
    expect(sent.pitch_in).toBe(16.9);
    expect(sent.pitch_out).toBe(16.9);
    expect(sent.depth_out - sent.depth_in).toBeGreaterThan(0);
  });
});

// #576 RV2-FE-D7. A DMP heading is a raw `i16 * 0.1`, so a damaged dump can carry
// 400 or -30. The core import gate wraps every azimuth into [0, 360) (B24-D2), so
// the import stores 40 / 330 - while the wizard's table showed the raw reading
// with the "must be between 0 and 360" error flag. Wrapped at the conversion, the
// table shows what will be stored and flags nothing the import accepts.
describe('DMP headings outside [0, 360) through the wizard conversion', () => {
  it('shows the wrapped bearing the import gate will store, with no range error', () => {
    const [row] = convertToEditableShots([makeDmpShot({ heading_in: 400, heading_out: -30 })], 5);
    expect({ azimuth_in: row!.azimuth_in, azimuth_out: row!.azimuth_out }).toEqual({
      azimuth_in: 40,
      azimuth_out: 330,
    });
    const rangeErrors = validateEditableShot(row!, 1, 0).filter((e) => e.field === 'azimuth_in' || e.field === 'azimuth_out');
    expect(rangeErrors).toEqual([]);
  });

  it('pin: leaves an in-range heading exactly as read', () => {
    const [row] = convertToEditableShots([makeDmpShot({ heading_in: 359.9, heading_out: 0 })], 5);
    expect([row!.azimuth_in, row!.azimuth_out]).toEqual([359.9, 0]);
  });
});
