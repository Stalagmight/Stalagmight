/**
 * A cleared date leaves the wizard as `null`, never as an empty string - on
 * BOTH of the wizard's payload emitters.
 *
 * `date` is `Option<NaiveDate>` on the Rust side, and chrono parses `""` as
 * TOO_SHORT ("premature end of input"), so an empty string is not "no date":
 * it is a malformed argument, and Tauri rejects the WHOLE `import_sections`
 * call before it reaches the command. The user sees an import that fails with
 * an error naming nothing, after clearing a date field (#478).
 *
 * The wizard emits section metadata from two places - `buildBatchImport` for
 * the batch, and the segment-placement handoff for a section dropped on the
 * map - and only one of them was fixed the first time. They now share
 * `resolveImportSectionDate`, so this file checks the rule itself and then
 * checks that each emitter is actually wired to it: a re-inlined `??` chain in
 * either one fails here.
 *
 * The second emitter lives in `buildSegmentPlacementPayload`, a `useCallback`
 * closed over a dozen refs and reachable from no test. Its metadata builder is
 * now `buildPlacementSectionData`, which is - a test-enablement extraction, no
 * behaviour moved with it.
 */
import { describe, it, expect, vi } from 'vitest';

// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. Stub it - this test exercises the payload builders, not the preview.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));

import { buildBatchImport } from '../DmpNewImportDialog';
import {
  buildPlacementSectionData,
  resolveImportSectionDate,
  type SectionEditState,
} from '../../../utils/batchImportHelpers';
import type { EditableShot } from '../../common/ShotTableEditor';
import type { DmpFile } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { VirtualConnection } from '../../../utils/segmentContext';
import type { ShotData } from '../../../types/project';

/** The date the DMP file itself carries, as a full timestamp. */
const FILE_DATE = '2026-02-01T09:30:00';
/** What the wizard default would contribute if the chain ever reached it. */
const DEFAULT_DATE = '2026-03-04';

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

