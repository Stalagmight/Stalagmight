/**
 * Duotone tool icons for the toolbar - a soft accent-fill silhouette plus a
 * solid accent line, where the object color encodes WHAT the tool acts on and
 * a shared modifier (slate move-compass) encodes the move action. Ported from
 * the project's Cave Survey icon spec.
 *
 * Each glyph is rendered inside a `.duotone` SVG whose `--o` custom property is
 * set to the tool's role color (`var(--sh-<role>)`). The duotone CSS classes
 * (`.s/.f/.d/.ink/.mvf/.mvc`) live in `src/index.css`. When the highlighting
 * level is "off", `--sh-*` resolve to a neutral so these render monochrome.
 */
import type { CSSProperties, ReactNode } from 'react';
import type { ToolType } from '../../store/uiStore';
import { TOOL_ROLE_MAP, syntacticVarName, type BlobEntity } from '../../theme/syntacticPalette';

/** Survey station glyph: accent ring + filled centre dot. */
function Station({ cx, cy, r = 3 }: { cx: number; cy: number; r?: number }) {
  return (
    <g>
      <circle className="f" cx={cx} cy={cy} r={r} />
      <circle className="s" cx={cx} cy={cy} r={r} />
      <circle className="d" cx={cx} cy={cy} r={Math.max(0.9, r * 0.34)} />
    </g>
  );
}

/** Four-way move compass (slate, via --sh-move) + faint cross. */
function MoveDeco() {
  return (
    <g>
      <path className="mvf" d="M12 2 L14 5 H10 Z" />
      <path className="mvf" d="M12 22 L10 19 H14 Z" />
      <path className="mvf" d="M2 12 L5 10 V14 Z" />
      <path className="mvf" d="M22 12 L19 14 V10 Z" />
      <rect className="mvc" x="11.3" y="5" width="1.4" height="14" rx="0.7" />
      <rect className="mvc" x="5" y="11.3" width="14" height="1.4" rx="0.7" />
    </g>
  );
}

/**
 * Compacted object glyphs, drawn small enough to sit INSIDE a full-canvas deco
 * ([`MoveDeco`]'s compass arms) without colliding
 * with it. The standalone [`ENTITY_GLYPHS`] fill the whole 24x24 box and would.
 *
 * A line is a plain polyline; a segment is a BRANCHING run of shots, so it
 * adds the fork (extra path + node) - the one visual difference between them.
 */
const INNER_GLYPHS: Record<BlobEntity, () => ReactNode> = {
  station: () => <Station cx={12} cy={12} r={3} />,
  shot: () => (
    <g>
      <path className="s" d="M8.5 15.5 L15.5 8.5" />
      <Station cx={8.5} cy={15.5} r={1.9} />
      <Station cx={15.5} cy={8.5} r={1.9} />
    </g>
  ),
  line: () => (
    <g>
      <path className="s" d="M7.5 15.5 L11 11 L14 13.5 L16.5 9.5" />
      <circle className="d" cx="7.5" cy="15.5" r="1.3" />
      <circle className="d" cx="11" cy="11" r="1.3" />
      <circle className="d" cx="14" cy="13.5" r="1.3" />
      <circle className="d" cx="16.5" cy="9.5" r="1.3" />
    </g>
  ),
  segment: () => (
    <g>
      <path className="s" d="M7.5 15.5 L11 11 L14 13.5 L16.5 9.5" />
      <path className="s" d="M11 11 L8.5 7.5" />
      <circle className="d" cx="7.5" cy="15.5" r="1.3" />
      <circle className="d" cx="11" cy="11" r="1.3" />
      <circle className="d" cx="14" cy="13.5" r="1.3" />
      <circle className="d" cx="16.5" cy="9.5" r="1.3" />
      <circle className="d" cx="8.5" cy="7.5" r="1.3" />
    </g>
  ),
  comment: () => (
    <g>
      <path
        className="f"
        d="M7.5 8 H16 A1 1 0 0 1 17 9 V13.5 A1 1 0 0 1 16 14.5 H11 L8.5 17 V14.5 H7.5 A1 1 0 0 1 6.5 13.5 V9 A1 1 0 0 1 7.5 8 Z"
      />
      <path
        className="s"
        d="M7.5 8 H16 A1 1 0 0 1 17 9 V13.5 A1 1 0 0 1 16 14.5 H11 L8.5 17 V14.5 H7.5 A1 1 0 0 1 6.5 13.5 V9 A1 1 0 0 1 7.5 8 Z"
      />
    </g>
  ),
};

