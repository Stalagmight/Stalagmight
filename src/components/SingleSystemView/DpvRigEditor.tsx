import { useEffect, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { PositiveNumberField } from '../common/PositiveNumberField';
import { Toggle } from '../common/Toggle';
import { formatGasLength, speedFromDisplay, speedToDisplay } from '../../utils/formatGas';
import { MAX_DPV_RANGE_RESERVE_FRACTION, dpvUsableRangeM, makeDefaultDpvRig } from '../../utils/dpv';
import { flushGasConfigWrite, queueGasConfigWrite } from '../../utils/gasConfigDisk';
import type { DpvRig, DpvUnit } from '../../types/ipc.generated';

/**
 * The optional DPV (scooter) rig editor (#317) - the "DPV" tab of the Plan-mode bottom
 * panel. Moved out of Settings so the rig lives alongside the rest of the dive's equipment
 * rather than in a separate dialog. Every commit updates both
 * the working `gasConfig` (via `setDpvRig`, which also resets scootered segments and nulls the
 * cached reach field so the map overlay refetches) and, through `queueGasConfigWrite`, the
 * persisted config on disk. The disk half MUST be debounced: `PositiveNumberField` commits on
 * every valid keystroke, not on blur, so a direct write would fire one `update_config` IPC +
 * config reload + LSQ-cache drop per digit typed (the same reason the swim-speed field
 * debounces, #241). `PositiveNumberField` blocks a non-positive speed/burn entry at the field
 * itself (it reverts to the last valid value), so an invalid rig cannot be constructed here -
 * Settings used to police this with a Save-blocking validation effect; that effect (and its
 * rig-specific checks) is gone along with the rig UI.
 */
export function DpvRigEditor() {
  const { t } = useTranslation();
  const { gasConfig, setDpvRig } = useGasPlanStore(
    useShallow((s) => ({
      gasConfig: s.gasConfig,
      setDpvRig: s.setDpvRig,
    }))
  );
  const units = gasConfig.units;
  const rig = gasConfig.dpv;

  // Flush any still-debounced rig write on unmount, so a value typed right before switching
  // tabs or modes still reaches disk (same contract as the swim-speed field's flush).
  useEffect(
    () => () => {
      void flushGasConfigWrite();
    },
    []
  );

  const applyRig = (next: DpvRig | null) => {
    setDpvRig(next);
    queueGasConfigWrite({ dpv: next });
  };
  const setRig = (patch: Partial<DpvRig>) => {
    if (!rig) return;
    applyRig({ ...rig, ...patch });
  };
  const setPrimary = (patch: Partial<DpvUnit>) => {
    if (!rig) return;
    setRig({ primary: { ...rig.primary, ...patch } });
  };
  const setBackup = (patch: Partial<DpvUnit>) => {
    if (!rig) return;
    setRig({ backup: { ...rig.backup, ...patch } });
  };

  return (
    // Plain padding, no top margin/rule: this is the whole "DPV" tab now, and a border-t at
    // the very top of the pane would just double the tab strip's own bottom border.
    <div className="p-3 text-sm text-text-primary" data-testid="dpv-rig-editor">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-xs uppercase tracking-wide opacity-60 font-semibold">
          {t('gasPanel.dpv.title')}
        </h3>
        <Toggle
          checked={rig != null}
          onChange={(v) => applyRig(v ? makeDefaultDpvRig() : null)}
          aria-label={t('gasPanel.dpv.enabled.aria')}
          data-testid="toggle-dpv-enabled"
        />
      </div>

      {rig && (
        <div className="space-y-1" data-testid="dpv-rig-fields">
          <Row
            label={t(
              units === 'imperial'
                ? 'gasPanel.dpv.primarySpeed.labelImperial'
                : 'gasPanel.dpv.primarySpeed.label'
            )}
            help={t('gasPanel.dpv.primarySpeed.help')}
          >
            <PositiveNumberField
              value={speedToDisplay(rig.primary.speed_m_per_min, units)}
              onCommit={(n) => setPrimary({ speed_m_per_min: speedFromDisplay(n, units) })}
              step={units === 'imperial' ? 5 : 1}
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              ariaLabel={t(
                units === 'imperial'
                  ? 'gasPanel.dpv.primarySpeed.ariaImperial'
                  : 'gasPanel.dpv.primarySpeed.aria'
              )}
              testId="input-dpv-primary-speed"
            />
          </Row>

          <Row label={t('gasPanel.dpv.primaryBurn.label')} help={t('gasPanel.dpv.primaryBurn.help')}>
            <PositiveNumberField
              value={rig.primary.burn_minutes}
              onCommit={(n) => setPrimary({ burn_minutes: n })}
              step={1}
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              ariaLabel={t('gasPanel.dpv.primaryBurn.aria')}
              testId="input-dpv-primary-burn"
            />
          </Row>

          <Row
            label={t(
              units === 'imperial'
                ? 'gasPanel.dpv.backupSpeed.labelImperial'
                : 'gasPanel.dpv.backupSpeed.label'
            )}
            help={t('gasPanel.dpv.backupSpeed.help')}
          >
            <PositiveNumberField
              value={speedToDisplay(rig.backup.speed_m_per_min, units)}
              onCommit={(n) => setBackup({ speed_m_per_min: speedFromDisplay(n, units) })}
              step={units === 'imperial' ? 5 : 1}
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              ariaLabel={t(
                units === 'imperial'
                  ? 'gasPanel.dpv.backupSpeed.ariaImperial'
                  : 'gasPanel.dpv.backupSpeed.aria'
              )}
              testId="input-dpv-backup-speed"
            />
          </Row>

          <Row label={t('gasPanel.dpv.backupBurn.label')} help={t('gasPanel.dpv.backupBurn.help')}>
            <PositiveNumberField
              value={rig.backup.burn_minutes}
              onCommit={(n) => setBackup({ burn_minutes: n })}
              step={1}
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              ariaLabel={t('gasPanel.dpv.backupBurn.aria')}
              testId="input-dpv-backup-burn"
            />
          </Row>

          <Row label={t('gasPanel.dpv.rangeReserve.label')} help={t('gasPanel.dpv.rangeReserve.help')}>
            <PositiveNumberField
              value={rig.range_reserve_fraction * 100}
              // Clamp the committed fraction into [0, MAX_DPV_RANGE_RESERVE_FRACTION] - the field
              // itself already blocks a negative entry (allowZero, not a plain positive-only gate);
              // only the upper bound needs clamping here, mirroring `GasConfig::normalize`'s cap.
              onCommit={(n) =>
                setRig({ range_reserve_fraction: Math.min(n / 100, MAX_DPV_RANGE_RESERVE_FRACTION) })
              }
              step={1}
              allowZero
              className="w-24 px-2 py-1 bg-background border border-border rounded text-sm text-right"
              ariaLabel={t('gasPanel.dpv.rangeReserve.aria')}
              testId="input-dpv-range-reserve"
            />
          </Row>

          <Row label={t('gasPanel.dpv.usableRange.label')} help={t('gasPanel.dpv.usableRange.help')}>
            <span className="font-mono text-sm" data-testid="dpv-usable-range-value">
              {formatGasLength(dpvUsableRangeM(rig), units, 0)}
            </span>
          </Row>
        </div>
      )}
    </div>
  );
}

function Row({ label, help, children }: { label: string; help?: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between py-1">
      <span className="text-text-secondary" title={help}>
        {label}
      </span>
      {children}
    </div>
  );
}
