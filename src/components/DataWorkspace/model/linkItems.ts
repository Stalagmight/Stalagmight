/**
 * A station's links as the chips show them (zero datum, branch start, merges,
 * closures), in chip order, with the partner's "<label>:<index>" kept apart
 * from the prose around it so the chips can set it bold. One list feeds the
 * chips, the "+N" list, the gutter's link count and the find, so what is
 * highlighted is exactly what a chip says.
 */
import type { StationRef } from '../../../types/project';
import type { DataModelCtx, StationRow } from '../types';
import { stationName, tr } from './ctx';
import { misclosureText } from './columns';
import { stationLinks, type ClosureLink } from './links';

export interface LinkItem {
  key: string;
  kind: 'zero' | 'branch' | 'merge' | 'closure';
  /** Prose before / after the partner's name ("Closes to " / ""). */
  before: string;
  /** The partner station's "<label>:<index>" (bold in a chip); null for the
   *  zero datum, which names no station. */
  name: string | null;
  after: string;
  /** A known closure's "Δ 0.7 m · 1.2 %"; '' otherwise. */
  misclosure: string;
  /** The station a click goes to; null for the zero datum. */
  target: StationRef | null;
  closure: ClosureLink | null;
}

const NAME_SLOT = '\u0000';

/** A translated "... {{name}} ..." split around the name. */
export function proseAround(ctx: Pick<DataModelCtx, 't'>, key: string, fallback: string): [string, string] {
  const text = tr(ctx, key, fallback, { name: NAME_SLOT });
  const at = text.indexOf(NAME_SLOT);
  return at < 0 ? [text, ''] : [text.slice(0, at), text.slice(at + NAME_SLOT.length)];
}

/** The chip's full text: what the "+N" list shows and what find searches. */
export function linkItemLabel(item: LinkItem): string {
  const main = `${item.before}${item.name ?? ''}${item.after}`;
  return item.misclosure ? `${main} · ${item.misclosure}` : main;
}

const NONE: readonly LinkItem[] = Object.freeze([]);

export function stationLinkItems(ctx: DataModelCtx, row: StationRow): readonly LinkItem[] {
  const links = stationLinks(ctx, row.sectionId, row.stationIndex, row.g);
  // Most stations: nothing (find runs this for every station row).
  if (!links.zeroDatum && links.merges.length === 0 && links.closures.length === 0) return NONE;
  const items: LinkItem[] = [];
  const base = { after: '', misclosure: '', closure: null };
  if (links.zeroDatum) {
    items.push({ ...base, key: 'zd', kind: 'zero', before: tr(ctx, 'dataWorkspace.links.zeroDatum', 'Zero datum'), name: null, target: null });
  }
  const from = links.branchFrom;
  if (from) {
    const [before, after] = proseAround(ctx, 'dataWorkspace.links.continuesFrom', 'Continues from {{name}}');
    items.push({
      ...base,
      key: 'from',
      kind: 'branch',
      before,
      after,
      name: stationName(ctx, from.section_id, from.station_index),
      target: from,
    });
  }
  for (const m of links.merges) {
    if (from && m.section_id === from.section_id && m.station_index === from.station_index) continue;
    items.push({
      ...base,
      key: `m-${m.section_id}-${m.station_index}`,
      kind: 'merge',
      before: '⇄ ',
      name: stationName(ctx, m.section_id, m.station_index),
      target: m,
    });
  }
  if (links.closures.length > 0) {
    const [before, after] = proseAround(ctx, 'dataWorkspace.links.closesTo', 'Closes to {{name}}');
    for (const c of links.closures) {
      items.push({
        key: `c-${c.closureId}-${c.partner.section_id}-${c.partner.station_index}`,
        kind: 'closure',
        before,
        after,
        name: stationName(ctx, c.partner.section_id, c.partner.station_index),
        misclosure: misclosureText(ctx, c.misclosure, c.errorPct),
        target: c.partner,
        closure: c,
      });
    }
  }
  return items;
}

/**
 * The text find searches for a station's links (the `ROW_LINKS_KEY`
 * pseudo-column): the chip labels one per line, so no literal query spans two
 * chips. '' when the station has none.
 */
export function linksSearchText(items: readonly LinkItem[]): string {
  return items.map(linkItemLabel).join('\n');
}

/** Which items a set of [start, end) ranges into `linksSearchText` touch. */
export function itemsHit(items: readonly LinkItem[], ranges: readonly (readonly [number, number])[]): Set<number> {
  const hit = new Set<number>();
  let at = 0;
  items.forEach((item, i) => {
    const end = at + linkItemLabel(item).length;
    if (ranges.some(([s, e]) => s < end && e > at)) hit.add(i);
    at = end + 1;
  });
  return hit;
}
