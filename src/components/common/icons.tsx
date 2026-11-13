import type { ReactNode, SVGProps } from 'react';

export interface IconProps extends Omit<SVGProps<SVGSVGElement>, 'children'> {
  size?: number;
  title?: string;
}

function BaseIcon({
  size = 16,
  title,
  className,
  children,
  ...props
}: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      role={title ? 'img' : 'presentation'}
      aria-hidden={title ? undefined : true}
      focusable="false"
      {...props}
    >
      {title ? <title>{title}</title> : null}
      {children}
    </svg>
  );
}

function BaseIcon24({
  size = 16,
  title,
  className,
  children,
  ...props
}: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      role={title ? 'img' : 'presentation'}
      aria-hidden={title ? undefined : true}
      focusable="false"
      {...props}
    >
      {title ? <title>{title}</title> : null}
      {children}
    </svg>
  );
}

// A stroked straight segment rendered as a filled, round-capped bar (same
// centerline + round caps as the original stroke). Fills rasterize cleanly under
// the WebKit/WKWebView page-zoom UI scale, where thin strokes get fattened - see
// the webkit-icon-stroke-zoom note. `w` is the bar weight (≈ former strokeWidth):
// 1.9 in the 20-unit BaseIcon space, ~2.2 in the 24-unit BaseIcon24 space.
function Bar({
  x1,
  y1,
  x2,
  y2,
  w = 1.9,
}: {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  w?: number;
}) {
  const cx = (x1 + x2) / 2;
  const cy = (y1 + y2) / 2;
  // Extend the rect by w (w/2 at each end) so the rounded ends sit *beyond* the
  // endpoints - matching strokeLinecap="round", which caps half a stroke-width
  // past each end. Without this the glyph reads noticeably short (arrows/chevrons).
  const len = Math.hypot(x2 - x1, y2 - y1) + w;
  const angle = (Math.atan2(y2 - y1, x2 - x1) * 180) / Math.PI;
  return (
    <rect
      x={cx - len / 2}
      y={cy - w / 2}
      width={len}
      height={w}
      rx={w / 2}
      transform={`rotate(${angle} ${cx} ${cy})`}
      fill="currentColor"
      stroke="none"
    />
  );
}

// A stroked circle rendered as a filled annulus (evenodd ring) of weight `w`.
function Annulus({
  cx,
  cy,
  r,
  w = 1.9,
}: {
  cx: number;
  cy: number;
  r: number;
  w?: number;
}) {
  const ri = r - w;
  return (
    <path
      fillRule="evenodd"
      fill="currentColor"
      stroke="none"
      d={`M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0ZM${cx - ri} ${cy}a${ri} ${ri} 0 1 0 ${2 * ri} 0a${ri} ${ri} 0 1 0 ${-2 * ri} 0Z`}
    />
  );
}

// A stroked arc rendered as a filled band (the curve preserved exactly), from
// angle `a1` to `a2` (degrees, SVG y-down) about (cx, cy) at centerline radius
// `r`, weight `w`.
function ArcBand({
  cx,
  cy,
  r,
  a1,
  a2,
  w = 1.9,
}: {
  cx: number;
  cy: number;
  r: number;
  a1: number;
  a2: number;
  w?: number;
}) {
  const ro = r + w / 2;
  const ri = r - w / 2;
  const rad = (a: number) => (a * Math.PI) / 180;
  const pt = (rr: number, a: number) =>
    `${cx + rr * Math.cos(rad(a))} ${cy + rr * Math.sin(rad(a))}`;
  const large = Math.abs(a2 - a1) > 180 ? 1 : 0;
  const sweep = a2 > a1 ? 1 : 0;
  return (
    <path
      fill="currentColor"
      stroke="none"
      d={`M${pt(ro, a1)}A${ro} ${ro} 0 ${large} ${sweep} ${pt(ro, a2)}L${pt(ri, a2)}A${ri} ${ri} 0 ${large} ${1 - sweep} ${pt(ri, a1)}Z`}
    />
  );
}

