import type { RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { ResetButton } from '../../common/ResetButton';
import { useColorScaleStore } from '../../../store/colorScaleStore';
import {
  setPalette,
  togglePinnedPalette,
  useEffectivePalette,
  usePersistedPalette,
  usePinnedPalettes,
} from '../../../store/colorScalePalette';
import { DEFAULT_PALETTE_CHOICE, THEME_PALETTE_ID, type ColorScaleKind, type LevelsScale } from '../../../utils/palettes';
import { colorScaleInputOrTheme } from './useColorScaleInputs';
import { LevelsSlider } from './LevelsSlider';
import { PalettePicker } from './PalettePicker';

export interface ColorScaleEditorProps {
  kind: ColorScaleKind;
  /** The data's extent for this kind (the auto range and slider domain). */
  domain: { min: number; max: number };
  histogram: readonly number[] | null;
  /** The range this editor edits (map or export), null = auto. */
  scale: LevelsScale | null;
  /** `live` while a Levels handle is mid-drag. */
  onScaleChange: (scale: LevelsScale | null, live: boolean) => void;
  precision: number;
  testIdPrefix: string;
  /** Where the palette list renders (see PalettePicker `portalInto`). */
  pickerPortalInto?: RefObject<HTMLElement | null> | undefined;
}

/**
 * The palette row (picker, reverse, reset) and the Levels row (histogram
 * slider, reset) for one kind. Shared by the map legend's cog popover and the
 * Export panel; the palette is the same persisted choice in both, the range is
 * whichever one the caller passes.
 */
export function ColorScaleEditor({
  kind,
  domain,
  histogram,
  scale,
  onScaleChange,
  precision,
  testIdPrefix,
  pickerPortalInto,
}: ColorScaleEditorProps) {
  const { t } = useTranslation();
  const persisted = usePersistedPalette(kind);
  const effective = useEffectivePalette(kind);
  const pinned = usePinnedPalettes();
  const setPalettePreview = useColorScaleStore((s) => s.setPalettePreview);
  const paletteIsDefault = persisted.id === THEME_PALETTE_ID && !persisted.reversed;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-col gap-1">
        <span className="text-[11px] font-medium text-text-secondary">{t('map.colorScale.palette')}</span>
        <div className="flex items-center gap-1">
          <PalettePicker
            kind={kind}
            value={persisted}
            pinned={pinned}
            onPreview={(choice) => setPalettePreview(choice ? { kind, choice } : null)}
            onSelect={(choice) => void setPalette(kind, choice)}
            onTogglePin={(id) => void togglePinnedPalette(id)}
            testIdPrefix={`${testIdPrefix}-palette`}
            portalInto={pickerPortalInto}
          />
          <button
            type="button"
            aria-pressed={persisted.reversed}
            aria-label={t('map.colorScale.reverseAria')}
            title={t('map.colorScale.reverseAria')}
            onClick={() => void setPalette(kind, { ...persisted, reversed: !persisted.reversed })}
            className={`px-1.5 py-1 rounded border text-[11px] ${
              persisted.reversed
                ? 'border-accent bg-accent-20 text-text-primary'
                : 'border-border text-text-secondary hover:text-text-primary'
            }`}
            data-testid={`${testIdPrefix}-reverse`}
          >
            {t('map.colorScale.reverse')}
          </button>
          <ResetButton
            onReset={() => void setPalette(kind, DEFAULT_PALETTE_CHOICE)}
            isDefault={paletteIsDefault}
            title={t('map.colorScale.resetPalette')}
            data-testid={`${testIdPrefix}-palette-reset`}
          />
        </div>
      </div>
      <div className="flex flex-col gap-1">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-medium text-text-secondary">{t('map.colorScale.range')}</span>
          <ResetButton
            onReset={() => onScaleChange(null, false)}
            isDefault={scale === null}
            title={t('map.colorScale.resetRange')}
            data-testid={`${testIdPrefix}-range-reset`}
          />
        </div>
        <LevelsSlider
          domain={domain}
          scale={scale}
          colorScale={colorScaleInputOrTheme(kind, effective, scale)}
          histogram={histogram}
          unit={t('common.units.meters')}
          precision={precision}
          onChange={(next, live) => onScaleChange(next, live)}
          histogramLabel={t('map.colorScale.histogram', { kind: t(`map.colorScale.kind.${kind}`) })}
          testIdPrefix={`${testIdPrefix}-levels`}
        />
      </div>
    </div>
  );
}
