import { useState, useRef, useEffect, useCallback, useId, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { getErrorMessage } from '../../utils/ipc';
import { setSource } from '../../utils/telemetry';
import { formatDate, formatRelativeTime, formatNumber } from '../../i18n/format';
import type { CommandResult, UndoHistoryItem, PreviewData } from '../../types/commands';
import {
  CircleIcon,
  DashedLineIcon,
  DotIcon,
  LayersIcon,
  LinkIcon,
  LinkOffIcon,
  MoveAllIcon,
  MoveHorizontalIcon,
  PencilIcon,
  PlusIcon,
  TargetIcon,
  TrashIcon,
} from '../common/icons';

/**
 * True when `relatedTarget` is a DOM node contained by `container`. WebKit
 * throws from `Node.contains()` when the argument is not a Node - a mouse/focus
 * leave into browser chrome or the WebGL canvas overlay hands us `window` or
 * `null` as the related target - so this guards with `instanceof Node` before
 * calling `contains` (matching the SectionsPanel / Dock leave handlers).
 */
export function relatedTargetWithin(
  container: HTMLElement | null,
  relatedTarget: EventTarget | null | undefined
): boolean {
  return relatedTarget instanceof Node && !!container?.contains(relatedTarget);
}

interface UndoHistoryDropdownProps {
  /** 'undo' for undo button, 'redo' for redo button */
  mode: 'undo' | 'redo';
  /** Whether the button should be enabled */
  disabled: boolean;
  /** Trigger button content */
  children: React.ReactNode;
  /** Callback when single undo/redo is triggered (only used when dropdown is closed;
   *  when open, the dropdown calls store undo/redo directly to enable stay-open + refresh) */
  onSingleAction: () => void;
  /** Test ID prefix for data-testid attributes */
  testId: string;
  /** Accessible name for the trigger. Needed when `children` is an icon-only
   *  (condensed) label that carries no text of its own. */
  ariaLabel?: string;
  /** Tighten the trigger's padding at the toolbar's final condensation stage. */
  compact?: boolean;
  /**
   * [#303] Let HOVER (and ArrowDown) open the panel even while `disabled` - the case being
   * an empty undo stack on a linked project, where the panel's only content is the Unwind
   * entry that continues the walk past the commit boundary.
   *
   * The BUTTON stays inert either way: Undo / Ctrl+Z on an empty stack must keep doing
   * nothing, and the Unwind is deliberately reachable only through its own entry plus a
   * confirmation. This opens the panel; it never performs anything. Together with a held
   * write lock it also drops the DOM `disabled` attribute, so the trigger keeps a tab stop
   * - without one the entry had no keyboard route at all - and `handleButtonClick` holds
   * the "performs nothing" line.
   *
   * The toolbar passes it for the undo trigger unless something else (placement mode)
   * is the reason for `disabled` - that reason must still suppress the panel.
   */
  hoverableWhenEmpty?: boolean;
}

/** Map command types to icon components (matching toolbar icon style) */
function getCommandIcon(commandType: string): ReactNode {
  switch (commandType) {
    case 'create_section':
      return <PlusIcon size={14} />;
    case 'update_shot':
    case 'update_section_metadata':
    case 'edit_shot_credit':
      return <PencilIcon size={14} />;
    case 'delete_section':
    case 'delete_shot':
      return <TrashIcon size={14} />;
    case 'merge_stations':
      return <LinkIcon size={14} />;
    case 'batch':
      return <LayersIcon size={14} />;
    case 'create_shot':
      return <PlusIcon size={14} />;
    case 'set_zero_datum':
      return <TargetIcon size={14} />;
    case 'remove_zero_datum':
      return <CircleIcon size={14} />;
    case 'move_section':
    case 'move_segment':
    case 'move_station':
      return commandType === 'move_segment' ? <MoveHorizontalIcon size={14} /> : <MoveAllIcon size={14} />;
    case 'unmerge_stations':
      return <LinkOffIcon size={14} />;
    // `set_shot_reference` is the pre-#27 tag. Persisted undo history is not
    // rewritten on upgrade, so both spellings have to map to the icon.
    case 'set_shot_kind':
    case 'set_shot_reference':
      return <DashedLineIcon size={14} />;
    default:
      return <DotIcon size={14} />;
  }
}

/** Format timestamp for display using Intl for consistent cross-runtime support */
function formatTimestamp(timestamp: string | null, t: (key: string) => string): string {
  if (!timestamp) {
    return t('common.states.unknown');
  }

  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) {
    return t('common.states.unknown');
  }

  const now = new Date();
  const diffMs = Math.abs(now.getTime() - date.getTime());

  if (diffMs < 86400000) {
    // Less than 1 day - show relative time
    return formatRelativeTime(date);
  }

  // 1 day or older - show date with time
  return formatDate(date, undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
}

/** Truncate description to ~40 chars with ellipsis */
function truncateDescription(description: string, maxLength = 40): string {
  if (description.length <= maxLength) {
    return description;
  }
  return description.slice(0, maxLength - 1) + '\u2026';
}

interface DetailPreviewPopupProps {
  previewData: PreviewData | null;
  anchorRect: DOMRect;
  dropdownRect: DOMRect;
  commandType: string;
}

/** Detail preview popup showing field changes */
function DetailPreviewPopup({ previewData, anchorRect, dropdownRect, commandType }: DetailPreviewPopupProps) {
  const { t } = useTranslation();
  const popupRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<'right' | 'left'>('right');

  // Calculate position on mount and when anchor changes
  useEffect(() => {
    const popupWidth = 220; // Approximate width of preview popup
    const padding = 8; // Gap between dropdown and preview
    const viewportWidth = window.innerWidth;

    // Calculate position to the right of the dropdown
    const rightEdge = dropdownRect.right + padding + popupWidth;

    // Flip to left if would overflow right edge
    if (rightEdge > viewportWidth) {
      setPosition('left');
    } else {
      setPosition('right');
    }
  }, [dropdownRect]);

  // If no preview data, show "No details" message
  const hasChanges = previewData && previewData.changes.length > 0;

  // Calculate actual position
  const style: React.CSSProperties = {
    position: 'fixed',
    top: anchorRect.top,
    zIndex: 60, // Above dropdown (z-50)
    ...(position === 'right'
      ? { left: dropdownRect.right + 8 }
      : { right: window.innerWidth - dropdownRect.left + 8 }),
  };

  return createPortal(
    <div
      ref={popupRef}
      className="detail-preview-popup bg-panel border border-border rounded-lg shadow-lg p-3 w-[220px]"
      style={style}
      data-testid="detail-preview-popup"
    >
      {!hasChanges ? (
        <div className="text-sm text-text-secondary">{t('undo.preview.noDetails')}</div>
      ) : (
        <>
          <div className="space-y-1">
            {previewData.changes.map((change, idx) => {
              // Batch sub-commands: only for actual batch commands
              const looksLikeBatchSubcommand = commandType === 'batch';

              if (looksLikeBatchSubcommand) {
                // Batch sub-command: description as header, details on next line
                return (
                  <div key={idx} className="text-xs" title={change.newValue ? `${change.field}: ${change.newValue}` : change.field}>
                    <div className="text-text-secondary truncate">{change.field}</div>
                    {change.newValue && (
                      <div className="text-text-primary truncate pl-2">{change.newValue}</div>
                    )}
                  </div>
                );
              }

              // Single-line "Field: value" format
              return (
                <div key={idx} className="text-xs truncate" title={
                  change.oldValue && change.newValue
                    ? `${change.field}: ${change.oldValue} → ${change.newValue}`
                    : change.newValue
                      ? `${change.field}: ${change.newValue}`
                      : change.oldValue
                        ? `${change.field}: ${change.oldValue}`
                        : change.field
                }>
                  <span className="text-text-secondary">{change.field}: </span>
                  {change.oldValue && change.newValue ? (
                    // Both values: "Field: old → new"
                    <>
                      <span className="text-text-primary">{change.oldValue}</span>
                      <span className="text-text-secondary"> &rarr; </span>
                      <span className="text-text-primary">{change.newValue}</span>
                    </>
                  ) : change.newValue ? (
                    // Info-only: "Field: value"
                    <span className="text-text-primary">{change.newValue}</span>
                  ) : change.oldValue ? (
                    // Removed: "Field: ̶v̶a̶l̶u̶e̶"
                    <span className="text-text-secondary line-through">{change.oldValue}</span>
                  ) : (
                    <span className="text-text-secondary">{t('undo.preview.empty')}</span>
                  )}
                </div>
              );
            })}
          </div>
          {previewData.additionalChanges > 0 && (
            <div className="mt-1 pt-1 border-t border-border text-xs text-text-secondary">
              {t('undo.preview.more', {
                count: previewData.additionalChanges,
                countFormatted: formatNumber(previewData.additionalChanges),
              })}
            </div>
          )}
        </>
      )}
    </div>,
    document.body
  );
}

export default function UndoHistoryDropdown({
  mode,
  disabled,
  children,
  onSingleAction,
  testId,
  ariaLabel,
  compact = false,
}: UndoHistoryDropdownProps) {
  const { t } = useTranslation();
  const modeLabel = mode === 'undo' ? t('undo.actions.undo') : t('undo.actions.redo');
  const [isOpen, setIsOpen] = useState(false);
  const [items, setItems] = useState<UndoHistoryItem[]>([]);
  // State for preview popup
  const [hoveredItem, setHoveredItem] = useState<UndoHistoryItem | null>(null);
  const [hoveredItemRect, setHoveredItemRect] = useState<DOMRect | null>(null);
  const [dropdownRect, setDropdownRect] = useState<DOMRect | null>(null);
  // State for keyboard navigation - index into items array (-1 = none)
  const [activeIndex, setActiveIndex] = useState(-1);
  // Separate hover highlight index (does not trigger focus stealing)
  const [hoverIndex, setHoverIndex] = useState(-1);
  // Action queue state for sequential execution
  const [isActionInProgress, setIsActionInProgress] = useState(false);
  const isActionInProgressRef = useRef(false);
  // Queue entries include action and context for better error messages
  interface QueuedAction {
    action: () => Promise<CommandResult>;
    context: {
      description: string;
      commandType: string;
      timestamp: string | null;
    };
  }
  const actionQueueRef = useRef<QueuedAction[]>([]);
  // aria-live announcement for remaining action count
  const [announcement, setAnnouncement] = useState('');
  // Flag to track if dropdown was opened via keyboard (for focus management)
  const keyboardNavActiveRef = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const hoverTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const leaveTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const keyboardOpenTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Token to detect stale hover callbacks (incremented on any close)
  const hoverTokenRef = useRef(0);
  // Track disabled in a ref to avoid closure issues
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;
  // Track isOpen in a ref for use in async callbacks / event listeners
  const isOpenRef = useRef(isOpen);
  isOpenRef.current = isOpen;
  // Unique ID for ARIA attributes
  const menuId = useId();

  const { getUndoStack, getRedoStack, undoTo, redoTo, undo, redo, canUndo, canRedo } = useProjectStore();
  const { openDialog } = useUIStore();

  // Clear preview/hover state without closing the dropdown (reusable helper)
  const clearPreviewState = useCallback(() => {
    setHoveredItem(null);
    setHoveredItemRect(null);
    setDropdownRect(null);
  }, []);

  // Centralized "hard close" helper that cancels all timers and closes
  const hardClose = useCallback(() => {
    // Increment token to invalidate any pending hover callback
    hoverTokenRef.current += 1;

    // Clear any pending hover timeout
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }

    // Clear any pending leave timeout
    if (leaveTimeoutRef.current) {
      clearTimeout(leaveTimeoutRef.current);
      leaveTimeoutRef.current = null;
    }

    // Clear any pending keyboard open timeout
    if (keyboardOpenTimeoutRef.current) {
      clearTimeout(keyboardOpenTimeoutRef.current);
      keyboardOpenTimeoutRef.current = null;
    }

    // Clear hovered item state
    clearPreviewState();

    // Cancel any pending queued actions to prevent stale actions from executing
    // against a different undo/redo stack after reopen
    actionQueueRef.current = [];

    // Reset keyboard navigation state
    setActiveIndex(-1);
    setHoverIndex(-1);
    keyboardNavActiveRef.current = false;

    setIsOpen(false);
  }, [clearPreviewState]);

  // Fetch stack data and return it (single fetch, no double call)
  const fetchStack = useCallback(async (): Promise<UndoHistoryItem[]> => {
    try {
      const stack = mode === 'undo' ? await getUndoStack() : await getRedoStack();
      setItems(stack);
      return stack;
    } catch (error) {
      console.error(`Failed to fetch ${mode} stack:`, error);
      setItems([]);
      return [];
    }
  }, [mode, getUndoStack, getRedoStack]);

  // Build detailed error message with context
  const buildErrorMessage = useCallback((
    errorReason: string,
    context: { description: string; commandType: string; timestamp: string | null }
  ): string => {
    const lines: string[] = [];
    lines.push(t('undo.errors.failedAction', { mode, description: context.description }));
    lines.push('');
    lines.push(t('undo.errors.reason', { reason: errorReason }));
    lines.push('');
    lines.push(t('undo.errors.commandType', { commandType: context.commandType }));
    if (context.timestamp) {
      lines.push(t('undo.errors.timestamp', { timestamp: formatTimestamp(context.timestamp, t) }));
    }
    return lines.join('\n');
  }, [mode, t]);

  // Process the action queue sequentially
  const processQueue = useCallback(async () => {
    // Already processing - the current processor will pick up new items
    if (isActionInProgressRef.current) return;

    while (actionQueueRef.current.length > 0) {
      const { action: nextAction, context } = actionQueueRef.current.shift()!;
      isActionInProgressRef.current = true;
      setIsActionInProgress(true);

      let result: CommandResult | undefined;
      try {
        result = await nextAction();
      } catch (error) {
        // Secondary safety net: unexpected thrown exceptions
        console.error(`Failed to execute queued ${mode} action:`, error);
        const errorReason = getErrorMessage(error);
        const errorStack = error instanceof Error ? error.stack : undefined;

        // Refresh stack so dropdown reflects actual backend state even after error
        await fetchStack();

        // Clear loading state, keep dropdown open, show error dialog
        isActionInProgressRef.current = false;
        setIsActionInProgress(false);
        actionQueueRef.current = [];
        openDialog('error', {
          title: t('undo.errors.failedTitle', { mode: modeLabel }),
          message: buildErrorMessage(errorReason, context),
          stackTrace: errorStack,
        });
        return;
      }

      // Primary error path: check CommandResult.success (fail closed: treat missing result as error)
      if (!result?.success) {
        console.error(`${mode} action failed:`, result?.error);
        const errorReason = result?.error ?? t('undo.errors.unknownError');

        // Refresh stack so dropdown reflects actual backend state even after error
        await fetchStack();

        // Clear loading state, keep dropdown open, show error dialog
        isActionInProgressRef.current = false;
        setIsActionInProgress(false);
        actionQueueRef.current = [];
        openDialog('error', {
          title: t('undo.errors.failedTitle', { mode: modeLabel }),
          message: buildErrorMessage(errorReason, context),
        });
        return;
      }

      // After action completes successfully, refresh the stack
      const refreshedStack = await fetchStack();

      // Reset navigation and scroll to top
      setActiveIndex(0);
      setHoverIndex(-1);
      if (scrollContainerRef.current) {
        scrollContainerRef.current.scrollTo({ top: 0 });
      }

      // Close-on-empty: if stack is now empty, discard remaining queued actions and hard
      // close.
      //

      // local entry is exactly when a history walk reaches a commit boundary, and closing
      // the panel there hides the pinned control that continues in that same direction.
      if (refreshedStack.length === 0) {
        actionQueueRef.current = [];
        isActionInProgressRef.current = false;
        setIsActionInProgress(false);
        setAnnouncement(t('undo.announcement.none'));
        hardClose();
        return;
      }
      // Announce remaining item count for screen readers
      const count = refreshedStack.length;
      setAnnouncement(t('undo.announcement.remaining', { count, countFormatted: formatNumber(count) }));
    }

    isActionInProgressRef.current = false;
    setIsActionInProgress(false);
  }, [mode, fetchStack, hardClose, openDialog, t, modeLabel, buildErrorMessage]);

  // Enqueue an action and start processing if idle
  const enqueueAction = useCallback((
    action: () => Promise<CommandResult>,
    context: { description: string; commandType: string; timestamp: string | null }
  ) => {
    // Immediately dismiss detail preview popup
    clearPreviewState();

    actionQueueRef.current.push({ action, context });
    processQueue();
  }, [clearPreviewState, processQueue]);

  // Open dropdown with delay
  const openDropdown = useCallback(() => {
    // [#303] `hoverableWhenEmpty` is the one waiver: an empty undo stack disables the

    if (disabledRef.current) return;

    // Clear any pending leave timeout
    if (leaveTimeoutRef.current) {
      clearTimeout(leaveTimeoutRef.current);
      leaveTimeoutRef.current = null;
    }

    // Capture current token to detect if mouse left before timeout fires
    const currentToken = hoverTokenRef.current;

    // Set hover timeout for 300ms delay
    hoverTimeoutRef.current = setTimeout(async () => {
      // Bail out if token changed (any close path was triggered) or disabled is now true
      if (
        hoverTokenRef.current !== currentToken ||
        (disabledRef.current)
      ) {
        return;
      }

      try {
        // Single fetch - use returned stack to decide whether to open
        const stack = await fetchStack();

        // Re-check token and disabled after async operation
        if (
          hoverTokenRef.current !== currentToken ||
          (disabledRef.current)
        ) {
          return;
        }

        // Open when there is something to show.
        if (stack.length > 0) {
          setIsOpen(true);
          return;
        }

      } catch {
        // Error already logged in fetchStack
      }
    }, 300);
  }, [fetchStack]);

  // Close dropdown with small delay buffer (for mouse leave)
  const closeDropdown = useCallback((event?: React.MouseEvent) => {
    // If focus/pointer moved to a related target inside the component, stay open.
    if (relatedTargetWithin(containerRef.current, event?.relatedTarget)) {
      // Moving within the component, don't close
      return;
    }

    // Increment token to invalidate any pending hover callback
    hoverTokenRef.current += 1;

    // Clear any pending hover timeout
    if (hoverTimeoutRef.current) {
      clearTimeout(hoverTimeoutRef.current);
      hoverTimeoutRef.current = null;
    }

    // Clear any existing leave timeout before scheduling new one
    if (leaveTimeoutRef.current) {
      clearTimeout(leaveTimeoutRef.current);
      leaveTimeoutRef.current = null;
    }

    // Small delay before closing to allow moving within component
    // Mouse leave still closes even during an action in flight (per spec)
    leaveTimeoutRef.current = setTimeout(() => {
      // Reset all navigation state on close
      setActiveIndex(-1);
      setHoverIndex(-1);
      keyboardNavActiveRef.current = false;
      setIsOpen(false);
    }, 100);
  }, []);

  // Cancel close if entering dropdown
  const cancelClose = useCallback(() => {
    if (leaveTimeoutRef.current) {
      clearTimeout(leaveTimeoutRef.current);
      leaveTimeoutRef.current = null;
    }
  }, []);

  // Handle item hover - show preview popup immediately (no delay per spec)
  const handleItemHover = useCallback((item: UndoHistoryItem, event: React.MouseEvent<HTMLButtonElement>) => {
    const target = event.currentTarget;
    const rect = target.getBoundingClientRect();
    const dropdown = dropdownRef.current;
    if (dropdown) {
      setDropdownRect(dropdown.getBoundingClientRect());
    }
    setHoveredItemRect(rect);
    setHoveredItem(item);
  }, []);

  // Handle item leave - clear preview
  const handleItemLeave = useCallback(() => {
    setHoveredItem(null);
    setHoveredItemRect(null);
  }, []);

  // Scroll active item into view
  const scrollActiveItemIntoView = useCallback((index: number) => {
    if (!scrollContainerRef.current || index < 0) return;
    const container = scrollContainerRef.current;
    const itemId = `${menuId}-item-${index}`;
    const itemElement = container.querySelector(`[id="${itemId}"]`) as HTMLElement | null;
    if (itemElement) {
      itemElement.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  }, [menuId]);

  // Handle item click with index validation - closes dropdown after action
  const handleItemClick = useCallback(async (index: number) => {
    setSource('toolbar');
    // Validate index before sending to backend
    if (!Number.isInteger(index) || index < 0) {
      console.error('Invalid undo/redo index:', index);
      return;
    }
    // Find the item to get context for error messages
    const item = items.find(i => i.index === index);
    const context = item
      ? { description: item.description, commandType: item.commandType, timestamp: item.timestamp }
      : { description: t('undo.actionIndex', { index }), commandType: 'unknown', timestamp: null };

    const action = mode === 'undo' ? undoTo : redoTo;
    enqueueAction(() => action(index), context);
    // Close dropdown immediately after clicking an item
    hardClose();
  }, [mode, undoTo, redoTo, enqueueAction, hardClose, items, t]);

  // Handle keyboard navigation on trigger button
  const handleButtonKeyDown = useCallback(async (event: React.KeyboardEvent) => {
    if (isOpen && items.length > 0) {
      switch (event.key) {
        case 'ArrowDown': {
          event.preventDefault();
          const nextIndex = activeIndex < items.length - 1 ? activeIndex + 1 : 0;
          setActiveIndex(nextIndex);
          scrollActiveItemIntoView(nextIndex);
          return;
        }
        case 'ArrowUp': {
          event.preventDefault();
          const prevIndex = activeIndex > 0 ? activeIndex - 1 : items.length - 1;
          setActiveIndex(prevIndex);
          scrollActiveItemIntoView(prevIndex);
          return;
        }
        case 'Home': {
          event.preventDefault();
          setActiveIndex(0);
          scrollActiveItemIntoView(0);
          return;
        }
        case 'End': {
          event.preventDefault();
          const lastIndex = items.length - 1;
          setActiveIndex(lastIndex);
          scrollActiveItemIntoView(lastIndex);
          return;
        }
        case 'Enter': {
          event.preventDefault();
          const item = items[activeIndex];
          if (item && activeIndex >= 0 && activeIndex < items.length) {
            handleItemClick(item.index);
          }
          return;
        }
        case 'Escape': {
          event.preventDefault();
          hardClose();
          buttonRef.current?.focus();
          return;
        }
      }
    }

    // Arrow Down opens dropdown and focuses first item.
    // [#303] `hoverableWhenEmpty` lets it through while `disabled` too - that is the state

    // entry landed. The button still performs nothing; this only opens the panel.
    if (
      event.key === 'ArrowDown' &&
      !isOpen &&
      (!disabledRef.current)
    ) {
      event.preventDefault();
      // Fetch stack and open
      const stack = await fetchStack();
      // [#303] Mirror of the hover path in `openDropdown`, deliberately: an empty stack

      // cannot disagree about whether there is anything in it.
      if (stack.length > 0) {
        // Mark as keyboard navigation for focus management
        keyboardNavActiveRef.current = true;
        setIsOpen(true);
        if (stack.length > 0) {
          // Set first item as active after React renders (store timeout for cleanup)
          keyboardOpenTimeoutRef.current = setTimeout(() => {
            setActiveIndex(0);
            scrollActiveItemIntoView(0);
            keyboardOpenTimeoutRef.current = null;
          }, 0);
        }

        // anyway - it is not in `items`. The keyboard-open focus effect lands on it
        // instead; deferring the focus here as well raced that effect and lost, because a
        // `setTimeout(0)` scheduled before the commit can fire on either side of it.
      }
    }
  }, [
    isOpen,
    items,
    activeIndex,
    fetchStack,
    scrollActiveItemIntoView,
    handleItemClick,
    hardClose,
  ]);

  // Handle keyboard navigation when dropdown is open
  const handleDropdownKeyDown = useCallback((event: React.KeyboardEvent) => {
    // [#303] Only `isOpen` gates the whole handler. An open panel with an EMPTY list is a

    // `items.length === 0` made it a dead end: Escape did not close it, and the entry's
    // own Enter had to survive this handler untouched. Each list branch carries its own
    // emptiness guard instead, placed BEFORE `preventDefault` so a key this panel has no
    // list use for still reaches the focused entry.
    if (!isOpen) return;

    switch (event.key) {
      case 'ArrowDown': {
        if (items.length === 0) break;
        event.preventDefault();
        const nextIndex = activeIndex < items.length - 1 ? activeIndex + 1 : 0;
        setActiveIndex(nextIndex);
        scrollActiveItemIntoView(nextIndex);
        break;
      }
      case 'ArrowUp': {
        if (items.length === 0) break;
        event.preventDefault();
        const prevIndex = activeIndex > 0 ? activeIndex - 1 : items.length - 1;
        setActiveIndex(prevIndex);
        scrollActiveItemIntoView(prevIndex);
        break;
      }
      case 'Enter': {
        if (items.length === 0) break;
        event.preventDefault();
        const item = items[activeIndex];
        if (item && activeIndex >= 0 && activeIndex < items.length) {
          handleItemClick(item.index);
        }
        break;
      }
      case 'Escape': {
        event.preventDefault();
        hardClose();
        // Return focus to trigger button
        buttonRef.current?.focus();
        break;
      }
      case 'Home': {
        if (items.length === 0) break;
        event.preventDefault();
        setActiveIndex(0);
        scrollActiveItemIntoView(0);
        break;
      }
      case 'End': {
        if (items.length === 0) break;
        event.preventDefault();
        const lastIndex = items.length - 1;
        setActiveIndex(lastIndex);
        scrollActiveItemIntoView(lastIndex);
        break;
      }
    }
  }, [isOpen, items, activeIndex, handleItemClick, hardClose, scrollActiveItemIntoView]);

  // Handle button click - when dropdown is open, perform action and stay open; when closed, delegate to parent
  // Note: when open, we call store undo/redo directly (not onSingleAction) because we need the Promise
  // to chain refresh logic. onSingleAction is fire-and-forget and used only for the closed-dropdown path.
  const handleButtonClick = useCallback(() => {
    if (disabledRef.current) return;
    setSource('toolbar');

    if (isOpenRef.current) {
      // Dropdown is open: perform single undo/redo and refresh (stay open)
      // Use first item for context (it's what will be undone/redone)
      const firstItem = items[0];
      const context = firstItem
        ? { description: firstItem.description, commandType: firstItem.commandType, timestamp: firstItem.timestamp }
        : { description: t('undo.singleAction'), commandType: 'unknown', timestamp: null };

      const action = mode === 'undo' ? undo : redo;
      enqueueAction(() => action(), context);
    } else {
      // Dropdown is not open: delegate to parent via onSingleAction (closed-only path)
      hardClose();
      onSingleAction();
    }
  }, [mode, undo, redo, enqueueAction, onSingleAction, hardClose, items, t]);

  // Close on Escape - use hardClose and return focus to button
  useEffect(() => {
    if (!isOpen) return;

    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        hardClose();
        // Return focus to trigger button for consistent behavior
        buttonRef.current?.focus();
      }
    };

    document.addEventListener('keydown', handleEscape);
    return () => {
      document.removeEventListener('keydown', handleEscape);
    };
  }, [isOpen, hardClose]);

  // Handle Ctrl+Z/Y keyboard shortcuts when dropdown is open - perform action and stay open
  // Uses stopPropagation + preventDefault to prevent the global useKeyboardShortcuts handler
  // from also executing the action (which would cause double-undo/redo)
  useEffect(() => {
    if (!isOpen) return;

    const handleShortcut = (event: KeyboardEvent) => {
      // Don't intercept when focus is in a text input - let native text undo/redo work
      if (
        event.target instanceof HTMLInputElement ||
        event.target instanceof HTMLTextAreaElement ||
        event.target instanceof HTMLSelectElement ||
        (event.target instanceof HTMLElement && event.target.isContentEditable)
      ) {
        return;
      }

      const key = event.key.toLowerCase();
      const isUndoKey = (event.ctrlKey || event.metaKey) && key === 'z' && !event.shiftKey;
      const isRedoKey = (event.ctrlKey || event.metaKey) && (key === 'y' || (key === 'z' && event.shiftKey));

      if (isUndoKey || isRedoKey) {
        // Prevent global keyboard shortcut handler from also firing (double-undo/redo)
        event.stopPropagation();
        event.preventDefault();

        // A chain-only panel has no local action for these shortcuts. Crossing a server
        // commit boundary remains an explicit click/Enter plus confirmation.
        if (items.length === 0) return;

        // Use first item for context
        const firstItem = items[0];
        const context = firstItem
          ? { description: firstItem.description, commandType: firstItem.commandType, timestamp: firstItem.timestamp }
          : { description: t('undo.keyboardAction'), commandType: 'unknown', timestamp: null };

        // Perform the appropriate action based on the key pressed
        const action = isUndoKey ? undo : redo;
        enqueueAction(() => action(), context);
      }
    };

    // Use capture phase to intercept before global handler
    document.addEventListener('keydown', handleShortcut, true);
    return () => {
      document.removeEventListener('keydown', handleShortcut, true);
    };
  }, [isOpen, undo, redo, enqueueAction, items, t]);

  // Close on outside mousedown / external scroll / resize via the shared hook.
  // The whole layer (trigger + absolutely-positioned dropdown) lives inside
  // containerRef, so that's the single "inside" element; the inner undo-history
  // list (scrollContainerRef) is the scroll-safe surface so paging through
  // history doesn't dismiss.
  useDismissableLayer({
    open: isOpen,
    onDismiss: hardClose,
    contains: [containerRef],
    scrollSafe: [scrollContainerRef],
  });

  // Cleanup timeouts on unmount
  useEffect(() => {
    return () => {
      if (hoverTimeoutRef.current) {
        clearTimeout(hoverTimeoutRef.current);
      }
      if (leaveTimeoutRef.current) {
        clearTimeout(leaveTimeoutRef.current);
      }
      if (keyboardOpenTimeoutRef.current) {
        clearTimeout(keyboardOpenTimeoutRef.current);
      }
    };
  }, []);

  // Focus dropdown when opened via keyboard (only when keyboardNavActiveRef is true)
  useEffect(() => {
    if (!isOpen || !keyboardNavActiveRef.current) return;

    // the container would otherwise hold - the panel is otherwise a place the keyboard can
    // enter and find nothing. Landing here rather than in a deferred callback is what
    // makes it deterministic: this runs after the commit that rendered the entry.
    const landing = dropdownRef.current;
    if (landing) {
      landing.focus();
      // Clear the flag after focusing to prevent re-focusing on hover state changes
      keyboardNavActiveRef.current = false;
    }
  }, [isOpen, items]);

  // Refetch stack when undo/redo availability changes (from external edits, etc.)
  // This ensures the dropdown stays in sync with the actual undo stack state
  const canUndoOrRedo = mode === 'undo' ? canUndo : canRedo;
  useEffect(() => {
    if (isOpen && !isActionInProgressRef.current) {
      // Refetch to sync with current backend state
      fetchStack();
    }
  }, [isOpen, canUndoOrRedo, fetchStack]);

  // Get the active item ID for aria-activedescendant
  const activeItemId = activeIndex >= 0 && items[activeIndex]
    ? `${menuId}-item-${activeIndex}`
    : undefined;

  return (
    <div
      ref={containerRef}
      className="relative"
      onMouseEnter={openDropdown}
      onMouseLeave={closeDropdown}
    >
      <button
        ref={buttonRef}
        onClick={handleButtonClick}
        onKeyDown={handleButtonKeyDown}

        // entry reachable by mouse only - the panel behind it had no keyboard route at

        // The button performs nothing either way: `handleButtonClick` still returns on
        // `disabledRef`, and the styling below stays keyed on the ORIGINAL `disabled` so
        // it reads inert.
        //
        // The accepted cost is a tab stop that can open a panel with nothing in it (the

        // worse deal: the toolbar passes it as `!placementModeActive`, so it is true on a
        // LOCAL project too, and there the panel can never open - a tab stop that does

        // `openDropdown` opens on, which keeps focusable and can-open the same set.
        disabled={disabled}
        aria-label={ariaLabel}
        aria-expanded={isOpen}
        aria-haspopup="menu"
        aria-controls={isOpen ? `${menuId}-menu` : undefined}
        className={`
          ${compact ? 'px-1.5 py-1' : 'px-3 py-1.5'} text-sm rounded transition-colors
          ${disabled
            ? 'opacity-50 cursor-not-allowed'
            : 'bg-background hover:bg-background-80'
          }
        `}
        data-testid={testId}
      >
        {children}
      </button>

      {isOpen && (
        <div
          ref={dropdownRef}
          id={`${menuId}-menu`}
          className="absolute left-0 top-full mt-1 w-72 bg-panel border border-border rounded-lg shadow-lg z-50"
          role="menu"
          aria-activedescendant={activeItemId}
          data-dropdown-open
          data-testid={`${testId}-dropdown`}
          onMouseEnter={cancelClose}
          onKeyDown={handleDropdownKeyDown}
          tabIndex={-1}
        >
          <div className="px-3 py-2 text-xs font-medium text-text-secondary uppercase tracking-wide border-b border-border">
            {mode === 'undo' ? t('undo.history.undo') : t('undo.history.redo')}
          </div>

          {/* Loading indicator shown at top of dropdown during action */}
          {isActionInProgress && (
            <div
              className="flex items-center justify-center gap-2 px-3 py-1.5 border-b border-border"
              data-testid={`${testId}-loading`}
            >
              <div className="w-4 h-4 border-2 border-accent border-t-transparent rounded-full animate-spin" />
              <span className="text-xs text-text-secondary">{t('undo.processing')}</span>
            </div>
          )}

          <div
            ref={scrollContainerRef}
            className="overflow-y-auto undo-history-scrollable"
            style={{
              // Fixed height for 5 items (~44px each = 220px)
              // Uses height (not maxHeight) to maintain size even with fewer items
              height: '220px',
            }}
          >
            {items.length === 0 ? (
              <div className="px-3 py-4 text-center text-text-secondary text-sm">
                {t('undo.history.empty', { mode: modeLabel })}
              </div>
            ) : (
              items.map((item, idx) => {
                // Keyboard nav takes precedence; otherwise use hover highlight
                const isKeyboardActive = idx === activeIndex;
                const isHoverActive = idx === hoverIndex;
                const isHighlighted = isKeyboardActive || isHoverActive;
                return (
                  <button
                    key={item.index}
                    id={`${menuId}-item-${idx}`}
                    role="menuitem"
                    onClick={() => handleItemClick(item.index)}
                    onMouseEnter={(e) => {
                      handleItemHover(item, e);
                      // Update hover index for visual highlight (does not affect keyboard nav)
                      setHoverIndex(idx);
                    }}
                    onMouseLeave={() => {
                      handleItemLeave();
                      setHoverIndex(-1);
                    }}
                    className={`
                      w-full px-3 py-1.5 flex items-center gap-2 transition-colors text-left
                      ${isHighlighted ? 'bg-background-50' : 'hover:bg-background-50'}
                      ${isActionInProgress ? 'opacity-50' : ''}
                    `}
                    data-testid={`${testId}-item-${item.index}`}
                    tabIndex={-1}
                  >
                    {/* Icon */}
                    <span className="w-5 h-5 flex items-center justify-center text-sm shrink-0">
                      {getCommandIcon(item.commandType)}
                    </span>

                    {/* Description and timestamp */}
                    <div className="flex-1 min-w-0">
                      <div
                        className="text-sm text-text-primary truncate"
                        title={item.description}
                      >
                        {truncateDescription(item.description)}
                      </div>
                      <div className="text-xs text-text-secondary">
                        {formatTimestamp(item.timestamp, t)}
                      </div>
                    </div>
                  </button>
                );
              })
            )}
          </div>

        </div>
      )}

      {/* Detail preview popup - rendered via portal */}
      {hoveredItem && hoveredItemRect && dropdownRect && (
        <DetailPreviewPopup
          previewData={hoveredItem.previewData}
          anchorRect={hoveredItemRect}
          dropdownRect={dropdownRect}
          commandType={hoveredItem.commandType}
        />
      )}

      {/* ARIA live region for announcing remaining action count */}
      <span
        role="status"
        aria-live="polite"
        aria-atomic="true"
        className="sr-only"
        data-testid={`${testId}-announcement`}
      >
        {announcement}
      </span>

      {/* CSS for auto-hide scrollbar and preview animation */}
      <style>{`
        .undo-history-scrollable {
          scrollbar-width: thin;
          scrollbar-color: transparent transparent;
        }
        .undo-history-scrollable:hover {
          scrollbar-color: var(--color-border) transparent;
        }
        .undo-history-scrollable::-webkit-scrollbar {
          width: 6px;
        }
        .undo-history-scrollable::-webkit-scrollbar-track {
          background: transparent;
        }
        .undo-history-scrollable::-webkit-scrollbar-thumb {
          background-color: transparent;
          border-radius: 3px;
        }
        .undo-history-scrollable:hover::-webkit-scrollbar-thumb {
          background-color: var(--color-border);
        }

        /* Detail preview popup fade-in animation */
        @keyframes previewFadeIn {
          from {
            opacity: 0;
          }
          to {
            opacity: 1;
          }
        }
        .detail-preview-popup {
          animation: previewFadeIn 150ms ease-out;
        }
        /* Respect prefers-reduced-motion */
        @media (prefers-reduced-motion: reduce) {
          .detail-preview-popup {
            animation: none;
          }
        }
      `}</style>
    </div>
  );
}