export function FitToViewIcon(props: IconProps) {
  return (
    <BaseIcon24 {...props}>
      {/* Corner brackets */}
      <Bar x1={3} y1={8} x2={3} y2={3} w={2.2} />
      <Bar x1={3} y1={3} x2={8} y2={3} w={2.2} />
      <Bar x1={16} y1={3} x2={21} y2={3} w={2.2} />
      <Bar x1={21} y1={3} x2={21} y2={8} w={2.2} />
      <Bar x1={21} y1={16} x2={21} y2={21} w={2.2} />
      <Bar x1={21} y1={21} x2={16} y2={21} w={2.2} />
      <Bar x1={8} y1={21} x2={3} y2={21} w={2.2} />
      <Bar x1={3} y1={21} x2={3} y2={16} w={2.2} />
      {/* Inward diagonals */}
      <Bar x1={9} y1={9} x2={5} y2={5} w={2.2} />
      <Bar x1={15} y1={9} x2={19} y2={5} w={2.2} />
      <Bar x1={15} y1={15} x2={19} y2={19} w={2.2} />
      <Bar x1={9} y1={15} x2={5} y2={19} w={2.2} />
    </BaseIcon24>
  );
}

export function CenterMapIcon(props: IconProps) {
  return (
    <BaseIcon24 {...props}>
      <Annulus cx={12} cy={12} r={8} w={2.2} />
      <Bar x1={12} y1={2} x2={12} y2={7} w={2.2} />
      <Bar x1={12} y1={17} x2={12} y2={22} w={2.2} />
      <Bar x1={2} y1={12} x2={7} y2={12} w={2.2} />
      <Bar x1={17} y1={12} x2={22} y2={12} w={2.2} />
      <circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none" />
    </BaseIcon24>
  );
}

export function ChevronDownIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={6} y1={8} x2={10} y2={12} />
      <Bar x1={10} y1={12} x2={14} y2={8} />
    </BaseIcon>
  );
}

export function ChevronUpIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={6} y1={12} x2={10} y2={8} />
      <Bar x1={10} y1={8} x2={14} y2={12} />
    </BaseIcon>
  );
}

export function ChevronLeftIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={12} y1={6} x2={8} y2={10} />
      <Bar x1={8} y1={10} x2={12} y2={14} />
    </BaseIcon>
  );
}

export function ChevronRightIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={8} y1={6} x2={12} y2={10} />
      <Bar x1={12} y1={10} x2={8} y2={14} />
    </BaseIcon>
  );
}

export function ArrowUpRightIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={5} y1={15} x2={15} y2={5} />
      <Bar x1={10} y1={5} x2={15} y2={5} />
      <Bar x1={15} y1={5} x2={15} y2={10} />
    </BaseIcon>
  );
}

// Undo / redo curved arrows. FILLED (fill=currentColor, stroke=none) rather than
// stroked: WebKit fattens strokes under the toolbar's UI zoom scale, so the
// condensed-toolbar undo/redo glyphs are solid paths that stay crisp at any zoom.
export function UndoIcon(props: IconProps) {
  return (
    <BaseIcon24 {...props}>
      <path
        fill="currentColor"
        stroke="none"
        d="M12.5 8c-2.65 0-5.05.99-6.9 2.6L2 7v9h9l-3.62-3.62c1.39-1.16 3.16-1.88 5.12-1.88 3.54 0 6.55 2.31 7.6 5.5l2.37-.78C21.08 11.03 17.15 8 12.5 8z"
      />
    </BaseIcon24>
  );
}

export function RedoIcon(props: IconProps) {
  return (
    <BaseIcon24 {...props}>
      <path
        fill="currentColor"
        stroke="none"
        d="M18.4 10.6C16.55 8.99 14.15 8 11.5 8c-4.65 0-8.58 3.03-9.96 7.22L3.9 16c1.05-3.19 4.05-5.5 7.6-5.5 1.95 0 3.73.72 5.12 1.88L13 16h9V7l-3.6 3.6z"
      />
    </BaseIcon24>
  );
}

