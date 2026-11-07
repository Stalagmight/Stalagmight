import { useProjectStore } from './projectStore';

/**
 * [#576 B15 follow-up] The stored project settings the open could not read, and
 * which of them the user can set again in Project Settings.
 *
 * While any is unread the backend refuses every save, so the default standing in
 * for it is never written over the stored value. Setting it again in Project
 * Settings makes the value in memory the user's own: the backend then takes that
 * key off its list (`AppState::lift_unreadable_project_setting`), and this mirrors
 * it in the store so the banner and autosave follow without a round trip. All three
 * keys the open can leave unread have a control there since the default north
 * reference got one (#576 NF-W3-2); a key outside this list clears only on reopen.
 */
export const SETTABLE_UNREADABLE_SETTINGS = ['validation', 'surface_elevation', 'default_azimuth_reference'] as const;

/** Whether `key` is still unread (and so still refusing saves). */
export function isSettingUnreadable(key: string): boolean {
  return useProjectStore.getState().unreadableProjectSettings.includes(key);
}

/** The user set `key` again and the backend accepted it: it no longer refuses saves. */
export function liftUnreadableSetting(key: string): void {
  useProjectStore.setState((s) => ({
    unreadableProjectSettings: s.unreadableProjectSettings.filter((k) => k !== key),
  }));
}
