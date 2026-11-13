/**
 * Workspace-wide find shortcuts, live only while the Data workspace is the
 * active mode and nothing modal owns the screen:
 *   Ctrl/Cmd+F  focus the find box (opens find; selects its text)
 *   Ctrl/Cmd+H  open replace (focuses the replacement when there is a query)
 *   F3 / Shift+F3  next / previous match
 * Listens in the capture phase and stops the event, so the browser's own find
 * and any configurable shortcut bound to the same keys don't also fire. None
 * of the default bindings use them (defaultConfig `shortcuts`; F-keys can't be
 * bound at all).
 */
import { useEffect, useRef, type RefObject } from 'react';
import { useUIStore, isScreenBusy } from '../../../store/uiStore';
import { useDataWorkspaceStore } from '../store';

interface FindShortcutTargets {
  findInput: RefObject<HTMLInputElement>;
  replaceInput: RefObject<HTMLInputElement>;
  next: () => void;
  prev: () => void;
}

export type FindShortcut = 'find' | 'replace' | 'next' | 'prev';

/** Which find shortcut a key event is, if any. */
export function findShortcutOf(e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>): FindShortcut | null {
  const mod = e.ctrlKey || e.metaKey;
  if (e.key === 'F3' && !mod && !e.altKey) return e.shiftKey ? 'prev' : 'next';
  if (!mod || e.altKey || e.shiftKey) return null;
  const k = e.key.toLowerCase();
  if (k === 'f') return 'find';
  if (k === 'h') return 'replace';
  return null;
}

/** Focus an input once React has rendered it (the replace row may be mounting). */
function focusSoon(ref: RefObject<HTMLInputElement>, select: boolean): void {
  const go = () => {
    const el = ref.current;
    if (!el) return false;
    el.focus();
    if (select) el.select();
    return true;
  };
  if (!go()) requestAnimationFrame(() => void go());
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
          store.setSearch({ open: true });
          focusSoon(findInput, true);
          break;
        case 'replace': {
          store.setSearch({ open: true, replaceOpen: true });
          focusSoon(store.search.query === '' ? findInput : replaceInput, true);
          break;
        }
        case 'next':
          next();
          break;
        case 'prev':
          prev();
          break;
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);
}
