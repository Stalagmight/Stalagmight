import { useTranslation } from 'react-i18next';
import type { DeleteShotImpact } from '../../types/commands';
import type { StationRef } from '../../types/project';
import { EntityBlob } from '../common/EntityBlob';

/** A station reference as the entity pill (Full level) or plain "section:index". */
function StationRefBlob({ station }: { station: StationRef }) {
  return (
    <EntityBlob entity="station">{`${station.section_id}:${station.station_index}`}</EntityBlob>
  );
}

function Pair({ a, b }: { a: StationRef; b: StationRef }) {
  return (
    <>
      <StationRefBlob station={a} /> {'<->'} <StationRefBlob station={b} />
    </>
  );
}

/**
 * The station-relationship fields shared by `DeleteShotImpact` and
 * `DeleteShotRangeImpact`, so this component renders either one.
 */
export type ShotImpactRelationships = Pick<
  DeleteShotImpact,
  | 'removed_merges'
  | 'removed_closures'
  | 'replacement_merges'
  | 'replacement_closure_updates'
  | 'closure_to_merge'
  | 'merge_rewire_skipped'
  | 'closure_rewire_skipped'
>;

export default function DeleteShotImpactDetails({
  impact,
  title,
}: {
  impact: ShotImpactRelationships | null;
  title?: string;
}) {
  const { t } = useTranslation();
  if (!impact) return null;

  const hasDetails =
    impact.removed_merges.length > 0 ||
    impact.removed_closures.length > 0 ||
    impact.replacement_merges.length > 0 ||
    impact.replacement_closure_updates.length > 0 ||
    impact.closure_to_merge.length > 0 ||
    impact.merge_rewire_skipped ||
    impact.closure_rewire_skipped;

  if (!hasDetails) return null;

  return (
    <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-2">
      <p className="font-medium text-warning">{title ?? t('dialogs.deleteShotImpact.defaultTitle')}</p>
      <div className="space-y-2 text-text-secondary">
        {impact.removed_merges.length > 0 && (
          <div>
            <p className="font-medium">{t('dialogs.deleteShotImpact.removedMerges')}</p>
            <ul className="list-disc list-inside">
              {impact.removed_merges.map((pair, index) => (
                <li key={`removed-merge-${index}`}><Pair a={pair.station_a} b={pair.station_b} /></li>
              ))}
            </ul>
          </div>
        )}

        {impact.removed_closures.length > 0 && (
          <div>
            <p className="font-medium">{t('dialogs.deleteShotImpact.removedClosures')}</p>
            <ul className="list-disc list-inside">
              {impact.removed_closures.map((closure) => {
                const closureId = closure.closure_id.toString();
                return (
                  <li key={`removed-closure-${closureId}`}>
                    #{closureId}: <Pair a={closure.station_a} b={closure.station_b} />
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {impact.replacement_merges.length > 0 && (
          <div>
            <p className="font-medium">{t('dialogs.deleteShotImpact.replacementMerges')}</p>
            <ul className="list-disc list-inside">
              {impact.replacement_merges.map((pair, index) => (
                <li key={`replacement-merge-${index}`}><Pair a={pair.station_a} b={pair.station_b} /></li>
              ))}
            </ul>
          </div>
        )}

        {impact.replacement_closure_updates.length > 0 && (
          <div>
            <p className="font-medium">{t('dialogs.deleteShotImpact.replacementClosures')}</p>
            <ul className="list-disc list-inside">
              {impact.replacement_closure_updates.map((update) => {
                const closureId = update.closure_id.toString();
                return (
                  <li key={`replacement-closure-${closureId}`}>
                    #{closureId}: <StationRefBlob station={update.old_station} /> {'->'} <StationRefBlob station={update.new_station} />
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {impact.closure_to_merge.length > 0 && (
          <div>
            <p className="font-medium">{t('dialogs.deleteShotImpact.closuresToMerges')}</p>
            <ul className="list-disc list-inside">
              {impact.closure_to_merge.map((pair, index) => (
                <li key={`closure-to-merge-${index}`}><Pair a={pair.station_a} b={pair.station_b} /></li>
              ))}
            </ul>
          </div>
        )}

        {(impact.merge_rewire_skipped || impact.closure_rewire_skipped) && (
          <div className="text-warning">
            {impact.merge_rewire_skipped && (
              <p>{t('dialogs.deleteShotImpact.mergeRewireSkipped')}</p>
            )}
            {impact.closure_rewire_skipped && (
              <p>{t('dialogs.deleteShotImpact.closureRewireSkipped')}</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
