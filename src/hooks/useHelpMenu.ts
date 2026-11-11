import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { isScreenBusy, useUIStore } from '../store/uiStore';

/**
 * Event the macOS Help menu emits for "Keyboard Shortcuts" (#665). The sender is
 * `src-tauri/src/app_menu.rs`, which keeps the same name.
 */
export const SHOW_SHORTCUTS_EVENT = 'menu:keyboard-shortcuts';

/**
 * Opens the keyboard shortcuts dialog when the Help menu asks for it: not during
 * station placement or a full-screen data table, and never on top of another
 * dialog or an overlay waiting for an answer. A menu-bar click is outside the
 * webview, so no modal backdrop can stop it; the screen-busy check has to.
 */
export function useHelpMenu(): void {
  useEffect(() => {
    const unlisten = listen(SHOW_SHORTCUTS_EVENT, () => {
      const state = useUIStore.getState();
      if (isScreenBusy(state) || state.dataTableFullScreen) return;
      state.openDialog('keyboard-shortcuts');
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);
}
