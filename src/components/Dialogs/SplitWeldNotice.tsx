import type { DeleteShotImpact } from '../../types/commands';
import { useProse } from '../../hooks/useProse';
import { Prose } from '../common/Prose';
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
 * Each station is a prose pill (RS-PROSE).
 */
export default function SplitWeldNotice({
  impact,
  prefix = '',
}: {
  impact: Pick<DeleteShotImpact, 'split_closure_to_merge' | 'split_max_move_m'> | null;
  /** Leading words, e.g. "If you choose Split Line, ". */
  prefix?: string;
}) {
  const prose = useProse();
  const precision = useDecimalPrecision();
  const pairs = impact?.split_closure_to_merge ?? [];
  if (!impact || pairs.length === 0) return null;
  const named = pairs.map((p) => `${prose.station(p.station_a, true)} - ${prose.station(p.station_b, true)}`);
  const one = named.length === 1;
  // The leading words are capitalised before any name joins them: a string
  // holding prose tokens is never cut.
  const lead = `${prefix}${one ? 'the loop closure' : 'the loop closures'}`;
  const closures = one ? ` ${named[0]} becomes a junction` : ` ${named.join(', ')} become junctions`;
  const moves =
    impact.split_max_move_m >= MOVE_SHOWN_M
      ? `; the stations past ${one ? 'it' : 'them'} move by up to ${formatFixed(impact.split_max_move_m, undefined, precision.length)} m`
      : '';
  const text = `${lead.charAt(0).toUpperCase()}${lead.slice(1)}${closures}${moves}.`;
  return (
    <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm" data-testid="split-weld-notice">
      <Prose text={text} />
    </div>
  );
}