/** Glyph body for each toolbar tool, keyed by tool id. */
const GLYPHS: Partial<Record<ToolType, () => ReactNode>> = {
  select: () => (
    <g>
      <path className="f" d="M5 3 L13 17 L14.2 11.2 L19.5 9.8 Z" />
      <path className="s" d="M5 3 L13 17 L14.2 11.2 L19.5 9.8 Z" />
      <rect className="s dash" x="14.5" y="14.5" width="6.5" height="6.5" rx="1" />
    </g>
  ),
  pan: () => (
    <g>
      <path
        className="f"
        d="M8 13 V6.5 A1 1 0 0 1 10 6.5 V12 V4.5 A1 1 0 0 1 12 4.5 V12 V5.5 A1 1 0 0 1 14 5.5 V12 V7.5 A1 1 0 0 1 16 7.5 V16 A4 4 0 0 1 12 20 H10.5 A4 4 0 0 1 6.8 15.6 L6.5 15.5 Z"
      />
      <path className="s" d="M8 13 V6.5 A1 1 0 0 1 10 6.5 V12" />
      <path className="s" d="M10 12 V4.5 A1 1 0 0 1 12 4.5 V12" />
      <path className="s" d="M12 12 V5.5 A1 1 0 0 1 14 5.5 V12" />
      <path className="s" d="M14 12 V7.5 A1 1 0 0 1 16 7.5 V14" />
      <path className="s" d="M8 13 L6.5 15.5 A4 4 0 0 0 10 20 H14 A4 4 0 0 0 18 16 V14" />
    </g>
  ),
  zoom: () => (
    <g>
      <circle className="f" cx="10" cy="10" r="6" />
      <circle className="s" cx="10" cy="10" r="6" />
      <path className="s thick" d="M14.5 14.5 L20 20" />
      <path className="s" d="M10 7.5 V12.5" />
      <path className="s" d="M7.5 10 H12.5" />
    </g>
  ),
  measure: () => (
    <g>
      <rect className="f" x="2" y="8" width="20" height="8" rx="1.4" />
      <rect className="s" x="2" y="8" width="20" height="8" rx="1.4" />
      <path className="s" d="M6 8 V11" />
      <path className="s" d="M10 8 V12.5" />
      <path className="s" d="M14 8 V11" />
      <path className="s" d="M18 8 V12.5" />
    </g>
  ),
  merge: () => (
    <g>
      <path className="s" d="M6.5 8 L14 12" />
      <path className="s" d="M6.5 16 L14 12" />
      {/* The merged objects are stations - color their circles with the station
          color (overriding the tool's --o for this subtree). */}
      <g className="sh-tone" style={{ '--o': 'var(--sh-station)' } as CSSProperties}>
        <Station cx={4} cy={8} r={2.4} />
        <Station cx={4} cy={16} r={2.4} />
        <Station cx={19} cy={12} r={2.6} />
      </g>
      <path className="s arrow" d="M12.5 9.8 L15.5 12 L12.5 14.2" />
    </g>
  ),
  comment: () => (
    <g>
      <path
        className="f"
        d="M5 4 H19 A1 1 0 0 1 20 5 V15 A1 1 0 0 1 19 16 H10 L6 20 V16 H5 A1 1 0 0 1 4 15 V5 A1 1 0 0 1 5 4 Z"
      />
      <path
        className="s"
        d="M5 4 H19 A1 1 0 0 1 20 5 V15 A1 1 0 0 1 19 16 H10 L6 20 V16 H5 A1 1 0 0 1 4 15 V5 A1 1 0 0 1 5 4 Z"
      />
      <path className="s ink" d="M8 8 H16" />
      <path className="s ink" d="M8 12 H13" />
    </g>
  ),
  move_comment: () => (
    <g>
      <MoveDeco />
      {INNER_GLYPHS.comment()}
    </g>
  ),
  move_station: () => (
    <g>
      <MoveDeco />
      {INNER_GLYPHS.station()}
    </g>
  ),
  move_shot: () => (
    <g>
      <MoveDeco />
      {INNER_GLYPHS.shot()}
    </g>
  ),
  move_segment: () => (
    <g>
      <MoveDeco />
      {INNER_GLYPHS.segment()}
    </g>
  ),
};

export interface ToolGlyphProps {
  tool: ToolType;
  size?: number;
}

/**
 * Render the duotone glyph for a tool, colored by its syntactic-highlighting
 * role via the `--o` custom property. Returns null for tools without a glyph
 * (e.g. the removed draw/delete tools).
 */
