/**
 * Context-menu rows for Sections (#327): the line menu's "Move to Section"
 * flyout, and the menu of a Section header in the Sections panel. Built here
 * and spliced into `ContextMenu.tsx`, whose read-only gate disables them like
 * every other edit row (only the view rows - Hide / Show on Map and Zoom to
 * Section, `CAVE_SECTION_VIEW_LABEL_KEYS` - stay live).
 *
 * Every action awaits its command before closing the menu, as the menu's other
 * actions do: a refusal is then toasted (executeCommand toasts while the
 * context menu is the open dialog).
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { Line, LineId } from '../../types/project';
import type { Section } from '../../types/ipc/Section';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { trackDialogComplete } from '../../utils/telemetry';
import { ToolGlyph } from './toolIcons';
import { EyeIcon, EyeOffIcon, LayersIcon, PencilIcon, TrashIcon } from './icons';

/** The fields of a ContextMenu row these builders fill. */
export interface SectionMenuItem {
  label: string;
  action: () => void;
  submenu?: SectionMenuItem[];
  icon?: ReactNode;
  disabled?: boolean;
  hint?: string | undefined;
  danger?: boolean;
  separator?: boolean;
}

interface ProjectSlice {
  lines: Line[];
  cave_sections?: Section[];
}

/** Run `work`, then close the menu (see the module comment). */
function closeAfter(closeDialog: () => void, work: () => Promise<unknown> | void) {
  return async () => {
    await work();
    trackDialogComplete('context-menu');
    closeDialog();
  };
}

/** The header menu's rows that change only the view, not the project: the
 *  read-only views keep them (ContextMenu's read-only gate). */
export const CAVE_SECTION_VIEW_LABEL_KEYS = [
  'caveSections.menu.hide',
  'caveSections.menu.show',
  'caveSections.menu.zoom',
] as const;

/**
 * Hide a Section on the map, or show it again, and say so through the
 * selection live region - the eye button, the header menu and the `H` key of
 * a focused header all go through here. A view setting, never an edit, so
 * the read-only views allow it.
 */
export function setSectionHiddenAnnounced(section: Pick<Section, 'id' | 'name'>, hidden: boolean, t: TFunction) {
  const ui = useUIStore.getState();
  ui.setSectionHidden(section.id, hidden);
  ui.announceSelection(
    t(hidden ? 'caveSections.announce.hidden' : 'caveSections.announce.shown', { name: section.name }),
  );
}

/**
 * Ask the Sections panel for a New Section row holding `lineIds`: an inline
 * field prefilled with the next free `Section N`. Nothing is created until a
 * name is entered there (one undo step); Escape drops it. Shared by the line
 * menu's "New Section..." and the panel's "New Section" button; shows the
 * Sections tab, which the line menu can be opened without (from the map).
 */
export function startNewSection(lineIds: readonly LineId[]): void {
  const ui = useUIStore.getState();
  if (ui.leftWorkspace !== 'sections') ui.selectLeftWorkspace('sections');
  ui.startNewSection(lineIds);
}

/**
 * Create the Section the New Section row named, in `color`, holding
 * `lineIds`: one `create_cave_section`. Resolves to its id, or null when it was
 * refused (executeCommand has said why).
 */
export async function createNamedSection(
  name: string,
  color: string,
  lineIds: readonly LineId[],
): Promise<number | null> {
  const result = await useProjectStore.getState().createCaveSection(name, color, lineIds);
  const id = result.success ? result.cave_section_id : undefined;
  if (id === undefined || id === null) return null;
  // An id coming back from an undone create may still be on the hidden list.
  if (useUIStore.getState().hiddenSections.includes(id)) useUIStore.getState().setSectionHidden(id, false);
  return id;
}

/**
 * The line menu's "Move to Section" row: a flyout of every Section, "No
 * Section" and "New Section...". It moves the right-clicked line, or the whole
 * line selection when the line is part of a selection of several lines (the
 * label then says how many). A destination every one of those lines is
 * already in is disabled.
 */
