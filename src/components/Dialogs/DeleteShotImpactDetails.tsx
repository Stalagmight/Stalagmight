import type { DeleteShotImpact, DeleteShotRangeImpact } from '../../types/commands';
import type { StationRef } from '../../types/project';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { identityNoticeMessage } from '../../utils/identityNotice';
import { StationBlob } from '../common/EntityBlob';

/**
 * A station as the line pill (Full level) or plain text: `Main Line / 17`.
 * Every station named here exists before the delete, so the open project
 * names it.
 */
function StationRefBlob({ station }: { station: StationRef }) {
  return <StationBlob station={station} withLine />;
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
 *
 * On the station graph (plan 5.4, W6) a delete never re-wires anything: a
 * station another line also passes simply stays for that line, so there are
 * no replacement merges, no re-anchored closures and nothing that "could not
 * be preserved". The wire still carries those fields (always empty); this
 * panel no longer reads them.
 *
 * What the delete does to station names and comments on its own (RS 4.3,
 * 12: an archived station it brings back, a letter, a comment a promotion
 * hides) is said in the notice's words, for the delete and, when Split is
 * offered too, for Split.
 */
export type ShotImpactRelationships = Pick<DeleteShotImpact, 'removed_merges' | 'removed_closures' | 'closure_to_merge'> &
  Partial<Pick<DeleteShotRangeImpact, 'closures_removed_across_one_shot' | 'identities' | 'split_identities'>>;

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
  const labels = useIdentityLabels();
  if (!impact) return null;

  // Named by their stations before the delete, as the user created them.
  const acrossOneShot = impact.closures_removed_across_one_shot ?? [];
  const names = identityNoticeMessage(impact.identities, labels.lineName);
  const splitNames = identityNoticeMessage(impact.split_identities, labels.lineName);
  const hasDetails =
    acrossOneShot.length > 0 ||
    impact.removed_merges.length > 0 ||
    impact.removed_closures.length > 0 ||
    impact.closure_to_merge.length > 0 ||
    names !== null ||
    splitNames !== null;

  if (!hasDetails) return null;

  return (
    <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-2">
      <p className="font-medium text-warning">{title}</p>
      <div className="space-y-2 text-text-secondary">
        {impact.removed_merges.length > 0 && (
          <div>
            <p className="font-medium">No longer shared with another line</p>
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

        {impact.closure_to_merge.length > 0 && (
          <div>
            <p className="font-medium">Loop closures that join their two stations into one</p>
            <ul className="list-disc list-inside">
              {impact.closure_to_merge.map((pair, index) => (
                <li key={`closure-to-merge-${index}`}><Pair a={pair.station_a} b={pair.station_b} /></li>
              ))}
            </ul>
          </div>
        )}

        {names !== null && (
          <div data-testid="impact-identities">
            <p className="font-medium">Station names and comments</p>
            <p>{names}</p>
          </div>
        )}

        {splitNames !== null && (
          <div data-testid="impact-split-identities">
            <p className="font-medium">If you choose Split Line instead</p>
            <p>{splitNames}</p>
          </div>
        )}

      </div>
    </div>
  );
}
