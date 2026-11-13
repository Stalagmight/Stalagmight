/**
 * Row tint: None / Section colour / Survey issues / Depth / Distance / Row type
 * (the map's Display colour modes), each with a one-line description, and a key
 * for the active one.
 */
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { TINT_MODES, type DataModelCtx } from '../types';
import { TintIcon } from './toolbarIcons';
import { MenuDivider, ToolbarMenu } from './ToolbarMenu';
import { TintLegend } from './TintLegend';

export function TintMenu({ ctx }: { ctx: DataModelCtx }) {
  const { t } = useTranslation();
  const tintMode = useDataWorkspaceStore((s) => s.tintMode);
  const setTintMode = useDataWorkspaceStore((s) => s.setTintMode);
  const modeLabel = t(`dataToolbar.tint.short.${tintMode}`);

  return (
    <ToolbarMenu
      testId="data-tint-button"
      menuTestId="data-tint-menu"
      icon={<TintIcon size={14} />}
      label={tintMode === 'none' ? t('dataToolbar.tint.button') : t('dataToolbar.tint.buttonMode', { mode: modeLabel })}
      title={`${t('dataToolbar.tint.title')}: ${t(`dataToolbar.tint.modes.${tintMode}`)}`}
      ariaLabel={t('dataToolbar.tint.button')}
      width={300}
      remeasureKey={tintMode}
    >
      <div role="radiogroup" aria-label={t('dataToolbar.tint.title')}>
        {TINT_MODES.map((mode) => (
          <label
            key={mode}
            className={`flex cursor-pointer items-start gap-2 rounded px-2 py-1 hover:bg-accent-10 ${
              tintMode === mode ? 'bg-accent-10' : ''
            }`}
          >
            <input
              type="radio"
              className="mt-0.5"
              name="data-tint-mode"
              checked={tintMode === mode}
              onChange={() => setTintMode(mode)}
              data-testid={`data-tint-${mode}`}
            />
            <span className="flex flex-col">
              <span>{t(`dataToolbar.tint.modes.${mode}`)}</span>
              <span className="text-[11px] leading-4 text-text-secondary">{t(`dataToolbar.tint.descriptions.${mode}`)}</span>
            </span>
          </label>
        ))}
      </div>
      {tintMode !== 'none' && (
        <>
          <MenuDivider />
          <div className="px-2 pb-1.5 pt-0.5">
            <TintLegend mode={tintMode} ctx={ctx} />
          </div>
        </>
      )}
    </ToolbarMenu>
  );
}
