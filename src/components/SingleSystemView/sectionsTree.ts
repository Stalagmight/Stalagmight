/**
 * The Sections panel as a tree (#327), flattened for the virtualized list.
 *
 * A project with no Sections is the flat list of lines it always was (every
 * row at level 1). Once it has one, each Section is a level-1 header with its
 * lines beneath at level 2, in project order, followed by an "Ungrouped"
 * header holding the lines in no Section. The Ungrouped header is shown even
 * when it is empty: it is where a line is dragged to leave its Section.
 *
 * Pure: everything here is unit-tested without React.
 */
import type { TFunction } from 'i18next';
import type { Line, LineId } from '../../types/project';
import type { Section } from '../../types/ipc/Section';
import type { Selection } from '../../store/uiStore';
import { UNGROUPED_GROUP_ID } from '../../store/sectionsViewSlice';
import { hasForbiddenNameChars } from '../../utils/nameValidation';

/** A group of the tree: a Section's id, or {@link UNGROUPED_GROUP_ID}. */
export type GroupId = number;

export type TreeRow =
  | {
      kind: 'group';
      key: string;
      group: GroupId;
      /** The Section; `null` for the Ungrouped group. */
      section: Section | null;
      lineIds: LineId[];
      collapsed: boolean;
      /** Hidden on the map (Sections only). */
      hidden: boolean;
    }
  | {
      kind: 'line';
      key: string;
      line: Line;
      level: 1 | 2;
      /** The group the row is listed under; `null` in the flat (no Sections) list. */
      group: GroupId | null;
      /** Its Section is hidden on the map. */
      hidden: boolean;
    };

export interface SectionTree {
  rows: TreeRow[];
  /** True once the project has a Section: the list is a tree. */
  grouped: boolean;
  /** line id -> the row index that represents it: its own row, or its
   *  group's header while that group is collapsed. */
  rowOfLine: Map<LineId, number>;
  /** group -> its header's row index. */
  rowOfGroup: Map<GroupId, number>;
  /** The group each line is listed under (grouped trees only). */
  groupOfLine: Map<LineId, GroupId>;
}

export const groupKey = (group: GroupId) => (group === UNGROUPED_GROUP_ID ? 'ungrouped' : `section:${group}`);

/** Build the flattened tree. `collapsed` and `hidden` hold group / Section ids. */
export function buildSectionTree(
  lines: readonly Line[],
  sections: readonly Section[],
  collapsed: ReadonlySet<GroupId>,
  hidden: ReadonlySet<number>,
): SectionTree {
  const rows: TreeRow[] = [];
  const rowOfLine = new Map<LineId, number>();
  const rowOfGroup = new Map<GroupId, number>();
  const groupOfLine = new Map<LineId, GroupId>();
  if (sections.length === 0) {
    lines.forEach((line, i) => {
      rows.push({ kind: 'line', key: `line:${line.id}`, line, level: 1, group: null, hidden: false });
      rowOfLine.set(line.id, i);
    });
    return { rows, grouped: false, rowOfLine, rowOfGroup, groupOfLine };
  }

  const known = new Set(sections.map((s) => s.id));
  const members = new Map<GroupId, Line[]>();
  for (const line of lines) {
    const section = line.cave_section_id;
    // A membership naming no Section (never written by the backend, but cheap
    // to tolerate) lists the line as ungrouped rather than losing it.
    const group = section !== undefined && section !== null && known.has(section) ? section : UNGROUPED_GROUP_ID;
    groupOfLine.set(line.id, group);
    const list = members.get(group);
    if (list) list.push(line);
    else members.set(group, [line]);
  }

  const pushGroup = (group: GroupId, section: Section | null) => {
    const groupLines = members.get(group) ?? [];
    const isCollapsed = collapsed.has(group);
    const isHidden = section !== null && hidden.has(section.id);
    const headerIndex = rows.length;
    rowOfGroup.set(group, headerIndex);
    rows.push({
      kind: 'group',
      key: groupKey(group),
      group,
      section,
      lineIds: groupLines.map((l) => l.id),
      collapsed: isCollapsed,
      hidden: isHidden,
    });
    for (const line of groupLines) {
      if (isCollapsed) {
        rowOfLine.set(line.id, headerIndex);
        continue;
      }
      rowOfLine.set(line.id, rows.length);
      rows.push({ kind: 'line', key: `line:${line.id}`, line, level: 2, group, hidden: isHidden });
    }
  };
  for (const section of sections) pushGroup(section.id, section);
  pushGroup(UNGROUPED_GROUP_ID, null);
  return { rows, grouped: true, rowOfLine, rowOfGroup, groupOfLine };
}

/** `aria-posinset` / `aria-setsize` for every row: headers among the headers,
 *  lines among their group's lines (or the whole flat list). The list is
 *  virtualized, so assistive tech cannot count the siblings itself. */
export function treePositions(rows: readonly TreeRow[]): { posinset: number; setsize: number }[] {
  const out = rows.map(() => ({ posinset: 1, setsize: 1 }));
  const headers = rows.flatMap((r, i) => (r.kind === 'group' ? [i] : []));
  headers.forEach((i, n) => {
    out[i] = { posinset: n + 1, setsize: headers.length };
  });
  let run: number[] = [];
  const flush = () => {
    run.forEach((i, n) => {
      out[i] = { posinset: n + 1, setsize: run.length };
    });
    run = [];
  };
  rows.forEach((r, i) => {
    if (r.kind === 'group') flush();
    else run.push(i);
  });
  flush();
  return out;
}

