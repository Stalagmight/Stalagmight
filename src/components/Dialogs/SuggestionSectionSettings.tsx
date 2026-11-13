/**
 * #567 item 2: the suggestion phase's right column already carries the
 * connection card, confidence badge, and merge/overlap controls before the
 * section-name field and full SectionMetadataForm are added on top - too
 * much for one column when the user is mainly deciding whether the
 * suggested connection looks right. Metadata is rarely read at that moment,
 * so it starts collapsed here (suggestion phase only; the manual-placing
 * phase's own settings block is untouched).
 *
 * `editState` lives in DmpNewImportDialog above this component, so
 * collapsing the disclosure never drops in-progress edits - only the
 * open/closed flag is local state here.
 */
import { useTranslation } from 'react-i18next';
import { Panel } from '../common/Panel';
import { SectionMetadataForm, type SectionPeopleProps } from '../common/SectionMetadataForm';
import type { SectionEditState } from '../../utils/batchImportHelpers';

export interface SuggestionSectionSettingsProps {
  editState: SectionEditState;
  /** The section's Surveyors / Explorers (the host resolves the wizard defaults in). */
  people: SectionPeopleProps;
  onMetadataChange: (field: keyof SectionEditState, value: string | number | boolean) => void;
}

export function SuggestionSectionSettings({ editState, people, onMetadataChange }: SuggestionSectionSettingsProps) {
  const { t } = useTranslation();

  return (
    <Panel
      title={t('dialogs.dmpNewImport.sectionSettings')}
      collapsible
      defaultOpen={false}
      testId="wizard-suggestion-section-settings"
      bare
    >
      <div>
        <label className="block text-sm text-text-secondary mb-1" data-testid="section-name-label">
          {t('common.labels.name')}
        </label>
        <input
          type="text"
          value={editState.name}
          onChange={(e) => onMetadataChange('name', e.target.value)}
          className="w-full px-3 py-2 bg-background border border-border rounded"
          placeholder={t('sectionMetadata.placeholders.name')}
          data-testid="section-name-input"
        />
      </div>
      <div className="space-y-4 mt-4" data-testid="wizard-editable-rest">
        <SectionMetadataForm
          name={editState.name}
          people={people}
          date={editState.date}
          color={editState.color}
          direction={editState.direction}
          quality={editState.quality}
          onNameChange={(v) => onMetadataChange('name', v)}
          onDateChange={(v) => onMetadataChange('date', v)}
          onColorChange={(v) => onMetadataChange('color', v)}
          onDirectionChange={(v) => onMetadataChange('direction', v)}
          onQualityChange={(v) => onMetadataChange('quality', v)}
          reverse={editState.reverse}
          onReverseChange={(v) => onMetadataChange('reverse', v)}
          showName={false}
          showDirection
          showReverse
        />
      </div>
    </Panel>
  );
}
