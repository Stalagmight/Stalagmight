/**
 * The station-index domains the zero-datum import has to agree on (#24).
 *
 * `ZeroDatumConfigDialog` writes ONE `stationIndex` from two pickers, and used
 * to feed it to a third consumer, each in a different index space:
 *   - the dropdown counts surviving editable rows (`survivors + 1` options);
 *   - `SectionPreview` numbers the dots it draws, i.e. the section's
 *     `shot_type === 'Standard'` shots, from 0;
 *   - `adjustStationForDeletedShots` maps from the `EditableShot.id` space,
 *     which is a position in the `isImportableDmpShot`-filtered file order.
 *
 * Those three coincide only for a section with no non-Standard shots, no
 * length-1 shots and no deletions. This file pins the first two back together:
 * the preview is now built with `editableShotsToDmpSection`, so the dots it
 * draws are exactly the dialog's stations.
 *
 * `.tsx` so vitest runs it under jsdom - `SectionPreview` imports maplibre-gl at
 * module load, which the node tier cannot execute.
 */
import { describe, it, expect, vi } from 'vitest';

vi.mock('maplibre-gl', () => ({ default: {} }));

import { computeStationsFromDmpShots } from '../../components/Dialogs/SectionPreview';
import { convertToEditableShots, editableShotsToDmpSection } from '../shotConversion';
import { blankEditableShot } from '../../components/common/editableShot';
import { isEmittedShot } from '../../components/common/emittedShots';
import type { DmpSection, DmpShot } from '../../types/commands';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import { makeDmpSection, makeDmpShot } from '../../test/factories/dmp';

function standardShot(length: number): DmpShot {
  return makeDmpShot({ length });
}

function section(shots: DmpShot[]): DmpSection {
  return makeDmpSection({ shots });
}

/** The dialog's `stationCount`: one station per EMITTED shot, plus the far end.
 *  The same predicate the payload builder and the preview use - the three
 *  disagreeing is the whole subject of this file. */
function dialogStationCount(shots: EditableShot[]): number {
  return shots.filter(isEmittedShot).length + 1;
}

/** The dots `SectionPreview` actually draws for a section. */
function clickableDots(preview: DmpSection): number {
  return computeStationsFromDmpShots(preview.shots.filter((s) => s.shot_type === 'Standard')).length;
}

