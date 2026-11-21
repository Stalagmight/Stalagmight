import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { StationAlias } from '../../types/ipc/StationAlias';

/**
 * The Station details' "Also known as": the Station IDs and names merged into
 * this station (its aliases, oldest first) that the "Also on" rows do not
 * already show, each with the line that knew it by that name when one still
 * does.
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
  aliases,
  lineName,
  shown,
  ownLine,
  ownName,
}: {
  aliases: ReadonlyArray<StationAlias> | undefined;
  lineName: (lineId: number) => string;
  /** Alias ids the "Also on" rows already show. */
  shown: ReadonlySet<number>;
  /** The line of the slot these details are on, and the name it shows. */
  ownLine: number;
  ownName: string;
}) {
  const { t } = useTranslation();
  const left = (aliases ?? []).filter((alias) => !shown.has(alias.id));
  if (left.length === 0) return null;
  return (
    <div className="pt-2 border-t border-border" data-testid="station-detail-aliases">
      <span className="text-sm text-text-secondary" title={t('detailPanel.station.alsoKnownAsHint')}>
        {t('detailPanel.station.alsoKnownAs')}:
      </span>
      <ul className="mt-1 space-y-1 text-sm">
        {left.map((alias) => (
          <li key={alias.id} data-testid={`station-detail-alias-${alias.id}`}>
            {alias.line === ownLine && (alias.name === null || alias.name === ownName)
              ? t('detailPanel.station.aliasIdOnLine', { line: lineName(alias.line), id: alias.id })
              : alias.name && alias.line !== undefined
                ? t('detailPanel.station.aliasOnLine', {
                    name: displayStationName(alias.name),
                    line: lineName(alias.line),
                    id: alias.id,
                  })
                : alias.name
                  ? t('detailPanel.station.alias', { name: displayStationName(alias.name), id: alias.id })
                  : t('detailPanel.station.aliasNoName', { id: alias.id })}
          </li>
        ))}
      </ul>
    </div>
  );
}
