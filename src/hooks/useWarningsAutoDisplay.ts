import { useEffect } from 'react';
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';

/**
 * Mirror the "Highlight warnings on tab switch" setting
 * (`config.map.highlight_warnings_on_tab_switch`, default on) into the UI store,
 * where the Warnings-tab visit selectors read it (`displaySelectors`). Call once
 * at the top of the App component.
 */
export function useWarningsAutoDisplay(): void {
  const on = useProjectStore((s) => s.config?.map.highlight_warnings_on_tab_switch ?? true);
  const setWarningsAutoDisplay = useUIStore((s) => s.setWarningsAutoDisplay);
  useEffect(() => {
    setWarningsAutoDisplay(on);
  }, [on, setWarningsAutoDisplay]);
}
