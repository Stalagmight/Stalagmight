/**
 * The Sections panel's two row kinds (#327): a line row (as it always was,
 * now a tree item) and a group header (a Section, or "Ungrouped").
 * Split out of `SectionsPanel.tsx`, which keeps the list, the keyboard model
 * and the actions.
 */
import { memo, useCallback, useEffect, useMemo, useRef, type KeyboardEvent, type MouseEvent, type MutableRefObject, type PointerEvent, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { type Line, type LineId } from '../../types/project';
import {
  isVirtualKind,
  shotKindFromFlags,
  type SurveyData,
  type LineMetadata,
} from '../../types/survey-data';
import { formatFixed, formatNumber } from '../../i18n/format';
import { panelDotOf } from '../../theme/pillColors';
import { useUIStore } from '../../store/uiStore';
import { IconButton } from '../common/Button';
import { ChevronDownIcon, ChevronRightIcon, ExternalLinkIcon, EyeIcon, EyeOffIcon } from '../common/icons';
import { useLongPress } from '../../hooks';
import { countsTowardLength } from '../../utils/shotKind';
import type { TreeRow } from './sectionsTree';
import { dropGroupAttr } from './sectionsTree';

/**
 * Compute total length + reference-shot count for the line row. The length
 * leaves out Reference shots, like the backend's `total_length` (#689).
 * Called once per *visible* (virtualized) line row by `LineRow`, and per line
 * of a group for its header's total.
 *
 * `meta` is resolved by the panel from a `Map<lineId, LineMetadata>`
 * built once per surveyData change, replacing the former per-row
 * `surveyData.lines.find(...)` that made a panel open O(n^2). Given the
 * metadata, the shot range is read straight from the `shotLength` / `shotFlags`
 * typed arrays - it never touches `line.shots`, so the lazy Shot
 * materialization stays cold (the Phase-A.5 memory optimization).
 *
 * Falls back to the legacy `line.shots` walk only on the unusual path where
 * `surveyData` isn't populated yet (e.g. legacy JSON refresh in-flight). That
 * branch stays cold in the common path because `meta` is present.
 */
export function calculateLineStats(
  line: Line,
  meta: LineMetadata | undefined,
  surveyData: SurveyData | null,
): {
  shotCount: number;
  length: number;
  referenceCount: number;
} {
  if (surveyData && meta) {
    let length = 0;
    let referenceCount = 0;
    const end = meta.shotOffset + meta.shotCount;
    for (let g = meta.shotOffset; g < end; g++) {
      const kind = shotKindFromFlags(surveyData.shotFlags[g]!);
      if (countsTowardLength(kind)) length += surveyData.shotLength[g]!;
      // Counts both virtual kinds: the panel shows a single "virtual shots"
      // tally, and #27's Jump/Reference split is surfaced in the table, not here.
      if (isVirtualKind(kind)) {
        referenceCount++;
      }
    }
    return { shotCount: meta.shotCount, length, referenceCount };
  }
  // Legacy fallback - only hit during the JSON refresh window where
  // `surveyData` may briefly be a stale legacy-converted copy. Same
  // behavior as before, just kept for safety.
  let length = 0;
  let referenceCount = 0;
  for (const shot of line.shots) {
    if (countsTowardLength(shot.kind)) length += shot.length;
    if (isVirtualKind(shot.kind)) referenceCount++;
  }
  return {
    shotCount: line.shots.length,
    length,
    referenceCount,
  };
}

/**
 * A line color as the `#rrggbb` a color input can hold. A value already in
 * that form is kept (lowercased, as the input reports it). Anything else is read
 * back from `drawn`, the element painted with it, whose computed background the
 * browser has already resolved to `rgb(...)`. An unpaintable color is black,
 * which is also what the input itself would have made of it.
 */
export function hexColorFor(color: string, drawn: Element | null): string {
  if (/^#[0-9a-f]{6}$/i.test(color)) return color.toLowerCase();
  const rgb = drawn ? /^rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(getComputedStyle(drawn).backgroundColor) : null;
  if (!rgb) return '#000000';
  return `#${rgb.slice(1, 4).map((c) => Number(c).toString(16).padStart(2, '0')).join('')}`;
}

/**
 * Wire a row's colour dot (an uncontrolled color input) so one pick is one
 * commit, and so one undo step. React's onChange is the DOM `input` event, so
 * it is not used. The native `change` is not enough on its own either: WebKit
 * (the Linux and macOS webviews) fires `change` for every color chosen while
 * its picker is open, such as each swatch clicked in the GTK dialog, and the
 * GTK Cancel button fires one more to put the old color back. The picker holds
 * the window's focus while it is open, so a `change` that arrives without
 * focus is held and only the color the picker closed on is sent when focus
 * comes back. Chromium (Windows) fires a single `change` after its picker has
 * closed, which commits at once. The effect also resyncs the dot when the
 * color changes elsewhere (undo, Edit Line).
 *
 * The input can only hold #rrggbb, and would turn any other color (a CSS name
 * or short hex typed in Edit Line) into #000000. So it is seeded with the
 * color the dot actually draws, and a pick is compared with that seed, not
 * with `color`: a GTK Cancel that puts the seed back is then never sent as a
 * pick of black. `onCommit` must be stable (a pick held while the picker is
 * open is dropped when the effect re-runs); it resolves false when the colour
 * was not applied, and the dot reverts.
 */
export function useColorInputCommit(
  inputRef: RefObject<HTMLInputElement>,
  id: number,
  color: string,
  onCommit: (id: number, color: string) => Promise<boolean>,
): void {
  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    const seed = hexColorFor(color, input.parentElement);
    input.value = seed;
    let pending = false;
    const commit = () => {
      if (!pending) return;
      pending = false;
      if (input.value.toLowerCase() === seed) return;
      void onCommit(id, input.value).then((applied) => {
        if (!applied) input.value = seed;
      });
    };
    const onChange = () => {
      pending = true;
      if (document.hasFocus()) commit();
    };
    input.addEventListener('change', onChange);
    window.addEventListener('focus', commit);
    return () => {
      input.removeEventListener('change', onChange);
      window.removeEventListener('focus', commit);
      // A pick still held when the row goes away is dropped, not sent: by then
      // the line, or the whole project, may no longer be the one it was for.
    };
  }, [inputRef, id, color, onCommit]);
}