export function ToolGlyph({ tool, size = 20 }: ToolGlyphProps) {
  const render = GLYPHS[tool];
  if (!render) return null;
  const role = TOOL_ROLE_MAP[tool];
  const style = { '--o': `var(${syntacticVarName(role)})` } as CSSProperties;
  return (
    <svg
      className="duotone"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      style={style}
      aria-hidden="true"
      focusable="false"
    >
      {render()}
    </svg>
  );
}

/**
 * Standalone object glyphs for each geometry entity (no action modifier), used
 * to mark entity hyperlinks. Same duotone language as the toolbar icons: the
 * object color (segment > line > shot > station) plus comment.
 */
const ENTITY_GLYPHS: Record<BlobEntity, () => ReactNode> = {
  station: () => <Station cx={12} cy={12} r={5.5} />,
  shot: () => (
    <g>
      <path className="s thick" d="M6.5 17 L17.5 7" />
      <Station cx={6.5} cy={17} r={2.8} />
      <Station cx={17.5} cy={7} r={2.8} />
    </g>
  ),
  line: () => (
    <g>
      <path className="s" d="M3 17.5 L8.5 8 L13.5 16 L19 8.5" />
      <circle className="d" cx="3" cy="17.5" r="1.7" />
      <circle className="d" cx="8.5" cy="8" r="1.7" />
      <circle className="d" cx="13.5" cy="16" r="1.7" />
      <circle className="d" cx="19" cy="8.5" r="1.7" />
    </g>
  ),
  segment: () => (
    <g>
      <path className="s" d="M3 18 L8 11 L13 15 L18 9" />
      <path className="s" d="M8 11 L6 5.5" />
      <circle className="d" cx="3" cy="18" r="1.6" />
      <circle className="d" cx="8" cy="11" r="1.6" />
      <circle className="d" cx="13" cy="15" r="1.6" />
      <circle className="d" cx="18" cy="9" r="1.6" />
      <circle className="d" cx="6" cy="5.5" r="1.6" />
    </g>
  ),
  comment: () => (
    <g>
      <path
        className="f"
        d="M5 5 H19 A1 1 0 0 1 20 6 V15 A1 1 0 0 1 19 16 H11 L7 20 V16 H5 A1 1 0 0 1 4 15 V6 A1 1 0 0 1 5 5 Z"
      />
      <path
        className="s"
        d="M5 5 H19 A1 1 0 0 1 20 6 V15 A1 1 0 0 1 19 16 H11 L7 20 V16 H5 A1 1 0 0 1 4 15 V6 A1 1 0 0 1 5 5 Z"
      />
    </g>
  ),
};

export interface EntitySymbolProps {
  entity: BlobEntity;
  size?: number;
  className?: string;
  testId?: string;
}

/** Render the duotone object symbol for an entity, colored by its --sh-<entity>. */
export function EntitySymbol({ entity, size = 14, className = '', testId }: EntitySymbolProps) {
  const render = ENTITY_GLYPHS[entity];
  // Inline-block (overriding the `.duotone { display: block }` toolbar default)
  // so the symbol sits next to text; nudged onto the text baseline.
  const style = {
    '--o': `var(${syntacticVarName(entity)})`,
    display: 'inline-block',
    verticalAlign: '-0.2em',
  } as CSSProperties;
  return (
    <svg
      className={`duotone ${className}`}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      style={style}
      aria-hidden="true"
      focusable="false"
      data-testid={testId}
    >
      {render()}
    </svg>
  );
}

/** A corner-badge glyph (rendered inside the dark disc): the action modifier. */
type BadgeContent = (cx: number, cy: number, r: number) => ReactNode;

/** Red ✕ (delete). */
const deleteX: BadgeContent = (cx, cy, r) => {
  const a = r * 0.62;
  return (
    <g>
      <path className="x" d={`M${cx - a} ${cy - a} L${cx + a} ${cy + a}`} />
      <path className="x" d={`M${cx + a} ${cy - a} L${cx - a} ${cy + a}`} />
    </g>
  );
};

/**
 * Letter badge for the two virtual shot kinds (#27): a yellow "J" or a blue
 * "R", matching the glyphs and colours the data table's J/R cell uses so the
 * context menu and the table read as the same vocabulary.
 */