export function caveSectionLineMenuItems(
  lineId: LineId,
  selectedLineIds: readonly LineId[],
  project: ProjectSlice | null | undefined,
  t: TFunction,
  closeDialog: () => void,
): SectionMenuItem[] {
  if (!project) return [];
  const lineIds = selectedLineIds.length > 1 && selectedLineIds.includes(lineId) ? [...selectedLineIds] : [lineId];
  const sectionOf = new Map(project.lines.map((l) => [l.id, l.cave_section_id ?? null]));
  const allIn = (section: number | null) => lineIds.every((id) => (sectionOf.get(id) ?? null) === section);
  const move = (section: number | null) =>
    closeAfter(closeDialog, () => useProjectStore.getState().moveLinesToCaveSection(lineIds, section));

  const sections = project.cave_sections ?? [];
  const submenu: SectionMenuItem[] = [
    ...sections.map((s) => ({
      label: s.name,
      action: move(s.id),
      disabled: allIn(s.id),
      hint: allIn(s.id) ? t('caveSections.menu.alreadyThere', { count: lineIds.length }) : undefined,
    })),
    {
      label: t('caveSections.menu.noSection'),
      action: move(null),
      disabled: allIn(null),
      hint: allIn(null) ? t('caveSections.menu.alreadyUngrouped', { count: lineIds.length }) : undefined,
    },
    {
      label: t('caveSections.menu.newSectionWithLines'),
      action: closeAfter(closeDialog, () => startNewSection(lineIds)),
    },
  ];
  return [
    {
      label: lineIds.length > 1
        ? t('caveSections.menu.moveLinesTo', { count: lineIds.length })
        : t('caveSections.menu.moveTo'),
      action: () => {},
      icon: <LayersIcon size={16} />,
      submenu,
    },
  ];
}

/** The menu of a Section header: Rename, Colour..., Apply Colour to Lines,
 *  Hide / Show on Map, Zoom to Section and Delete Section (its lines stay,
 *  ungrouped). Zoom to Section is off while the Section is hidden: there is
 *  nothing drawn to frame. */
export function caveSectionHeaderMenuItems(
  sectionId: number,
  project: ProjectSlice | null | undefined,
  t: TFunction,
  closeDialog: () => void,
): SectionMenuItem[] {
  const section = project?.cave_sections?.find((s) => s.id === sectionId);
  if (!project || !section) return [];
  const lineIds = project.lines.filter((l) => l.cave_section_id === sectionId).map((l) => l.id);
  const empty = lineIds.length === 0;
  const hidden = useUIStore.getState().hiddenSections.includes(sectionId);
  const store = () => useProjectStore.getState();
  const dot = <span className="inline-block w-3 h-3 rounded-full" style={{ backgroundColor: section.color }} />;
  return [
    {
      label: t('caveSections.menu.rename'),
      icon: <PencilIcon size={16} />,
      action: () => {
        useUIStore.getState().requestSectionRename(sectionId);
        trackDialogComplete('context-menu');
        closeDialog();
      },
    },
    {
      label: t('caveSections.menu.color'),
      icon: dot,
      action: () => {
        trackDialogComplete('context-menu');
        closeDialog();
        // Open the header's own colour input, so a pick commits through the
        // same one-pick-one-step path as a click on the dot. Synchronous, inside
        // the menu click, so the picker still counts as user-initiated.
        document.querySelector<HTMLInputElement>(`[data-testid="section-color-${sectionId}"]`)?.click();
      },
    },
    {
      label: t('caveSections.menu.applyColor'),
      icon: dot,
      action: closeAfter(closeDialog, () => store().applyCaveSectionColor(sectionId)),
      disabled: empty,
      hint: empty ? t('caveSections.menu.noLines') : t('caveSections.menu.applyColorHint', { count: lineIds.length }),
    },
    {
      label: t(hidden ? 'caveSections.menu.show' : 'caveSections.menu.hide'),
      icon: hidden ? <EyeIcon size={16} /> : <EyeOffIcon size={16} />,
      action: () => {
        setSectionHiddenAnnounced(section, !hidden, t);
        trackDialogComplete('context-menu');
        closeDialog();
      },
    },
    {
      label: t('caveSections.menu.zoom'),
      icon: <ToolGlyph tool="zoom" size={16} />,
      action: () => {
        useUIStore.getState().triggerZoomToLines(lineIds);
        trackDialogComplete('context-menu');
        closeDialog();
      },
      disabled: empty || hidden,
      hint: hidden && !empty ? t('caveSections.menu.zoomHidden') : undefined,
    },
    { label: '', action: () => {}, separator: true },
    {
      label: t('caveSections.menu.delete'),
      icon: <TrashIcon size={16} />,
      action: closeAfter(closeDialog, () => store().deleteCaveSection(sectionId)),
      danger: true,
      hint: t('caveSections.menu.deleteHint'),
    },
  ];
}
