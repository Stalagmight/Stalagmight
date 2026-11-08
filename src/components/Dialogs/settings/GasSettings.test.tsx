/**
 * Component test (#241 P3) - GasSettings must use the shared PositiveNumberField
 * (its own docstring already claims coverage: "the Gas settings panel share[s] one
 * deletable-number control"), not a raw `<input type="number">` + `parseFloat`. The
 * raw form re-introduces the non-deletable/parseFloat trap PositiveNumberField was
 * built to fix: `parseFloat` accepts ANY finite string (including negative/zero),
 * so a bad value commits straight through onChange with no gate; PositiveNumberField
 * only calls onCommit for a strictly-positive finite number.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

import { GasSettings } from './GasSettings';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';

let onChange: ReturnType<typeof vi.fn>;

function renderPanel(overrides: Partial<Config['gas']> = {}) {
  const config: Config = {
    ...defaultConfig,
    gas: { ...defaultConfig.gas, ...overrides },
  };
  render(
    <GasSettings
      config={config}
      onChange={onChange}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings"
    />
  );
  return config;
}

beforeEach(() => {
  onChange = vi.fn();
});

describe('GasSettings numeric fields use PositiveNumberField (#241)', () => {
  it('commits a valid positive swim speed', () => {
    renderPanel({ swim_speed_m_per_min: 10 });
    fireEvent.change(screen.getByTestId('input-gas-swim-speed'), { target: { value: '12' } });
    expect(onChange).toHaveBeenCalledWith({
      gas: expect.objectContaining({ swim_speed_m_per_min: 12 }),
    });
  });

  it('never commits a non-positive swim speed (raw parseFloat would have)', () => {
    renderPanel({ swim_speed_m_per_min: 10 });
    const input = screen.getByTestId('input-gas-swim-speed');
    fireEvent.change(input, { target: { value: '-5' } });
    fireEvent.change(input, { target: { value: '0' } });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('commits a valid positive jump multiplier', () => {
    renderPanel({ jump_rmv_multiplier: 1 });
    fireEvent.change(screen.getByTestId('input-gas-jump-multiplier'), { target: { value: '1.5' } });
    expect(onChange).toHaveBeenCalledWith({
      gas: expect.objectContaining({ jump_rmv_multiplier: 1.5 }),
    });
  });

  it('never commits a non-positive jump multiplier (raw parseFloat would have)', () => {
    renderPanel({ jump_rmv_multiplier: 1 });
    const input = screen.getByTestId('input-gas-jump-multiplier');
    fireEvent.change(input, { target: { value: '-1' } });
    fireEvent.change(input, { target: { value: '0' } });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('keeps the field deletable: an intermediate blank draft does not force-commit', () => {
    renderPanel({ swim_speed_m_per_min: 10 });
    const input = screen.getByTestId('input-gas-swim-speed') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '' } });
    expect(input.value).toBe('');
    expect(onChange).not.toHaveBeenCalled();
  });
});

// DPV rig UI + the M5 cross-language twin tests moved with the rig editor to
// src/components/SingleSystemView/DpvRigEditor.test.tsx and
// src/utils/__tests__/dpv.test.ts (#317 - the rig moved out of Settings into Plan
// mode's DPV tab).
