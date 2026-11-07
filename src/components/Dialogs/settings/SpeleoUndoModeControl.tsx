import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useSpeleoStore } from '../../../store/speleoStore';
import { humanizeSpeleoError } from '../../../utils/speleoError';
import { isProjectChangedRefusal } from '../../../store/commandResult';
import type { UndoMode } from '../../../types/ipc.generated';

/**
 * [#303] The open SpeleoDB project's undo-history policy: Full (each revision stores the
 * operations that produced it, so undo chains back through the project's history) or
 * Disabled (undo works locally but is stripped before every push).
 *
 * It lives in this dialog rather than in the application Settings because it is a
 * PROJECT setting: the value is written into the `.stalx` and rides to the server on the
 * next push, so it becomes the policy for everyone who edits the project. That is also
 * why it needs an edit session - the same gate every other edit to the project file goes
 * through, including force editing - and why the control says so instead of just being
 * greyed out.
 *
 * The panel around it edits SERVER-side metadata; this one field does not, which the
 * copy is explicit about. It is here anyway because this is the dialog a user opens to
 * ask "how is this project set up".
 */
export function SpeleoUndoModeControl() {
  const { t } = useTranslation();
  const mode = useSpeleoStore((s) => s.link?.undoMode ?? 'full');
  // Force editing deliberately uses the same edit gate as ordinary commands. It carries
  // conflict risk by design, but the user already accepted that responsibility when they
  // entered the mode, so this project-file setting remains available there too.
  const canEdit = useSpeleoStore((s) => s.link?.editState === 'editing');
  const setUndoMode = useSpeleoStore((s) => s.setUndoMode);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const apply = async (next: UndoMode) => {
    setBusy(true);
    setError(null);
    try {
      await setUndoMode(next);
    } catch (e) {
      // [#576 Q-W44-7-D1] The undo-mode write is a background verb: a refusal because
      // the project changed is dropped, nothing shown.
      if (isProjectChangedRefusal(e)) return;
      setError(humanizeSpeleoError(e, t('speleodb.undoMode.failed')));
    } finally {
      setBusy(false);
    }
  };

  const option = (value: UndoMode, label: string, hint: string) => {
    const selected = mode === value;
    return (
      <label
        key={value}
        className={`flex gap-2 items-start rounded border px-2 py-1.5 cursor-pointer
          ${selected ? 'border-accent bg-accent-10' : 'border-border hover:bg-background-50'}
          ${canEdit && !busy ? '' : 'opacity-60 cursor-not-allowed'}`}
        data-testid={`speleodb-undo-mode-${value}`}
      >
        <input
          type="radio"
          name="speleodb-undo-mode"
          className="mt-1"
          checked={selected}
          disabled={!canEdit || busy}
          onChange={() => void apply(value)}
          data-testid={`speleodb-undo-mode-${value}-input`}
        />
        <span className="min-w-0">
          <span className="block text-sm text-text-primary">{label}</span>
          <span className="block text-xs text-text-secondary">{hint}</span>
        </span>
      </label>
    );
  };

  return (
    <div className="py-2 border-t border-border" data-testid="speleodb-undo-mode">
      <span className="block text-sm font-medium text-text-primary mb-1">
        {t('speleodb.undoMode.title')}
      </span>
      <p className="text-xs text-text-secondary pb-2">{t('speleodb.undoMode.description')}</p>

      <div className="space-y-1.5">
        {option('full', t('speleodb.undoMode.full'), t('speleodb.undoMode.fullHint'))}
        {option('disabled', t('speleodb.undoMode.disabled'), t('speleodb.undoMode.disabledHint'))}
      </div>

      {!canEdit && (
        <p className="text-xs text-warning pt-1" data-testid="speleodb-undo-mode-lock-hint">
          {t('speleodb.undoMode.lockRequired')}
        </p>
      )}
      {error && (
        <p className="text-xs text-error pt-1" data-testid="speleodb-undo-mode-error">
          {error}
        </p>
      )}
    </div>
  );
}
