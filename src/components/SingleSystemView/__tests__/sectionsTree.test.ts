import { describe, expect, it } from 'vitest';
import type { Line } from '../../../types/ipc/Line';
import type { Section } from '../../../types/ipc/Section';
import { UNGROUPED_GROUP_ID } from '../../../store/sectionsViewSlice';
import {
  buildSectionTree,
  defaultSectionName,
  dropChangesSomething,
  dropGroupAttr,
  linesToCarry,
  parseDropGroup,
  sectionNameProblem,
  treeArrowAction,
  treePositions,
} from '../sectionsTree';

function line(id: number, section?: number): Line {
  return {
    id,
    name: `L${id}`,
    color: '#123456',
    direction: 'in',
    quality: 1,
    shots: [],
    next_shot_id: 0,
    reverse_parity: false,
    gauge_shot_uuid: null,
    gauge_neighbour_uuid: null,
    ...(section === undefined ? {} : { cave_section_id: section }),
  } as Line;
}

const SECTIONS: Section[] = [
  { id: 7, name: 'Upper level', color: '#ff0000' },
  { id: 3, name: 'Sump', color: '#00ff00' },
];

const kinds = (rows: ReturnType<typeof buildSectionTree>['rows']) =>
  rows.map((r) => (r.kind === 'group' ? `G${r.group}` : `L${r.line.id}@${r.level}`));

describe('buildSectionTree', () => {
  it('is the flat list of lines while the project has no Section', () => {
    const tree = buildSectionTree([line(1), line(2)], [], new Set(), new Set());
    expect(tree.grouped).toBe(false);
    expect(kinds(tree.rows)).toEqual(['L1@1', 'L2@1']);
    expect(tree.rows.every((r) => r.kind === 'line' && r.group === null)).toBe(true);
    expect(tree.rowOfLine.get(2)).toBe(1);
  });

  it('lists Sections in panel order, their lines in project order, then Ungrouped', () => {
    const lines = [line(1), line(2, 3), line(4, 7), line(5, 3), line(6)];
    const tree = buildSectionTree(lines, SECTIONS, new Set(), new Set());
    expect(tree.grouped).toBe(true);
    expect(kinds(tree.rows)).toEqual(['G7', 'L4@2', 'G3', 'L2@2', 'L5@2', `G${UNGROUPED_GROUP_ID}`, 'L1@2', 'L6@2']);
    const sump = tree.rows[2]!;
    expect(sump.kind === 'group' && sump.lineIds).toEqual([2, 5]);
    expect(tree.groupOfLine.get(6)).toBe(UNGROUPED_GROUP_ID);
    expect(tree.rowOfGroup.get(3)).toBe(2);
  });

  it('keeps an empty Ungrouped group as a drop target', () => {
    const tree = buildSectionTree([line(1, 7)], SECTIONS, new Set(), new Set());
    expect(kinds(tree.rows)).toEqual(['G7', 'L1@2', 'G3', `G${UNGROUPED_GROUP_ID}`]);
  });

  it('a collapsed group lists no lines; they resolve to its header', () => {
    const lines = [line(1, 7), line(2, 7), line(3)];
    const tree = buildSectionTree(lines, SECTIONS, new Set([7]), new Set());
    expect(kinds(tree.rows)).toEqual(['G7', 'G3', `G${UNGROUPED_GROUP_ID}`, 'L3@2']);
    expect(tree.rows[0]!.kind === 'group' && tree.rows[0]!.collapsed).toBe(true);
    expect(tree.rowOfLine.get(1)).toBe(0);
    expect(tree.rowOfLine.get(2)).toBe(0);
  });

  it('marks a hidden Section and its lines', () => {
    const tree = buildSectionTree([line(1, 3), line(2)], SECTIONS, new Set(), new Set([3]));
    const header = tree.rows.find((r) => r.kind === 'group' && r.group === 3)!;
    expect(header.kind === 'group' && header.hidden).toBe(true);
    const member = tree.rows.find((r) => r.kind === 'line' && r.line.id === 1)!;
    expect(member.kind === 'line' && member.hidden).toBe(true);
    const ungrouped = tree.rows.find((r) => r.kind === 'line' && r.line.id === 2)!;
    expect(ungrouped.kind === 'line' && ungrouped.hidden).toBe(false);
  });

  it('lists a line naming no Section as ungrouped instead of losing it', () => {
    const tree = buildSectionTree([line(1, 99)], SECTIONS, new Set(), new Set());
    expect(tree.groupOfLine.get(1)).toBe(UNGROUPED_GROUP_ID);
    expect(kinds(tree.rows).slice(-1)).toEqual(['L1@2']);
  });
});

