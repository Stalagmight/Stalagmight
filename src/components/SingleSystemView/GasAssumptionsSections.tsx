import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  pressureFromDisplay,
  pressureToDisplay,
  rmvFromDisplay,
  rmvToDisplay,
  speedFromDisplay,
  speedToDisplay,
} from '../../utils/formatGas';
import { PositiveNumberField } from '../common/PositiveNumberField';
import type { Diver, GasConfig, GasUnits, ReservePreset } from '../../types/ipc.generated';

export const RESERVE_PRESETS: readonly ReservePreset[] = [
  'all_thirds',
  'one_stage',
  'two_stage',
  'free',
];
export const UNIT_OPTIONS: readonly GasUnits[] = ['metric', 'imperial'];

/** Stage cylinders a strategy requires in the rig - mirrors the Rust `required_stages`
 *  (gas.rs). The Half-Minus strategies are greyed until the diver carries that many stages;
 *  Free (#245) needs none (the diver sets each tank's usable gas by hand). */
export const REQUIRED_STAGES: Record<ReservePreset, number> = {
  all_thirds: 0,
  one_stage: 1,
  two_stage: 2,
  free: 0,
};

function SectionLabel({ children }: { children: ReactNode }) {
  return <h3 className="text-xs font-semibold uppercase tracking-wide opacity-60">{children}</h3>;
}

export interface GasAssumptionsSectionsProps {
  units: GasUnits;
  diver: Diver;
  gasConfig: GasConfig;
  setDiver: (diver: Diver) => void;
  setGasConfig: (gasConfig: GasConfig) => void;
  persistSwimSpeed: (v: number) => void;
  stageCount: number;
  strategy: ReservePreset;
  isStaged: boolean;
  reserveBar: number;
  safetyMarginBar: number;
  pendingPushy: ReservePreset | null;
  selectPreset: (preset: ReservePreset) => void;
  confirmPushy: () => void;
  setPendingPushy: (preset: ReservePreset | null) => void;
  setReserveBar: (n: number) => void;
  setSafetyMargin: (n: number) => void;
}

/**
 * The four inputs the R9/R10 entry card's "Adjust assumptions" disclosure reveals, and that the
 * full rig renders unconditionally once an origin is chosen (#395): Diver RMV, Swim speed, Gas
 * strategy and Units. Shared JSX so neither render branch in `GasPlanPanel` duplicates the
 * inputs - only the surrounding chrome (entry card vs. always-visible sidebar section) differs.
 */
