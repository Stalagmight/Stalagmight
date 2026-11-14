/**
 * A section's group-header row in the Data grid: collapse chevron, colour
 * dot, full name and id, station / shot counts with the surveyed length, and
 * - for a branch section - a "continues from <row>" chip that jumps to the
 * parent station. It starts in the gutter, right after the link rail
 * (`groupContentStart: 'gutter'`), so the section's name sits where the row
 * IDs below it start.
 *
 * `DataSectionCorner` names the same section in the grid's top-left header
 * corner once its header row has scrolled away.
 */
import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { ChevronDownIcon, ChevronRightIcon } from '../common/icons';
import { formatFixed, formatNumber } from '../../i18n/format';
import { useDataWorkspaceStore } from './store';
import type { DataModelCtx } from './types';
import { stationName } from './model/ctx';
import { branchFromOfSection, stationRefKey } from './model/links';
import type { SectionStats } from './useDataWorkspaceModel';
import type { DataNav } from './useDataNavigation';
import { GUTTER_STRIPE, RAIL_LANE } from './DataRail';

export function DataSectionHeader({
  sectionId,
  ctx,
  stats,
  collapsed,
  nav,
}: {
  sectionId: number;
  ctx: DataModelCtx;
  stats: SectionStats | undefined;
  collapsed: boolean;
  nav: DataNav;
}) {
  const { t } = useTranslation();
  const color = ctx.sectionsById.get(sectionId)?.color;
  const parent = branchFromOfSection(ctx, sectionId);
  const parentName = parent ? stationName(ctx, parent.section_id, parent.station_index) : null;
  const fullName = sectionFullName(ctx, sectionId);
  return (
    <div className="dw-section-header flex h-full min-w-0 items-center gap-1.5 pr-3" data-testid={`data-section-header-${sectionId}`}>
      <button
        type="button"
        className="-ml-1 flex h-5 w-5 shrink-0 items-center justify-center rounded text-text-secondary hover:bg-accent-10 hover:text-text-primary"
        aria-expanded={!collapsed}
        aria-label={t(collapsed ? 'dataWorkspace.section.expand' : 'dataWorkspace.section.collapse')}
        title={t(collapsed ? 'dataWorkspace.section.expand' : 'dataWorkspace.section.collapse')}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          useDataWorkspaceStore.getState().toggleSectionCollapsed(sectionId);
        }}
        data-testid={`data-section-toggle-${sectionId}`}
      >
        {collapsed ? <ChevronRightIcon size={14} /> : <ChevronDownIcon size={14} />}
      </button>
      <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ backgroundColor: color }} aria-hidden="true" />
      <span className="min-w-0 truncate font-semibold" title={fullName}>
        {fullName}
      </span>
      <span className="shrink-0 text-text-secondary">{`#${sectionId}`}</span>
      {stats && (
        <span className="ml-1.5 shrink-0 tabular-nums text-text-secondary" data-testid={`data-section-counts-${sectionId}`}>
          {t('dataWorkspace.section.counts', {
            stations: formatNumber(stats.stations),
            shots: formatNumber(stats.shots),
            length: formatFixed(stats.length, ctx.locale, ctx.decimals?.length ?? ctx.precision),
          })}
        </span>
      )}
      {parent && parentName && (
        <span className="ml-1.5 flex min-w-0 shrink items-center gap-1 text-text-secondary">
          <span className="shrink-0">{t('dataWorkspace.section.continuesFrom')}</span>
          <button
            type="button"
            className="dw-chip sh-pill inline-flex min-w-0 items-center rounded-full px-2 py-0.5 text-xs leading-none text-text-primary"
            style={{ '--o': 'var(--sh-section)' } as CSSProperties}
            title={t('dataWorkspace.links.continuesFromTitle', { name: parentName })}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              nav.goToRow(stationRefKey(parent));
            }}
            data-testid={`data-section-continues-${sectionId}`}
          >
            <span className="truncate">{parentName}</span>
          </button>
        </span>
      )}
    </div>
  );
}

function sectionFullName(ctx: DataModelCtx, sectionId: number): string {
  return ctx.sectionNames.get(sectionId) ?? String(sectionId);
}

/** Left inset of the corner's dot: centred over the rail's lane below it. */
const CORNER_DOT_LEFT = GUTTER_STRIPE + RAIL_LANE - 5;

/**
 * The section the rows at the top of the grid belong to, for the header
 * corner above the gutter once that section's header row has scrolled away:
 * its colour dot (over the rail's lane), full name and `#id`.
 */
export function DataSectionCorner({ sectionId, ctx }: { sectionId: number; ctx: DataModelCtx }) {
  const fullName = sectionFullName(ctx, sectionId);
  return (
    <div
      className="dw-section-corner flex h-full w-full min-w-0 items-center gap-1.5 pr-2 text-xs"
      style={{ paddingLeft: CORNER_DOT_LEFT }}
      title={`${fullName} #${sectionId}`}
      data-testid={`data-section-corner-${sectionId}`}
    >
      <span
        className="h-2.5 w-2.5 shrink-0 rounded-full"
        style={{ backgroundColor: ctx.sectionsById.get(sectionId)?.color }}
        aria-hidden="true"
      />
      <span className="min-w-0 truncate font-semibold text-text-primary">{fullName}</span>
      <span className="shrink-0 font-normal text-text-secondary">{`#${sectionId}`}</span>
    </div>
  );
}