/** The props both row kinds share: virtualization, roving focus, a11y. */
interface RowBaseProps {
  index: number;
  /** Tree position among the row's siblings (`aria-posinset` / `aria-setsize`). */
  posinset: number;
  setsize: number;
  isFocused: boolean;
  isDefaultFocus: boolean;
  /** Virtualizer offset (px) of this row from the top of the sizer. */
  virtualStart: number;
  /** Virtualizer measure callback; attached to the row so its real height feeds
   *  back into the scroll math. */
  measureElement: (node: Element | null) => void;
  /** Set by keyboard nav to a not-yet-mounted target index; the row focuses
   *  itself once its ref attaches. */
  pendingFocusIndexRef: MutableRefObject<number | null>;
  optionRefs: MutableRefObject<(HTMLLIElement | null)[]>;
  onKeyDown: (e: KeyboardEvent, index: number) => void;
  colorEditable: boolean;
  /** The dot was clicked while editing is blocked. */
  onColorBlocked: () => void;
  /** The dot's tooltip while editing is blocked: why it cannot be changed. */
  colorBlockedText: string;
  /** Stop a click on a control inside the row (focus button, color dot) from
   *  also selecting or zooming the row. */
  onControlMouseDown: (e: MouseEvent) => void;
  onControlDoubleClick: (e: MouseEvent) => void;
}

/** One ref serves three roles: the panel's roving-tabindex registry, the
 *  virtualizer's height measurement (needs `data-index` on the row), and
 *  focusing a keyboard target the moment it mounts. */
function useRowRef(
  index: number,
  optionRefs: MutableRefObject<(HTMLLIElement | null)[]>,
  measureElement: (node: Element | null) => void,
  pendingFocusIndexRef: MutableRefObject<number | null>,
) {
  return useCallback((el: HTMLLIElement | null) => {
    optionRefs.current[index] = el;
    measureElement(el);
    if (el && pendingFocusIndexRef.current === index) {
      pendingFocusIndexRef.current = null;
      el.focus();
    }
  }, [optionRefs, index, measureElement, pendingFocusIndexRef]);
}

