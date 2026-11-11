import { act, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import '../../../i18n';
import { ExportColorScaleControls } from '../ExportColorScaleControls';
import { useProjectStore } from '../../../store/projectStore';
import { useColorScaleStore } from '../../../store/colorScaleStore';
import { defaultConfig } from '../../../types/config';
import type { Config, ValueHistogram } from '../../../types/ipc.generated';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

const depth: ValueHistogram = { min: 0, max: 40, counts: [3, 1, 4, 1, 5] };
const distance: ValueHistogram = { min: 0, max: 900, counts: [2, 7, 1, 8, 2] };

const maxField = () => Number((screen.getByTestId('export-color-scale-levels-max-input') as HTMLInputElement).value);

function histogramCalls(): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === 'export_value_histogram').length;
}

describe('ExportColorScaleControls', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    useProjectStore.setState({
      project: { name: 'Synthetic', sections: [] } as never,
      config: structuredClone(defaultConfig) as Config,
    });
    useColorScaleStore.getState().resetSession();
  });

  it("never shows one kind's histogram under the other while the new one loads", async () => {
    let resolveDistance: (h: ValueHistogram) => void = () => {};
    invokeIpcMock.mockImplementation((_cmd: string, args: { kind: string }) =>
      args.kind === 'depth'
        ? Promise.resolve(depth)
        : new Promise<ValueHistogram>((r) => {
            resolveDistance = r;
          }),
    );
    const { rerender } = render(<ExportColorScaleControls kind="depth" />);
    await screen.findByTestId('export-color-scale-depth');
    expect(maxField()).toBe(40);

    rerender(<ExportColorScaleControls kind="distance" />);
    // The depth reply must not stand in for distance (0..40 m over a 0..900 m field).
    expect(screen.queryByTestId('export-color-scale-depth')).toBeNull();
    expect(screen.queryByTestId('export-color-scale-levels-max-input')).toBeNull();
    expect(screen.getByTestId('export-color-scale-loading')).toBeInTheDocument();

    await act(async () => {
      resolveDistance(distance);
    });
    expect(screen.getByTestId('export-color-scale-distance')).toBeInTheDocument();
    expect(maxField()).toBe(900);
  });

  it('refetches when the closure-adjustment settings change the solve', async () => {
    invokeIpcMock.mockResolvedValue(depth);
    render(<ExportColorScaleControls kind="depth" />);
    await screen.findByTestId('export-color-scale-depth');
    expect(histogramCalls()).toBe(1);

    const config = useProjectStore.getState().config!;
    act(() => {
      useProjectStore.setState({ config: { ...config, closure_adjustment: { ...config.closure_adjustment } } });
    });
    await waitFor(() => expect(histogramCalls()).toBe(2));
  });
});