// "External link" / open-out icon: a box with an arrow breaking out of the
// open top-right corner. The arrowhead's two arms are collinear with the box's
// top and right edges (aligned with the square), with a gap so the shapes stay
// distinct. Used as the section focus-mode entry affordance.
export function ExternalLinkIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Open box (top-right corner left open for the arrow to break out) */}
      <Bar x1={8} y1={5} x2={5} y2={5} />
      <Bar x1={5} y1={5} x2={3} y2={7} />
      <Bar x1={3} y1={7} x2={3} y2={15} />
      <Bar x1={3} y1={15} x2={5} y2={17} />
      <Bar x1={5} y1={17} x2={13} y2={17} />
      <Bar x1={13} y1={17} x2={15} y2={15} />
      <Bar x1={15} y1={15} x2={15} y2={12} />
      {/* Breakout arrow */}
      <Bar x1={11} y1={5} x2={15} y2={5} />
      <Bar x1={15} y1={5} x2={15} y2={9} />
      <Bar x1={9} y1={11} x2={15} y2={5} />
    </BaseIcon>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={10} y1={4} x2={10} y2={16} />
      <Bar x1={4} y1={10} x2={16} y2={10} />
    </BaseIcon>
  );
}

export function MinusIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={4} y1={10} x2={16} y2={10} />
    </BaseIcon>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path fill="currentColor" stroke="none" d="M4.33 5.67 L5.67 4.33 L15.67 14.33 L14.33 15.67 Z" />
      <path fill="currentColor" stroke="none" d="M15.67 5.67 L14.33 4.33 L4.33 14.33 L5.67 15.67 Z" />
    </BaseIcon>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={4} y1={10} x2={8} y2={14} />
      <Bar x1={8} y1={14} x2={16} y2={6} />
    </BaseIcon>
  );
}

export function CircleIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Annulus cx={10} cy={10} r={6} />
    </BaseIcon>
  );
}

export function DotIcon(props: IconProps) {
  return (
    <BaseIcon {...props} fill="currentColor" stroke="none">
      <circle cx="10" cy="10" r="2.5" />
    </BaseIcon>
  );
}

export function CheckCircleIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Annulus cx={10} cy={10} r={7} />
      <Bar x1={6} y1={10} x2={9} y2={13} />
      <Bar x1={9} y1={13} x2={14} y2={7} />
    </BaseIcon>
  );
}

export function AlertTriangleIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={10} y1={3} x2={18} y2={17} />
      <Bar x1={18} y1={17} x2={2} y2={17} />
      <Bar x1={2} y1={17} x2={10} y2={3} />
      <Bar x1={10} y1={8} x2={10} y2={12} />
      <circle cx="10" cy="14.5" r="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function AlertCircleIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Annulus cx={10} cy={10} r={7} />
      <Bar x1={10} y1={6} x2={10} y2={11} />
      <circle cx="10" cy="14" r="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function DatabaseIcon(props: IconProps) {
  return (
    <BaseIcon24 {...props}>
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M3 5 C3 2.8 7.03 1 12 1 C16.97 1 21 2.8 21 5 L21 19 C21 21.2 16.97 23 12 23 C7.03 23 3 21.2 3 19 Z M4.95 5.6 C4.95 4.05 8.21 2.8 12 2.8 C15.79 2.8 19.05 4.05 19.05 5.6 L19.05 18.4 C19.05 19.95 15.79 21.2 12 21.2 C8.21 21.2 4.95 19.95 4.95 18.4 Z" />
      <path fill="currentColor" stroke="none" d="M3.3 10.4 C5.1 12 8.3 13 12 13 C15.7 13 18.9 12 20.7 10.4 C18.9 13.4 15.7 14.9 12 14.9 C8.3 14.9 5.1 13.4 3.3 10.4 Z" />
    </BaseIcon24>
  );
}