/** What a tree key does from row `index` (WAI-ARIA tree pattern). */
export type TreeKeyAction =
  | { type: 'focus'; index: number }
  | { type: 'collapse'; group: GroupId }
  | { type: 'expand'; group: GroupId }
  | { type: 'none' };

/**
 * Left / Right in the tree: Right expands a collapsed header and steps into an
 * expanded one; Left collapses an expanded header and steps from a line up to
 * its header. Up / Down / Home / End stay with the panel's own handler (they
 * already walk the flattened rows).
 */
export function treeArrowAction(rows: readonly TreeRow[], index: number, key: 'ArrowLeft' | 'ArrowRight'): TreeKeyAction {
  const row = rows[index];
  if (!row) return { type: 'none' };
  if (row.kind === 'group') {
    if (key === 'ArrowRight') {
      if (row.collapsed) return { type: 'expand', group: row.group };
      const next = rows[index + 1];
      return next && next.kind === 'line' ? { type: 'focus', index: index + 1 } : { type: 'none' };
    }
    return row.collapsed ? { type: 'none' } : { type: 'collapse', group: row.group };
  }
  if (key === 'ArrowLeft' && row.level === 2) {
    for (let i = index - 1; i >= 0; i--) {
      if (rows[i]!.kind === 'group') return { type: 'focus', index: i };
    }
  }
  return { type: 'none' };
}

/** `Section N` with the smallest N >= 1 no Section is named yet. */
export function defaultSectionName(sections: readonly Pick<Section, 'name'>[], base = 'Section'): string {
  const taken = new Set(sections.map((s) => s.name));
  for (let n = 1; ; n++) {
    const name = `${base} ${n}`;
    if (!taken.has(name)) return name;
  }
}

/** The most characters a Section name may have (the backend's limit). */
export const SECTION_NAME_MAX = 200;

export type SectionNameProblem =
  | { kind: 'empty' }
  | { kind: 'tooLong' }
  | { kind: 'invalidChars' }
  | { kind: 'taken'; name: string };

/** Why `raw` cannot name Section `selfId` (null when it can): empty, too long,
 *  a character no name may hold, or another Section's name. Mirrors the
 *  backend so the panel can say so inline before sending anything; the
 *  backend still decides. */
export function sectionNameProblem(
  raw: string,
  sections: readonly Pick<Section, 'id' | 'name'>[],
  selfId: number | null,
): SectionNameProblem | null {
  const name = raw.trim();
  if (name === '') return { kind: 'empty' };
  if ([...name].length > SECTION_NAME_MAX) return { kind: 'tooLong' };
  if (hasForbiddenNameChars(raw)) return { kind: 'invalidChars' };
  if (sections.some((s) => s.id !== selfId && s.name === name)) return { kind: 'taken', name };
  return null;
}

/** The message for a {@link SectionNameProblem}. */
export function sectionNameProblemMessage(problem: SectionNameProblem, t: TFunction): string {
  switch (problem.kind) {
    case 'taken':
      return t('caveSections.rename.taken', { name: problem.name });
    case 'empty':
      return t('caveSections.rename.empty');
    case 'tooLong':
      return t('caveSections.rename.tooLong');
    case 'invalidChars':
      return t('identity.nameInvalidChars');
  }
}

/**
 * The selection after a modified click on a group header: Shift adds every
 * line of the group; Ctrl/Cmd toggles them - removes them all when every one
 * is selected already, else adds the missing ones (like a Ctrl-click on a line
 * row toggles that line). Other selected items stay.
 */
export function groupClickSelection(
  current: readonly Selection[],
  groupLineIds: readonly LineId[],
  mode: 'add' | 'toggle',
): Selection[] {
  const have = new Set(current.filter((s) => s.type === 'line').map((s) => s.lineId));
  const group = new Set(groupLineIds);
  if (mode === 'toggle' && groupLineIds.every((id) => have.has(id))) {
    return current.filter((s) => !(s.type === 'line' && s.lineId !== undefined && group.has(s.lineId)));
  }
  const added: Selection[] = groupLineIds.filter((id) => !have.has(id)).map((id) => ({ type: 'line', lineId: id }));
  return [...current, ...added];
}

/**
 * The lines a drag (or a menu action) started on `lineId` carries: the whole
 * line selection when the line is part of a selection of several lines, else
 * just that line. Keeps project order.
 */
export function linesToCarry(lineId: LineId, selectedLineIds: ReadonlySet<LineId>, order: readonly LineId[]): LineId[] {
  if (selectedLineIds.size > 1 && selectedLineIds.has(lineId)) {
    return order.filter((id) => selectedLineIds.has(id));
  }
  return [lineId];
}

/** The drop-target attribute value of a group (`data-drop-group`). */
export const dropGroupAttr = (group: GroupId) => groupKey(group);

/** The group a `data-drop-group` value names, or null when it names none. */
export function parseDropGroup(value: string | null | undefined): GroupId | null {
  if (!value) return null;
  if (value === 'ungrouped') return UNGROUPED_GROUP_ID;
  const m = /^section:(\d+)$/.exec(value);
  return m ? Number(m[1]) : null;
}

/** Whether dropping `lineIds` on `group` would change anything: at least one
 *  of them is listed elsewhere. */
export function dropChangesSomething(
  lineIds: readonly LineId[],
  group: GroupId,
  groupOfLine: ReadonlyMap<LineId, GroupId>,
): boolean {
  return lineIds.some((id) => (groupOfLine.get(id) ?? UNGROUPED_GROUP_ID) !== group);
}
