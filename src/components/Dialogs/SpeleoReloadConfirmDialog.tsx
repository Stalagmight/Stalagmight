import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { answerSpeleoReloadConfirm, useSpeleoReloadConfirm } from '../../utils/speleoReloadConfirm';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * In-app confirmation for SpeleoDB "Reload project" when the open project has
 * unsaved or un-uploaded work: reloading discards it. Driven by
 * `confirmSpeleoReloadDiscard()`; mounted once in App, outside the dialog slot.
 */
export function SpeleoReloadConfirmDialog() {
  const { t } = useTranslation();
  const pending = useSpeleoReloadConfirm((s) => s.pending);
  const path = useProjectStore((s) => s.project?.file_path);
  const stale = pending !== null && pending.path !== path;
  // The question was about a project that is no longer open: withdraw it (keep).
  useEffect(() => {
    if (stale) answerSpeleoReloadConfirm(false);
  }, [stale]);
  if (!pending || stale) return null;
  const cancel = () => answerSpeleoReloadConfirm(false);
  return (
    <DialogWrapper title={t('speleodb.reloadConfirm.title')} onClose={cancel} size="md" data-testid="speleodb-reload-confirm-dialog">
      <div className="space-y-4">
        <p className="text-sm text-text-secondary" data-testid="speleodb-reload-confirm-body">
          {t('speleodb.reloadConfirm.body')}
        </p>
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          {/* Cancel is the safe answer, so it takes the initial focus. */}
          <Button onClick={cancel} data-primary-action="" data-testid="speleodb-reload-confirm-cancel">
            {t('speleodb.reloadConfirm.cancel')}
          </Button>
          <Button variant="danger" onClick={() => answerSpeleoReloadConfirm(true)} data-testid="speleodb-reload-confirm-discard">
            {t('speleodb.reloadConfirm.discard')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