function kindLetter(letter: 'J' | 'R', cssClass: string, struck = false): BadgeContent {
  return (cx, cy, r) => (
    <g>
      <text
        className={cssClass}
        x={cx}
        y={cy}
        // `central` centres the cap on the badge; a capital's baseline would
        // otherwise sit the glyph low.
        dominantBaseline="central"
        textAnchor="middle"
        fontSize={r * 1.7}
        fontWeight="700"
      >
        {letter}
      </text>
      {/* "Unmark" variant: a short horizontal rule through the letter, in the
          shared action red (--sh-delete, same as the delete X) so removing a
          mark reads as removal rather than as more of the same kind. */}
      {struck && (
        <path
          className="kind-strike"
          d={`M${(cx - r * 0.55).toFixed(2)} ${cy} L${(cx + r * 0.55).toFixed(2)} ${cy}`}
        />
      )}
    </g>
  );
}

const jumpLetter = kindLetter('J', 'kind-jump');
const referenceLetter = kindLetter('R', 'kind-reference');
const jumpLetterStruck = kindLetter('J', 'kind-jump', true);
const referenceLetterStruck = kindLetter('R', 'kind-reference', true);

/** Orange pencil (edit) - a filled pencil, rotated diagonally. */
const editPencil: BadgeContent = (cx, cy, r) => {
  const k = r * 0.92;
  return (
    <g transform={`translate(${cx} ${cy}) rotate(45) scale(${k})`}>
      {/* eraser + body + triangular tip */}
      <path className="edit-pencil" d="M-0.34 -1 H0.34 V0.55 L0 1 L-0.34 0.55 Z" />
    </g>
  );
};

/** 5-point star path centered at (cx,cy). */
function starD(cx: number, cy: number, outer: number, inner: number): string {
  const pts: string[] = [];
  for (let i = 0; i < 10; i++) {
    const rr = i % 2 === 0 ? outer : inner;
    const ang = -Math.PI / 2 + (i * Math.PI) / 5;
    pts.push(`${(cx + rr * Math.cos(ang)).toFixed(2)} ${(cy + rr * Math.sin(ang)).toFixed(2)}`);
  }
  return `M${pts.join(' L')}Z`;
}

/** Golden star (zero datum). */
const datumStar: BadgeContent = (cx, cy, r) => (
  <path className="datum-star" d={starD(cx, cy, r * 1.02, r * 0.42)} />
);

/** Open eye (mark as permanent) - the "this shot is a real surveyed leg" badge.
 *  The crossed variant is gone: #27 replaced the single reference symbol with
 *  the J/R letter badges, so nothing needed the slash any more. */
const openEye: BadgeContent = (cx, cy, r) => {
  const rx = r * 0.98, ry = r * 0.62, pr = r * 0.32;
  return (
    <g>
      <ellipse className="eye" cx={cx} cy={cy} rx={rx} ry={ry} />
      <circle className="eye-pupil" cx={cx} cy={cy} r={pr} />
    </g>
  );
};

/**
 * Two horizontal arrows running in opposite directions (top one right, bottom
 * one left) - reverse. Slate `--sh-move`, the shared "spatial rearrangement"
 * modifier the move compass uses, so reverse reads as a sibling of move.
 *
 * Filled shafts and heads, never stroked: WebKit fattens thin strokes under the
 * setZoom UI scale (the same reason `dataLines` is filled), and at badge size a
 * fattened shaft would swallow its own arrowhead.
 */
const reverseArrows: BadgeContent = (cx, cy, r) => {
  const a = r * 0.68;          // half-length
  const dy = r * 0.38;         // vertical offset of each arrow from centre
  const t = r * 0.42;          // shaft thickness
  const hw = r * 0.38;         // arrowhead length
  const hh = r * 0.48;         // arrowhead half-height
  const n = (v: number) => v.toFixed(2);
  return (
    <g>
      {/* Shafts run UNDER their heads (they extend a hair past the base) -
          butt them up exactly and antialiasing leaves a visible seam. */}
      <rect
        className="mvc"
        x={n(cx - a)}
        y={n(cy - dy - t / 2)}
        width={n(a * 2 - hw * 0.6)}
        height={n(t)}
        rx={n(t / 2)}
      />
      <path
        className="mvf"
        d={`M${n(cx + a)} ${n(cy - dy)} L${n(cx + a - hw)} ${n(cy - dy - hh)} V${n(cy - dy + hh)} Z`}
      />
      <rect
        className="mvc"
        x={n(cx - a + hw * 0.6)}
        y={n(cy + dy - t / 2)}
        width={n(a * 2 - hw * 0.6)}
        height={n(t)}
        rx={n(t / 2)}
      />
      <path
        className="mvf"
        d={`M${n(cx - a)} ${n(cy + dy)} L${n(cx - a + hw)} ${n(cy + dy - hh)} V${n(cy + dy + hh)} Z`}
      />
    </g>
  );
};