/** The colour dot: a span drawing any CSS colour, with an invisible color
 *  input over it that only opens the system picker. Disabled, the input lets
 *  clicks through to the span, which says why instead of selecting the row,
 *  and `disabled:opacity-0` keeps it hidden against the global
 *  `input:disabled { opacity: 0.5 }`. A colour too close to the panel (a
 *  near-white line on a light theme, a near-black one on a dark theme) gets
 *  the pills' ring, drawn as a border so the hover ring stays free (RS-FX6,
 *  P3-17). */
function ColorDot({
  color,
  inputRef,
  editable,
  blockedText,
  onBlocked,
  onControlMouseDown,
  onControlDoubleClick,
  label,
  dotTestId,
  inputTestId,
}: {
  color: string;
  inputRef: RefObject<HTMLInputElement>;
  editable: boolean;
  blockedText: string;
  onBlocked: () => void;
  onControlMouseDown: (e: MouseEvent) => void;
  onControlDoubleClick: (e: MouseEvent) => void;
  label: string;
  dotTestId: string;
  inputTestId: string;
}) {
  const theme = useUIStore((s) => s.resolvedTheme);
  const { ring } = panelDotOf(color, theme);
  return (
    <span
      className={`relative w-3 h-3 rounded-full flex-shrink-0 ${
        editable ? 'hover:ring-2 hover:ring-text-secondary hover:ring-offset-1 hover:ring-offset-panel' : ''
      }`}
      style={{ backgroundColor: color, ...(ring ? { border: `1px solid ${ring}` } : {}) }}
      title={editable ? undefined : blockedText}
      onMouseDown={editable ? undefined : onControlMouseDown}
      onDoubleClick={editable ? undefined : onControlDoubleClick}
      onClick={editable ? undefined : (e) => {
        e.stopPropagation();
        onBlocked();
      }}
      data-testid={dotTestId}
    >
      {/* A 20px target centered on the 12px dot. */}
      <input
        ref={inputRef}
        type="color"
        className="absolute -top-1 -left-1 w-5 h-5 opacity-0 cursor-pointer disabled:opacity-0 disabled:pointer-events-none"
        disabled={!editable}
        onMouseDown={onControlMouseDown}
        onPointerDown={(e) => e.stopPropagation()}
        onDoubleClick={onControlDoubleClick}
        tabIndex={-1}
        title={editable ? label : undefined}
        aria-label={label}
        data-testid={inputTestId}
      />
    </span>
  );
}

interface LineRowProps extends RowBaseProps {
  line: Line;
  /** Tree depth: 1 in the flat list, 2 under a group header. */
  level: 1 | 2;
  /** The group the row is listed under, as a drop target (grouped trees only). */
  dropGroup: number | null;
  /** Its Section is hidden on the map: drawn dimmed, with a hint. */
  dimmed: boolean;
  /** Part of the group a drag would drop into: a light tint. */
  inDropTarget: boolean;
  /** Flat-array metadata for this line (id -> LineMetadata), resolved by
   *  the panel from its memoized map. `undefined` only during the legacy JSON
   *  refresh window; the stats then fall back to the `line.shots` walk. */
  meta: LineMetadata | undefined;
  surveyData: SurveyData | null;
  isSelected: boolean;
  isHovered: boolean;
  precision: number;
  onMouseDown: (line: Line, e?: MouseEvent) => void;
  /** Completes a plain click deferred by `onMouseDown` (a press on a row of a
   *  multi-line selection may start a drag of the whole selection). */
  onClick: (line: Line, e: MouseEvent) => void;
  /** Starts a possible drag of the line (or the selection it belongs to). */
  onDragPointerDown: (line: Line, e: PointerEvent) => void;
  onDoubleClick: (line: Line, e: MouseEvent) => void;
  onContextMenu: (e: MouseEvent, lineId: number) => void;
  onMouseEnter: (lineId: LineId) => void;
  onMouseLeave: () => void;
  onFocus: (lineId: LineId, index: number) => void;
  onEnterFocus: (line: Line, e: MouseEvent) => void;
  /** Resolves false when the color was not applied, so the dot can revert. */
  onColorChange: (lineId: LineId, color: string) => Promise<boolean>;
  onLongPress: (lineId: number, x: number, y: number) => void;
}

