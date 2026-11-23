import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';

/**
 * The Station details' "Also known as": the stations merges archived in this
 * one (its records, RS 3, oldest first) that the "Also on" rows do not
 * already show, each with the line whose own station it was when one still
 * passes here (a history record names none). RS-UI redesigns this list
 * ("Side / 0 is archived", RS 6 and 7).
 *
 * A merge makes ONE station with the survivor's Station ID (plan 5.1), and a
 * 0.6.0 file's merge group converts the same way; each line keeps showing the
 * station by its own name (plan 5.3 per line), and the retired Station IDs are
 * kept in the file, so a surveyor can still find a station from the field
 * notes. This shows them, each fact once (demo finding N-3): an alias another
 * line through the station knows it by is on that line's "Also on" row
 * (`shown`), and the alias of the line these details are on (`ownLine`), whose
 * name the Name row already says, reads `Station ID 9 on North Passage`.
 * Nothing is rendered when no alias is left to show.
 */
export function StationAliases({
  records,
  lineName,
  shown,
  ownLine,
  ownName,
}: {
  records: ReadonlyArray<ArchivedStation> | undefined;
  lineName: (lineId: number) => string;
  /** Alias ids the "Also on" rows already show. */
  shown: ReadonlySet<number>;
  /** The line of the slot these details are on, and the name it shows. */
  ownLine: number;
  ownName: string;
}) {
  const { t } = useTranslation();
  // One entry per line a record names (its first, in line order), or one
  // with no line for a history record.
  const left = (records ?? [])
    .filter((record) => !shown.has(record.id))
    .map((record) => ({ id: record.id, name: record.name, line: record.lines[0] }));
  if (left.length === 0) return null;
  return (
    <div className="pt-2 border-t border-border" data-testid="station-detail-aliases">
      <span className="text-sm text-text-secondary" title={t('detailPanel.station.alsoKnownAsHint')}>
        {t('detailPanel.station.alsoKnownAs')}:
      </span>
      <ul className="mt-1 space-y-1 text-sm">
        {left.map((alias) => (
          <li key={alias.id} data-testid={`station-detail-alias-${alias.id}`}>
            {alias.line === ownLine && alias.name === ownName
              ? t('detailPanel.station.aliasIdOnLine', { line: lineName(alias.line), id: alias.id })
              : alias.name !== '' && alias.line !== undefined
                ? t('detailPanel.station.aliasOnLine', {
                    name: displayStationName(alias.name),
                    line: lineName(alias.line),
                    id: alias.id,
                  })
                : alias.name !== ''
                  ? t('detailPanel.station.alias', { name: displayStationName(alias.name), id: alias.id })
                  : t('detailPanel.station.aliasNoName', { id: alias.id })}
          </li>
        ))}
      </ul>
    </div>
  );
}