/** Green plus (add). */
const greenPlus: BadgeContent = (cx, cy, r) => {
  const a = r * 0.6;
  return (
    <g>
      <path className="plus" d={`M${(cx - a).toFixed(2)} ${cy} H${(cx + a).toFixed(2)}`} />
      <path className="plus" d={`M${cx} ${(cy - a).toFixed(2)} V${(cy + a).toFixed(2)}`} />
    </g>
  );
};

/** Broken chain (remove closure / unmerge) - two link hooks with a gap, mapped
 * from the link-off art (20-unit space) into the badge. */
const brokenChain: BadgeContent = (cx, cy, r) => {
  const k = r * 0.108;
  const px = (x: number) => (cx + (x - 10) * k).toFixed(2);
  const py = (y: number) => (cy + (y - 10) * k).toFixed(2);
  const ar = (3 * k).toFixed(2);
  return (
    <g>
      <path className="chain" d={`M${px(8)} ${py(12)} L${px(6)} ${py(14)} A${ar} ${ar} 0 0 1 ${px(2)} ${py(10)} L${px(4)} ${py(8)}`} />
      <path className="chain" d={`M${px(12)} ${py(8)} L${px(14)} ${py(6)} A${ar} ${ar} 0 0 1 ${px(18)} ${py(10)} L${px(16)} ${py(12)}`} />
    </g>
  );
};

/** Chat bubble (comments) - a rounded speech bubble with a small tail. */
const chatBubble: BadgeContent = (cx, cy, r) => {
  const w = r * 1.4, h = r * 0.96, rx = r * 0.32;
  const x = cx - w / 2;
  const y = cy - h / 2 - r * 0.16;
  const by = y + h;
  const tail = `M${(cx - w * 0.18).toFixed(2)} ${(by - r * 0.08).toFixed(2)} L${(cx - w * 0.02).toFixed(2)} ${(by + r * 0.46).toFixed(2)} L${(cx + w * 0.22).toFixed(2)} ${(by - r * 0.08).toFixed(2)} Z`;
  return (
    <g>
      <rect className="chat-bubble" x={x.toFixed(2)} y={y.toFixed(2)} width={w.toFixed(2)} height={h.toFixed(2)} rx={rx.toFixed(2)} ry={rx.toFixed(2)} />
      <path className="chat-bubble" d={tail} />
    </g>
  );
};

/** "Data rows" / hamburger (shot data) - three stacked lines. */
const dataLines: BadgeContent = (cx, cy, r) => {
  const hw = r * 0.6, gap = r * 0.44;
  // Draw each row as a FILLED stadium (rounded rect) instead of a stroked line:
  // WebKit fattens thin strokes under the setZoom UI scale (the project rule -
  // draw UI icons filled, not stroked). Height + rx reproduce the prior
  // stroke-width 1.6 with round caps, so the glyph is pixel-identical at 1x.
  const t = 1.6;
  const row = (dy: number) => (
    <rect
      className="data-lines"
      x={(cx - hw - t / 2).toFixed(2)}
      y={(cy + dy - t / 2).toFixed(2)}
      width={(hw * 2 + t).toFixed(2)}
      height={t.toFixed(2)}
      rx={(t / 2).toFixed(2)}
      ry={(t / 2).toFixed(2)}
    />
  );
  return (
    <g>
      {row(-gap)}
      {row(0)}
      {row(gap)}
    </g>
  );
};

/** Capital "A" (labels / names). */
const letterA: BadgeContent = (cx, cy, r) => (
  <text
    className="label-a"
    x={cx.toFixed(2)}
    y={cy.toFixed(2)}
    fontSize={(r * 1.72).toFixed(2)}
    textAnchor="middle"
    dominantBaseline="central"
  >
    A
  </text>
);

/** Bottom-right action-badge placement (cx/cy/r) per entity. The entity itself is
 * the centered object glyph; the badge sits in the bottom-right corner (overlap
 * with the object is expected). */