export const LineRow = memo(function LineRow({
  line,
  level,
  dropGroup,
  dimmed,
  inDropTarget,
  index,
  posinset,
  setsize,
  meta,
  surveyData,
  isSelected,
  isHovered,
  isFocused,
  isDefaultFocus,
  precision,
  virtualStart,
  measureElement,
  pendingFocusIndexRef,
  onMouseDown,
  onClick,
  onDragPointerDown,
  onDoubleClick,
  onContextMenu,
  onMouseEnter,
  onMouseLeave,
  onFocus,
  onKeyDown,
  onEnterFocus,
  onControlMouseDown,
  onControlDoubleClick,
  colorEditable,
  onColorChange,
  onColorBlocked,
  colorBlockedText,
  onLongPress,
  optionRefs,
}: LineRowProps) {
  const { t } = useTranslation();
  const stats = useMemo(
    () => calculateLineStats(line, meta, surveyData),
    [line, meta, surveyData],
  );

  const handleLongPress = useCallback(
    (pos: { x: number; y: number }) => {
      onLongPress(line.id, pos.x, pos.y);
    },
    [onLongPress, line.id],
  );
  const longPressHandlers = useLongPress(handleLongPress);

  const colorRef = useRef<HTMLInputElement>(null);
  useColorInputCommit(colorRef, line.id, line.color, onColorChange);
  const refCallback = useRowRef(index, optionRefs, measureElement, pendingFocusIndexRef);

  return (
    <li
      ref={refCallback}
      data-index={index}
      role="treeitem"
      aria-level={level}
      aria-posinset={posinset}
      aria-setsize={setsize}
      aria-selected={isSelected}
      tabIndex={isFocused ? 0 : (isDefaultFocus ? 0 : -1)}
      data-testid={`line-row-${line.id}`}
      data-drop-group={dropGroup === null ? undefined : dropGroupAttr(dropGroup)}
      data-hidden-on-map={dimmed ? 'true' : undefined}
      title={dimmed ? t('caveSections.hiddenLineHint') : t('sections.actions.zoom')}
      style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${virtualStart}px)` }}
      className={`${level === 2 ? 'pl-7 pr-3' : 'px-3'} py-2 border-l-2 border-transparent cursor-pointer group transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset ${
        isSelected
          ? 'bg-accent-20 ring-2 ring-accent ring-inset'
          : (isHovered ? 'bg-accent-10 border-accent' : (inDropTarget ? 'bg-accent-10' : 'hover:bg-accent-10'))
      }`}
      onMouseDown={(e) => onMouseDown(line, e)}
      onClick={(e) => onClick(line, e)}
      onDoubleClick={(e) => onDoubleClick(line, e)}
      onContextMenu={(e) => onContextMenu(e, line.id)}
      {...longPressHandlers}
      onPointerDown={(e) => {
        longPressHandlers.onPointerDown(e);
        onDragPointerDown(line, e);
      }}
      onMouseEnter={() => onMouseEnter(line.id)}
      onMouseLeave={onMouseLeave}
      onFocus={() => onFocus(line.id, index)}
      onKeyDown={(e) => onKeyDown(e, index)}
    >
      <div
        className={`flex items-center gap-2 ${dimmed ? 'opacity-50' : ''}`}
        data-testid={`line-item-${line.id}`}
      >
        <ColorDot
          color={line.color}
          inputRef={colorRef}
          editable={colorEditable}
          blockedText={colorBlockedText}
          onBlocked={onColorBlocked}
          onControlMouseDown={onControlMouseDown}
          onControlDoubleClick={onControlDoubleClick}
          label={t('sections.actions.color')}
          dotTestId={`line-color-dot-${line.id}`}
          inputTestId={`line-color-${line.id}`}
        />
        <span className="flex-1 truncate">
          {line.name ?? t('detailPanel.section.unnamed')}
        </span>
        <IconButton
          onClick={(e) => onEnterFocus(line, e)}
          onMouseDown={onControlMouseDown}
          onPointerDown={(e) => e.stopPropagation()}
          onDoubleClick={onControlDoubleClick}
          icon={<ExternalLinkIcon size={12} />}
          size="sm"
          variant="ghost"
          tooltip={t('sections.actions.focus')}
          aria-label={t('sections.actions.focus')}
          className="opacity-0 group-hover:opacity-100 text-text-secondary hover:text-accent"
          tabIndex={-1}
          data-testid={`line-focus-${line.id}`}
        />
      </div>
      <div className={`text-xs text-text-secondary mt-1 ml-5 ${dimmed ? 'opacity-50' : ''}`}>
        {t('sections.stats', {
          shots: formatNumber(stats.shotCount),
          length: `${formatFixed(stats.length, undefined, precision)}${t('common.units.meters')}`,
        })}
      </div>
    </li>
  );
});

type GroupRow = Extract<TreeRow, { kind: 'group' }>;

interface GroupHeaderRowProps extends RowBaseProps {
  row: GroupRow;
  /** Total surveyed length of the group's lines, metres. */
  length: number;
  precision: number;
  /** A drag is over this group and dropping would move something. */
  isDropTarget: boolean;
  /** The inline rename is open on this header. */
  renaming: boolean;
  /** Why the typed name cannot be used, shown under the field. */
  renameError: string | null;
  /** Returns true when the field closes (the name was taken, or the commit
   *  came from leaving the field and was dropped). */
  onRenameCommit: (value: string, viaBlur: boolean) => boolean;
  onRenameCancel: () => void;
  onRenameInput: () => void;
  onToggleCollapsed: (row: GroupRow) => void;
  onToggleHidden: (sectionId: number, hidden: boolean) => void;
  onClick: (row: GroupRow, e: MouseEvent) => void;
  onDoubleClick: (row: GroupRow, e: MouseEvent) => void;
  onContextMenu: (row: GroupRow, x: number, y: number) => void;
  onFocus: (index: number) => void;
  onColorChange: (sectionId: number, color: string) => Promise<boolean>;
}

/** A group header: a Section (colour dot, name, line count and length, eye,
 *  collapse chevron) or the "Ungrouped" lines (no dot, no eye). */
export const GroupHeaderRow = memo(function GroupHeaderRow({
  row,
  length,
  precision,
  isDropTarget,
  renaming,
  renameError,
  onRenameCommit,
  onRenameCancel,
  onRenameInput,
  onToggleCollapsed,
  onToggleHidden,
  onClick,
  onDoubleClick,
  onContextMenu,
  onFocus,
  onColorChange,
  index,
  posinset,
  setsize,
  isFocused,
  isDefaultFocus,
  virtualStart,
  measureElement,
  pendingFocusIndexRef,
  optionRefs,
  onKeyDown,
  colorEditable,
  onColorBlocked,
  colorBlockedText,
  onControlMouseDown,
  onControlDoubleClick,
}: GroupHeaderRowProps) {
  const { t } = useTranslation();
  const section = row.section;
  const sectionId = section?.id ?? -1;
  const colorRef = useRef<HTMLInputElement>(null);
  useColorInputCommit(colorRef, sectionId, section?.color ?? '#000000', onColorChange);
  const refCallback = useRowRef(index, optionRefs, measureElement, pendingFocusIndexRef);
  const handleLongPress = useCallback(
    (pos: { x: number; y: number }) => {
      onContextMenu(row, pos.x, pos.y);
    },
    [onContextMenu, row],
  );
  const longPressHandlers = useLongPress(handleLongPress, { enabled: section !== null });

  const name = section ? section.name : t('caveSections.ungrouped');
  const testId = section ? `section-header-${section.id}` : 'sections-ungrouped-header';
  const stats = t('caveSections.stats', {
    count: row.lineIds.length,
    lines: formatNumber(row.lineIds.length),
    length: `${formatFixed(length, undefined, precision)} ${t('common.units.meters')}`,
  });

  return (
    <li
      ref={refCallback}
      data-index={index}
      role="treeitem"
      aria-level={1}
      aria-posinset={posinset}
      aria-setsize={setsize}
      aria-expanded={!row.collapsed}
      aria-label={`${name}, ${stats}${row.hidden ? `, ${t('caveSections.hiddenState')}` : ''}`}
      tabIndex={isFocused ? 0 : (isDefaultFocus ? 0 : -1)}
      data-testid={testId}
      data-drop-group={dropGroupAttr(row.group)}
      data-drop-target={isDropTarget ? 'true' : undefined}
      data-hidden-on-map={row.hidden ? 'true' : undefined}
      title={t(section ? (row.hidden ? 'caveSections.headerHintHidden' : 'caveSections.headerHint') : 'caveSections.ungroupedHint')}
      style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${virtualStart}px)` }}
      className={`pl-1 pr-2 py-1.5 ${index > 0 ? 'border-t border-border' : ''} cursor-pointer group outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset ${
        isDropTarget ? 'bg-accent-20 ring-2 ring-accent ring-inset' : (isFocused ? 'bg-background-50' : 'hover:bg-accent-10')
      }`}
      onClick={(e) => onClick(row, e)}
      onDoubleClick={(e) => onDoubleClick(row, e)}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        if (section) onContextMenu(row, e.clientX, e.clientY);
      }}
      {...longPressHandlers}
      onFocus={(e) => {
        if (e.target === e.currentTarget) onFocus(index);
      }}
      onKeyDown={(e) => onKeyDown(e, index)}
    >
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            onToggleCollapsed(row);
          }}
          onDoubleClick={onControlDoubleClick}
          className="p-0.5 rounded text-text-secondary hover:text-text-primary hover:bg-background-50"
          aria-label={t(row.collapsed ? 'caveSections.actions.expand' : 'caveSections.actions.collapse', { name })}
          title={t(row.collapsed ? 'caveSections.actions.expand' : 'caveSections.actions.collapse', { name })}
          data-testid={section ? `section-collapse-${section.id}` : 'sections-ungrouped-collapse'}
        >
          {row.collapsed ? <ChevronRightIcon size={14} /> : <ChevronDownIcon size={14} />}
        </button>
        {section && (
          <ColorDot
            color={section.color}
            inputRef={colorRef}
            editable={colorEditable}
            blockedText={colorBlockedText}
            onBlocked={onColorBlocked}
            onControlMouseDown={onControlMouseDown}
            onControlDoubleClick={onControlDoubleClick}
            label={t('caveSections.actions.color')}
            dotTestId={`section-color-dot-${section.id}`}
            inputTestId={`section-color-${section.id}`}
          />
        )}
        <div className={`flex-1 min-w-0 ${row.hidden ? 'opacity-50' : ''}`}>
          {renaming && section ? (
            <RenameField
              sectionId={section.id}
              initial={section.name}
              error={renameError}
              onCommit={onRenameCommit}
              onCancel={onRenameCancel}
              onInput={onRenameInput}
            />
          ) : (
            <div className="flex items-baseline gap-2 min-w-0">
              <span
                className={`truncate text-sm font-medium ${section ? '' : 'text-text-secondary'}`}
                data-testid={section ? `section-name-${section.id}` : 'sections-ungrouped-name'}
              >
                {name}
              </span>
              <span
                className="flex-shrink-0 text-xs text-text-secondary"
                data-testid={section ? `section-stats-${section.id}` : 'sections-ungrouped-stats'}
              >
                {stats}
              </span>
            </div>
          )}
        </div>
        {section && (
          <IconButton
            onClick={(e) => {
              e.stopPropagation();
              onToggleHidden(section.id, !row.hidden);
            }}
            onDoubleClick={onControlDoubleClick}
            icon={row.hidden ? <EyeOffIcon size={14} /> : <EyeIcon size={14} />}
            size="sm"
            variant="ghost"
            tooltip={t(row.hidden ? 'caveSections.actions.show' : 'caveSections.actions.hide')}
            // A toggle: one name, its state in aria-pressed (pressed = hidden).
            aria-label={t('caveSections.actions.hideToggle', { name: section.name })}
            aria-pressed={row.hidden}
            className={row.hidden ? 'text-text-secondary' : 'text-text-secondary opacity-60 group-hover:opacity-100 hover:text-accent'}
            tabIndex={-1}
            data-testid={`section-toggle-visibility-${section.id}`}
          />
        )}
      </div>
    </li>
  );
});

