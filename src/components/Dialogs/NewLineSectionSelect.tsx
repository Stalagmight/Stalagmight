/**
 * The New Line dialog's optional target Section (#327): the line goes straight
 * into it when created. Renders nothing while the project has no Sections, so
 * the dialog looks as it always did until the user groups lines.
 */
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { Select } from '../common/Select';

const NONE = 'none';

export function NewLineSectionSelect({
  value,
  onChange,
  disabled,
}: {
  value: number | null;
  onChange: (sectionId: number | null) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const sections = useProjectStore((s) => s.project?.cave_sections);
  if (!sections || sections.length === 0) return null;
  // A Section deleted (or undone away) while the dialog was open reads as none.
  const current = value !== null && sections.some((s) => s.id === value) ? String(value) : NONE;
  return (
    <div>
      <label id="new-line-section-label" className="block text-sm text-text-secondary mb-1">
        {t('caveSections.newLine.label')}
      </label>
      <Select
        value={current}
        onChange={(v) => onChange(v === NONE ? null : Number(v))}
        options={[
          { value: NONE, label: t('caveSections.newLine.none') },
          ...sections.map((s) => ({ value: String(s.id), label: s.name })),
        ]}
        disabled={disabled === true}
        aria-labelledby="new-line-section-label"
        data-testid="import-line-section-select"
      />
    </div>
  );
}

/** The Section the dialog targets, if it still exists. */
export function liveSectionId(value: number | null): number | null {
  if (value === null) return null;
  const sections = useProjectStore.getState().project?.cave_sections ?? [];
  return sections.some((s) => s.id === value) ? value : null;
}
