/**
 * The Sections panel's view state (#327), kept in the UI store as one slice so
 * `uiStore.ts` only spreads it in. Nothing here is an edit: hiding a Section is
 * a per-project display setting (`hiddenSections`, saved through
 * `save_display_setting`, never undone, never pushed to SpeleoDB), and the
 * collapse state is session-only.
 */
import { invokeIpc } from '../utils/ipc';
import { syncDirtyAfterLocalSave } from './localStateDirty';
import type { LineId } from '../types/project';

/** The display_settings key (and the store field) of the hidden Sections. */
export const HIDDEN_SECTIONS_SETTING = 'hiddenSections';

/** Stands for the "Ungrouped" group in a collapsed-groups list: Section ids
 *  are unsigned, so it can never be a real one. */
export const UNGROUPED_GROUP_ID = -1;

/** A New Section the panel is asking a name for. */
export interface PendingNewSection {
  id: number;
  lineIds: readonly LineId[];
}

export interface SectionsViewSlice {
  /** The Sections hidden on the map (ids). Restored per project by
   *  `applyDisplaySettings`; a load is authoritative. */
  hiddenSections: readonly number[];
  /** Hide or show one Section on the map, and save the setting. */
  setSectionHidden: (id: number, hidden: boolean) => void;
  /** Forget the hidden ids that name no Section of the project any more (a
   *  deleted Section, an undone creation, a file another build rewrote), and
   *  save the setting when that changed it: a Section created later can reuse
   *  the id and must not start hidden. Run on every load and every refresh of
   *  the project. `undefined` (no Sections list at all) changes nothing. */
  pruneHiddenSections: (sections: readonly { id: number }[] | undefined) => void;
  /** Collapsed groups per project (keyed by the project's `created_at`, the
   *  identity the store already uses across a project switch). A group is a
   *  Section id, or {@link UNGROUPED_GROUP_ID}. Session-only. */
  collapsedSections: Readonly<Record<string, readonly number[]>>;
  setSectionCollapsed: (projectKey: string, id: number, collapsed: boolean) => void;
  /** A Section whose header should open its inline rename once the panel
   *  shows it (the header menu's Rename). */
  pendingSectionRename: number | null;
  requestSectionRename: (id: number | null) => void;
  /** A New Section waiting for its name in the panel (the panel's New Section
   *  button, or "New Section..." in a line's menu). Nothing exists until the
   *  name is entered: then ONE `create_cave_section` makes it, holding
   *  `lineIds`; Escape drops it without a trace. `id` tells requests apart, so
   *  asking again starts the field afresh. */
  pendingNewSection: PendingNewSection | null;
  /** Ask the panel for a New Section row holding `lineIds`. */
  startNewSection: (lineIds: readonly LineId[]) => void;
  /** Drop the pending New Section row (created, cancelled, or no longer possible). */
  endNewSection: () => void;
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
  ).then(syncDirtyAfterLocalSave, (e) => {
    console.warn('Failed to save hidden sections:', e);
  });
}

/** `hidden` without the ids no Section of `sections` has, or `null` when
 *  nothing goes (or the Sections are unknown). */
export function prunedHiddenSections(
  hidden: readonly number[],
  sections: readonly { id: number }[] | undefined,
): readonly number[] | null {
  if (hidden.length === 0 || sections === undefined) return null;
  const existing = new Set(sections.map((section) => section.id));
  const kept = hidden.filter((id) => existing.has(id));
  return kept.length === hidden.length ? null : kept;
}

/** Numbers the New Section requests ({@link PendingNewSection.id}). */
let newSectionRequests = 0;

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
    pruneHiddenSections: (sections) => {
      let next: readonly number[] | null = null;
      set((state) => {
        next = prunedHiddenSections(state.hiddenSections, sections);
        return next ? { hiddenSections: next } : {};
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
    pendingNewSection: null,
    startNewSection: (lineIds) => {
      newSectionRequests += 1;
      set({ pendingNewSection: { id: newSectionRequests, lineIds: [...lineIds] } });
    },
    endNewSection: () => set({ pendingNewSection: null }),
    triggerZoomToLines: (lineIds) => {
      if (lineIds.length === 0) return;
      set({ requestZoomToLine: [...lineIds] });
    },
  };
}
