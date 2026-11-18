/**
 * The Sections panel's view state (#327), kept in the UI store as one slice so
 * `uiStore.ts` only spreads it in. Nothing here is an edit: hiding a Section is
 * a per-project display setting (`hiddenSections`, saved through
 * `save_display_setting`, never undone, never pushed to SpeleoDB), and the
 * collapse state is session-only.
 */
import { invokeIpc } from '../utils/ipc';
import type { LineId } from '../types/project';

/** The display_settings key (and the store field) of the hidden Sections. */
export const HIDDEN_SECTIONS_SETTING = 'hiddenSections';

/** Stands for the "Ungrouped" group in a collapsed-groups list: Section ids
 *  are unsigned, so it can never be a real one. */
export const UNGROUPED_GROUP_ID = -1;

export interface SectionsViewSlice {
  /** The Sections hidden on the map (ids). Restored per project by
   *  `applyDisplaySettings`; a load is authoritative. */
  hiddenSections: readonly number[];
  /** Hide or show one Section on the map, and save the setting. */
  setSectionHidden: (id: number, hidden: boolean) => void;
  /** Collapsed groups per project (keyed by the project's `created_at`, the
   *  identity the store already uses across a project switch). A group is a
   *  Section id, or {@link UNGROUPED_GROUP_ID}. Session-only. */
  collapsedSections: Readonly<Record<string, readonly number[]>>;
  setSectionCollapsed: (projectKey: string, id: number, collapsed: boolean) => void;
  /** A Section just created elsewhere (the context menu's "New Section...")
   *  whose header should open its inline rename once the panel shows it. */
  pendingSectionRename: number | null;
  requestSectionRename: (id: number | null) => void;
  /** Frame several lines at once (a Section's extent). Shares the single
   *  `requestZoomToLine` channel MapCanvas already serves. */
  triggerZoomToLines: (lineIds: readonly LineId[]) => void;
}

type SliceSet = (
  partial:
    | Partial<SectionsViewSlice & { requestZoomToLine: LineId | readonly LineId[] | null }>
    | ((state: SectionsViewSlice) => Partial<SectionsViewSlice>),
) => void;

/** Persist the hidden Sections as the JSON array the backend parses. A failed
 *  write only loses the setting for the next open; the map already changed. */
function saveHiddenSections(ids: readonly number[]): void {
  invokeIpc(
    'save_display_setting',
    { key: HIDDEN_SECTIONS_SETTING, value: JSON.stringify(ids) },
    { purpose: 'persist display setting', fields: { key: HIDDEN_SECTIONS_SETTING } },
  ).catch((e) => {
    console.warn('Failed to save hidden sections:', e);
  });
}

export function createSectionsViewSlice(set: SliceSet): SectionsViewSlice {
  return {
    hiddenSections: [],
    setSectionHidden: (id, hidden) => {
      let next: readonly number[] | null = null;
      set((state) => {
        const has = state.hiddenSections.includes(id);
        if (has === hidden) return {};
        next = hidden ? [...state.hiddenSections, id] : state.hiddenSections.filter((s) => s !== id);
        return { hiddenSections: next };
      });
      if (next !== null) saveHiddenSections(next);
    },
    collapsedSections: {},
    setSectionCollapsed: (projectKey, id, collapsed) =>
      set((state) => {
        const current = state.collapsedSections[projectKey] ?? [];
        if (current.includes(id) === collapsed) return {};
        const updated = collapsed ? [...current, id] : current.filter((s) => s !== id);
        return { collapsedSections: { ...state.collapsedSections, [projectKey]: updated } };
      }),
    pendingSectionRename: null,
    requestSectionRename: (pendingSectionRename) => set({ pendingSectionRename }),
    triggerZoomToLines: (lineIds) => {
      if (lineIds.length === 0) return;
      set({ requestZoomToLine: [...lineIds] });
    },
  };
}
