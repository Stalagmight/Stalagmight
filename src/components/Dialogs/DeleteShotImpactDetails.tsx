import type { DeleteShotImpact, DeleteShotRangeImpact } from '../../types/commands';
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
 * `DeleteShotRangeImpact`, so this component renders either one. The range
 * preview adds the loop closures its gap-close would remove because they would
 * span a single shot (#737); the single-shot delete never shortens a closure to
 * one shot, so it has no such field.
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
> &
  Partial<Pick<DeleteShotRangeImpact, 'closures_removed_across_one_shot'>>;

export default function DeleteShotImpactDetails({
  impact,
  title = 'This deletion will update station relationships:',
  gapCloseAction = 'Close Gap & Delete',
}: {
  impact: ShotImpactRelationships | null;
  title?: string;
  /**
   * The label of the button that closes the gap. The closures across one shot
   * are removed only by that action, not by Split, so the heading names it.
   */
  gapCloseAction?: string;
}) {
  if (!impact) return null;

  // Named by their stations before the delete, as the user created them.
  const acrossOneShot = impact.closures_removed_across_one_shot ?? [];
  const hasDetails =
    acrossOneShot.length > 0 ||
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
      <p className="font-medium text-warning">{title}</p>
      <div className="space-y-2 text-text-secondary">
        {impact.removed_merges.length > 0 && (
          <div>
            <p className="font-medium">Removed merges</p>
            <ul className="list-disc list-inside">
              {impact.removed_merges.map((pair, index) => (
                <li key={`removed-merge-${index}`}><Pair a={pair.station_a} b={pair.station_b} /></li>
              ))}
            </ul>
          </div>
        )}

        {impact.removed_closures.length > 0 && (
          <div>
            <p className="font-medium">Removed closures</p>
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

        {acrossOneShot.length > 0 && (
          <div data-testid="impact-closures-across-one-shot">
            <p className="font-medium">
              {gapCloseAction} removes these loop closures (they would span a single shot)
            </p>
            <ul className="list-disc list-inside">
              {acrossOneShot.map((closure) => {
                const closureId = closure.closure_id.toString();
                return (
                  <li key={`across-one-shot-closure-${closureId}`}>
                    #{closureId}: <Pair a={closure.station_a} b={closure.station_b} />
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {impact.replacement_merges.length > 0 && (
          <div>
            <p className="font-medium">Replacement merges</p>
            <ul className="list-disc list-inside">
              {impact.replacement_merges.map((pair, index) => (
                <li key={`replacement-merge-${index}`}><Pair a={pair.station_a} b={pair.station_b} /></li>
              ))}
            </ul>
          </div>
        )}

        {impact.replacement_closure_updates.length > 0 && (
          <div>
            <p className="font-medium">Replacement closures</p>
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
            <p className="font-medium">Closures becoming merges</p>
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
              <p>Some merge connectivity could not be preserved (only same-section candidates).</p>
            )}
            {impact.closure_rewire_skipped && (
              <p>Some closures could not be replaced (no valid replacement or duplicate).</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