export function EyeIcon(props: IconProps) {
  // Filled (lens band knocked out via evenodd + filled pupil) rather than
  // stroked, so it holds weight at any UI scale (WebKit fattens thin strokes
  // under page-zoom; fills rasterize cleanly).
  return (
    <BaseIcon {...props}>
      <path
        fillRule="evenodd"
        fill="currentColor"
        stroke="none"
        d="M2 10 C2 10 5 5 10 5 C15 5 18 10 18 10 C18 10 15 15 10 15 C5 15 2 10 2 10 Z M3.9 10 C3.9 10 6.34 6.34 10 6.9 C13.66 6.34 16.1 10 16.1 10 C16.1 10 13.66 13.66 10 13.1 C6.34 13.66 3.9 10 3.9 10 Z"
      />
      <circle cx="10" cy="10" r="2.5" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function MapIcon(props: IconProps) {
  // Hollow folded-map outline (band) + the two fold creases, rendered as fills.
  return (
    <BaseIcon {...props}>
      <path
        fillRule="evenodd"
        fill="currentColor"
        stroke="none"
        d="M3 4 L8 2 L12 4 L17 2 V16 L12 18 L8 16 L3 18 Z M4.44 5.24 L8.46 3.84 L11.4 5.8 L15.75 3.43 L15.56 14.76 L11.54 16.16 L8.6 14.2 L4.25 16.57 Z"
      />
      <path
        fill="currentColor"
        stroke="none"
        d="M7.05 4.3 H8.95 V13.8 H7.05 Z M11.05 6.3 H12.95 V15.8 H11.05 Z"
      />
    </BaseIcon>
  );
}

export function DownloadIcon(props: IconProps) {
  // Faithful outline download (shaft bar + chevron band + tray frame) as fills,
  // so it keeps weight at any UI scale (WebKit fattens thin strokes at zoom).
  return (
    <BaseIcon {...props}>
      <path fill="currentColor" stroke="none" d="M9.05 3 H10.95 V11.5 H9.05 Z" />
      <path fill="currentColor" stroke="none" d="M6 9 L10 13 L14 9 L12.3 7.85 L10 11.1 L7.7 7.85 Z" />
      <rect x="4" y="15.05" width="12" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function DeviceCableIcon(props: IconProps) {
  // A handheld instrument on its cable: Import from MNemo over USB.
  return (
    <BaseIcon {...props}>
      <rect x="6" y="2.5" width="8" height="10" rx="1.5" />
      <path d="M8.5 6 H11.5" />
      <path d="M10 12.5 V15 C10 16.6 11 17.5 12.6 17.5 H15.5" />
    </BaseIcon>
  );
}

export function FilePlusIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M4 2 H12 L16 6 V18 H4 Z M5.9 3.9 H12 L14.1 6 V16.1 H5.9 Z" />
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M12 2 L12 6 L16 6 L16 4.4 L14 2.4 Z M14.1 4.4 L14.1 4.9 L12.9 4.9 L12.9 4.4 Z" />
      <rect x="9.05" y="9" width="1.9" height="6" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="6.9" y="11.05" width="6.2" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function FolderOpenIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M3 5 H8 L10 7 H17 V16 H3 Z M4.9 6.9 H7.5 L9.5 8.9 H15.1 V14.1 H4.9 Z" />
    </BaseIcon>
  );
}

export function PencilIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Pencil body outline */}
      <Bar x1={4} y1={14} x2={5} y2={10} />
      <Bar x1={5} y1={10} x2={12} y2={3} />
      <Bar x1={12} y1={3} x2={16} y2={7} />
      <Bar x1={16} y1={7} x2={9} y2={14} />
      <Bar x1={9} y1={14} x2={4} y2={14} />
      {/* Ferrule crease */}
      <Bar x1={11} y1={4} x2={15} y2={8} />
    </BaseIcon>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Lid + handle */}
      <Bar x1={3} y1={6} x2={17} y2={6} />
      <Bar x1={7} y1={4} x2={13} y2={4} />
      {/* Can outline */}
      <Bar x1={5} y1={6} x2={5} y2={17} />
      <Bar x1={15} y1={6} x2={15} y2={17} />
      <Bar x1={5} y1={17} x2={15} y2={17} />
      {/* Slots */}
      <Bar x1={8} y1={9} x2={8} y2={15} />
      <Bar x1={12} y1={9} x2={12} y2={15} />
    </BaseIcon>
  );
}

export function LinkIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Lower-left loop */}
      <Bar x1={8} y1={12} x2={6} y2={14} />
      <ArcBand cx={4.707} cy={11.293} r={3} a1={64.5} a2={205.5} />
      <Bar x1={2} y1={10} x2={4} y2={8} />
      {/* Upper-right loop */}
      <Bar x1={12} y1={8} x2={14} y2={6} />
      <ArcBand cx={15.293} cy={8.707} r={3} a1={244.5} a2={385.5} />
      <Bar x1={18} y1={10} x2={16} y2={12} />
      {/* Connecting bar */}
      <Bar x1={7} y1={13} x2={13} y2={7} />
    </BaseIcon>
  );
}

