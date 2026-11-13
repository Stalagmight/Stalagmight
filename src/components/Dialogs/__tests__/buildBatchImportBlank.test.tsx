/**
 * A row added in the wizard's shot table and left wholly blank must not be
 * imported.
 *
 * The completeness gate deliberately lets such a row through - a row you added
 * and never filled is an invitation, not an obligation, and refusing to save
 * because of it was the bug. That is only safe if every payload path drops the
 * row before serializing. The zero-datum and segment-placement paths filtered
 * `isBlankNewShot`; `buildBatchImport` filtered only `isDeleted`, so a row typed
 * into and then cleared reached the backend as NaN length, depths and azimuths.
 */
import { describe, it, expect, vi } from 'vitest';

// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. Stub it - this test exercises the batch builder, not the preview.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));

import { buildBatchImport } from '../DmpNewImportDialog';
import { NonFiniteShotError } from '../../../utils/importPayloadGuard';
import type { EditableShot } from '../../common/ShotTableEditor';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import type { DmpFile } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { VirtualConnection } from '../../../utils/segmentContext';

const measured = (id: number): EditableShot =>
  ({
    id,
    azimuth_in: 10,
    azimuth_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
  }) as unknown as EditableShot;

/** What the trailing blank row materializes into: NaN measurements, isNew. */
const blank = (id: number): EditableShot =>
  ({
    ...measured(id),
    isNew: true,
    length: NaN,
    azimuth_in: NaN,
    azimuth_out: NaN,
    depth_in: NaN,
    depth_out: NaN,
  }) as unknown as EditableShot;

const dmpSection = (name: string, shots: number) => ({
  name,
  shots: Array.from({ length: shots }, () => ({
    length: 5,
    azimuth_in: 10,
    azimuth_out: 10,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    temperature: 0,
    marker_index: 0,
  })),
});

function build(shots: EditableShot[]) {
  const dmpFile = { version: 1, sections: [dmpSection('S', 2)] } as unknown as DmpFile;
  const selectionConfig: DmpSelectionConfig = {
    selectedIndices: new Set([0]),
    defaultSurveyors: [{ name: 'S' }],
    defaultExplorers: [],
    defaultDate: null,
    fileHash: null,
    sectionHashes: [],
  };
  const connections: VirtualConnection[] = [
    {
      importSectionIndex: 0,
      importStationIndex: 0,
      targetType: 'existing',
      existingSectionId: 99,
      existingStationIndex: 0,
    },
  ];
  const sectionEdits = new Map<number, SectionEditState>([
    [
      0,
      {
        name: 'S',
        surveyors: [{ name: 'S' }],
        date: '2026-01-01',
        color: '#ffffff',
        direction: 'in',
        quality: 1,
        reverse: false,
        shots,
      } as unknown as SectionEditState,
    ],
  ]);
  return buildBatchImport(connections, null, dmpFile, selectionConfig, sectionEdits);
}

/** Every shot the batch would send, across the section configs it produced. */
const emitted = (batch: { configs: { shots?: unknown[] }[] }) =>
  batch.configs.flatMap((c) => c.shots ?? []);

describe('buildBatchImport with a blank new row', () => {
  it('drops a wholly blank new row instead of sending NaN', () => {
    const shots = emitted(build([measured(0), measured(1), blank(2)]));
    expect(shots).toHaveLength(2);
    for (const s of shots) {
      for (const v of Object.values(s as Record<string, unknown>)) {
        expect(typeof v === 'number' && Number.isNaN(v)).toBe(false);
      }
    }
  });

  it('keeps the measured rows untouched', () => {
    expect(emitted(build([measured(0), measured(1)]))).toHaveLength(2);
  });

  it('refuses a partly filled row rather than dropping it or sending NaN', () => {
    // Half-filled is NOT blank - dropping it would throw away data the user
    // typed. The UI gates ask them to finish it; this last check makes sure no
    // route reaches IPC with the NaN, and says which section and row.
    const partial = { ...blank(2), length: 7 } as EditableShot;
    expect(() => build([measured(0), partial])).toThrow(NonFiniteShotError);
    expect(() => build([measured(0), partial])).toThrow(/Section "S" row 2/);
  });
});
