/**
 * The line detail's "Section" row (#327): the name of the Section the line is
 * in. Nothing for an ungrouped line, so a project without Sections shows the
 * detail panel as it always did.
 */
import { useTranslation } from 'react-i18next';
import type { Line } from '../../types/project';
import { useProjectStore } from '../../store/projectStore';
import { DetailRow } from './DetailPanelParts';

export function LineSectionDetailRow({ line }: { line: Pick<Line, 'cave_section_id'> }) {
  const { t } = useTranslation();
  const name = useProjectStore(
    (s) => s.project?.cave_sections?.find((section) => section.id === line.cave_section_id)?.name,
  );
  if (name === undefined) return null;
  return <DetailRow label={t('caveSections.detailLabel')} value={name} testId="line-detail-section" />;
}
