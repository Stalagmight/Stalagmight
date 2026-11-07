/**
 * The sections of a DMP file as the wizard's edits leave them (split out of
 * DmpNewImportDialog with #576 D4), for the previews that draw a segment the
 * way it will be imported.
 */
import type { DmpSection } from '../../../types/commands';
import type { VirtualConnection } from '../../../utils/segmentContext';
import { adjustStationForDeletedShots, type SectionEditState } from '../../../utils/batchImportHelpers';
import { editableShotsToDmpSection } from '../../../utils/shotConversion';

/**
 * Build a DmpSection reflecting edits (deleted shots removed, interpolated
 * values applied). Falls back to the raw section when no edits exist.
 */
export function buildEffectiveDmpSection(raw: DmpSection, edits: SectionEditState | undefined): DmpSection {
  if (!edits) return raw;
  return {
    ...editableShotsToDmpSection(raw, edits.shots),
    name: edits.name,
    direction: edits.direction,
  };
}

/**
 * Adjust VirtualConnection station indices to account for deleted shots in
 * edited sections (so layout offsets align with post-treatment geometry).
 */
export function adjustConnectionsForEdits(
  connections: VirtualConnection[],
  sectionEdits: Map<number, SectionEditState>,
): VirtualConnection[] {
  return connections.map(conn => {
    const importEdits = sectionEdits.get(conn.importSectionIndex);
    const adjustedImport = importEdits
      ? adjustStationForDeletedShots(importEdits.shots, conn.importStationIndex)
      : conn.importStationIndex;

    const adjustedTarget =
      conn.targetType === 'importing' && conn.targetImportIndex != null && conn.targetImportStationIndex != null
        ? (sectionEdits.get(conn.targetImportIndex)
            ? adjustStationForDeletedShots(sectionEdits.get(conn.targetImportIndex)!.shots, conn.targetImportStationIndex)
            : conn.targetImportStationIndex)
        : conn.targetImportStationIndex;

    return {
      ...conn,
      importStationIndex: adjustedImport,
      ...(adjustedTarget != null ? { targetImportStationIndex: adjustedTarget } : {}),
    };
  });
}