export function LinkOffIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Lower-left loop */}
      <Bar x1={8} y1={12} x2={6} y2={14} />
      <ArcBand cx={4.707} cy={11.293} r={3} a1={64.5} a2={205.5} />
      <Bar x1={2} y1={10} x2={4} y2={8} />
      {/* Upper-right loop */}
      <Bar x1={12} y1={8} x2={14} y2={6} />
      <ArcBand cx={15.293} cy={8.707} r={3} a1={244.5} a2={385.5} />
      <Bar x1={18} y1={10} x2={16} y2={12} />
      {/* Break slash */}
      <Bar x1={6} y1={6} x2={14} y2={14} />
    </BaseIcon>
  );
}

export function RulerIcon(props: IconProps) {
  // Hollow ruler frame (outline) + tick marks, rendered as fills.
  return (
    <BaseIcon {...props}>
      <path
        fillRule="evenodd"
        fill="currentColor"
        stroke="none"
        d="M4 6 H16 A1 1 0 0 1 17 7 V13 A1 1 0 0 1 16 14 H4 A1 1 0 0 1 3 13 V7 A1 1 0 0 1 4 6 Z M4.9 7.9 H15.1 V12.1 H4.9 Z"
      />
      <path
        fill="currentColor"
        stroke="none"
        d="M5.05 6 H6.95 V9.3 H5.05 Z M8.05 6 H9.95 V10.3 H8.05 Z M11.05 6 H12.95 V9.3 H11.05 Z M14.05 6 H15.95 V10.3 H14.05 Z"
      />
    </BaseIcon>
  );
}

export function MoveHorizontalIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={4} y1={10} x2={16} y2={10} />
      <Bar x1={6} y1={8} x2={4} y2={10} />
      <Bar x1={4} y1={10} x2={6} y2={12} />
      <Bar x1={14} y1={8} x2={16} y2={10} />
      <Bar x1={16} y1={10} x2={14} y2={12} />
    </BaseIcon>
  );
}

// Fullscreen enter/exit pair (data-table fullscreen mode): two diagonal arrows
// pointing out of (Expand) or into (Shrink) the corners.
export function ExpandIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={11} y1={9} x2={16} y2={4} />
      <Bar x1={12} y1={4} x2={16} y2={4} />
      <Bar x1={16} y1={4} x2={16} y2={8} />
      <Bar x1={9} y1={11} x2={4} y2={16} />
      <Bar x1={8} y1={16} x2={4} y2={16} />
      <Bar x1={4} y1={16} x2={4} y2={12} />
    </BaseIcon>
  );
}

export function ShrinkIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={16} y1={4} x2={11} y2={9} />
      <Bar x1={11} y1={5} x2={11} y2={9} />
      <Bar x1={11} y1={9} x2={15} y2={9} />
      <Bar x1={4} y1={16} x2={9} y2={11} />
      <Bar x1={9} y1={15} x2={9} y2={11} />
      <Bar x1={9} y1={11} x2={5} y2={11} />
    </BaseIcon>
  );
}

export function MoveAllIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={10} y1={3} x2={10} y2={17} />
      <Bar x1={3} y1={10} x2={17} y2={10} />
      <Bar x1={8} y1={5} x2={10} y2={3} />
      <Bar x1={10} y1={3} x2={12} y2={5} />
      <Bar x1={8} y1={15} x2={10} y2={17} />
      <Bar x1={10} y1={17} x2={12} y2={15} />
      <Bar x1={5} y1={8} x2={3} y2={10} />
      <Bar x1={3} y1={10} x2={5} y2={12} />
      <Bar x1={15} y1={8} x2={17} y2={10} />
      <Bar x1={17} y1={10} x2={15} y2={12} />
    </BaseIcon>
  );
}

export function LayersIcon(props: IconProps) {
  // Hollow top diamond (outline band) + a chevron band hinting the layer below.
  return (
    <BaseIcon {...props}>
      <path
        fillRule="evenodd"
        fill="currentColor"
        stroke="none"
        d="M3 8 L10 4 L17 8 L10 12 Z M4.9 8 L10 5.9 L15.1 8 L10 10.1 Z"
      />
      <path
        fill="currentColor"
        stroke="none"
        d="M3 12 L10 16 L17 12 L15.2 11.2 L10 14.1 L4.8 11.2 Z"
      />
    </BaseIcon>
  );
}

