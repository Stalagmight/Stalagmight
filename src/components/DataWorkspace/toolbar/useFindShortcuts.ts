/**
 * Workspace-wide find shortcuts, live only while the Data workspace is the
 * active mode and nothing modal owns the screen:
 *   Ctrl/Cmd+F  focus the find box (opens find; selects its text)
 *   Ctrl/Cmd+H, or Ctrl/Cmd+Alt+F  open replace (focuses the replacement when
 *               there is a query; Cmd+Alt+F is the macOS one, where the app
 *               menu owns Cmd+H for Hide)
 *   F3 / Shift+F3  next / previous match (reopens a closed find on its query)
 * Listens in the capture phase and stops the event, so the browser's own find
 * and any configurable shortcut bound to the same keys don't also fire. None
 * of the default bindings use them (defaultConfig `shortcuts`; F-keys can't be
 * bound at all). Letters are read from `key`, falling back to the physical key
 * (`code`) on non-Latin layouts.
 */
import { useEffect, useRef, type RefObject } from 'react';
import { useUIStore, isScreenBusy } from '../../../store/uiStore';
import { useDataWorkspaceStore } from '../store';
import { isMac } from '../../../utils/shortcuts';

interface FindShortcutTargets {
  findInput: RefObject<HTMLInputElement>;
  replaceInput: RefObject<HTMLInputElement>;
  next: () => void;
  prev: () => void;
}

export type FindShortcut = 'find' | 'replace' | 'next' | 'prev';

/** Which find shortcut a key event is, if any. */
export function findShortcutOf(
  e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'> & { code?: string },
): FindShortcut | null {
  const mod = e.ctrlKey || e.metaKey;
  if (e.key === 'F3' && !mod && !e.altKey) return e.shiftKey ? 'prev' : 'next';
  if (!mod || e.shiftKey) return null;
  const latin = /^[a-z]$/i.test(e.key) ? e.key.toLowerCase() : null;
  const physical = e.code && /^Key[A-Z]$/.test(e.code) ? e.code.slice(3).toLowerCase() : null;
  // With Alt the character is layout noise (macOS Option+F is "ƒ"): use the key.
  const k = e.altKey ? physical : (latin ?? physical);
  if (e.altKey) return k === 'f' ? 'replace' : null;
  if (k === 'f') return 'find';
  if (k === 'h') return 'replace';
  return null;
}

/** i18n key of the "show replace" tooltip, naming this platform's shortcut. */
export function replaceShortcutKey(): string {
  return isMac() ? 'dataToolbar.find.openReplaceMac' : 'dataToolbar.find.openReplace';
}

/** Focus an input once React has rendered it (the replace row may still be
 *  mounting): now, or on one of the next few frames. */
function focusSoon(ref: RefObject<HTMLInputElement>, select: boolean, framesLeft = 10): void {
  const el = ref.current;
  if (el) {
    el.focus();
    if (select) el.select();
    return;
  }
  if (framesLeft > 0) requestAnimationFrame(() => focusSoon(ref, select, framesLeft - 1));
}

export function useFindShortcuts(targets: FindShortcutTargets): void {
  const ref = useRef(targets);
  ref.current = targets;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const which = findShortcutOf(e);
      if (!which) return;
      const ui = useUIStore.getState();
      if (ui.appMode !== 'data' || isScreenBusy(ui)) return;
      e.preventDefault();
      e.stopPropagation();
      const { findInput, replaceInput, next, prev } = ref.current;
      const store = useDataWorkspaceStore.getState();
      switch (which) {
        case 'find':
          if (!store.search.open) store.setSearch({ open: true });
          focusSoon(findInput, true);
          break;
        case 'replace': {
          if (!store.search.open || !store.search.replaceOpen) store.setSearch({ open: true, replaceOpen: true });
          focusSoon(store.search.query === '' ? findInput : replaceInput, true);
          break;
        }
        case 'next':
        case 'prev':
          // A closed find with a query reopens on it (the first match from the
          // active row is selected); otherwise step through the matches.
          if (!store.search.open && store.search.query !== '') store.setSearch({ open: true });
          else if (which === 'next') next();
          else prev();
          break;
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);
}
