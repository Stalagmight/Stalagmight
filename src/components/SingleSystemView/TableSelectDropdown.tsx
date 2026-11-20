import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { Select, type SelectOption } from '../common/Select';
import { DatabaseIcon } from '../common/icons';
import { EntitySymbol } from '../common/toolIcons';
import { syntacticLevelAtLeast, syntacticVarName } from '../../theme/syntacticPalette';

const LINES_VALUE = 'lines';
const RAW_VALUE = 'raw';

/**
 * Fullscreen-toolbar view picker: the Lines-backed data table, or the raw
 * members of the saved `.stalx` file. WHICH member is browsed is chosen
 * in the data-table sub-header next to the table-name pill; selecting "Raw
 * data" here enters raw mode with an empty selection that the sub-header
 * resolves to the last-browsed (or first) table.
 */
export default function TableSelectDropdown() {
  const { t } = useTranslation();
  const rawTableSelection = useUIStore((s) => s.rawTableSelection);
  const setRawTableSelection = useUIStore((s) => s.setRawTableSelection);
  const syntacticLevel = useUIStore((s) => s.syntacticLevel);
  const showIcons = syntacticLevelAtLeast(syntacticLevel, 'icons');

  const handleChange = useCallback(
    (value: string) => {
      setRawTableSelection(value === RAW_VALUE ? '' : null);
    },
    [setRawTableSelection]
  );

  const options: SelectOption[] = [
    {
      value: LINES_VALUE,
      label: t('dataTable.tableSelect.sections'),
      ...(showIcons
        ? {
            icon: (
              <span style={{ color: `var(${syntacticVarName('line')})` }}>
                <EntitySymbol entity="line" size={13} />
              </span>
            ),
          }
        : {}),
    },
    {
      value: RAW_VALUE,
      label: t('dataTable.tableSelect.rawData'),
      labelSuffix: (
        <i className="italic text-text-secondary">{t('dataTable.tableSelect.readOnly')}</i>
      ),
      ...(showIcons ? { icon: <DatabaseIcon size={13} /> } : {}),
    },
  ];

  return (
    <Select
      value={rawTableSelection === null ? LINES_VALUE : RAW_VALUE}
      onChange={handleChange}
      options={options}
      className="w-48 text-sm"
      aria-label={t('dataTable.title')}
      data-testid="table-select-dropdown"
    />
  );
}