export function TargetIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Annulus cx={10} cy={10} r={6} />
      <circle cx="10" cy="10" r="2" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function DashedLineIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={3} y1={10} x2={5} y2={10} />
      <Bar x1={7} y1={10} x2={9} y2={10} />
      <Bar x1={11} y1={10} x2={13} y2={10} />
      <Bar x1={15} y1={10} x2={17} y2={10} />
    </BaseIcon>
  );
}

/* --- Plan-mode route picks (#298). One glyph per right-click entry, so the menu reads at a
   glance rather than showing four identical fallback arrows. --- */

/** U-turn arrow: up one side, over the top, down the other side into an arrowhead pointing
 *  back the way it came - "turn around here". 24x24 space, stroke-based so it stays legible
 *  as a tiny white-stroke glyph on a dark chip (map marker) as well as `currentColor` in the
 *  React icon. Shared (not duplicated) between {@link UTurnIcon}, the final-turn map marker
 *  (`useGasPlanDropMarkers.ts`) and the gas legend swatch (`GasLegend.tsx`) - #395. */
export const U_TURN_ARROW_PATH = 'M7 18V9A5 5 0 0 1 17 9V15M13 15L17 19L21 15';

/** Start point: a flag on a pole - "the dive begins here". */
export function FlagIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={5} y1={3} x2={5} y2={17} />
      <path d="M5 4h9l-2.5 3L14 10H5z" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

/** Way point: a map pin with a dot, distinct from the endpoint's target rings. */
export function WaypointIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path d="M10 18c3.5-4.2 5.5-7 5.5-9.5a5.5 5.5 0 1 0-11 0C4.5 11 6.5 13.8 10 18z" />
      <circle cx="10" cy="8.5" r="1.8" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

/** Stage drop: the same downward triangle the map draws for a stage's drop marker. */
export function StageDropIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path d="M3 6h14l-7 11z" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

/** Turnaround (final turn, #395): a U-turn arrow - "the back-gas leg runs out here". */
export function UTurnIcon(props: IconProps) {
  return (
    <BaseIcon24 {...props}>
      <path d={U_TURN_ARROW_PATH} />
    </BaseIcon24>
  );
}

/** Marked point: a bookmark - an annotation, deliberately not a route glyph. */
export function BookmarkIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path d="M6 3h8v14l-4-3.5L6 17z" />
    </BaseIcon>
  );
}

export function LockIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Body */}
      <Bar x1={5} y1={9} x2={15} y2={9} />
      <Bar x1={5} y1={17} x2={15} y2={17} />
      <Bar x1={5} y1={9} x2={5} y2={17} />
      <Bar x1={15} y1={9} x2={15} y2={17} />
      {/* Shackle */}
      <Bar x1={7} y1={9} x2={7} y2={6} />
      <ArcBand cx={10} cy={6} r={3} a1={180} a2={360} />
      <Bar x1={13} y1={6} x2={13} y2={9} />
    </BaseIcon>
  );
}

/** Open padlock: the body is identical to `LockIcon`; the shackle has swung up
 *  out of the body, so its right leg is gone and the arch is both raised and
 *  tightened - center (10.5, 4) at r 2.5, against LockIcon's (10, 6) at r 3.
 *  That lifts the arch's crown a full 1.5 units (3 -> 1.5), which is as high as
 *  it goes: the band's outer edge tops out at cy - r - w/2 = 0.55, just inside
 *  the 20-unit viewBox. The left leg lengthens to (8, 9)-(8, 4) to keep the
 *  arch joined to the body top, and lands where the arch's flat left end
 *  already is; the arch's right end at x 13 is exactly where LockIcon's right
 *  leg stands, so the pair reads as the same shackle latched vs sprung at
 *  14px. */
export function LockOpenIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Body (identical to LockIcon) */}
      <Bar x1={5} y1={9} x2={15} y2={9} />
      <Bar x1={5} y1={17} x2={15} y2={17} />
      <Bar x1={5} y1={9} x2={5} y2={17} />
      <Bar x1={15} y1={9} x2={15} y2={17} />
      {/* Shackle: hinged on the left, swung up and open on the right */}
      <Bar x1={8} y1={9} x2={8} y2={4} />
      <ArcBand cx={10.5} cy={4} r={2.5} a1={180} a2={360} />
    </BaseIcon>
  );
}

