/**
 * One row's slice of the link rail (see `model/links.ts` `RailCell`): a small
 * inline SVG in the gutter, 33px tall so it overdraws the row's 1px bottom
 * border and the slices join into one continuous rail. Station dot / junction
 * ring, the shot line (solid Normal, dashed Jump, dotted Reference), the zero
 * datum ring, the closure arrow, break marks where a filter omitted rows, and
 * the elbow a branch section takes off its parent's station.
 */
import type { ReactElement } from 'react';
import type { RailCell, RailLine, RailSegment } from './model/links';

export const RAIL_WIDTH = 28;
const H = 33;
const CY = 16.5;
const LANE = 14;
const NODE_R = 4;
const BG = 'var(--sg-header-bg, var(--cc-panel))';

const DASH: Record<RailLine, string | undefined> = {
  solid: undefined,
  dashed: '4 3',
  dotted: '0.1 3.6',
};

function Segment({ seg, from, to, color }: { seg: RailSegment; from: number; to: number; color: string }) {
  return (
    <line
      className={`dw-rail-line dw-rail-line-${seg.style}`}
      x1={LANE}
      x2={LANE}
      y1={from}
      y2={to}
      stroke={color}
      strokeWidth={2}
      strokeLinecap={seg.style === 'dotted' ? 'round' : 'butt'}
      strokeDasharray={DASH[seg.style]}
    />
  );
}

/** Two slanted ticks across the lane: rows omitted here. */
function BreakMarks({ y, color }: { y: number; color: string }) {
  return (
    <g className="dw-rail-break" stroke={color} strokeWidth={1.25} strokeLinecap="round">
      <line x1={LANE - 4.5} y1={y + 1.5} x2={LANE + 4.5} y2={y - 1.5} style={{ stroke: BG, strokeWidth: 4 }} />
      <line x1={LANE - 4.5} y1={y} x2={LANE + 4.5} y2={y - 3} />
      <line x1={LANE - 4.5} y1={y + 3} x2={LANE + 4.5} y2={y} />
    </g>
  );
}

export function DataRail({ cell }: { cell: RailCell }) {
  const { color } = cell;
  const nodeGap = cell.node ? NODE_R + 1 : 0;
  const parts: ReactElement[] = [];

  if (cell.above) {
    const top = cell.above.edge === 'cut' ? 7 : 0;
    parts.push(<Segment key="a" seg={cell.above} from={top} to={CY - nodeGap} color={color} />);
    if (cell.above.edge !== 'edge') parts.push(<BreakMarks key="ab" y={5} color={color} />);
  }
  if (cell.below) {
    const bottom = cell.below.edge === 'cut' ? H - 7 : H;
    parts.push(<Segment key="b" seg={cell.below} from={CY + nodeGap} to={bottom} color={color} />);
    if (cell.below.edge === 'cut') parts.push(<BreakMarks key="bb" y={H - 4} color={color} />);
  }

  if (cell.elbow) {
    const parent = cell.elbow.parentColor;
    if (cell.elbow.down) {
      parts.push(
        <path
          key="elbow"
          className="dw-rail-elbow"
          d={`M 8 ${CY} H ${LANE - 4} Q ${LANE} ${CY} ${LANE} ${CY + 4} V ${H}`}
          fill="none"
          stroke={color}
          strokeWidth={2}
        />,
      );
    } else {
      parts.push(
        <line key="elbow" className="dw-rail-elbow" x1={8} y1={CY} x2={LANE - NODE_R - 1} y2={CY} stroke={parent} strokeWidth={1.5} />,
      );
    }
    parts.push(
      <circle key="parent" className="dw-rail-parent" cx={5} cy={CY} r={3} stroke={parent} strokeWidth={1.5} style={{ fill: BG }} />,
    );
  }

  if (cell.zeroDatum) {
    parts.push(
      <circle key="zd" className="dw-rail-zero" cx={LANE} cy={CY} r={NODE_R + 3} fill="none" style={{ stroke: 'var(--cc-zero-datum)' }} strokeWidth={1.5} />,
    );
  }
  if (cell.node === 'dot') {
    parts.push(<circle key="n" className="dw-rail-dot" cx={LANE} cy={CY} r={NODE_R} fill={color} />);
  } else if (cell.node === 'ring') {
    parts.push(
      <circle key="n" className="dw-rail-ring" cx={LANE} cy={CY} r={NODE_R} stroke={color} strokeWidth={2} style={{ fill: BG }} />,
    );
  }
  if (cell.closure) {
    parts.push(
      <g key="cl" className="dw-rail-closure" stroke={color} strokeWidth={1.5} fill="none" strokeLinecap="round">
        <line x1={23} y1={CY - 6} x2={23} y2={CY + 5} strokeDasharray="0.1 2.6" />
        <path d={`M 20.5 ${CY + 3.5} L 23 ${CY + 6.5} L 25.5 ${CY + 3.5}`} />
      </g>,
    );
  }

  return (
    <svg
      className="dw-rail pointer-events-none absolute left-[3px] top-0"
      width={RAIL_WIDTH}
      height={H}
      viewBox={`0 0 ${RAIL_WIDTH} ${H}`}
      aria-hidden="true"
      data-rail={cell.kind}
    >
      {parts}
    </svg>
  );
}
