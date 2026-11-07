import type { ToolType } from '../store/uiStore';
import type { Config } from '../types/config';
import { isNavigationTool } from './toolGroups';

/**
 * Whether the Pan/Zoom tools are available (#409). Off unless the user opts in:
 * the map pans on a plain drag and zooms on the wheel under every tool, so the
 * tools are redundant for anyone who can do both.
 *
 * `config` is null until `loadConfig()` resolves, and false is the right answer
 * there for anything that only SHOWS the tools. Anything that RESETS state off
 * the answer must first check that config is non-null, or it will fight a user
 * whose saved setting is on (see MapCanvas).
 */
export function navigationToolsEnabled(config: Config | null | undefined): boolean {
  return config?.map.controls.show_navigation_tools ?? false;
}

/**
 * Whether the active tool must be kicked back to Select because the navigate
 * group was turned off (#409). This is the RESET case navigationToolsEnabled's
 * doc warns about: it must stay false while config is still null (boot), or the
 * reset would fight a user whose SAVED setting is on by kicking them off their
 * restored Pan/Zoom tool before the real config arrives. Pure so that null-config
 * semantics can be pinned by a unit test; MapCanvas's effect is just this plus
 * `setActiveTool('select')`.
 */
export function shouldResetNavigationTool(
  config: Config | null | undefined,
  activeTool: ToolType
): boolean {
  return config != null && !navigationToolsEnabled(config) && isNavigationTool(activeTool);
}

/**
 * Tools that should not allow feature interactions (clicks, hovers, context menus).
 * When these tools are active, users should only be able to perform the tool's
 * primary action (pan/zoom) without accidentally selecting or interacting with features.
 */
export const NON_INTERACTIVE_TOOLS: ToolType[] = ['pan', 'zoom'];

/**
 * Check if a tool is non-interactive (should disable feature interactions).
 */
export function isNonInteractiveTool(tool: ToolType): boolean {
  return NON_INTERACTIVE_TOOLS.includes(tool);
}

/**
 * Tools that never mutate survey geometry (safe in the read-only
 * closure-adjusted view). Everything else (merge, move_*, comment,
 * move_comment) edits the underlying measured data and is disabled
 * while the closure-adjusted view is active.
 */
export const READ_ONLY_TOOLS: ToolType[] = ['select', 'pan', 'zoom', 'measure'];

/** Whether a tool mutates geometry and must be disabled in the adjusted view. */
export function isMutatingTool(tool: ToolType): boolean {
  return !READ_ONLY_TOOLS.includes(tool);
}

/**
 * The RESTING map cursor for a tool - what the canvas shows while the tool is
 * active and nothing is hovered.
 *
 * Single owner on purpose (#316). Three copies of this table used to live in
 * MapCanvas and had drifted: two still named tools that no longer exist
 * (`draw`, `delete`, `move_section`) and they disagreed on `merge`
 * (`cell` vs `crosshair`) and on the move tools (`move` vs `default`). Any
 * writer that guesses wrong re-opens the class of bug where one cursor writer
 * silently clobbers another's.
 *
 * This is the tool's resting cursor only. A hovered feature's `pointer`, Move
 * Station/Shot's arrow fallback, the Ctrl-held pan `grab`, a running modifier
 * pan and placement mode all outrank it; `components/Map/mapCursor.ts` weighs
 * them and is the only writer of the canvas cursor (#576 D3).
 *
 * The switch is exhaustive over `ToolType`: adding a tool without giving it a
 * cursor is a compile error, not a silently blank canvas.
 */
export function toolCursor(tool: ToolType): string {
  switch (tool) {
    case 'select':
      return 'default';
    case 'pan':
      return 'grab';
    case 'zoom':
      return 'zoom-in';
    case 'merge':
      return 'cell';
    case 'measure':
      return 'crosshair';
    case 'move_station':
    case 'move_segment':
    case 'move_shot':
      return 'move';
    case 'comment':
    case 'move_comment':
      return 'crosshair';
    default: {
      // The `never` binding makes a new ToolType without a cursor a COMPILE
      // error here rather than a silently blank canvas; at runtime a value that
      // reached us from outside the type system falls back to the arrow.
      const _exhaustive: never = tool;
      void _exhaustive;
      return 'default';
    }
  }
}