export function MessageIcon(props: IconProps) {
  // Hollow speech-bubble outline (band), rendered as a fill.
  return (
    <BaseIcon {...props}>
      <path
        fillRule="evenodd"
        fill="currentColor"
        stroke="none"
        d="M4 4 H16 A1 1 0 0 1 17 5 V13 A1 1 0 0 1 16 14 H8 L5 17 V14 H4 A1 1 0 0 1 3 13 V5 A1 1 0 0 1 4 4 Z M5.4 5.9 H14.6 A0.5 0.5 0 0 1 15.1 6.4 V11.6 A0.5 0.5 0 0 1 14.6 12.1 H8.2 L6.4 13.9 V12.1 H5.4 A0.5 0.5 0 0 1 4.9 11.6 V6.4 A0.5 0.5 0 0 1 5.4 5.9 Z"
      />
    </BaseIcon>
  );
}

export function BugIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Body (band) */}
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M10 7 A4 5 0 1 1 10 17 A4 5 0 1 1 10 7 Z M10 8.9 A2.1 3.1 0 1 0 10 15.1 A2.1 3.1 0 1 0 10 8.9 Z" />
      {/* Head (band) */}
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M10 4 A2 2 0 1 1 10 8 A2 2 0 1 1 10 4 Z M10 5.5 A0.5 0.5 0 1 0 10 6.5 A0.5 0.5 0 1 0 10 5.5 Z" />
      {/* Antennae + legs (bars) */}
      <path fill="currentColor" stroke="none" d="M8.84 4.84 L9.16 4.16 L6.34 1.66 L5.66 2.34 Z" />
      <path fill="currentColor" stroke="none" d="M11.16 4.84 L10.84 4.16 L13.66 1.66 L14.34 2.34 Z" />
      <path fill="currentColor" stroke="none" d="M6.4 10.79 L5.6 9.21 L2.6 7.21 L3.4 8.79 Z" />
      <path fill="currentColor" stroke="none" d="M6.4 12.21 L5.6 13.79 L2.6 14.79 L3.4 13.21 Z" />
      <path fill="currentColor" stroke="none" d="M13.6 10.79 L14.4 9.21 L17.4 7.21 L16.6 8.79 Z" />
      <path fill="currentColor" stroke="none" d="M13.6 12.21 L14.4 13.79 L17.4 14.79 L16.6 13.21 Z" />
    </BaseIcon>
  );
}

export function MenuIcon(props: IconProps) {
  // Filled bars (not strokes) so the icon keeps a consistent weight at any UI
  // scale - WebKit/WKWebView render thin strokes heavy under page-zoom, but
  // fills rasterize cleanly. Matches the duotone glyphs' filled base.
  return (
    <BaseIcon {...props}>
      <rect x="3" y="4.05" width="14" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="3" y="9.05" width="14" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="3" y="14.05" width="14" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function SettingsIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M10 6.1 A3.9 3.9 0 1 1 10 13.9 A3.9 3.9 0 1 1 10 6.1 M10 8 A2 2 0 1 0 10 12 A2 2 0 1 0 10 8" />
      <rect x="9.05" y="2" width="1.9" height="2.5" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="9.05" y="15.5" width="1.9" height="2.5" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="2" y="9.05" width="2.5" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="15.5" y="9.05" width="2.5" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <path fill="currentColor" stroke="none" d="M3.63 4.97 L5.03 3.57 L6.43 4.97 L5.03 6.37 Z" />
      <path fill="currentColor" stroke="none" d="M13.57 14.97 L14.97 13.57 L16.37 14.97 L14.97 16.37 Z" />
      <path fill="currentColor" stroke="none" d="M3.63 15.03 L5.03 16.43 L6.43 15.03 L5.03 13.63 Z" />
      <path fill="currentColor" stroke="none" d="M13.57 5.03 L14.97 6.43 L16.37 5.03 L14.97 3.63 Z" />
    </BaseIcon>
  );
}