/** The inline rename field of a Section header: Enter or leaving it commits,
 *  Escape cancels. Opens with the whole name selected. */
function RenameField({
  sectionId,
  initial,
  error,
  onCommit,
  onCancel,
  onInput,
}: {
  sectionId: number;
  initial: string;
  error: string | null;
  onCommit: (value: string, viaBlur: boolean) => boolean;
  onCancel: () => void;
  onInput: () => void;
}) {
  const { t } = useTranslation();
  return (
    <SectionNameField
      initial={initial}
      error={error}
      onCommit={onCommit}
      onCancel={onCancel}
      onInput={onInput}
      label={t('caveSections.actions.renameLabel')}
      testId={`section-rename-${sectionId}`}
      inputTestId={`section-rename-input-${sectionId}`}
      errorTestId={`section-rename-error-${sectionId}`}
    />
  );
}

/** How many frames a name field keeps trying to take the focus: it can mount
 *  inside a closed side panel that a programmatic reveal opens a frame later
 *  ("New Section..." from a line's menu on the map). */
const FOCUS_RETRY_FRAMES = 10;

/** A Section name field: Enter commits (and stays open, saying why, when the
 *  name cannot be used), leaving it commits, Escape cancels. Opens focused,
 *  with the whole name selected. */
function SectionNameField({
  initial,
  error,
  onCommit,
  onCancel,
  onInput,
  label,
  testId,
  inputTestId,
  errorTestId,
}: {
  initial: string;
  error: string | null;
  onCommit: (value: string, viaBlur: boolean) => boolean;
  onCancel: () => void;
  onInput: () => void;
  label: string;
  testId: string;
  inputTestId: string;
  errorTestId: string;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  // Set once the field has settled (committed, or cancelled with Escape): the
  // blur that follows its removal must not commit again.
  const settledRef = useRef(false);
  useEffect(() => {
    let frames = 0;
    let frame: number | null = null;
    const take = () => {
      frame = null;
      const input = inputRef.current;
      if (!input || settledRef.current) return;
      input.focus();
      input.select();
      if (document.activeElement !== input && ++frames < FOCUS_RETRY_FRAMES) frame = requestAnimationFrame(take);
    };
    take();
    return () => {
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, []);
  const errorId = `${errorTestId}-message`;
  return (
    <div onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()} data-testid={testId}>
      <input
        ref={inputRef}
        type="text"
        defaultValue={initial}
        maxLength={400}
        aria-label={label}
        aria-invalid={error !== null}
        aria-describedby={error ? errorId : undefined}
        className={`w-full select-text text-sm px-1 py-0.5 bg-panel border rounded focus:outline-none focus:ring-1 focus:ring-accent ${
          error ? 'border-error' : 'border-border'
        }`}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') {
            e.preventDefault();
            if (onCommit(e.currentTarget.value, false)) settledRef.current = true;
          } else if (e.key === 'Escape') {
            e.preventDefault();
            settledRef.current = true;
            onCancel();
          }
        }}
        onInput={onInput}
        onBlur={(e) => {
          if (settledRef.current) return;
          settledRef.current = onCommit(e.currentTarget.value, true);
        }}
        onMouseDown={(e) => e.stopPropagation()}
        onPointerDown={(e) => e.stopPropagation()}
        data-testid={inputTestId}
      />
      {error && (
        <div id={errorId} role="alert" className="mt-0.5 text-xs text-error" data-testid={errorTestId}>
          {error}
        </div>
      )}
    </div>
  );
}