describe('treePositions', () => {
  it('counts headers among headers and lines within their group', () => {
    const tree = buildSectionTree([line(1, 7), line(2, 7), line(3)], SECTIONS, new Set(), new Set());
    expect(treePositions(tree.rows)).toEqual([
      { posinset: 1, setsize: 3 },
      { posinset: 1, setsize: 2 },
      { posinset: 2, setsize: 2 },
      { posinset: 2, setsize: 3 },
      { posinset: 3, setsize: 3 },
      { posinset: 1, setsize: 1 },
    ]);
  });

  it('counts the whole flat list as one set', () => {
    const tree = buildSectionTree([line(1), line(2), line(3)], [], new Set(), new Set());
    expect(treePositions(tree.rows).map((p) => `${p.posinset}/${p.setsize}`)).toEqual(['1/3', '2/3', '3/3']);
  });
});

describe('treeArrowAction', () => {
  const expanded = buildSectionTree([line(1, 7), line(2)], SECTIONS, new Set(), new Set()).rows;
  // G7, L1, G3, Gungrouped, L2

  it('Right expands a collapsed header, then steps into an expanded one', () => {
    const collapsed = buildSectionTree([line(1, 7)], SECTIONS, new Set([7]), new Set()).rows;
    expect(treeArrowAction(collapsed, 0, 'ArrowRight')).toEqual({ type: 'expand', group: 7 });
    expect(treeArrowAction(expanded, 0, 'ArrowRight')).toEqual({ type: 'focus', index: 1 });
  });

  it('Right on an expanded but empty header does nothing', () => {
    expect(treeArrowAction(expanded, 2, 'ArrowRight')).toEqual({ type: 'none' });
  });

  it('Left collapses an expanded header, and does nothing on a collapsed one', () => {
    expect(treeArrowAction(expanded, 0, 'ArrowLeft')).toEqual({ type: 'collapse', group: 7 });
    const collapsed = buildSectionTree([line(1, 7)], SECTIONS, new Set([7]), new Set()).rows;
    expect(treeArrowAction(collapsed, 0, 'ArrowLeft')).toEqual({ type: 'none' });
  });

  it('Left on a line steps up to its header; a flat list line has none', () => {
    expect(treeArrowAction(expanded, 1, 'ArrowLeft')).toEqual({ type: 'focus', index: 0 });
    expect(treeArrowAction(expanded, 4, 'ArrowLeft')).toEqual({ type: 'focus', index: 3 });
    const flat = buildSectionTree([line(1)], [], new Set(), new Set()).rows;
    expect(treeArrowAction(flat, 0, 'ArrowLeft')).toEqual({ type: 'none' });
    expect(treeArrowAction(flat, 0, 'ArrowRight')).toEqual({ type: 'none' });
  });
});

describe('Section names', () => {
  it('defaults to the first free "Section N"', () => {
    expect(defaultSectionName([])).toBe('Section 1');
    expect(defaultSectionName([{ name: 'Section 1' }, { name: 'Section 3' }])).toBe('Section 2');
    expect(defaultSectionName([{ name: 'Abschnitt 1' }], 'Abschnitt')).toBe('Abschnitt 2');
  });

  it('refuses empty, overlong and taken names like the backend', () => {
    const sections = [{ id: 1, name: 'Upper' }, { id: 2, name: 'Lower' }];
    expect(sectionNameProblem('  ', sections, 1)).toEqual({ kind: 'empty' });
    expect(sectionNameProblem('x'.repeat(201), sections, 1)).toEqual({ kind: 'tooLong' });
    expect(sectionNameProblem('x'.repeat(200), sections, 1)).toBeNull();
    expect(sectionNameProblem(' Lower ', sections, 1)).toEqual({ kind: 'taken', name: 'Lower' });
    // Its own name is not "taken".
    expect(sectionNameProblem('Upper', sections, 1)).toBeNull();
    // Names compare exactly after trimming, as the backend does.
    expect(sectionNameProblem('lower', sections, 1)).toBeNull();
  });
});

describe('drag targets', () => {
  it('carries the whole line selection only when the pressed line is part of it', () => {
    const order = [5, 1, 3, 2];
    expect(linesToCarry(3, new Set([1, 3]), order)).toEqual([1, 3]);
    expect(linesToCarry(2, new Set([1, 3]), order)).toEqual([2]);
    expect(linesToCarry(1, new Set([1]), order)).toEqual([1]);
  });

  it('round-trips the drop-group attribute', () => {
    expect(parseDropGroup(dropGroupAttr(7))).toBe(7);
    expect(parseDropGroup(dropGroupAttr(UNGROUPED_GROUP_ID))).toBe(UNGROUPED_GROUP_ID);
    expect(parseDropGroup(null)).toBeNull();
    expect(parseDropGroup('line:4')).toBeNull();
  });

  it('a drop changes something only when some carried line is listed elsewhere', () => {
    const groupOf = new Map([[1, 7], [2, 7], [3, UNGROUPED_GROUP_ID]]);
    expect(dropChangesSomething([1, 2], 7, groupOf)).toBe(false);
    expect(dropChangesSomething([1, 3], 7, groupOf)).toBe(true);
    expect(dropChangesSomething([3], UNGROUPED_GROUP_ID, groupOf)).toBe(false);
    expect(dropChangesSomething([1], UNGROUPED_GROUP_ID, groupOf)).toBe(true);
  });
});
