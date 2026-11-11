import { useProjectStore } from '../../store/projectStore';
import { isUnwoundNotEdited, useSpeleoStore } from '../../store/speleoStore';
import { LockIcon, SectionsIcon } from '../common/icons';
import { SpeleoBadgedIcon } from '../common/SpeleoBadgedIcon';

/**
 * What the SpeleoDB tab has to tell at a glance about the open project:
 * - `unuploaded`: changes SpeleoDB does not have yet - unsaved edits, or saved
 *   ones not uploaded. Not raised for a copy that is only stepped back to an
 *   older version without edits (#303): uploading that would publish a revert.
 * - `read-only`: no write lock held.
 * - `normal`: neither (or a local project, which has no SpeleoDB state).
 * Un-uploaded work outranks read-only: it is the state that can lose work.
 */
export type SpeleoTabState = 'unuploaded' | 'read-only' | 'normal';

export function useSpeleoTabState(): SpeleoTabState {
  const link = useSpeleoStore((s) => s.link);
  const isDirty = useProjectStore((s) => s.isDirty);
  if (!link) return 'normal';
  if (isDirty || (link.hasUnpushedEdits && !isUnwoundNotEdited(link))) return 'unuploaded';
  return link.editState === 'editing' ? 'normal' : 'read-only';
}

const ICON_SIZE = 16;

export function SectionsTabIcon() {
  return <SectionsIcon size={ICON_SIZE} data-testid="workspace-tab-sections-icon" />;
}

/**
 * The SpeleoDB mark, badged with a lock while read-only. Un-uploaded changes get
 * no badge: they are not an error, so the mark simply turns yellow with the tab's
 * text. The mark paints in `currentColor` (no tint and no colour class of its
 * own), so it always takes the tab's colour: the warning text at full strength
 * while the tab is selected, dimmed while it is not (see `LeftWorkspace`).
 */
export function SpeleoTabIcon({ state }: { state: SpeleoTabState }) {
  return (
    <span className="inline-flex" data-testid="workspace-tab-speleodb-icon" data-state={state}>
      <SpeleoBadgedIcon
        size={ICON_SIZE}
        testIdPrefix="workspace-tab-speleodb-icon"
        badge={state === 'read-only' ? <LockIcon size={10} /> : undefined}
        badgeColor="text-text-primary"
      />
    </span>
  );
}
