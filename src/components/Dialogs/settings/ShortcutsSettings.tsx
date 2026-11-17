import { Fragment, useState, useMemo, useCallback, useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { ShortcutRecorder } from '../../Settings/ShortcutRecorder';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import type { Config, ShortcutsConfig } from '../../../types/config';
import { defaultConfig } from '../../../types/config';
import type { ToolType } from '../../../store/uiStore';
import {
  normalizeShortcut,
  checkReservedConflict,
  findConflict,
  formatShortcut,
  formatActionName,
  isMac,
} from '../../../utils/shortcuts';
import {
  SHORTCUT_CATEGORIES,
  RESERVED_DISPLAY_ENTRIES,
} from '../../../utils/shortcutCategories';
import { navigationToolsEnabled } from '../../../utils/toolUtils';
import { isNavigationTool } from '../../../utils/toolGroups';

/**
 * Shortcut actions that select a tool from the navigate group (Pan / Zoom) -
 * the group hidden unless `map.controls.show_navigation_tools` is on (#409).
 *
 * Derived from the category table crossed with the tool-group table rather than
 * naming Pan/Zoom a third time: a tool that joins or leaves the navigate group
 * is picked up here with no edit. Actions that are not tools (`save`,
 * `toggle_grid`, ...) never match a group and drop out.
 */
const NAVIGATION_TOOL_ACTIONS: ReadonlySet<keyof ShortcutsConfig> = new Set(
  SHORTCUT_CATEGORIES.flatMap((category) => category.actions).filter((action) =>
    isNavigationTool(action as ToolType)
  )
);

/**
 * Action name label with hover tooltip for truncated names.
 * Uses a custom tooltip (not native title) for keyboard accessibility and consistent styling.
 *
 * `dimmed` is the disabled presentation for a row whose tool is currently hidden
 * (#409); the controls next to it dim themselves through their own `disabled`.
 */
function ShortcutNameLabel({ name, dimmed = false }: { name: string; dimmed?: boolean }) {
  const [showTooltip, setShowTooltip] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const [isTruncated, setIsTruncated] = useState(false);

  // Check if text is actually truncated
  const checkTruncation = useCallback(() => {
    const el = containerRef.current;
    if (el) {
      setIsTruncated(el.scrollWidth > el.clientWidth);
    }
  }, []);

  return (
    <div className={`relative flex-1 min-w-[8rem] ${dimmed ? 'opacity-60' : ''}`}>
      <div
        ref={containerRef}
        className="text-sm truncate"
        onMouseEnter={() => { checkTruncation(); setShowTooltip(true); }}
        onMouseLeave={() => setShowTooltip(false)}
        onFocus={() => { checkTruncation(); setShowTooltip(true); }}
        onBlur={() => setShowTooltip(false)}
        tabIndex={-1}
      >
        {name}
      </div>
      {showTooltip && isTruncated && (
        <div
          className="absolute left-0 bottom-full mb-1 bg-panel-90 text-text-primary text-xs px-2 py-1 rounded border border-border whitespace-nowrap pointer-events-none z-10"
          role="tooltip"
        >
          {name}
        </div>
      )}
    </div>
  );
}


/**
 * Shortcuts settings category content.
 * Preserves the table layout from the original SettingsDialog shortcuts tab.
 */
export function ShortcutsSettings({
  config,
  onChange,
  changedFields: _changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const [shortcutSearch, setShortcutSearch] = useState('');
  const [collapsedCategories, setCollapsedCategories] = useState<Set<string>>(new Set());
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const noteIdBase = useId();

  /**
   * Pan/Zoom rows stay listed and stay in the registry below - their keys remain
   * reserved, so re-enabling the tools needs no migration - but while the tools
   * are hidden (#409) the rows are shown disabled and cannot be rebound. Read
   * through the same helper the toolbar and the shortcut handler use; never
   * re-derive the flag.
   */
  const navigationEnabled = navigationToolsEnabled(config);
  const isActionDisabled = useCallback(
    (action: keyof ShortcutsConfig): boolean =>
      !navigationEnabled && NAVIGATION_TOOL_ACTIONS.has(action),
    [navigationEnabled]
  );

  // Track error state for parent
  const hasErrorRef = useRef(false);

  // Report validation error state
  useEffect(() => {
    const hasError = shortcutError !== null;
    if (hasError !== hasErrorRef.current) {
      hasErrorRef.current = hasError;
      onValidationError('shortcuts', hasError);
    }
  }, [shortcutError, onValidationError]);

  // Build registry map for conflict detection
  const shortcutRegistry = useMemo(() => {
    const registry = new Map<string, string[]>();
    const shortcuts = config.shortcuts;
    for (const category of SHORTCUT_CATEGORIES) {
      for (const action of category.actions) {
        const bindings = shortcuts[action];
        if (Array.isArray(bindings)) {
          registry.set(action, bindings);
        }
      }
    }
    return registry;
  }, [config.shortcuts]);

  // Filter actions based on search
  const filterAction = useCallback(
    (action: keyof ShortcutsConfig): boolean => {
      if (!shortcutSearch.trim()) return true;
      const search = shortcutSearch.toLowerCase();

      const actionName = formatActionName(action).toLowerCase();
      if (actionName.includes(search)) return true;

      const bindings = config.shortcuts[action];
      if (Array.isArray(bindings)) {
        for (const binding of bindings) {
          const formatted = formatShortcut(binding, isMac()).toLowerCase();
          if (formatted.includes(search)) return true;
          if (binding.toLowerCase().includes(search)) return true;
        }
      }

      return false;
    },
    [shortcutSearch, config.shortcuts]
  );

  // Toggle category collapse
  const toggleCategory = useCallback((categoryId: string) => {
    setCollapsedCategories((prev) => {
      const next = new Set(prev);
      if (next.has(categoryId)) {
        next.delete(categoryId);
      } else {
        next.add(categoryId);
      }
      return next;
    });
  }, []);

  // Handle shortcut change
  const handleShortcutChange = useCallback(
    (action: keyof ShortcutsConfig, slot: 'primary' | 'secondary', newValue: string | null) => {
      // A disabled row's controls are inert, so this is belt-and-braces: no path
      // may rebind or clear a hidden tool's binding while it is hidden (#409).
      if (isActionDisabled(action)) return;

      setShortcutError(null);

      if (newValue) {
        // Check for reserved shortcut
        const reservedConflict = checkReservedConflict(newValue);
        if (reservedConflict) {
          setShortcutError(
            t('settings.shortcuts.errors.reserved', {
              shortcut: formatShortcut(newValue, isMac()),
              reason: reservedConflict.reason,
            })
          );
          return;
        }

        // Check for conflicts with other actions
        const conflict = findConflict(newValue, shortcutRegistry, action);
        if (conflict) {
          setShortcutError(
            t('settings.shortcuts.errors.conflict', {
              shortcut: formatShortcut(newValue, isMac()),
              action: formatActionName(conflict),
            })
          );
          return;
        }

        // Check for intra-action duplicate
        const normalized = normalizeShortcut(newValue);
        const currentBindings = config.shortcuts[action] || [];
        const otherSlotIndex = slot === 'primary' ? 1 : 0;
        const otherSlotValue = currentBindings[otherSlotIndex];
        if (otherSlotValue && normalizeShortcut(otherSlotValue) === normalized) {
          setShortcutError(
            t('settings.shortcuts.errors.duplicateSlot', {
              shortcut: formatShortcut(newValue, isMac()),
              slot: slot === 'primary'
                ? t('settings.shortcuts.slots.secondary')
                : t('settings.shortcuts.slots.primary'),
            })
          );
          return;
        }
      }

      // Update config
      const currentBindings = [...(config.shortcuts[action] || [])];
      const slotIndex = slot === 'primary' ? 0 : 1;

      if (newValue) {
        const normalized = normalizeShortcut(newValue);
        if (normalized) {
          currentBindings[slotIndex] = normalized;
        }
      } else {
        if (slotIndex < currentBindings.length) {
          currentBindings.splice(slotIndex, 1);
        }
      }

      const filteredBindings = currentBindings.filter((b) => b && b.trim());

      const newShortcuts: ShortcutsConfig = {
        ...config.shortcuts,
        [action]: filteredBindings,
      };

      onChange({ shortcuts: newShortcuts } as Partial<Config>);
    },
    [config.shortcuts, isActionDisabled, onChange, shortcutRegistry, t]
  );

  // Reset single action to defaults
  const handleResetAction = useCallback(
    (action: keyof ShortcutsConfig) => {
      if (isActionDisabled(action)) return;

      setShortcutError(null);

      const newShortcuts: ShortcutsConfig = {
        ...config.shortcuts,
        [action]: [...defaultConfig.shortcuts[action]],
      };

      onChange({ shortcuts: newShortcuts } as Partial<Config>);
    },
    [config.shortcuts, isActionDisabled, onChange]
  );

  // Reset all shortcuts to defaults
  const handleResetAll = useCallback(() => {
    setShortcutError(null);
    onChange({ shortcuts: { ...defaultConfig.shortcuts } } as Partial<Config>);
  }, [onChange]);

  // Handle blocked key from ShortcutRecorder
  const handleBlockedKey = useCallback((key: string) => {
    setShortcutError(t('settings.shortcuts.errors.blockedKey', { key }));
  }, [t]);

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'shortcuts')}
      aria-labelledby={getTabId(panelIdBase, 'shortcuts')}
      data-testid="settings-panel-shortcuts"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.shortcuts.title')}</h2>

      <div className="space-y-4" data-testid="shortcuts-tab-content">
        {/* Reserved shortcuts section (always visible, above search) */}
        <div className="mb-4">
          <h3 className="text-sm font-medium mb-2 text-text-secondary">{t('settings.shortcuts.reserved.title')}</h3>
          <div
            className="bg-background-50 rounded-lg p-3 grid grid-cols-2 gap-2 text-sm"
            data-testid="reserved-shortcuts"
          >
            {RESERVED_DISPLAY_ENTRIES.map(({ shortcuts, descriptionKey }) => (
              <div key={shortcuts.join('+')} className="flex items-center gap-2">
                <span className="inline-flex items-center gap-1 shrink-0">
                  {shortcuts.map((s, i) => (
                    <span key={s} className="inline-flex items-center gap-1">
                      {i > 0 && <span className="text-text-secondary text-xs">/</span>}
                      <span className="inline-flex items-center justify-center px-1.5 py-0.5 text-xs font-medium bg-panel border border-border rounded min-w-[1.5rem] text-text-secondary">
                        {formatShortcut(s, isMac())}
                      </span>
                    </span>
                  ))}
                </span>
                <span className="text-text-secondary text-xs">{t(descriptionKey)}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Search bar */}
        <div className="relative">
          <input
            type="text"
            value={shortcutSearch}
            onChange={(e) => setShortcutSearch(e.target.value)}
            placeholder={t('settings.shortcuts.search.placeholder')}
            aria-label={t('settings.shortcuts.search.aria')}
            className="w-full px-3 py-2 pl-9 bg-background border border-border rounded text-sm"
            data-testid="shortcuts-search"
          />
          <svg
            className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text-secondary"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth={2}
              d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
            />
          </svg>
        </div>

        {/* Error message */}
        {shortcutError && (
          <div
            role="alert"
            aria-live="assertive"
            className="p-2 bg-error-10 border border-error-30 rounded text-error text-sm"
            data-testid="shortcut-error"
          >
            {shortcutError}
          </div>
        )}

        {/* Shortcuts list - outer container handles scrolling */}
        <div className="space-y-4">
          {SHORTCUT_CATEGORIES.map((category) => {
            const filteredActions = category.actions.filter(filterAction);
            if (filteredActions.length === 0) return null;

            const isCollapsed = collapsedCategories.has(category.id);
            // The disabled-tools note is rendered directly under the LAST
            // disabled row, so the explanation sits with the rows it explains
            // instead of at the foot of the category. -1 when the search filter
            // left no disabled row on screen, which drops the note with them.
            const lastDisabledIndex = filteredActions.reduce(
              (last, action, index) => (isActionDisabled(action) ? index : last),
              -1
            );

            return (
              <div key={category.id} data-testid={`shortcut-category-${category.id}`}>
                {/* Category header */}
                <button
                  type="button"
                  onClick={() => toggleCategory(category.id)}
                  className="flex items-center gap-2 w-full text-left text-sm font-medium mb-2 hover:text-accent transition-colors"
                  data-testid={`shortcut-category-toggle-${category.id}`}
                >
                  <svg
                    className={`w-4 h-4 transition-transform ${isCollapsed ? '' : 'rotate-90'}`}
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={2}
                      d="M9 5l7 7-7 7"
                    />
                  </svg>
                  {t(category.labelKey)}
                </button>

                {/* Category content */}
                {!isCollapsed && (
                  <div className="space-y-0.5 pl-6">
                    {filteredActions.map((action, idx) => {
                      const bindings = config.shortcuts[action] || [];
                      const primary = bindings[0] || null;
                      const secondary = bindings[1] || null;
                      const full = formatActionName(action);
                      const hasChanges =
                        JSON.stringify(bindings) !== JSON.stringify(defaultConfig.shortcuts[action]);
                      const isOdd = idx % 2 === 1;
                      // Hidden tool (#409): the row stays (and stays in the
                      // conflict registry) but reads and behaves as disabled.
                      const rowDisabled = isActionDisabled(action);
                      const canReset = hasChanges && !rowDisabled;

                      return (
                        <Fragment key={action}>
                          <div
                            className={`flex items-center gap-3 py-0.5 rounded ${isOdd ? 'bg-panel-20' : 'bg-transparent'}`}
                            data-testid={`shortcut-row-${action}`}
                            aria-disabled={rowDisabled || undefined}
                            data-disabled={rowDisabled ? 'true' : undefined}
                            aria-describedby={
                              rowDisabled ? `${noteIdBase}-${category.id}` : undefined
                            }
                          >
                            {/* Action name */}
                            <ShortcutNameLabel name={full} dimmed={rowDisabled} />

                            {/* Primary slot */}
                            <ShortcutRecorder
                              value={primary}
                              onChange={(value) => handleShortcutChange(action, 'primary', value)}
                              onBlockedKey={handleBlockedKey}
                              ariaLabel={t('settings.shortcuts.slots.primaryAria', { action: full })}
                              disabled={rowDisabled}
                              data-testid={`shortcut-primary-${action}`}
                            />

                            {/* Secondary slot */}
                            <ShortcutRecorder
                              value={secondary}
                              onChange={(value) => handleShortcutChange(action, 'secondary', value)}
                              onBlockedKey={handleBlockedKey}
                              ariaLabel={t('settings.shortcuts.slots.secondaryAria', { action: full })}
                              disabled={rowDisabled}
                              data-testid={`shortcut-secondary-${action}`}
                            />

                            {/* Reset button */}
                            <button
                              type="button"
                              onClick={() => handleResetAction(action)}
                              disabled={!canReset}
                              aria-disabled={!canReset}
                              className={`p-1 rounded hover:bg-panel transition-colors ${
                                canReset
                                  ? 'text-text-secondary hover:text-text'
                                  : 'text-border cursor-not-allowed'
                              }`}
                              title={t('settings.shortcuts.resetOne.title')}
                              data-testid={`shortcut-reset-${action}`}
                            >
                              <svg
                                className="w-4 h-4"
                                fill="none"
                                stroke="currentColor"
                                viewBox="0 0 24 24"
                              >
                                <path
                                  strokeLinecap="round"
                                  strokeLinejoin="round"
                                  strokeWidth={2}
                                  d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                                />
                              </svg>
                            </button>
                          </div>

                          {/* Why the rows above are disabled, and where to turn
                              the tools back on. */}
                          {idx === lastDisabledIndex && (
                            <p
                              id={`${noteIdBase}-${category.id}`}
                              className="text-xs text-text-secondary py-1"
                              data-testid={`shortcut-navigation-note-${category.id}`}
                            >
                              {t('settings.shortcuts.navigationDisabled.note')}
                            </p>
                          )}
                        </Fragment>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        {/* Reset All button */}
        <div className="pt-2 border-t border-border">
          <Button variant="danger" onClick={handleResetAll} data-testid="shortcuts-reset-all">
            {t('settings.shortcuts.resetAll')}
          </Button>
        </div>
      </div>
    </div>
  );
}