/**
 * The New Section row, above the list: the colour the Section will get and a
 * name field prefilled with the next free `Section N`. Nothing exists until
 * the name is entered; Escape drops the row (see `pendingNewSection`).
 */
export function NewSectionRow({
  initial,
  color,
  lineCount,
  error,
  onCommit,
  onCancel,
  onInput,
}: {
  initial: string;
  color: string;
  /** How many lines the Section will hold ("New Section..." from a line). */
  lineCount: number;
  error: string | null;
  onCommit: (value: string, viaBlur: boolean) => boolean;
  onCancel: () => void;
  onInput: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      className="flex items-start gap-1.5 pl-2 pr-2 py-1.5 border-b border-border bg-accent-10"
      // A press on the row around the field must not take the focus out of it
      // (leaving the field creates the Section).
      onMouseDown={(e) => {
        if (e.target !== e.currentTarget && (e.target as HTMLElement).closest('input')) return;
        e.preventDefault();
      }}
      data-testid="sections-new-section-row"
    >
      <span
        className="mt-1.5 w-3 h-3 rounded-full flex-shrink-0"
        style={{ backgroundColor: color }}
        aria-hidden="true"
        data-testid="sections-new-section-color"
      />
      <div className="flex-1 min-w-0">
        <SectionNameField
          initial={initial}
          error={error}
          onCommit={onCommit}
          onCancel={onCancel}
          onInput={onInput}
          label={t('caveSections.create.label')}
          testId="sections-new-section-field"
          inputTestId="sections-new-section-input"
          errorTestId="sections-new-section-error"
        />
        <div className="mt-0.5 text-xs text-text-secondary" data-testid="sections-new-section-hint">
          {lineCount > 0
            ? t('caveSections.create.hintWithLines', { count: lineCount, lines: formatNumber(lineCount) })
            : t('caveSections.create.hint')}
        </div>
      </div>
    </div>
  );
}
