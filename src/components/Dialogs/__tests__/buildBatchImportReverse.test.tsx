/**
 * Reverse mirroring of placement indices in the DMP batch builder (#24).
 *
 * `buildBatchImport` mirrors a placement's TARGET station index when the target
 * section is itself created reversed by this batch. That rule only applies to a
 * placement derived from a virtual connection; a directPlacement (segment "From
 * existing", or a manual drop) addresses a live project section this batch never
 * renumbers. The regression these tests pin: the target mirror used to be driven
 * by `connectionMap.get(idx)` even when the emitted placement came from
 * `directPlacementMap`, so an unrelated auto-derived connection could mirror the
 * user's chosen station onto the wrong physical end of a live section.
 *
 * .tsx so vitest runs it under jsdom - the module under test pulls in React.
 */
import { describe, it, expect, vi } from 'vitest';

// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. Stub it - this test exercises the batch builder, not the preview.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));

import { buildBatchImport, type DirectPlacement } from '../DmpNewImportDialog';
import type { DmpFile, DmpSection, DmpShot, ImportPlacement } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import type { VirtualConnection } from '../../../utils/segmentContext';
import type { EditableShot } from '../../common/ShotTableEditor';

const TARGET_SHOTS = 8;
/** The station the user picked off the LIVE project. */
const PICKED_STATION = 5;
const LIVE_SECTION_ID = 42;

function dmpShot(): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 10,
    heading_out: 10,
    length: 5,
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
  };
}

function dmpSection(name: string, shotCount: number): DmpSection {
  return {
    version: 1,
    name,
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: shotCount }, dmpShot),
    content_hash: `hash-${name}`,
  };
}

/** `id` is the shot's 0-based index in the file - `adjustStationForDeletedShots` compares station indices against it. */
function editableShot(id: number): EditableShot {
  return {
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
  } as unknown as EditableShot;
}

function edits(name: string, shotCount: number, reverse: boolean): SectionEditState {
  return {
    name,
    surveyors: [{ name: 'S' }],
    date: '2026-01-01',
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    reverse,
    shots: Array.from({ length: shotCount }, (_, i) => editableShot(i)),
  };
}

/**
 * Section 0 is a segment the user placed by hand onto live section 42, station 5.
 * It ALSO has an auto-derived import-to-import connection to section 1, which is
 * being created reversed - the unrelated connection whose shot count must not
 * reach the direct placement.
 */
function build(directPlacements: DirectPlacement[] | undefined) {
  const dmpFile: DmpFile = {
    version: 1,
    sections: [dmpSection('Segment', 3), dmpSection('TargetB', TARGET_SHOTS)],
  };
  const selectionConfig: DmpSelectionConfig = {
    selectedIndices: new Set([0, 1]),
    defaultSurveyors: [{ name: 'S' }],
    defaultExplorers: [],
    defaultDate: null,
    fileHash: null,
    sectionHashes: [],
  };
  const connections: VirtualConnection[] = [
    // Anchors section 1 to the live project so the BFS reaches both sections.
    {
      importSectionIndex: 1,
      importStationIndex: 0,
      targetType: 'existing',
      existingSectionId: 99,
      existingStationIndex: 0,
    },
    // The auto-derived import-to-import connection whose reversed target must
    // NOT reach a direct placement.
    {
      importSectionIndex: 0,
      importStationIndex: 0,
      targetType: 'importing',
      targetImportIndex: 1,
      targetImportStationIndex: PICKED_STATION,
    },
  ];
  const sectionEdits = new Map<number, SectionEditState>([
    [0, edits('Segment', 3, false)],
    [1, edits('TargetB', TARGET_SHOTS, true)],
  ]);

  return buildBatchImport(
    connections,
    null,
    dmpFile,
    selectionConfig,
    sectionEdits,
    undefined,
    directPlacements
  );
}

const DIRECT: DirectPlacement = {
  anchorSectionIndex: 0,
  anchorStationIndex: 0,
  placement: {
    type: 'from_existing',
    import_station: 0,
    existing_station: { section_id: LIVE_SECTION_ID, station_index: PICKED_STATION },
  },
};

describe('buildBatchImport reverse mirroring', () => {
  it('leaves a direct placement onto a live section unmirrored', () => {
    const { configs } = build([DIRECT]);

    const segment = configs.find((c) => c.name === 'Segment');
    if (!segment) throw new Error('Segment config was not emitted');
    expect(segment.placement.type).toBe('from_existing');
    const placement = segment.placement as Extract<
      ImportPlacement,
      { type: 'from_existing' }
    >;
    expect(placement.existing_station.section_id).toBe(LIVE_SECTION_ID);
    // The bug mirrored this to TARGET_SHOTS - PICKED_STATION = 3, merging the
    // segment onto the wrong physical station of section 42.
    expect(placement.existing_station.station_index).toBe(PICKED_STATION);
  });

  it('still mirrors a connection-derived target that this batch reverses', () => {
    // Same fixture without the direct placement: section 0 now genuinely
    // connects to reversed section 1, so its target index must mirror.
    const { configs } = build(undefined);

    const segment = configs.find((c) => c.name === 'Segment');
    if (!segment) throw new Error('Segment config was not emitted');
    expect(segment.placement.type).toBe('from_importing');
    const placement = segment.placement as Extract<
      ImportPlacement,
      { type: 'from_importing' }
    >;
    expect(placement.target_station_index).toBe(TARGET_SHOTS - PICKED_STATION);
  });
});

describe('buildBatchImport station names', () => {
  it('names the stations of every emitted section after its MNemo numbers', () => {
    // Section 0 arrives with edits (its middle row deleted); section 1 without
    // (so it is converted straight from the file). Each row's MNemo number is
    // its 1-based position among the importable shots.
    const dmpFile: DmpFile = {
      version: 1,
      sections: [dmpSection('Edited', 3), dmpSection('Untouched', 2)],
    };
    const selectionConfig: DmpSelectionConfig = {
      selectedIndices: new Set([0, 1]),
      defaultSurveyors: [{ name: 'S' }],
      defaultExplorers: [],
      defaultDate: null,
      fileHash: null,
      sectionHashes: [],
    };
    const connections: VirtualConnection[] = [0, 1].map((importSectionIndex) => ({
      importSectionIndex,
      importStationIndex: 0,
      targetType: 'existing' as const,
      existingSectionId: 99,
      existingStationIndex: 0,
    }));
    const edited = edits('Edited', 3, false);
    edited.shots = edited.shots.map((shot, i) => ({
      ...shot,
      mnemoStation: i + 1,
      ...(i === 1 ? { isDeleted: true } : {}),
    }));

    const { configs } = buildBatchImport(
      connections,
      null,
      dmpFile,
      selectionConfig,
      new Map([[0, edited]]),
    );

    const named = (name: string) => configs.find((c) => c.name === name)?.station_names;
    expect(named('Edited')).toEqual([
      [0, '0'],
      [1, '1'],
      [2, '3'],
    ]);
    expect(named('Untouched')).toEqual([
      [0, '0'],
      [1, '1'],
      [2, '2'],
    ]);
  });
});
