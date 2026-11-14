/**
 * The few glyphs the Data workspace toolbar needs that `common/icons.tsx` does
 * not have yet (magnifier, funnel, tint, more). Same visual language: a 20px
 * grid, filled shapes rather than thin strokes (they hold weight under WebKit
 * page zoom), `currentColor`, decorative unless titled.
 */
import type { ReactNode, SVGProps } from 'react';

interface Props extends Omit<SVGProps<SVGSVGElement>, 'children'> {
  size?: number;
}

function Glyph({ size = 16, children, ...props }: Props & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="currentColor"
      stroke="none"
      aria-hidden
      focusable="false"
      {...props}
    >
      {children}
    </svg>
  );
}

export function SearchIcon(props: Props) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M8.5 2.5 a6 6 0 1 1 0 12 a6 6 0 0 1 0 -12 Z M8.5 4.4 a4.1 4.1 0 1 0 0 8.2 a4.1 4.1 0 0 0 0 -8.2 Z"
      />
      <path d="M12.3 13.65 L13.65 12.3 L17.6 16.25 L16.25 17.6 Z" />
    </Glyph>
  );
}

export function FilterIcon(props: Props) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M2.5 3.5 H17.5 L11.75 10.4 V16.2 L8.25 14.4 V10.4 Z M5.3 4.9 L9.65 10 V13.55 L10.35 13.9 V10 L14.7 4.9 Z"
      />
    </Glyph>
  );
}

export function TintIcon(props: Props) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M10 2.5 a7.5 7.5 0 1 1 0 15 a7.5 7.5 0 0 1 0 -15 Z M10 4.4 a5.6 5.6 0 0 0 0 11.2 Z"
      />
    </Glyph>
  );
}

export function MoreIcon(props: Props) {
  return (
    <Glyph {...props}>
      <circle cx="4.5" cy="10" r="1.6" />
      <circle cx="10" cy="10" r="1.6" />
      <circle cx="15.5" cy="10" r="1.6" />
    </Glyph>
  );
}
