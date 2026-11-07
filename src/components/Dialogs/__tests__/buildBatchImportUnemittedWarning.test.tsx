/**
 * #535: `buildBatchImport`'s "N reachable section(s) were not emitted"
 * `console.warn` fired repeatedly during a normal DMP manual-placement run.
 *
 * Manual placement calls `buildBatchImport` fresh at every incremental step
 * with the FULL accumulated connection graph. A section already imported in an
 * earlier step (`alreadyImported`) is `continue`d before `emittedPosition` is
 * ever set for it, but it stays reachable in the graph every subsequent call -
 * so counting "reachable and not in `emittedPosition`" toward the warning
 * counted every already-successful step as a fresh gap, climbing by one on
 * every healthy incremental placement. Same shape for `emptySections`
 * (already reported via `reportEmptySections`) - a reachable section whose
 * every shot got deleted in the wizard produces nothing to send, which is not
 * a "gap" either.
 *
 * (`deferredImportSet` looks like a third source in the exclusion filter, but
 * is dead there in practice: `bfsReachable` is called with `deferredImportSet`
 * as its `blocked` set at both call sites, so a deferred index can never be in
 * `reachableOrder` to begin with. Kept as defensive belt-and-braces, not
 * exercised by a dedicated test here.)
 *
 * .tsx so vitest runs it under jsdom - the module under test pulls in React.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';

// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. Stub it - this test exercises the batch builder, not the preview.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));

import { buildBatchImport } from '../DmpNewImportDialog';
import type { DmpFile, DmpSection } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import type { VirtualConnection } from '../../../utils/segmentContext';
import type { EditableShot } from '../../common/ShotTableEditor';
import { makeDmpFile, makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function dmpSection(name: string, shotCount: number): DmpSection {
  return makeDmpSection({ name, shots: makeDmpShots(shotCount), content_hash: `hash-${name}` });
}

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

function edits(name: string, shotCount: number): SectionEditState {
  return {
    name,
    surveyor: 'S',
    date: '2026-01-01',
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    reverse: false,
    shots: Array.from({ length: shotCount }, (_, i) => editableShot(i)),
  };
}

/**
 * Section 0: anchored to the live project (the section just manually placed
 * this step). Section 1: imported in an EARLIER step (`alreadyImported`), and
 * still reachable this step via its importing-type connection to section 0 -
 * mirrors what every subsequent `buildBatchImport` call sees once a segment
 * has been placed and moves on.
 */
function build(alreadyImported: Map<number, number> | undefined) {
  const dmpFile: DmpFile = makeDmpFile({ sections: [dmpSection('Segment', 3), dmpSection('AlreadyDone', 4)] });
  const selectionConfig: DmpSelectionConfig = {
    selectedIndices: new Set([0, 1]),
    defaultSurveyor: 'S',
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
    {
      importSectionIndex: 1,
      importStationIndex: 0,
      targetType: 'importing',
      targetImportIndex: 0,
      targetImportStationIndex: 2,
    },
  ];
  const sectionEdits = new Map<number, SectionEditState>([
    [0, edits('Segment', 3)],
    [1, edits('AlreadyDone', 4)],
  ]);

  return buildBatchImport(
    connections,
    null,
    dmpFile,
    selectionConfig,
    sectionEdits,
    undefined,
    undefined,
    alreadyImported,
  );
}

describe('buildBatchImport unemitted-reachable warning', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('does not warn when the only reachable-but-unemitted section was already imported in an earlier step', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const alreadyImported = new Map<number, number>([[1, 555]]);

    const { configs } = build(alreadyImported);

    // Only section 0 (the one placed THIS step) is emitted; section 1 is
    // reachable but already imported, and must not be re-sent.
    expect(configs).toHaveLength(1);
    expect(configs[0]?.name).toBe('Segment');
    expect(warn).not.toHaveBeenCalled();
  });

  it('does not warn when the only reachable-but-unemitted section produced nothing (every shot deleted)', () => {
    // Section 0 is anchored (reachable) but every shot was deleted in the
    // wizard's shot table, so it emits nothing - `emptySections`, not a gap.
    const dmpFile: DmpFile = makeDmpFile({ sections: [dmpSection('Segment', 3)] });
    const selectionConfig: DmpSelectionConfig = {
      selectedIndices: new Set([0]),
      defaultSurveyor: 'S',
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
    const allDeleted: SectionEditState = {
      ...edits('Segment', 3),
      shots: [0, 1, 2].map((i) => ({ ...editableShot(i), isDeleted: true })),
    };
    const sectionEdits = new Map<number, SectionEditState>([[0, allDeleted]]);

    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { configs, emptySections } = buildBatchImport(
      connections,
      null,
      dmpFile,
      selectionConfig,
      sectionEdits,
    );

    expect(configs).toHaveLength(0);
    expect(emptySections).toEqual([0]);
    expect(warn).not.toHaveBeenCalled();
  });

  it('still warns when a reachable section genuinely never resolves', () => {
    // A malformed 'existing' connection: it still anchors (and so reaches)
    // section 0, but is missing `existingStationIndex`, so the placement
    // resolver can build neither a `from_existing` nor a `from_importing`
    // placement for it - a real, unexplained gap that is not `alreadyImported`,
    // not `deferredImportSet`, and not `emptySections`.
    const dmpFile: DmpFile = makeDmpFile({ sections: [dmpSection('Segment', 3)] });
    const selectionConfig: DmpSelectionConfig = {
      selectedIndices: new Set([0]),
      defaultSurveyor: 'S',
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
        // existingStationIndex deliberately omitted.
      },
    ];
    const sectionEdits = new Map<number, SectionEditState>([[0, edits('Segment', 3)]]);

    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { configs } = buildBatchImport(connections, null, dmpFile, selectionConfig, sectionEdits);

    expect(configs).toHaveLength(0);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0]?.[0]).toContain('reachable section(s) were not emitted');
  });
});
