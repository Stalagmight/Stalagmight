/**
 * A station's links as chips (zero datum, "Continues from", merges, closures
 * with their misclosure), laid out by LinkChipRow (whole chips, then "+N").
 * The grid draws them across the station row's shot-only cells, which a
 * station has no value in (`spanCells`). The partner's "<label>:<index>" is
 * bold, the prose around it is not. A find match in the links outlines the
 * chip(s) it falls in.
 *
 * `GutterLinks` is the fallback when no shot-only column is shown: a link
 * icon with the count at the gutter's right, its tooltip listing the links;
 * a click goes to the first linked station.
 */
import type { CSSProperties, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { CellMarks } from '../common/SpreadsheetGrid/types';
import { LinkIcon } from '../common/icons';
import { getErrorColor } from '../../utils/colors';
import { testIdPart } from '../../utils/testId';
import type { DataModelCtx, StationRow } from './types';
import { decimalsOf, fmt } from './model/ctx';
import { stationRefKey } from './model/links';
import { itemsHit, linkItemLabel, type LinkItem } from './model/linkItems';
import { dataRowPart } from './dataTestIds';
import { LinkChipRow, type LinkChip } from './LinkChipRow';
import type { DataNav } from './useDataNavigation';

const CHIP_CLS =
  'dw-chip sh-pill inline-flex min-w-0 shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-xs leading-none text-text-primary';

const stop = (e: { stopPropagation(): void }) => e.stopPropagation();

function chipStyle(color: string): CSSProperties {
  return { '--o': color } as CSSProperties;
}

/** The items a find match falls in, and the one holding the current match. */
export function linkMatchState(items: readonly LinkItem[], marks: CellMarks | undefined): { hit: Set<number>; current: Set<number> } {
  if (!marks) return { hit: new Set(), current: new Set() };
  return { hit: itemsHit(items, marks.ranges), current: marks.current ? itemsHit(items, [marks.current]) : new Set() };
}

export const matchClass = (on: boolean, current: boolean): string =>
  on ? (current ? ' dw-match-outline dw-match-outline-current' : ' dw-match-outline') : '';

/** "Closes to **Main:3**": the prose normal, the station name bold. */
export function LinkText({ item }: { item: LinkItem }): ReactNode {
  return (
    <>
      {item.before}
      {item.name !== null && <span className="font-semibold">{item.name}</span>}
      {item.after}
    </>
  );
}

function itemTitle(item: LinkItem, ctx: DataModelCtx, t: TFunction): string {
  const name = item.name ?? '';
  switch (item.kind) {
    case 'zero':
      return t('dataWorkspace.links.zeroDatumTitle');
    case 'branch':
      return t('dataWorkspace.links.continuesFromTitle', { name });
    case 'merge':
      return t('dataWorkspace.links.mergeTitle', { name });
    case 'closure': {
      const title = t('dataWorkspace.links.closureTitle', { name });
      const c = item.closure;
      if (!item.misclosure || !c || c.misclosure === null || c.errorPct === null) return title;
      return `${title}\n${t('dataWorkspace.links.misclosureTitle', {
        distance: fmt(ctx, c.misclosure, decimalsOf(ctx).length),
        percent: fmt(ctx, c.errorPct, 1),
      })}`;
    }
  }
}

function chipColor(item: LinkItem): string {
  if (item.kind === 'zero') return 'var(--cc-zero-datum)';
  if (item.kind === 'branch') return 'var(--sh-section)';
  if (item.kind === 'merge') return 'var(--sh-merge)';
  const c = item.closure;
  // Tinted by the error level; the station colour while not computed.
  return item.misclosure && c && c.errorPct !== null ? getErrorColor(c.errorPct) : 'var(--sh-station)';
}

function chipTestId(item: LinkItem, part: string): string {
  const target = item.target ? `${item.target.section_id}-${item.target.station_index}` : '';
  switch (item.kind) {
    case 'zero':
      return `data-chip-zero-${part}`;
    case 'branch':
      return `data-chip-branch-${part}-${target}`;
    case 'merge':
      return `data-chip-merge-${part}-${target}`;
    case 'closure':
      return `data-chip-closure-${part}-${testIdPart(item.closure?.closureId ?? '')}-${target}`;
  }
}

export function StationLinkChips({
  row,
  items,
  ctx,
  nav,
  marks,
}: {
  row: StationRow;
  items: readonly LinkItem[];
  ctx: DataModelCtx;
  nav: DataNav;
  /** Find matches in `linksSearchText(items)`. */
  marks?: CellMarks | undefined;
}) {
  const { t } = useTranslation();
  const part = dataRowPart(row);
  const { hit, current } = linkMatchState(items, marks);
  const chips: LinkChip[] = items.map((item, i) => {
    const cls = `${CHIP_CLS}${matchClass(hit.has(i), current.has(i))}`;
    const testId = chipTestId(item, part);
    const title = itemTitle(item, ctx, t);
    const label = linkItemLabel(item);
    if (!item.target) {
      return {
        key: item.key,
        label,
        node: (
          <span className={`${cls} cursor-default`} style={chipStyle(chipColor(item))} title={title} data-testid={testId}>
            <LinkText item={item} />
          </span>
        ),
      };
    }
    const target = item.target;
    return {
      key: item.key,
      label,
      node: (
        <button
          type="button"
          className={cls}
          style={chipStyle(chipColor(item))}
          title={title}
          onMouseDown={stop}
          onClick={(e) => {
            e.stopPropagation();
            nav.goToRow(stationRefKey(target));
          }}
          data-testid={testId}
        >
          <span className="truncate">
            <LinkText item={item} />
          </span>
          {item.misclosure && (
            <>
              <span className="shrink-0 opacity-60" aria-hidden="true">
                ·
              </span>
              <span
                className="shrink-0 tabular-nums"
                data-testid={`data-chip-misclosure-${part}-${testIdPart(item.closure?.closureId ?? '')}`}
              >
                {item.misclosure}
              </span>
            </>
          )}
        </button>
      ),
    };
  });
  if (chips.length === 0) return null;
  return (
    // The span sits over the row's cells: keep presses on it (between the
    // chips too) from starting a cell selection under it.
    <span className="flex h-full w-full min-w-0 items-center px-2" onMouseDown={stop} data-testid={`data-links-span-${part}`}>
      <LinkChipRow chips={chips} testId={`data-links-${part}`} moreTestId={`data-links-more-${part}`} />
    </span>
  );
}

/** The links of a station whose chips have nowhere to go (no shot-only
 *  column shown): an icon + count; the tooltip lists them. */
export function GutterLinks({
  row,
  items,
  nav,
  marks,
}: {
  row: StationRow;
  items: readonly LinkItem[];
  nav: DataNav;
  marks?: CellMarks | undefined;
}) {
  const { t } = useTranslation();
  if (items.length === 0) return null;
  const part = dataRowPart(row);
  const { hit, current } = linkMatchState(items, marks);
  const first = items.find((i) => i.target !== null)?.target ?? null;
  const list = items.map(linkItemLabel).join('\n');
  const cls = `mr-0.5 flex h-5 shrink-0 items-center gap-0.5 rounded px-0.5 text-[11px] tabular-nums text-text-secondary${matchClass(hit.size > 0, current.size > 0)}`;
  const testId = `data-row-links-${part}`;
  const title = list;
  const ariaLabel = t('dataWorkspace.gutter.linksAria', { count: items.length, list });
  const content = (
    <>
      <LinkIcon size={12} />
      {items.length}
    </>
  );
  // Only a zero datum: nothing to go to.
  if (first === null) {
    return (
      <span className={`${cls} cursor-default`} title={title} aria-label={ariaLabel} onMouseDown={stop} data-testid={testId}>
        {content}
      </span>
    );
  }
  return (
    <button
      type="button"
      tabIndex={-1}
      className={`${cls} hover:bg-accent-10 hover:text-accent`}
      title={title}
      aria-label={ariaLabel}
      onMouseDown={stop}
      onClick={(e) => {
        e.stopPropagation();
        nav.goToRow(stationRefKey(first));
      }}
      data-testid={testId}
    >
      {content}
    </button>
  );
}
