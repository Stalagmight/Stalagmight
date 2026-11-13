/**
 * The row-name gutter of the Data workspace grid: a 3px tint stripe, the link
 * rail, the row name (stations bold "Label:idx"; shots indented "Label:pos"
 * with a muted "i→i+1"), a revealed-row marker, and the "Show on map" button
 * (on row hover / the active row). Hovering the name opens the hover card
 * after a short delay, beside the gutter so it never covers the rows being
 * read; it closes on scroll, on any mouse press outside it and when a context
 * menu opens. Hovering a linked station highlights its partner rows.
 */
import { memo, useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { Tooltip } from '../common/Tooltip';
import { EyeIcon, PinIcon } from '../common/icons';
import type { DataModelCtx, DataRow } from './types';
import type { RailCell } from './model/links';
import { linkedRowKeys, stationLinks } from './model/links';
import { sectionLabelOf } from './model/ctx';
import { stationDisplayName } from './model/columns';
import { DataRail, RAIL_WIDTH } from './DataRail';
import { DataHoverCard } from './DataHoverCard';
import { dataRowPart } from './dataTestIds';
import type { DataNav } from './useDataNavigation';

export const DATA_GUTTER_WIDTH = 232;
const NAME_LEFT = 3 + RAIL_WIDTH + 4;

export type RevealReason = 'search' | 'nav' | null;

interface Props {
  row: DataRow;
  ctx: DataModelCtx;
  rail: RailCell | null;
  tint: string | null;
  revealed: RevealReason;
  nav: DataNav;
}

/** Hover card trigger. The card closes (until the pointer leaves the name)
 *  when anything scrolls, on a mouse press anywhere but in the card itself,
 *  and when a context menu opens - also when that happens before the card's
 *  open delay ran out. */
function HoverName({
  row,
  ctx,
  nav,
  anchorRef,
  children,
}: {
  row: DataRow;
  ctx: DataModelCtx;
  nav: DataNav;
  anchorRef: RefObject<HTMLElement | null>;
  children: ReactNode;
}) {
  const [hovered, setHovered] = useState(false);
  const [suppressed, setSuppressed] = useState(false);
  useEffect(() => {
    if (!hovered) return undefined;
    const suppress = (e: Event) => {
      // The card's own links and buttons work as usual.
      if (e.type === 'mousedown' && e.target instanceof Element && e.target.closest('.dw-hover-card')) return;
      setSuppressed(true);
    };
    window.addEventListener('scroll', suppress, true);
    window.addEventListener('mousedown', suppress, true);
    window.addEventListener('contextmenu', suppress, true);
    return () => {
      window.removeEventListener('scroll', suppress, true);
      window.removeEventListener('mousedown', suppress, true);
      window.removeEventListener('contextmenu', suppress, true);
    };
  }, [hovered]);
  if (row.kind === 'section') return <>{children}</>;
  const press = () => setSuppressed(true);
  return (
    // A press on the name while the card is still pending (not open yet) must
    // cancel it too: the window listeners above only run once it is open.
    <span className="contents" onMouseDown={press} onContextMenu={press}>
      <Tooltip
        className="min-w-0 flex-1"
        tooltipClassName="dw-hover-card"
        tooltipTestId={`data-hover-${dataRowPart(row)}`}
        openDelayMs={350}
        closeDelayMs={250}
        placement="right"
        anchorRef={anchorRef}
        open={suppressed ? false : undefined}
        onHoverChange={(on) => {
          setHovered(on);
          if (!on) setSuppressed(false);
          if (row.kind !== 'station') return;
          const keys = on ? linkedRowKeys(stationLinks(ctx, row.sectionId, row.stationIndex, row.g)) : [];
          nav.setHoveredPartners(keys.length > 0 ? keys : null);
        }}
        content={<DataHoverCard row={row} />}
      >
        {children}
      </Tooltip>
    </span>
  );
}

function DataRowLabelImpl({ row, ctx, rail, tint, revealed, nav }: Props) {
  const { t } = useTranslation();
  const gutterRef = useRef<HTMLDivElement>(null);
  const part = dataRowPart(row);
  const label = sectionLabelOf(ctx, row.sectionId);
  let name: ReactNode = null;
  if (row.kind === 'station') {
    const stationName = stationDisplayName(ctx, row.g);
    name = (
      <span className="dw-row-name flex min-w-0 items-baseline gap-1.5 truncate" data-testid={`data-row-name-${part}`}>
        <span className="truncate">{`${label}:${row.stationIndex}`}</span>
        {stationName && <span className="truncate text-[11px] font-normal text-text-secondary">{stationName}</span>}
      </span>
    );
  } else if (row.kind === 'shot') {
    name = (
      <span className="dw-row-name flex min-w-0 items-baseline gap-1.5 truncate pl-3" data-testid={`data-row-name-${part}`}>
        <span className="truncate">{`${label}:${row.position}`}</span>
        <span className="shrink-0 text-[11px] opacity-70">{`${row.shotIndex}→${row.shotIndex + 1}`}</span>
      </span>
    );
  } else {
    // A header row's gutter is blank but for the rail's branch elbow: the band
    // beside it names the section. While the row is active or selected the
    // label shows (CSS), so the highlighted gutter is never an empty box.
    name = (
      <span className="dw-group-label flex min-w-0 items-center gap-1.5" data-testid={`data-row-name-${part}`}>
        <span
          className="h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: ctx.sectionsById.get(row.sectionId)?.color }}
          aria-hidden="true"
        />
        <span className="truncate">{label}</span>
      </span>
    );
  }

  return (
    <div ref={gutterRef} className="relative flex h-full w-full min-w-0 items-center text-left text-xs">
      {tint && <span className="absolute inset-y-0 left-0 w-[3px]" style={{ backgroundColor: tint }} aria-hidden="true" />}
      {rail && <DataRail cell={rail} />}
      <div className="flex min-w-0 flex-1 items-center" style={{ paddingLeft: NAME_LEFT }}>
        <HoverName row={row} ctx={ctx} nav={nav} anchorRef={gutterRef}>
          {name}
        </HoverName>
      </div>
      {revealed && (
        <span
          className="mr-0.5 flex shrink-0 items-center text-accent"
          title={t(revealed === 'search' ? 'dataWorkspace.gutter.revealedSearch' : 'dataWorkspace.gutter.revealedNav')}
          data-testid={`data-row-revealed-${part}`}
        >
          <EyeIcon size={11} />
          <span className="dw-revealed-mark" aria-hidden="true" />
        </span>
      )}
      {row.kind !== 'section' && (
        <button
          type="button"
          tabIndex={-1}
          className="dw-map-btn mr-1 flex h-5 w-5 shrink-0 items-center justify-center rounded text-text-secondary hover:bg-accent-10 hover:text-accent"
          aria-label={t('dataWorkspace.menu.showOnMap')}
          title={t('dataWorkspace.gutter.showOnMap', { shortcut: t('dataWorkspace.shortcuts.showOnMap') })}
          onMouseDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            nav.showOnMap(row);
          }}
          data-testid={`data-row-map-${part}`}
        >
          <PinIcon size={13} />
        </button>
      )}
    </div>
  );
}

export const DataRowLabel = memo(DataRowLabelImpl);
