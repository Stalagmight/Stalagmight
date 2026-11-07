import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { SettingRow } from './SettingRow';
import { Select } from '../../common/Select';
import type { AzimuthReference } from '../../../types/ipc/AzimuthReference';
import type { NorthReference } from '../../../types/ipc/NorthReference';

/** The control's value: one of the backend's north references, or nothing recorded. */
export type NorthChoice = NorthReference | 'none';

/** A correction (degrees east) a reference may carry: finite, within one turn. */
const MAX_CORRECTION = 180;

/**
 * The staged "Default north reference" of the Project Settings dialog (#576 NF-W3-2):
 * the reference a section that declares none inherits (`default_azimuth_reference`).
 *
 * Seeded from the project. The draft is compared against what it was seeded with, not
 * the stored number, so an untouched control hands back the STORED value - its
 * correction source (file-declared, a geomagnetic model) included - and never turns a
 * recorded reference into a hand-entered one. Anything the user changes is `manual`.
 */
export function useNorthReferenceDraft(stored: AzimuthReference | null | undefined) {
  const [north, setNorth] = useState<NorthChoice>('none');
  const [correction, setCorrection] = useState('');
  const [seeded, setSeeded] = useState<{ north: NorthChoice; correction: string }>({ north: 'none', correction: '' });
  // Re-seeded when the stored VALUE changes, not on every refresh that hands out a new
  // project object - that would throw away what the user is typing.
  const storedKey = JSON.stringify(stored ?? null);
  useEffect(() => {
    const value = JSON.parse(storedKey) as AzimuthReference | null;
    const next = value
      ? { north: value.north, correction: value.north === 'true' ? '' : String(value.correction) }
      : { north: 'none' as const, correction: '' };
    setNorth(next.north);
    setCorrection(next.correction);
    setSeeded(next);
  }, [storedKey]);

  const takesCorrection = north === 'magnetic' || north === 'grid';
  const parsed = correction.trim() === '' ? 0 : Number(correction);
  const valid = !takesCorrection || (Number.isFinite(parsed) && Math.abs(parsed) <= MAX_CORRECTION);
  const changed = north !== seeded.north || (takesCorrection && correction !== seeded.correction);
  const reference = useMemo((): AzimuthReference | null => {
    if (!changed) return stored ?? null;
    if (north === 'none') return null;
    return { north, correction: north === 'true' ? 0 : parsed, source: 'manual' };
  }, [changed, stored, north, parsed]);

  return { north, setNorth, correction, setCorrection, takesCorrection, valid, changed, reference };
}

type NorthReferenceDraft = ReturnType<typeof useNorthReferenceDraft>;

/** The General-tab row: which north, and for magnetic or grid north the correction. */
export function DefaultNorthReferenceRow({
  draft,
  disabledReason,
}: {
  draft: NorthReferenceDraft;
  /**
   * Why the row is disabled - shown under it - or null. It is a survey edit (#576
   * NF-W3-2-PLAN): disabled wherever editing is blocked, Plan and Export included.
   */
  disabledReason: string | null;
}) {
  const { t } = useTranslation();
  const disabled = disabledReason !== null;
  const options = (['none', 'magnetic', 'true', 'grid'] as const).map((value) => ({
    value,
    label: t(`dialogs.projectSettings.northReference.options.${value}`),
  }));
  return (
    <SettingRow
      label={t('dialogs.projectSettings.northReference.label')}
      helpText={t('dialogs.projectSettings.northReference.help')}
      data-testid="project-settings-north-reference-row"
    >
      <div className="flex flex-col items-end gap-1">
        <div className="flex items-center gap-2" title={disabledReason ?? undefined}>
          <Select
            value={draft.north}
            onChange={(value) => draft.setNorth(value as NorthChoice)}
            options={options}
            disabled={disabled}
            size="sm"
            className="w-40"
            aria-label={t('dialogs.projectSettings.northReference.label')}
            data-testid="project-settings-north-reference"
          />
          {draft.takesCorrection && (
            <>
              <input
                type="number"
                step="any"
                value={draft.correction}
                disabled={disabled}
                onChange={(e) => draft.setCorrection(e.target.value)}
                placeholder="0"
                className={`w-20 px-2 py-1 bg-background border rounded text-sm disabled:opacity-60 ${draft.valid ? 'border-border' : 'border-error'}`}
                aria-label={t('dialogs.projectSettings.northReference.correctionAria')}
                aria-invalid={!draft.valid}
                data-testid="project-settings-north-correction"
              />
              <span className="text-xs text-text-secondary">{t('dialogs.projectSettings.northReference.correctionUnit')}</span>
            </>
          )}
        </div>
        {disabledReason !== null && (
          <p className="max-w-[14rem] text-right text-xs text-text-secondary" data-testid="project-settings-north-reference-blocked">
            {disabledReason}
          </p>
        )}
      </div>
    </SettingRow>
  );
}