const dmpSection = () => ({
  name: 'Sump',
  date: FILE_DATE,
  direction: 'in' as const,
  shots: Array.from({ length: 2 }, () => ({
    // `Standard`, and never length 1: `isImportableDmpShot` drops everything
    // else, and a section with no importable shots is skipped before it can
    // emit a date at all.
    shot_type: 'Standard',
    length: 5,
    heading_in: 10,
    heading_out: 10,
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

const editsWithDate = (date: string): SectionEditState =>
  ({
    name: 'Sump',
    surveyors: [{ name: 'Diver' }],
    date,
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    reverse: false,
    shots: [measured(0), measured(1)],
  }) as unknown as SectionEditState;

/**
 * The batch emitter, run over one section that connects to an existing one.
 * `editedDate` of `undefined` means "no edits recorded for this section", which
 * is the route that reaches `defaultDate`.
 */
function batchDate(opts: { editedDate?: string; defaultDate?: string | null }): string | null {
  const dmpFile = { version: 1, sections: [dmpSection()] } as unknown as DmpFile;
  const selectionConfig: DmpSelectionConfig = {
    selectedIndices: new Set([0]),
    defaultSurveyors: [{ name: 'Diver' }],
    defaultExplorers: [],
    defaultDate: opts.defaultDate ?? null,
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
  const sectionEdits =
    opts.editedDate === undefined
      ? undefined
      : new Map<number, SectionEditState>([[0, editsWithDate(opts.editedDate)]]);

  const { configs } = buildBatchImport(connections, null, dmpFile, selectionConfig, sectionEdits);
  expect(configs).toHaveLength(1);
  return configs[0]!.date;
}

/** The segment-placement emitter, over the same inputs. */
function placementDate(opts: { editedDate?: string; defaultDate?: string | null }): string | null {
  return buildPlacementSectionData({
    dmpSection: dmpSection(),
    edits: opts.editedDate === undefined ? undefined : editsWithDate(opts.editedDate),
    defaultDate: opts.defaultDate ?? null,
    palette: ['#4ecdc4'],
    paletteIndex: 0,
    shots: [] as ShotData[],
  }).date;
}

describe('resolveImportSectionDate', () => {
  it('turns a cleared date into null rather than an empty string', () => {
    expect(resolveImportSectionDate('', null, FILE_DATE)).toBeNull();
  });

  it('does not let a cleared date fall back to the default or the file', () => {
    // The `??` half: "" is not nullish, so the chain stops there - clearing the
    // field has to mean cleared, not "use the file's date after all".
    expect(resolveImportSectionDate('', DEFAULT_DATE, FILE_DATE)).toBeNull();
  });

  it('nulls an empty wizard default the same way', () => {
    expect(resolveImportSectionDate(undefined, '', FILE_DATE)).toBeNull();
  });

  it('keeps a real edited date exactly as typed', () => {
    expect(resolveImportSectionDate('2026-07-19', DEFAULT_DATE, FILE_DATE)).toBe('2026-07-19');
  });

  it('falls back to the wizard default, then to the file day', () => {
    expect(resolveImportSectionDate(undefined, DEFAULT_DATE, FILE_DATE)).toBe(DEFAULT_DATE);
    expect(resolveImportSectionDate(undefined, null, FILE_DATE)).toBe('2026-02-01');
    expect(resolveImportSectionDate(undefined, null, '2026-02-01 09:30:00')).toBe('2026-02-01');
  });

  it('is null when nothing anywhere carries a date', () => {
    expect(resolveImportSectionDate(undefined, null, null)).toBeNull();
  });
});

describe('buildBatchImport section date', () => {
  it('sends null for a cleared date field', () => {
    expect(batchDate({ editedDate: '' })).toBeNull();
  });

  it('sends null rather than the file date a cleared field overrode', () => {
    expect(batchDate({ editedDate: '', defaultDate: DEFAULT_DATE })).toBeNull();
  });

  it('sends null for an empty wizard default', () => {
    expect(batchDate({ defaultDate: '' })).toBeNull();
  });

  it('still sends a real date unchanged', () => {
    expect(batchDate({ editedDate: '2026-07-19' })).toBe('2026-07-19');
    expect(batchDate({ defaultDate: DEFAULT_DATE })).toBe(DEFAULT_DATE);
  });
});

describe('buildPlacementSectionData section date', () => {
  it('sends null for a cleared date field', () => {
    expect(placementDate({ editedDate: '' })).toBeNull();
  });

  it('sends null rather than the file date a cleared field overrode', () => {
    expect(placementDate({ editedDate: '', defaultDate: DEFAULT_DATE })).toBeNull();
  });

  it('sends null for an empty wizard default', () => {
    expect(placementDate({ defaultDate: '' })).toBeNull();
  });

  it('still sends a real date unchanged', () => {
    expect(placementDate({ editedDate: '2026-07-19' })).toBe('2026-07-19');
    expect(placementDate({ defaultDate: DEFAULT_DATE })).toBe(DEFAULT_DATE);
  });

  it('carries the rest of the metadata across unchanged', () => {
    // A guard on the extraction itself: the date is not the only field the
    // handoff sends, and moving the builder must not have dropped one.
    const data = buildPlacementSectionData({
      dmpSection: dmpSection(),
      edits: undefined,
      defaultDate: null,
      palette: ['#111111', '#222222'],
      paletteIndex: 1,
      shots: [],
    });
    expect(data).toMatchObject({
      name: 'Sump',
      // Overlay-only data: the people ride on the batch import, not here.
      surveyor: null,
      color: '#222222',
      direction: 'in',
      quality: 1.0,
      shots: [],
    });
  });

  it('falls back to the default colour for an anchor outside the selection', () => {
    // `indexOf` answers -1 for a section that is not selected, and the raw
    // index is what the handoff has always used. Pinned so the extraction
    // cannot quietly "fix" it into palette slot 0.
    const data = buildPlacementSectionData({
      dmpSection: dmpSection(),
      edits: undefined,
      defaultDate: null,
      palette: ['#111111', '#222222'],
      paletteIndex: -1,
      shots: [],
    });
    expect(data.color).toBe('#4ecdc4');
  });
});
