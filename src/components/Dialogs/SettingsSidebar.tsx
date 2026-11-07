import { useRef, useEffect, useId } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * Category identifiers for settings sidebar navigation.
 */
export type SettingsCategory =
  | 'general'
  | 'units'
  | 'gas'
  | 'display'
  | 'map'
  | 'validation'
  | 'export'
  | 'shortcuts'
  | 'cache'
  | 'speleodb'
  | 'advanced'
  | 'appearance';

/**
 * Category configuration with display labels.
 */
interface CategoryConfig {
  id: SettingsCategory;
  labelKey: string;
}

const CATEGORIES: CategoryConfig[] = [
  { id: 'general', labelKey: 'settings.sidebar.general' },
  { id: 'appearance', labelKey: 'settings.sidebar.appearance' },
  { id: 'units', labelKey: 'settings.sidebar.units' },
  { id: 'gas', labelKey: 'settings.sidebar.gas' },
  { id: 'display', labelKey: 'settings.sidebar.display' },
  { id: 'map', labelKey: 'settings.sidebar.map' },
  { id: 'validation', labelKey: 'settings.sidebar.validation' },
  { id: 'export', labelKey: 'settings.sidebar.export' },
  { id: 'shortcuts', labelKey: 'settings.sidebar.shortcuts' },
  { id: 'cache', labelKey: 'settings.sidebar.cache' },
  { id: 'speleodb', labelKey: 'settings.sidebar.speleodb' },
  { id: 'advanced', labelKey: 'settings.sidebar.advanced' },
];

interface SettingsSidebarProps {
  /** Currently active (selected) category */
  activeCategory: SettingsCategory;
  /** Callback when a category is activated (via Enter key or click) */
  onCategoryChange: (category: SettingsCategory) => void;
  /** Currently focused category index for roving tabindex */
  focusedIndex: number;
  /** Callback when focused index changes (via arrow keys) */
  onFocusChange: (index: number) => void;
  /** Map of category IDs to error state (true if category has validation errors) */
  categoryErrors?: Partial<Record<SettingsCategory, boolean>>;
  /** Base ID for panel aria-controls (generated if not provided) */
  panelIdBase?: string;
}

/**
 * Get the tab ID for a given base and category.
 */
export function getTabId(idBase: string, category: SettingsCategory): string {
  return `${idBase}-tab-${category}`;
}

/**
 * Get the panel ID for a given base and category.
 */
export function getPanelId(idBase: string, category: SettingsCategory): string {
  return `${idBase}-panel-${category}`;
}

/**
 * Settings sidebar navigation with vertical tab pattern.
 *
 * Implements WAI-ARIA tab pattern with manual activation:
 * - Arrow keys move focus (focusedIndex) without changing selection
 * - Enter/Space key activates the focused category (sets activeCategory)
 * - Uses roving tabindex (focused tab has tabindex=0, others have -1)
 */
