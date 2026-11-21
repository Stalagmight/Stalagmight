import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { StationAlias } from '../../types/ipc/StationAlias';

/**
 * The Station details' "Also known as": the Station IDs and names merged into
 * this station (its aliases, oldest first), each with the line that knew it by
 * that name when one still does.
 *
 * A merge makes ONE station with the survivor's Station ID (plan 5.1), and a
 * 0.6.0 file's merge group converts the same way; each line keeps showing the
 * station by its own name (plan 5.3 per line), and the retired Station IDs are
 * kept in the file, so a surveyor can still find a station from the field
 * notes. This shows them. Nothing is rendered for a station without any.
 */
export function StationAliases({
  aliases,
  lineName,
}: {
  aliases: ReadonlyArray<StationAlias> | undefined;
  lineName: (lineId: number) => string;
}) {
  const { t } = useTranslation();
  if (!aliases || aliases.length === 0) return null;
  return (
    <div className="pt-2 border-t border-border" data-testid="station-detail-aliases">
      <span className="text-sm text-text-secondary" title={t('detailPanel.station.alsoKnownAsHint')}>
        {t('detailPanel.station.alsoKnownAs')}:
      </span>
      <ul className="mt-1 space-y-1 text-sm">
        {aliases.map((alias) => (
          <li key={alias.id} data-testid={`station-detail-alias-${alias.id}`}>
            {alias.name && alias.line !== undefined
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
