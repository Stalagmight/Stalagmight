import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { StationAlias } from '../../types/ipc/StationAlias';

/**
 * The Station details' "Also known as": the Station IDs and names merged into
 * this station (its aliases, oldest first).
 *
 * A merge makes ONE station with the survivor's name and Station ID (plan
 * 5.1), and a 0.6.0 file's merge group converts the same way, so a station a
 * surveyor knows from the field notes by its old name can carry another one
 * now. The old names and ids are kept in the file; this shows them. Nothing is
 * rendered for a station without any.
 */
export function StationAliases({ aliases }: { aliases: ReadonlyArray<StationAlias> | undefined }) {
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
            {alias.name
              ? t('detailPanel.station.alias', { name: displayStationName(alias.name), id: alias.id })
              : t('detailPanel.station.aliasNoName', { id: alias.id })}
          </li>
        ))}
      </ul>
    </div>
  );
}
