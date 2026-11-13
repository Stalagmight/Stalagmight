/**
 * The mNemo wizard's people reach the backend: every section config the batch
 * builds carries the section's Surveyors / Explorers pills (or the step-1
 * defaults when the section never got lists of its own) and its date as the
 * `ImportSectionConfig` overrides of every imported shot. The legacy
 * free-text `surveyor` is never sent.
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
import type { EditableShot } from '../../common/ShotTableEditor';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import type { DmpFile } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { Person } from '../../../types/ipc.generated';
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

const dmpSection = (name: string) => ({
  name,
  date: '2025-07-04T10:00:00',
  direction: 'in',
  shots: Array.from({ length: 2 }, () => ({
    shot_type: 'Standard',
    length: 5,
    heading_in: 10,
    heading_out: 10,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    temperature: 0,
    marker_index: 0,
  })),
});

const DEFAULT_SURVEYORS: Person[] = [{ name: 'Default Surveyor' }];
const DEFAULT_EXPLORERS: Person[] = [{ name: 'Default Explorer', email: 'de@example.org' }];

function build(edits: Partial<SectionEditState> | undefined, defaultDate: string | null = null) {
  const dmpFile = { version: 1, sections: [dmpSection('S')] } as unknown as DmpFile;
  const selectionConfig: DmpSelectionConfig = {
    selectedIndices: new Set([0]),
    defaultSurveyors: DEFAULT_SURVEYORS,
    defaultExplorers: DEFAULT_EXPLORERS,
    defaultDate,
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
    edits === undefined
      ? undefined
      : new Map<number, SectionEditState>([
          [
            0,
            {
              name: 'S',
              date: '2026-01-01',
              color: '#ffffff',
              direction: 'in',
              quality: 1,
              reverse: false,
              shots: [measured(0), measured(1)],
              ...edits,
            },
          ],
        ]);
  const { configs } = buildBatchImport(connections, null, dmpFile, selectionConfig, sectionEdits);
  expect(configs).toHaveLength(1);
  return configs[0]!;
}

describe('buildBatchImport survey credit', () => {
  it("sends the section's own pills as overrides, never the legacy text", () => {
    // Injection: build the config with `surveyor: <joined names>` and no
    // `...importCreditFields(...)`, and the people never reach a shot.
    const config = build({
      surveyors: [{ name: 'Ann' }, { name: 'Bo', email: 'bo@example.org' }],
      explorers: [{ name: 'Cy' }],
    });
    expect(config.surveyor).toBeNull();
    expect(config.surveyors).toEqual([{ name: 'Ann' }, { name: 'Bo', email: 'bo@example.org' }]);
    expect(config.explorers).toEqual([{ name: 'Cy' }]);
    expect(config.override_date).toBe('2026-01-01');
  });

  it('falls back to the step-1 defaults for a section without edits', () => {
    const config = build(undefined, '2026-03-02');
    expect(config.surveyors).toEqual(DEFAULT_SURVEYORS);
    expect(config.explorers).toEqual(DEFAULT_EXPLORERS);
    expect(config.override_date).toBe('2026-03-02');
    expect(config.date).toBe('2026-03-02');
  });

  it('keeps the defaults for an edit state without lists (one overlap treatment created)', () => {
    // Overlap treatment builds edit states that know nothing about people.
    // Injection: seed them with `surveyors: []` again and the defaults vanish
    // from every overlap-treated section.
    const config = build({});
    expect(config.surveyors).toEqual(DEFAULT_SURVEYORS);
    expect(config.explorers).toEqual(DEFAULT_EXPLORERS);
  });

  it('sends nothing for a role the user emptied, so the file credit stays', () => {
    // An emptied list is a choice, not "use the defaults"; and an empty list is
    // not sent at all (it would replace the file's people with nobody).
    const config = build({ surveyors: [], explorers: [] });
    expect('surveyors' in config).toBe(false);
    expect('explorers' in config).toBe(false);
  });

  it('sends no date override for a cleared date', () => {
    const config = build({ date: '' });
    expect(config.date).toBeNull();
    expect('override_date' in config).toBe(false);
  });
});
