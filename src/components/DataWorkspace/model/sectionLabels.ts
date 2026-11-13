/**
 * Unique, unambiguous, <= 16 character section labels for the Data
 * workspace's row names (`<label>:<id>`).
 *
 * Rules, in order:
 *   1. The display name is the section's name with control / bidi characters
 *      removed (`displayStationName`), whitespace collapsed and trimmed; an
 *      empty name falls back to the default ("Section <id>").
 *   2. Sections that share the SAME display name are told apart by their id:
 *      "Main #12", "Main #13" (the base truncated with an ellipsis when the
 *      id needs the room).
 *   3. A unique name that fits is used as is.
 *   4. A unique name that does not fit is elided in the middle so both its
 *      start and its end survive ("Downstre…sump 1A"). When two long names
 *      elide to the same label, other prefix / suffix splits are tried, then a
 *      window around where they first differ, and finally the id suffix.
 *   5. Any label still shared (a literal name equal to a derived one) gets the
 *      id suffix. Labels carrying " #<id>" are unique because ids are.
 *
 * Lengths count user-perceived characters (grapheme clusters), so an accented
 * letter or an emoji is never cut in half. The ellipsis is the single character
 * U+2026. Deterministic: the same sections always give the same labels.
 */
import { displayStationName } from '../../../utils/formatters';

export const SECTION_LABEL_MAX = 16;
const ELLIPSIS = '…';

type Segmenter = { segment(input: string): Iterable<{ segment: string }> };
let segmenter: Segmenter | null | undefined;

/** Split into grapheme clusters (code points where Intl.Segmenter is missing). */
export function graphemes(text: string): string[] {
  if (segmenter === undefined) {
    const Ctor = (Intl as unknown as { Segmenter?: new (l?: string, o?: { granularity: string }) => Segmenter })
      .Segmenter;
    segmenter = Ctor ? new Ctor(undefined, { granularity: 'grapheme' }) : null;
  }
  if (!segmenter) return Array.from(text);
  const out: string[] = [];
  for (const s of segmenter.segment(text)) out.push(s.segment);
  return out;
}

/** The name as a label shows it: sanitized, single-spaced, trimmed. */
export function cleanSectionName(name: string | null | undefined): string {
  if (!name) return '';
  // Whitespace first, so a tab or newline separates words instead of vanishing.
  return displayStationName(name.replace(/\s+/gu, ' ')).replace(/ +/gu, ' ').trim();
}

const join = (g: readonly string[], from: number, to: number) => g.slice(from, to).join('');

/** `prefix…suffix` with the spaces next to the ellipsis dropped. */
function elide(g: readonly string[], prefix: number, suffix: number): string {
  const head = join(g, 0, prefix).trimEnd();
  const tail = suffix > 0 ? join(g, g.length - suffix, g.length).trimStart() : '';
  return `${head}${ELLIPSIS}${tail}`;
}

/** A label that fits in `max` graphemes, eliding the end with an ellipsis. */
function truncateEnd(g: readonly string[], max: number): string {
  if (g.length <= max) return g.join('');
  return `${join(g, 0, Math.max(0, max - 1)).trimEnd()}${ELLIPSIS}`;
}

/** The id-tagged form: "Main #12", or "Main passa… #12" when the id needs room. */
function withId(name: string, id: number): string {
  const tag = ` #${id}`;
  const room = Math.max(1, SECTION_LABEL_MAX - tag.length);
  return `${truncateEnd(graphemes(name), room)}${tag}`;
}

/** Prefix / suffix splits for a middle elision, the balanced one first, then
 *  alternating toward a longer suffix and a longer prefix. */
function splitOrder(): Array<[number, number]> {
  const budget = SECTION_LABEL_MAX - 1;
  const balanced = Math.ceil(budget / 2);
  const out: Array<[number, number]> = [[balanced, budget - balanced]];
  for (let k = 1; k <= budget; k++) {
    const shorterPrefix = balanced - k;
    const longerPrefix = balanced + k;
    if (shorterPrefix >= 0) out.push([shorterPrefix, budget - shorterPrefix]);
    if (longerPrefix <= budget) out.push([longerPrefix, budget - longerPrefix]);
  }
  return out;
}
const SPLITS = splitOrder();

function commonPrefixLength(a: readonly string[], b: readonly string[]): number {
  const n = Math.min(a.length, b.length);
  let i = 0;
  while (i < n && a[i] === b[i]) i++;
  return i;
}