export function GasAssumptionsSections({
  units,
  diver,
  gasConfig,
  setDiver,
  setGasConfig,
  persistSwimSpeed,
  stageCount,
  strategy,
  isStaged,
  reserveBar,
  safetyMarginBar,
  pendingPushy,
  selectPreset,
  confirmPushy,
  setPendingPushy,
  setReserveBar,
  setSafetyMargin,
}: GasAssumptionsSectionsProps) {
  const { t } = useTranslation();

  return (
    <>
      {/* Diver RMV */}
      <section className="space-y-1">
        <SectionLabel>{t('gasPanel.diver.rmvLabel')}</SectionLabel>
        <div className="flex items-center gap-2">
          <PositiveNumberField
            value={rmvToDisplay(diver.rmv_l_per_min, units)}
            onCommit={(n) => setDiver({ ...diver, rmv_l_per_min: rmvFromDisplay(n, units) })}
            step={units === 'imperial' ? 0.05 : 1}
            displayPrecision={units === 'imperial' ? 2 : 1}
            className="w-24 bg-transparent border border-border rounded px-2 py-1"
            testId="gas-rmv-input"
          />
          <span className="text-xs opacity-70">
            {t(units === 'imperial' ? 'gasPanel.diver.rmvUnitImperial' : 'gasPanel.diver.rmvUnit')}
          </span>
        </div>
      </section>

      {/* Swim speed - the diver's pace; feeds the in-leg gas-to-reach computation. Unlike the
          panel's other (per-session) inputs, swim speed PERSISTS: a commit updates the working
          copy (so the overlay/route refetch immediately) AND writes the value back to the saved
          config - the same gas.swim_speed_m_per_min that Settings ▸ Gas edits - so a speed set
          while planning sticks across Plan exit / restart and stays in sync with Settings (#236).
          The disk write is debounced (see persistSwimSpeed) so rapid typing doesn't fire one
          config save per keystroke; only this field is written back, reserve / units / diver
          stay per-session (#241). */}
      <section className="space-y-1">
        <SectionLabel>{t('gasPanel.swimSpeed.label')}</SectionLabel>
        <div className="flex items-center gap-2">
          <PositiveNumberField
            value={speedToDisplay(gasConfig.swim_speed_m_per_min, units)}
            onCommit={(n) => {
              const v = speedFromDisplay(n, units);
              setGasConfig({ ...gasConfig, swim_speed_m_per_min: v });
              persistSwimSpeed(v);
            }}
            step={units === 'imperial' ? 1 : 0.5}
            displayPrecision={1}
            className="w-24 bg-transparent border border-border rounded px-2 py-1"
            testId="gas-swim-speed-input"
          />
          <span className="text-xs opacity-70">
            {t(units === 'imperial' ? 'gasPanel.swimSpeed.unitImperial' : 'gasPanel.swimSpeed.unit')}
          </span>
        </div>
      </section>

      {/* Gas strategy */}
      <section className="space-y-1">
        <SectionLabel>{t('gasPanel.reserve.label')}</SectionLabel>
        <div className="flex gap-1">
          {RESERVE_PRESETS.map((preset) => {
            const needs = REQUIRED_STAGES[preset];
            const disabled = stageCount < needs;
            return (
              <button
                key={preset}
                type="button"
                disabled={disabled}
                title={disabled ? t('gasPanel.reserve.needsStages', { count: needs }) : undefined}
                className={`text-xs px-2 py-1 rounded border disabled:opacity-40 disabled:cursor-not-allowed ${
                  gasConfig.default_reserve === preset
                    ? 'border-accent ring-1 ring-accent'
                    : 'border-border hover:bg-border'
                }`}
                onClick={() => selectPreset(preset)}
                data-testid={`gas-reserve-${preset}`}
              >
                {t(`gasPanel.reserve.${preset}`)}
              </button>
            );
          })}
        </div>
        {pendingPushy && (
          <div
            className="mt-1 p-2 rounded border border-accent text-xs space-y-1"
            data-testid="gas-pushy-ack"
          >
            <p className="font-semibold">{t('gasPanel.reserve.advancedTitle')}</p>
            <p className="opacity-80">{t('gasPanel.reserve.advancedBody')}</p>
            <div className="flex gap-2 pt-1">
              <button
                type="button"
                className="px-2 py-0.5 rounded border border-accent hover:bg-border"
                onClick={confirmPushy}
                data-testid="gas-pushy-ack-confirm"
              >
                {t('gasPanel.reserve.advancedConfirm')}
              </button>
              <button
                type="button"
                className="px-2 py-0.5 rounded border border-border hover:bg-border"
                onClick={() => setPendingPushy(null)}
                data-testid="gas-pushy-ack-cancel"
              >
                {t('gasPanel.reserve.advancedCancel')}
              </button>
            </div>
          </div>
        )}
        {isStaged && (
          <div className="flex items-center gap-2 pt-1">
            <span className="text-xs opacity-70">{t('gasPanel.reserve.stageReserveLabel')}</span>
            <PositiveNumberField
              value={pressureToDisplay(reserveBar, units)}
              onCommit={(n) => setReserveBar(pressureFromDisplay(n, units))}
              step={units === 'imperial' ? 5 : 1}
              displayPrecision={0}
              className="w-20 bg-transparent border border-border rounded px-2 py-1"
              testId="gas-stage-reserve-input"
            />
            <span className="text-xs opacity-70">
              {t(units === 'imperial' ? 'gasPanel.tanks.psi' : 'gasPanel.tanks.bar')}
            </span>
          </div>
        )}
        {strategy === 'all_thirds' && (
          <div className="flex items-center gap-2 pt-1" title={t('gasPanel.reserve.safetyMarginHint')}>
            <span className="text-xs opacity-70">{t('gasPanel.reserve.safetyMarginLabel')}</span>
            <PositiveNumberField
              value={pressureToDisplay(safetyMarginBar, units)}
              onCommit={(n) => setSafetyMargin(pressureFromDisplay(n, units))}
              step={units === 'imperial' ? 5 : 1}
              displayPrecision={0}
              allowZero
              className="w-20 bg-transparent border border-border rounded px-2 py-1"
              testId="gas-safety-margin-input"
            />
            <span className="text-xs opacity-70">
              {t(units === 'imperial' ? 'gasPanel.tanks.psi' : 'gasPanel.tanks.bar')}
            </span>
          </div>
        )}
      </section>

      {/* Units */}
      <section className="space-y-1">
        <SectionLabel>{t('gasPanel.units.label')}</SectionLabel>
        <div className="flex gap-1">
          {UNIT_OPTIONS.map((u) => (
            <button
              key={u}
              type="button"
              className={`text-xs px-2 py-1 rounded border ${
                units === u ? 'border-accent ring-1 ring-accent' : 'border-border hover:bg-border'
              }`}
              onClick={() => setGasConfig({ ...gasConfig, units: u })}
              data-testid={`gas-units-${u}`}
            >
              {t(`gasPanel.units.${u}`)}
            </button>
          ))}
        </div>
        <p className="text-xs opacity-60">{t('gasPanel.tanks.editHint')}</p>
      </section>
    </>
  );
}