describe('zero-datum station domains', () => {
  it('a length-1 shot desynchronises the raw preview from the shot table, with nothing deleted', () => {
    // A plain DMP section: four Standard shots, one of them the length-1 filler
    // the importer skips by default.
    const raw = section([standardShot(5), standardShot(1), standardShot(5), standardShot(5)]);
    const editable = convertToEditableShots(raw.shots, raw.version);

    expect(editable).toHaveLength(3);
    expect(dialogStationCount(editable)).toBe(4);

    // What the preview used to draw: the raw section, five clickable dots for a
    // dialog that only offers four stations. Clicking the last dot committed a
    // station the dropdown cannot even name.
    expect(clickableDots(raw)).toBe(5);

    // What it draws now.
    expect(clickableDots(editableShotsToDmpSection(raw, editable))).toBe(dialogStationCount(editable));
  });

  it('a deletion keeps the dots and the dropdown in step', () => {
    const raw = section([standardShot(5), standardShot(5), standardShot(5)]);
    const editable = convertToEditableShots(raw.shots, raw.version);
    const afterDelete = editable.map((s, i) => (i === 0 ? { ...s, isDeleted: true } : s));

    expect(dialogStationCount(afterDelete)).toBe(3);
    expect(clickableDots(raw)).toBe(4);
    expect(clickableDots(editableShotsToDmpSection(raw, afterDelete))).toBe(3);
  });

  it('emits every surviving row as Standard so the preview filter cannot drop one', () => {
    const raw = section([standardShot(5), standardShot(5)]);
    const editable = convertToEditableShots(raw.shots, raw.version);
    // A row the user re-typed to length 1 must still be drawn and still be
    // sent: `isImportableDmpShot` is an import-time default, not a live filter.
    const edited = editable.map((s, i) => (i === 0 ? { ...s, length: 1 } : s));

    const preview = editableShotsToDmpSection(raw, edited);
    expect(preview.shots.every((s) => s.shot_type === 'Standard')).toBe(true);
    expect(clickableDots(preview)).toBe(dialogStationCount(edited));
  });

  /**
   * A row added and left blank is dropped from the payload, so drawing it put a
   * NaN length and NaN azimuths into the preview - and every station is computed
   * from the last, so the whole centerline after it collapsed to NaN and
   * `fitBounds` was handed NaNs. It also drew one station more than the import
   * would create, so a click on the preview committed a station index one past
   * the one meant.
   */
  it('drops a blank new row from the preview geometry', () => {
    const raw = section([standardShot(5), standardShot(5)]);
    const withBlank = [...convertToEditableShots(raw.shots, raw.version), blankEditableShot(99)];

    const preview = editableShotsToDmpSection(raw, withBlank);
    expect(preview.shots).toHaveLength(2);

    const stations = computeStationsFromDmpShots(preview.shots);
    expect(stations).toHaveLength(dialogStationCount(withBlank));
    expect(stations.every((s) => Number.isFinite(s.x) && Number.isFinite(s.y) && Number.isFinite(s.z))).toBe(true);
  });

  it('keeps a partly filled row - it is not blank, and the gate blocks Confirm', () => {
    // Only a WHOLLY blank row is dropped. A row holding one measurement is the
    // user's work in progress: it is drawn, counted and refused at Confirm,
    // which is the one place that can say why.
    const raw = section([standardShot(5)]);
    const partial = { ...blankEditableShot(99), length: 7 };
    const rows = [...convertToEditableShots(raw.shots, raw.version), partial];
    expect(editableShotsToDmpSection(raw, rows).shots).toHaveLength(2);
    expect(dialogStationCount(rows)).toBe(3);
  });

  it('carries the raw section identity through, only rewriting the geometry', () => {
    const raw = section([standardShot(5), standardShot(5)]);
    const preview = editableShotsToDmpSection(raw, convertToEditableShots(raw.shots, raw.version));
    expect(preview.name).toBe(raw.name);
    expect(preview.content_hash).toBe(raw.content_hash);
    expect(preview.direction).toBe(raw.direction);
  });
});

/**
 * A gap in the measurements costs one leg, not the whole section.
 *
 * Stations are computed from one another, so a single non-finite contribution
 * reaches every station after it: the centerline disappears and the map is asked
 * to fit bounds it cannot compute. Filtering unmeasured rows out upstream is the
 * fix for the source we know about (above); this is the backstop for the next
 * one, and it keeps the station COUNT intact so indices still address the rows
 * they came from.
 */
describe('computeStationsFromDmpShots with unmeasured shots', () => {
  it('carries the previous position through a wholly unmeasured shot', () => {
    const unmeasured: DmpShot = {
      ...standardShot(NaN),
      heading_in: NaN,
      heading_out: NaN,
      depth_in: NaN,
      depth_out: NaN,
    };
    const stations = computeStationsFromDmpShots([standardShot(5), unmeasured, standardShot(5)]);

    expect(stations).toHaveLength(4);
    expect(stations[2]).toEqual(stations[1]);
    // The measured shot after the gap still runs its own length from there.
    expect(stations[3]!.x).toBeCloseTo(2 * stations[1]!.x, 10);
    expect(stations[3]!.y).toBeCloseTo(2 * stations[1]!.y, 10);
  });

  it('keeps the depth of a shot whose bearing is missing', () => {
    const noBearing: DmpShot = {
      ...standardShot(5),
      heading_in: NaN,
      heading_out: NaN,
      depth_out: 3,
    };
    expect(computeStationsFromDmpShots([noBearing])[1]).toEqual({ x: 0, y: 0, z: 3 });
  });
});