export function QuitIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Power glyph: ring (band) with a top gap + a vertical stem */}
      <path fill="currentColor" stroke="none" d="M6 5 A6 6 0 1 0 14 5 L12.74 6.45 A4.1 4.1 0 1 1 7.26 6.45 Z" />
      <rect x="9.05" y="2" width="1.9" height="7" rx="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function ListIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <rect x="6.95" y="4.05" width="10.1" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="6.95" y="9.05" width="10.1" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="6.95" y="14.05" width="10.1" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <circle cx="3.25" cy="5" r="0.95" fill="currentColor" stroke="none" />
      <circle cx="3.25" cy="10" r="0.95" fill="currentColor" stroke="none" />
      <circle cx="3.25" cy="15" r="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function SaveIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Floppy disk */}
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M4 4 H13 L16 7 V16 H4 Z M5.45 5.1 H12.33 L14.73 7.4 V14.9 H5.45 Z" />
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M6 4 H12 V8 H6 Z M7.45 5.1 H10.55 V6.9 H7.45 Z" />
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M6 11 H14 V16 H6 Z M7.45 12.45 H12.55 V14.55 H7.45 Z" />
    </BaseIcon>
  );
}

export function ExportIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Box with an arrow leaving upward */}
      <path fill="currentColor" stroke="none" d="M9.05 4.5 H10.95 V13 H9.05 Z" />
      <path fill="currentColor" stroke="none" d="M6 8 L10 4 L14 8 L12.3 9.15 L10 5.9 L7.7 9.15 Z" />
      <path fill="currentColor" stroke="none" d="M4 12 L4 16 L16 16 L16 12 L14.1 12 L14.1 14.1 L5.9 14.1 L5.9 12 Z" />
    </BaseIcon>
  );
}

export function InfoIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <path fillRule="evenodd" fill="currentColor" stroke="none" d="M10 3 A7 7 0 1 1 10 17 A7 7 0 1 1 10 3 M10 4.9 A5.1 5.1 0 1 0 10 15.1 A5.1 5.1 0 1 0 10 4.9" />
      <rect x="9.05" y="9.05" width="1.9" height="5" rx="0.95" fill="currentColor" stroke="none" />
      <circle cx="10" cy="6.55" r="0.95" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

export function SlidersIcon(props: IconProps) {
  // Filled tracks + knobs.
  return (
    <BaseIcon {...props}>
      <rect x="3" y="5.05" width="14" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <rect x="3" y="13.05" width="14" height="1.9" rx="0.95" fill="currentColor" stroke="none" />
      <circle cx="8" cy="6" r="2.5" fill="currentColor" stroke="none" />
      <circle cx="13" cy="14" r="2.5" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

// Pushpin ("pin this panel open"). Filled cap + body + pleated collar + needle.
export function PinIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <rect x="6" y="3" width="8" height="2.2" rx="1.1" fill="currentColor" stroke="none" />
      <rect x="8.6" y="5" width="2.8" height="3" fill="currentColor" stroke="none" />
      <path d="M6.4 8 H13.6 L11.8 11 H8.2 Z" fill="currentColor" stroke="none" />
      <path d="M9.15 11 H10.85 L10 16.8 Z" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

/**
 * Survey sections: a short survey line (three shots between four stations), the
 * thing the Sections panel lists. Filled bars and discs, never strokes, so it
 * keeps its weight under the WebKit UI zoom.
 */
export function SectionsIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      <Bar x1={3.5} y1={15} x2={8} y2={7} />
      <Bar x1={8} y1={7} x2={12.5} y2={12} />
      <Bar x1={12.5} y1={12} x2={16.5} y2={4.5} />
      <circle cx="3.5" cy="15" r="2.2" fill="currentColor" stroke="none" />
      <circle cx="8" cy="7" r="2.2" fill="currentColor" stroke="none" />
      <circle cx="12.5" cy="12" r="2.2" fill="currentColor" stroke="none" />
      <circle cx="16.5" cy="4.5" r="2.2" fill="currentColor" stroke="none" />
    </BaseIcon>
  );
}

/** Sign in: an arrow entering a door frame. Filled bars (no strokes). */
export function SignInIcon(props: IconProps) {
  return (
    <BaseIcon {...props}>
      {/* Door frame, open on the left */}
      <Bar x1={11.5} y1={3.5} x2={16} y2={3.5} />
      <Bar x1={16} y1={3.5} x2={16} y2={16.5} />
      <Bar x1={16} y1={16.5} x2={11.5} y2={16.5} />
      {/* Arrow going in */}
      <Bar x1={3.5} y1={10} x2={12} y2={10} />
      <Bar x1={8.5} y1={6.5} x2={12} y2={10} />
      <Bar x1={12} y1={10} x2={8.5} y2={13.5} />
    </BaseIcon>
  );
}
