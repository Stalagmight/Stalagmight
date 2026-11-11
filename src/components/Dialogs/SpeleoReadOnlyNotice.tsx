import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useSpeleoReadOnlyNotice } from '../../utils/autoAcquireSpeleoLock';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';

export function SpeleoReadOnlyNotice() {
  const { t } = useTranslation();
  const notice = useSpeleoReadOnlyNotice(s => s.notice);
  const path = useProjectStore(s => s.project?.file_path);
  const generation = useSpeleoStore(s => s.sessionGeneration);
  if (!notice || notice.path !== path || notice.sessionGeneration !== generation) return null;
  const dismiss = () => useSpeleoReadOnlyNotice.setState({ notice: null });
  return (
    <DialogWrapper title={t('speleodb.readOnlyNotice.title')} onClose={dismiss} size="sm" data-testid="speleodb-read-only-dialog">
      <p className="text-sm text-text-secondary mb-4">{notice.message}</p>
      <div className="flex justify-end">
        <Button variant="primary" onClick={dismiss} data-testid="speleodb-read-only-continue">
          {t('speleodb.readOnlyNotice.continue')}
        </Button>
      </div>
    </DialogWrapper>
  );
}
