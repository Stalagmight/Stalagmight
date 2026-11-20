import type { DeleteShotImpact } from '../../types/commands';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { formatFixed } from '../../i18n/format';

/** Moves below half a millimetre read as "nothing moves". */
const MOVE_SHOWN_M = 0.0005;

/**
 * What Delete Shot + Split welds (review finding UX-5). When the split leaves
 * a loop closure the only connection between the two halves' pieces, the
 * orphan-closure sweep makes it a junction and the stranded piece moves onto
 * it by the misclosure, as 0.6.0's Delete Shot + Split did. The backend's
 * preview (`preview_delete_shot`) runs the split on a copy and sends the
 * closures by their two stations before the delete, and the largest move.
 */
export default function SplitWeldNotice({
  impact,
  prefix = '',
}: {
  impact: Pick<DeleteShotImpact, 'split_closure_to_merge' | 'split_max_move_m'> | null;
  /** Leading words, e.g. "If you choose Split Line, ". */
  prefix?: string;
}) {
  const labels = useIdentityLabels();
  const precision = useDecimalPrecision();
  const pairs = impact?.split_closure_to_merge ?? [];
  if (!impact || pairs.length === 0) return null;
  const named = pairs.map((p) => `${labels.station(p.station_a, true)} - ${labels.station(p.station_b, true)}`);
  const one = named.length === 1;
  const closures = one
    ? `the loop closure ${named[0]} becomes a junction`
    : `the loop closures ${named.join(', ')} become junctions`;
  const moves =
    impact.split_max_move_m >= MOVE_SHOWN_M
      ? `; the stations past ${one ? 'it' : 'them'} move by up to ${formatFixed(impact.split_max_move_m, undefined, precision.length)} m`
      : '';
  const text = `${prefix}${closures}${moves}.`;
  return (
    <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm" data-testid="split-weld-notice">
      {text.charAt(0).toUpperCase() + text.slice(1)}
    </div>
  );
}
