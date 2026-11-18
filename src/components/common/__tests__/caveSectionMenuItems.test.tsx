import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

import type { TFunction } from 'i18next';
import en from '../../../i18n/locales/en.json';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { caveSectionHeaderMenuItems, caveSectionLineMenuItems, createSectionWithRename } from '../caveSectionMenuItems';
import type { Line } from '../../../types/ipc/Line';

/** A t() over the real English strings (plural `_one` / `_other` by count). */
const t = ((key: string, opts?: Record<string, unknown>) => {
  const path = key.split('.');
  const count = opts?.count as number | undefined;
  let node: unknown = en;
  for (const part of path.slice(0, -1)) node = (node as Record<string, unknown>)[part];
  const leaf = path[path.length - 1]!;
  const table = node as Record<string, string>;
  const text = (count !== undefined && table[`${leaf}_${count === 1 ? 'one' : 'other'}`]) || table[leaf] || key;
  return text.replace(/\{\{(\w+)\}\}/g, (_m, k: string) => String(opts?.[k] ?? ''));
}) as unknown as TFunction;

const project = {
  lines: [
    { id: 1, cave_section_id: 7 },
    { id: 2, cave_section_id: 7 },
    { id: 3 },
  ] as unknown as Line[],
  cave_sections: [
    { id: 7, name: 'Upper level', color: '#ff0000' },
    { id: 8, name: 'Sump', color: '#00ff00' },
  ],
};

const ok = { success: true, warnings: [], error: null, line_ids: [] };

describe('Move to Section', () => {
  const move = vi.fn().mockResolvedValue(ok);
  const close = vi.fn();
  beforeEach(() => {
    move.mockClear();
    close.mockClear();
    useProjectStore.setState({ moveLinesToCaveSection: move });
  });

  it('offers every Section, No Section and New Section..., with the current one disabled', () => {
    const [row] = caveSectionLineMenuItems(1, [], project, t, close);
    expect(row!.label).toBe('Move to Section');
    expect(row!.submenu!.map((i) => [i.label, i.disabled ?? false])).toEqual([
      ['Upper level', true],
      ['Sump', false],
      ['No Section', false],
      ['New Section...', false],
    ]);
    expect(row!.submenu![0]!.hint).toBe('The line is already in this section');
  });

  it('moves the line, then closes the menu', async () => {
    const [row] = caveSectionLineMenuItems(1, [], project, t, close);
    await (row!.submenu![1]!.action as () => Promise<void>)();
    expect(move).toHaveBeenCalledWith([1], 8);
    expect(close).toHaveBeenCalled();
  });

  it('acts on the whole line selection when the line is part of it', async () => {
    const [row] = caveSectionLineMenuItems(3, [1, 3], project, t, close);
    expect(row!.label).toBe('Move 2 Lines to Section');
    // Mixed membership: no destination holds them all.
    expect(row!.submenu!.slice(0, 3).every((i) => !i.disabled)).toBe(true);
    await (row!.submenu![2]!.action as () => Promise<void>)();
    expect(move).toHaveBeenCalledWith([1, 3], null);
  });

  it('a selection the line is not in is ignored', () => {
    const [row] = caveSectionLineMenuItems(3, [1, 2], project, t, close);
    expect(row!.label).toBe('Move to Section');
    expect(row!.submenu![2]!.disabled).toBe(true);
  });

  it('New Section... creates "Section 1" with the lines and asks for the inline rename', async () => {
    const create = vi.fn().mockResolvedValue({ ...ok, cave_section_id: 9 });
    useProjectStore.setState({ createCaveSection: create, project: { ...project } as never });
    useUIStore.setState({ hiddenSections: [9], pendingSectionRename: null });
    expect(await createSectionWithRename([3], t)).toBe(9);
    expect(create).toHaveBeenCalledWith('Section 1', expect.stringMatching(/^#/), [3]);
    expect(useUIStore.getState().pendingSectionRename).toBe(9);
    // An id an undone create left on the hidden list starts shown.
    expect(useUIStore.getState().hiddenSections).toEqual([]);
  });
});

describe('Section header menu', () => {
  beforeEach(() => useUIStore.setState({ hiddenSections: [], selectionAnnouncement: '' }));

  it('lists Rename, Color..., Apply Color to Lines, Hide on Map, Zoom to Section and Delete Section', () => {
    const items = caveSectionHeaderMenuItems(7, project, t, () => {});
    expect(items.map((i) => (i.separator ? '---' : i.label))).toEqual([
      'Rename', 'Color...', 'Apply Color to Lines', 'Hide on Map', 'Zoom to Section', '---', 'Delete Section',
    ]);
    expect(items.find((i) => i.label === 'Delete Section')!.danger).toBe(true);
    expect(items.find((i) => i.label === 'Apply Color to Lines')!.hint).toBe('Give its 2 lines the section color');
  });

  it('an empty Section has nothing to colour or frame', () => {
    const items = caveSectionHeaderMenuItems(8, project, t, () => {});
    expect(items.find((i) => i.label === 'Apply Color to Lines')!.disabled).toBe(true);
    expect(items.find((i) => i.label === 'Zoom to Section')!.disabled).toBe(true);
  });

  it('Rename asks the panel for the inline rename; Zoom frames the lines', () => {
    useUIStore.setState({ pendingSectionRename: null, requestZoomToLine: null });
    const close = vi.fn();
    const items = caveSectionHeaderMenuItems(7, project, t, close);
    items.find((i) => i.label === 'Rename')!.action();
    expect(useUIStore.getState().pendingSectionRename).toBe(7);
    items.find((i) => i.label === 'Zoom to Section')!.action();
    expect(useUIStore.getState().requestZoomToLine).toEqual([1, 2]);
    expect(close).toHaveBeenCalledTimes(2);
  });

  it('Hide on Map hides the Section and announces it; a hidden one offers Show on Map', () => {
    const close = vi.fn();
    caveSectionHeaderMenuItems(7, project, t, close).find((i) => i.label === 'Hide on Map')!.action();
    expect(useUIStore.getState().hiddenSections).toEqual([7]);
    expect(useUIStore.getState().selectionAnnouncement).toBe('Upper level is hidden on the map');
    expect(close).toHaveBeenCalledTimes(1);

    const items = caveSectionHeaderMenuItems(7, project, t, close);
    expect(items.some((i) => i.label === 'Hide on Map')).toBe(false);
    items.find((i) => i.label === 'Show on Map')!.action();
    expect(useUIStore.getState().hiddenSections).toEqual([]);
    expect(useUIStore.getState().selectionAnnouncement).toBe('Upper level is shown on the map');
  });

  it('a hidden Section has nothing drawn to frame: Zoom to Section is off and says why', () => {
    useUIStore.setState({ hiddenSections: [7] });
    const zoom = caveSectionHeaderMenuItems(7, project, t, () => {}).find((i) => i.label === 'Zoom to Section')!;
    expect(zoom.disabled).toBe(true);
    expect(zoom.hint).toBe('Show the section on the map first');
  });

  it('is empty for a Section that no longer exists', () => {
    expect(caveSectionHeaderMenuItems(99, project, t, () => {})).toEqual([]);
  });
});
