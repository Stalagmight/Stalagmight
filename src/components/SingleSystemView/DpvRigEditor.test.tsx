/**
 * Component test (#317) - the DPV rig editor, the Plan panel's "DPV" tab, which replaced
 * the Settings-dialog rig UI. Every commit here must both update the working `gasConfig` (via
 * `setDpvRig`, the real gasPlanStore action) and persist to disk through the shared
 * `queueGasConfigWrite` queue. The disk half is trailing-debounced because
 * `PositiveNumberField` commits on every valid KEYSTROKE, not on blur - the per-keystroke
 * one-write test below is the guard for that (each write is a config reload + LSQ-cache
 * drop, so an un-debounced writer regresses #241). Tests flush the queue explicitly instead
 * of waiting out the debounce window.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

import { DpvRigEditor } from './DpvRigEditor';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useProjectStore } from '../../store/projectStore';
import { flushGasConfigWrite } from '../../utils/gasConfigDisk';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import { speedToDisplay } from '../../utils/formatGas';
import type { DpvRig } from '../../types/ipc.generated';

const RIG: DpvRig = {
  primary: { speed_m_per_min: 24, burn_minutes: 60 },
  backup: { speed_m_per_min: 18, burn_minutes: 45 },
  range_reserve_fraction: 0.2,
};

beforeEach(async () => {
  // Drain anything a previous test left in the shared write queue into the OLD store mock,
  // so it cannot pollute the fresh mock's call counts below.
  await flushGasConfigWrite();
  useGasPlanStore.setState({ gasConfig: structuredClone(defaultConfig.gas) });
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    updateGasConfig: vi.fn(async () => ({ success: true, error: null, warnings: [], section_ids: [] })),
  });
});

describe('DpvRigEditor with no rig configured', () => {
  it('renders the toggle off and no fields region', () => {
    render(<DpvRigEditor />);
    expect(screen.getByTestId('toggle-dpv-enabled')).toHaveAttribute('aria-checked', 'false');
    expect(screen.queryByTestId('dpv-rig-fields')).not.toBeInTheDocument();
  });
});

describe('DpvRigEditor enable/disable', () => {
  it('enabling seeds a default rig into the working config and persists it to disk', async () => {
    render(<DpvRigEditor />);
    fireEvent.click(screen.getByTestId('toggle-dpv-enabled'));

    expect(useGasPlanStore.getState().gasConfig.dpv).toEqual(
      expect.objectContaining({ primary: expect.any(Object), backup: expect.any(Object) })
    );
    expect(screen.getByTestId('dpv-rig-fields')).toBeInTheDocument();

    await flushGasConfigWrite();
    const updateGasConfig = useProjectStore.getState().updateGasConfig;
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    expect(updateGasConfig).toHaveBeenCalledWith(
      expect.objectContaining({ dpv: expect.objectContaining({ primary: expect.any(Object) }) })
    );
  });

  it('disabling clears the rig back to null in the working config and persists null to disk', async () => {
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: RIG } });
    render(<DpvRigEditor />);

    fireEvent.click(screen.getByTestId('toggle-dpv-enabled'));

    expect(useGasPlanStore.getState().gasConfig.dpv).toBeNull();
    expect(screen.queryByTestId('dpv-rig-fields')).not.toBeInTheDocument();

    await flushGasConfigWrite();
    const updateGasConfig = useProjectStore.getState().updateGasConfig;
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    expect(updateGasConfig).toHaveBeenCalledWith(expect.objectContaining({ dpv: null }));
  });
});

describe('DpvRigEditor disk-write debounce', () => {
  it('typing a value digit by digit lands ONE disk write carrying the final value', async () => {
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: RIG } });
    render(<DpvRigEditor />);

    // PositiveNumberField commits from onChange, so this is three commits ('1', '10', '100') -
    // exactly the sequence that used to mean three update_config IPCs + three LSQ-cache drops.
    const input = screen.getByTestId('input-dpv-primary-burn');
    fireEvent.change(input, { target: { value: '1' } });
    fireEvent.change(input, { target: { value: '10' } });
    fireEvent.change(input, { target: { value: '100' } });

    // The working copy tracks every commit immediately...
    expect(useGasPlanStore.getState().gasConfig.dpv?.primary.burn_minutes).toBe(100);
    // ...but nothing has hit the disk inside the debounce window.
    const updateGasConfig = useProjectStore.getState().updateGasConfig;
    expect(updateGasConfig).not.toHaveBeenCalled();

    await flushGasConfigWrite();
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    expect(updateGasConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        dpv: expect.objectContaining({ primary: expect.objectContaining({ burn_minutes: 100 }) }),
      })
    );
  });
});

describe('DpvRigEditor range-reserve cap clamp', () => {
  it('clamps a commit above the cap to 0.9 in both the working config and the persisted write', async () => {
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: RIG } });
    render(<DpvRigEditor />);

    const input = screen.getByTestId('input-dpv-range-reserve');
    fireEvent.change(input, { target: { value: '95' } });

    expect(useGasPlanStore.getState().gasConfig.dpv?.range_reserve_fraction).toBe(0.9);

    await flushGasConfigWrite();
    const updateGasConfig = useProjectStore.getState().updateGasConfig;
    expect(updateGasConfig).toHaveBeenCalledWith(
      expect.objectContaining({ dpv: expect.objectContaining({ range_reserve_fraction: 0.9 }) })
    );
  });
});

describe('DpvRigEditor imperial units', () => {
  it('flips the primary/backup speed labels and values to feet/min', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, units: 'imperial', dpv: RIG },
    });
    render(<DpvRigEditor />);

    const primary = screen.getByTestId('input-dpv-primary-speed') as HTMLInputElement;
    const backup = screen.getByTestId('input-dpv-backup-speed') as HTMLInputElement;
    expect(Number(primary.value)).toBeCloseTo(speedToDisplay(RIG.primary.speed_m_per_min, 'imperial'), 5);
    expect(Number(backup.value)).toBeCloseTo(speedToDisplay(RIG.backup.speed_m_per_min, 'imperial'), 5);
  });

  it('renders the metric value unconverted under metric units (regression guard)', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, units: 'metric', dpv: RIG },
    });
    render(<DpvRigEditor />);

    expect(screen.getByTestId('input-dpv-primary-speed')).toHaveValue(RIG.primary.speed_m_per_min);
    expect(screen.getByTestId('input-dpv-backup-speed')).toHaveValue(RIG.backup.speed_m_per_min);
  });
});
