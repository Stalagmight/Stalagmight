import type { Config } from '../../../types/config';
import type { SettingsCategory } from '../SettingsSidebar';

/**
 * Common props for all settings category content components.
 */
export interface SettingsCategoryProps {
  /** Current config values (local state) */
  config: Config;
  saveBeforeConnect?: () => Promise<boolean>;
  onConnectionBusyChange?: (busy: boolean) => void;
  /** Callback to update config values */
  onChange: (updates: Partial<Config>) => void;
  /** Set of changed field paths (e.g., 'general.auto_save') for highlighting */
  changedFields: Set<string>;
  /** Report validation error state for this category */
  onValidationError: (category: SettingsCategory, hasError: boolean) => void;
  /** Base ID for aria-labelledby association with sidebar tab */
  panelIdBase: string;
}

/**
 * Helper to check if a field path has been changed.
 */
export function isFieldChanged(changedFields: Set<string>, ...paths: string[]): boolean {
  return paths.some((path) => changedFields.has(path));
}

/**
 * Read a nested value from an object by dot-separated path.
 * e.g. getByPath(config, 'general.auto_save') => config.general.auto_save
 */
export function getByPath(obj: unknown, path: string): unknown {
  let current = obj;
  for (const key of path.split('.')) {
    if (current == null || typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[key];
  }
  return current;
}

// ---------------------------------------------------------------------------
// Config handler factories
// ---------------------------------------------------------------------------
// These reduce boilerplate in settings components where dozens of handlers
// follow the same parse-then-patch pattern.

type OnChange = SettingsCategoryProps['onChange'];

/**
 * Create a numeric input handler that parses string -> number and calls onChange.
 * @param onChange  The settings onChange callback
 * @param updater  Builds the config patch from the parsed number
 * @param parser   parseInt or parseFloat (default: parseFloat)
 */
export function createNumericHandler(
  onChange: OnChange,
  updater: (num: number) => Partial<Config>,
  parser: (value: string) => number = parseFloat,
): (value: string) => void {
  return (value: string) => {
    const num = parser(value);
    if (!isNaN(num)) {
      onChange(updater(num));
    }
  };
}

/**
 * Create a boolean toggle handler.
 */
export function createToggleHandler(
  onChange: OnChange,
  updater: (checked: boolean) => Partial<Config>,
): (checked: boolean) => void {
  return (checked: boolean) => {
    onChange(updater(checked));
  };
}

/**
 * Create a string select / text handler.
 */
export function createSelectHandler(
  onChange: OnChange,
  updater: (value: string) => Partial<Config>,
): (value: string) => void {
  return (value: string) => {
    onChange(updater(value));
  };
}

/** parseInt wrapper suitable as a `parser` argument. */
export const parseIntRadix10 = (v: string): number => parseInt(v, 10);