const BADGE_POS: Record<BlobEntity, { bx: number; by: number; br: number }> = {
  station: { bx: 17.4, by: 17.4, br: 6.2 },
  shot: { bx: 17.6, by: 17.6, br: 5.9 },
  segment: { bx: 17.6, by: 17.6, br: 5.7 },
  line: { bx: 17.6, by: 17.6, br: 5.7 },
  comment: { bx: 17.6, by: 17.6, br: 5.7 },
};

/** Entity glyph + a corner action badge (object color via --o; the badge modifier
 * color comes from its own class - red ✕ / orange pencil / gold star). */
function EntityBadgedSymbol({
  entity,
  content,
  size = 16,
  className = '',
  testId,
}: EntitySymbolProps & { content: BadgeContent }) {
  const g = BADGE_POS[entity];
  const style = {
    '--o': `var(${syntacticVarName(entity)})`,
    display: 'inline-block',
    verticalAlign: '-0.2em',
  } as CSSProperties;
  return (
    <svg
      className={`duotone ${className}`}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      style={style}
      aria-hidden="true"
      focusable="false"
      data-testid={testId}
    >
      {ENTITY_GLYPHS[entity]()}
      <circle className="badge-bg" cx={g.bx} cy={g.by} r={g.br} />
      {content(g.bx, g.by, g.br)}
    </svg>
  );
}

/** Duotone "delete <entity>" (entity color + red ✕ badge). */
export function EntityDeleteSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={deleteX} />;
}
/** Duotone "edit <entity>" (entity color + orange pencil badge). */
export function EntityEditSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={editPencil} />;
}
/** Duotone "zero datum on <entity>" (entity color + golden star badge). */
export function EntityDatumSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={datumStar} />;
}
/** Duotone "mark <entity> as jump" (entity color + yellow J badge, #27). */
export function EntityJumpSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={jumpLetter} />;
}
/** Duotone "mark <entity> as reference" (entity color + blue R badge, #27). */
export function EntityReferenceKindSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={referenceLetter} />;
}
/** Duotone "unmark jump" (entity color + struck-through J badge, #27). */
export function EntityJumpUnmarkSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={jumpLetterStruck} />;
}
/** Duotone "unmark reference" (entity color + struck-through R badge, #27). */
export function EntityReferenceUnmarkSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={referenceLetterStruck} />;
}
/** Duotone "mark <entity> as permanent" (entity color + open-eye badge). */
export function EntityPermanentSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={openEye} />;
}
/** Duotone "add <entity>" (entity color + green plus badge). */
export function EntityAddSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={greenPlus} />;
}
/** Duotone "unlink <entity>" (entity color + broken-chain badge) - remove
 * closure / unmerge. */
export function EntityUnlinkSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={brokenChain} />;
}
/** Duotone "<entity> comment" (entity color + chat-bubble badge). */
export function EntityCommentSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={chatBubble} />;
}
/** Duotone "<entity> data" (entity color + data-rows/hamburger badge). */
export function EntityDataSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={dataLines} />;
}
/** Duotone "<entity> label/name" (entity color + capital-A badge). */
export function EntityLabelSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={letterA} />;
}

/** Compacted entity glyph under a full-canvas action deco (move compass /
 * reverse arrows). The object keeps its own `--sh-<entity>` color; the deco is
 * the shared slate action tone. */
function EntityDecoSymbol({
  entity,
  deco,
  size = 16,
  className = '',
  testId,
}: EntitySymbolProps & { deco: ReactNode }) {
  const style = {
    '--o': `var(${syntacticVarName(entity)})`,
    display: 'inline-block',
    verticalAlign: '-0.2em',
  } as CSSProperties;
  return (
    <svg
      className={`duotone ${className}`}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      style={style}
      aria-hidden="true"
      focusable="false"
      data-testid={testId}
    >
      {deco}
      {INNER_GLYPHS[entity]()}
    </svg>
  );
}

/** Duotone "move/reposition <entity>" (entity color + slate move compass) - the
 * menu counterpart of the move tools, e.g. a line's "Adjust Position...". */
export function EntityMoveSymbol(props: EntitySymbolProps) {
  return <EntityDecoSymbol {...props} deco={<MoveDeco />} />;
}
/** Duotone "reverse <entity>" (entity color + slate counter-arrow badge, #24). */
export function EntityReverseSymbol(props: EntitySymbolProps) {
  return <EntityBadgedSymbol {...props} content={reverseArrows} />;
}

/** CSS `var(--sh-<role>)` reference for a tool's accent color (icon + button tint). */
export function toolAccentVar(tool: ToolType): string {
  return `var(${syntacticVarName(TOOL_ROLE_MAP[tool])})`;
}