export function SettingsSidebar({
  activeCategory,
  onCategoryChange,
  focusedIndex,
  onFocusChange,
  categoryErrors = {},
  panelIdBase,
}: SettingsSidebarProps) {
  const { t } = useTranslation();
  const generatedIdBase = useId();
  const idBase = panelIdBase ?? generatedIdBase;
  const tabRefs = useRef<(HTMLDivElement | null)[]>([]);
  // Track if focus change came from keyboard navigation (vs click/external)
  const focusFromKeyboardRef = useRef(false);

  // Compute effective focused index with fallback for invalid values
  // This ensures at least one tab always has tabIndex=0 for keyboard accessibility
  const effectiveFocusedIndex = (() => {
    if (focusedIndex >= 0 && focusedIndex < CATEGORIES.length) {
      return focusedIndex;
    }
    // Fallback: try to use activeCategory's index, else default to 0
    const activeIndex = CATEGORIES.findIndex((c) => c.id === activeCategory);
    return activeIndex >= 0 ? activeIndex : 0;
  })();

  // Focus the element when focusedIndex changes from keyboard navigation
  useEffect(() => {
    if (focusFromKeyboardRef.current && focusedIndex >= 0 && focusedIndex < CATEGORIES.length) {
      tabRefs.current[focusedIndex]?.focus();
      focusFromKeyboardRef.current = false;
    }
  }, [focusedIndex]);

  const handleKeyDown = (event: React.KeyboardEvent, currentIndex: number) => {
    // Only handle keyboard events when the tab element itself is focused,
    // not when focus is on nested controls
    if (event.target !== event.currentTarget) return;

    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        event.stopPropagation();
        if (currentIndex < CATEGORIES.length - 1) {
          focusFromKeyboardRef.current = true;
          onFocusChange(currentIndex + 1);
        }
        break;

      case 'ArrowUp':
        event.preventDefault();
        event.stopPropagation();
        if (currentIndex > 0) {
          focusFromKeyboardRef.current = true;
          onFocusChange(currentIndex - 1);
        }
        break;

      case 'Home':
        event.preventDefault();
        event.stopPropagation();
        focusFromKeyboardRef.current = true;
        onFocusChange(0);
        break;

      case 'End':
        event.preventDefault();
        event.stopPropagation();
        focusFromKeyboardRef.current = true;
        onFocusChange(CATEGORIES.length - 1);
        break;

      case 'Enter':
      case ' ':
        event.preventDefault();
        event.stopPropagation();
        {
          const category = CATEGORIES[currentIndex];
          if (category) {
            onCategoryChange(category.id);
          }
        }
        break;
    }
  };

  const handleClick = (category: SettingsCategory, index: number) => {
    // Don't set focusFromKeyboardRef - click already focuses the element
    onFocusChange(index);
    onCategoryChange(category);
  };

  const handleFocus = (index: number) => {
    // Only notify parent if the index actually changed
    // This prevents redundant updates when focus comes from click or effect
    if (index !== focusedIndex) {
      onFocusChange(index);
    }
  };

  return (
    <div
      role="tablist"
      aria-orientation="vertical"
      aria-label={t('settings.sidebar.ariaLabel')}
      className="w-[150px] flex-shrink-0 border-r border-border bg-panel flex flex-col overflow-hidden"
      data-testid="settings-sidebar"
    >
      <div className="flex-1 overflow-auto min-h-0">
        {CATEGORIES.map((category, index) => {
          const isActive = activeCategory === category.id;
          const isFocused = effectiveFocusedIndex === index;
          const hasError = categoryErrors[category.id] === true;
          const isAdvanced = category.id === 'advanced';

          return (
            <div key={category.id}>
              {/* Visual separator before Advanced */}
              {isAdvanced && (
                <div className="mx-3 my-2 border-t border-border" aria-hidden="true" />
              )}
              <div
                ref={(el) => {
                  tabRefs.current[index] = el;
                }}
                id={getTabId(idBase, category.id)}
                role="tab"
                aria-selected={isActive}
                aria-controls={getPanelId(idBase, category.id)}
                tabIndex={isFocused ? 0 : -1}
                data-testid={`settings-tab-${category.id}`}
                className={`
                  relative px-4 py-2 text-sm cursor-pointer select-none
                  transition-colors outline-none
                  ${isActive ? 'bg-accent-20 text-accent font-medium' : 'text-text-primary hover:bg-accent-10'}
                  ${isFocused ? 'ring-2 ring-inset ring-accent' : ''}
                `}
                onClick={() => handleClick(category.id, index)}
                onKeyDown={(e) => handleKeyDown(e, index)}
                onFocus={() => handleFocus(index)}
              >
                <span className="flex items-center gap-2">
                  {t(category.labelKey)}
                  {/* Error indicator (red dot) */}
                  {hasError && (
                    <span
                      className="w-2 h-2 rounded-full bg-error flex-shrink-0"
                      aria-label={t('settings.sidebar.hasErrors')}
                      data-testid={`settings-tab-error-${category.id}`}
                    />
                  )}
                </span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export { CATEGORIES };
