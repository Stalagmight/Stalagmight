/**
 * "Where does this survey go?" - the Ariane import's placement step (#425).
 *
 * Before this, a file with no georeference was placed at the origin in silence
 * (`determine_placement`, tml/import_options.rs), and a file WITH one had its
 * coordinates applied without ever being shown. Both are now a visible choice.
 *
 * Deliberately NOT the MNemo wizard's manual-placement engine. This dialog only
 * ever opens on a brand-new, empty project (see `openOrImport.ts` - Ariane is a
 * new-project-only format, and an open project routes through
 * `import-as-new-project` first), so there is no existing station to attach to
 * and nothing on the map to drag against. What is left is the datum, which is
 * three radio buttons and two number fields.
 */
import { useTranslation } from 'react-i18next';
import type { TmlRootPlacement } from '../../types/commands';

/** Where the coordinates on offer came from, so the option can say so. */
export interface TmlFileAnchor {
  latitude: number;
  longitude: number;
  /** `root` - the first imported section's own first shot carries them, which
   *  is what the importer would have used anyway. `elsewhere` - the file has a
   *  georeference but on some other section, which the importer ignores. */
  source: 'root' | 'elsewhere';
}

export type TmlPlacementChoice = 'file' | 'origin' | 'manual';

export interface TmlPlacementState {
  choice: TmlPlacementChoice;
  /** Raw text, not numbers: an in-progress "-" or "" must not read as 0. */
  latitude: string;
  longitude: string;
}

export const EMPTY_PLACEMENT: TmlPlacementState = {
  choice: 'origin',
  latitude: '',
  longitude: '',
};

/** A finite coordinate inside its range, or `null`. Rejects `""` and `"-"`
 *  (which `Number` maps to 0 and NaN respectively) explicitly. */
function parseCoordinate(text: string, limit: number): number | null {
  const trimmed = text.trim();
  if (trimmed === '') return null;
  const value = Number(trimmed);
  if (!Number.isFinite(value) || Math.abs(value) > limit) return null;
  return value;
}

/**
 * The payload for `execute_tml_import`, or `null` when the user's choice is not
 * yet answerable (manual coordinates that do not parse). A `null` return is the
 * Import gate's cue to stay disabled - it is NOT the same as sending `null` to
 * the backend, which would mean "decide for me".
 */
export function placementPayload(
  state: TmlPlacementState,
  anchor: TmlFileAnchor | null
): TmlRootPlacement | null {
  if (state.choice === 'origin') return { type: 'origin' };
  if (state.choice === 'file') {
    return anchor
      ? { type: 'zero_datum', latitude: anchor.latitude, longitude: anchor.longitude }
      : null;
  }
  const latitude = parseCoordinate(state.latitude, 90);
  const longitude = parseCoordinate(state.longitude, 180);
  if (latitude === null || longitude === null) return null;
  return { type: 'zero_datum', latitude, longitude };
}

export interface TmlPlacementStepProps {
  state: TmlPlacementState;
  onChange: (next: TmlPlacementState) => void;
  /** The coordinates the file offers, when it offers any. */
  anchor: TmlFileAnchor | null;
  /** Name of the section this placement applies to, so the user can see that
   *  deselecting sections moved it. */
  rootSectionName: string | null;
  disabled: boolean;
}

export function TmlPlacementStep({
  state,
  onChange,
  anchor,
  rootSectionName,
  disabled,
}: TmlPlacementStepProps) {
  const { t } = useTranslation();
  const manualInvalid =
    state.choice === 'manual' && placementPayload(state, anchor) === null;

  const option = (choice: TmlPlacementChoice, label: string, hint?: string) => (
    <label className="flex items-start gap-2 text-sm cursor-pointer">
      <input
        type="radio"
        name="tml-placement"
        className="mt-0.5"
        checked={state.choice === choice}
        disabled={disabled}
        onChange={() => onChange({ ...state, choice })}
        data-testid={`tml-placement-${choice}`}
      />
      <span>
        <span className="text-text-primary">{label}</span>
        {hint && <span className="block text-xs text-text-tertiary">{hint}</span>}
      </span>
    </label>
  );

  return (
    <div className="space-y-2" data-testid="tml-placement-step">
      <div className="text-sm font-medium">{t('dialogs.tmlImport.placement.heading')}</div>
      <div className="text-xs text-text-tertiary">
        {rootSectionName
          ? t('dialogs.tmlImport.placement.appliesTo', { section: rootSectionName })
          : t('dialogs.tmlImport.placement.appliesToNothing')}
      </div>
      <div className="space-y-2 p-3 border border-border rounded">
        {anchor &&
          option(
            'file',
            t('dialogs.tmlImport.placement.useFile'),
            t(
              anchor.source === 'root'
                ? 'dialogs.tmlImport.placement.useFileHint'
                : 'dialogs.tmlImport.placement.useFileElsewhereHint',
              {
                latitude: anchor.latitude.toFixed(6),
                longitude: anchor.longitude.toFixed(6),
              }
            )
          )}
        {option(
          'origin',
          t('dialogs.tmlImport.placement.origin'),
          t('dialogs.tmlImport.placement.originHint')
        )}
        {option('manual', t('dialogs.tmlImport.placement.manual'))}
        {state.choice === 'manual' && (
          <div className="flex gap-2 pl-6">
            <label className="text-xs text-text-secondary">
              {t('dialogs.tmlImport.placement.latitude')}
              <input
                type="text"
                inputMode="decimal"
                value={state.latitude}
                disabled={disabled}
                onChange={(e) => onChange({ ...state, latitude: e.target.value })}
                className="block w-32 px-2 py-1 bg-background border border-border rounded text-sm"
                data-testid="tml-placement-latitude"
              />
            </label>
            <label className="text-xs text-text-secondary">
              {t('dialogs.tmlImport.placement.longitude')}
              <input
                type="text"
                inputMode="decimal"
                value={state.longitude}
                disabled={disabled}
                onChange={(e) => onChange({ ...state, longitude: e.target.value })}
                className="block w-32 px-2 py-1 bg-background border border-border rounded text-sm"
                data-testid="tml-placement-longitude"
              />
            </label>
          </div>
        )}
        {manualInvalid && (
          <div className="text-xs text-error pl-6" data-testid="tml-placement-error">
            {t('dialogs.tmlImport.placement.invalid')}
          </div>
        )}
      </div>
    </div>
  );
}
