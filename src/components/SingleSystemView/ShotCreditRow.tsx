import { useTranslation } from 'react-i18next';
import type { Section } from '../../types/ipc/Section';
import type { Shot } from '../../types/ipc/Shot';
import { creditOverride } from '../../utils/shotCredit';
import { DetailRow } from './DetailRow';

/**
 * Who surveyed a shot and when (#707), shown only where that differs from its
 * section's credit, e.g. in a section combined from two trips.
 */
export function ShotCreditRow({ shot, section }: { shot: Pick<Shot, 'credit'>; section: Pick<Section, 'surveyor' | 'date'> }) {
  const { t } = useTranslation();
  const credit = creditOverride(shot, { surveyor: section.surveyor, date: section.date });
  if (!credit) return null;
  const unknown = t('detailPanel.shot.creditUnknown');
  // Both values, for the shot and for its section, so a row that differs only
  // in its date still says what differs.
  const describe = (surveyor: string | null, date: string | null) =>
    `${surveyor ?? unknown} · ${date ?? unknown}`;
  return (
    <DetailRow
      testId="shot-credit"
      label={t('detailPanel.labels.surveyed')}
      value={
        <>
          {describe(credit.surveyor, credit.date)}{' '}
          <span className="text-text-secondary">
            ({t('detailPanel.shot.creditSection', { credit: describe(section.surveyor, section.date) })})
          </span>
        </>
      }
    />
  );
}