/** A window around the first grapheme where the group's names differ. */
function windowLabel(g: readonly string[], firstDiff: number): string {
  const lead = 4;
  const width = SECTION_LABEL_MAX - lead - 2;
  const start = Math.max(lead, firstDiff - 3);
  if (start + width + 1 >= g.length) return elide(g, lead, Math.min(g.length - lead, SECTION_LABEL_MAX - lead - 1));
  return `${join(g, 0, lead).trimEnd()}${ELLIPSIS}${join(g, start, start + width).trim()}${ELLIPSIS}`;
}

function allDistinct(labels: readonly string[], taken: ReadonlySet<string>): boolean {
  const seen = new Set<string>();
  for (const l of labels) {
    if (seen.has(l) || taken.has(l)) return false;
    seen.add(l);
  }
  return true;
}

export function buildSectionLabels(
  sections: readonly { id: number; name: string | null }[],
  defaultName: (id: number) => string,
): Map<number, string> {
  const names = sections.map((s) => {
    const clean = cleanSectionName(s.name);
    return { id: s.id, name: clean !== '' ? clean : cleanSectionName(defaultName(s.id)) || String(s.id) };
  });

  const byName = new Map<string, number>();
  for (const n of names) byName.set(n.name, (byName.get(n.name) ?? 0) + 1);

  const labels = new Map<number, string>();
  const tagged = new Set<number>();
  const taken = new Set<string>();
  const long: { id: number; g: string[] }[] = [];

  for (const n of names) {
    if ((byName.get(n.name) ?? 0) > 1) {
      labels.set(n.id, withId(n.name, n.id));
      tagged.add(n.id);
      continue;
    }
    const g = graphemes(n.name);
    if (g.length <= SECTION_LABEL_MAX) {
      labels.set(n.id, n.name);
      taken.add(n.name);
    } else {
      long.push({ id: n.id, g });
    }
  }
  for (const id of tagged) taken.add(labels.get(id)!);

  // Long unique names: group by the default (balanced) elision; a group of one
  // keeps it, a larger group searches for a form that separates its members.
  const [p0, s0] = SPLITS[0]!;
  const groups = new Map<string, { id: number; g: string[] }[]>();
  for (const item of long) {
    const key = elide(item.g, p0, s0);
    const list = groups.get(key);
    if (list) list.push(item);
    else groups.set(key, [item]);
  }
  const groupKeys = [...groups.keys()].sort();
  for (const key of groupKeys) {
    const group = groups.get(key)!;
    if (group.length === 1 && !taken.has(key)) {
      labels.set(group[0]!.id, key);
      taken.add(key);
      continue;
    }
    let chosen: string[] | null = null;
    for (const [p, s] of SPLITS) {
      const candidate = group.map((m) => elide(m.g, p, s));
      if (allDistinct(candidate, taken)) {
        chosen = candidate;
        break;
      }
    }
    if (!chosen && group.length > 1) {
      let firstDiff = Infinity;
      for (let i = 0; i < group.length; i++) {
        for (let j = i + 1; j < group.length; j++) {
          firstDiff = Math.min(firstDiff, commonPrefixLength(group[i]!.g, group[j]!.g));
        }
      }
      const candidate = group.map((m) => windowLabel(m.g, firstDiff));
      if (allDistinct(candidate, taken)) chosen = candidate;
    }
    group.forEach((m, i) => {
      const label = chosen ? chosen[i]! : withId(m.g.join(''), m.id);
      if (!chosen) tagged.add(m.id);
      labels.set(m.id, label);
      taken.add(label);
    });
  }

  // Last pass: a label still shared by several sections (a literal name equal
  // to a derived one) is tagged with the id on every untagged holder.
  const nameById = new Map(names.map((n) => [n.id, n.name]));
  for (let pass = 0; pass < 2; pass++) {
    const holders = new Map<string, number[]>();
    for (const [id, label] of labels) {
      const list = holders.get(label);
      if (list) list.push(id);
      else holders.set(label, [id]);
    }
    let changed = false;
    for (const ids of holders.values()) {
      if (ids.length < 2) continue;
      for (const id of ids) {
        if (tagged.has(id)) continue;
        labels.set(id, withId(nameById.get(id) ?? String(id), id));
        tagged.add(id);
        changed = true;
      }
    }
    if (!changed) break;
  }
  return labels;
}
