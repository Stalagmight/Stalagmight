/**
 * The image export's Station IDs / Station names checkboxes.
 *
 * Same rules as the Display menu's pair: ticking one unticks the other, and
 * unticking one leaves the other alone. Station names is offered only when the
 * project has names (or when it is already ticked, so it can be unticked).
 */
import { useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
  initReactI18next: { type: '3rdParty', init: () => undefined },
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: vi.fn(async () => undefined),
}));

import ExportControls from '../ExportControls';
import { DEFAULT_IMAGE_OPTIONS } from '../../../store/exportStore';
import { useProjectStore } from '../../../store/projectStore';
import { setStationLabelOption } from '../../../utils/exportOptions';
import { emptySurveyData } from '../../../types/survey-data';
import type { ExportImageOptions } from '../../../types/commands';

const IDS = 'export-show-station-labels';
const NAMES = 'export-show-station-names';

let latest: ExportImageOptions = DEFAULT_IMAGE_OPTIONS;

function Harness({ initial }: { initial: ExportImageOptions }) {
  const [options, setOptions] = useState(initial);
  latest = options;
  return (
    <ExportControls
      variant="panel"
      exportType="image"
      onExportTypeChange={() => undefined}
      dataFormat="csv"
      onDataFormatChange={() => undefined}
      imageOptions={options}
      onImageOptionsChange={setOptions}
    />
  );
}

async function renderOpen(initial: ExportImageOptions = DEFAULT_IMAGE_OPTIONS) {
  render(<Harness initial={initial} />);
  await userEvent.click(screen.getByTestId('export-labels-options-toggle'));
}

function setNames(names: Array<[number, string]>) {
  const d = emptySurveyData();
  d.stationNames = new Map(names);
  useProjectStore.setState({ surveyData: d });
}

beforeEach(() => {
  latest = DEFAULT_IMAGE_OPTIONS;
  setNames([[0, '7']]);
});

describe('setStationLabelOption', () => {
  it('ticking one unticks the other; unticking leaves the other alone', () => {
    const ids = setStationLabelOption(DEFAULT_IMAGE_OPTIONS, 'show_station_labels', true);
    expect([ids.show_station_labels, ids.show_station_names]).toEqual([true, false]);
    const names = setStationLabelOption(ids, 'show_station_names', true);
    expect([names.show_station_labels, names.show_station_names]).toEqual([false, true]);
    const none = setStationLabelOption(names, 'show_station_names', false);
    expect([none.show_station_labels, none.show_station_names]).toEqual([false, false]);
    // A new object every time; the input is untouched.
    expect(names).not.toBe(ids);
    expect(ids.show_station_labels).toBe(true);
  });
});

describe('ExportControls station label checkboxes', () => {
  it('labels the boxes Station IDs and Station names, side by side', async () => {
    await renderOpen();
    expect(screen.getByTestId(IDS).closest('label')).toHaveTextContent('Station IDs');
    expect(screen.getByTestId(NAMES).closest('label')).toHaveTextContent('Station names');
    expect(screen.getByTestId(IDS).closest('label')!.nextElementSibling).toBe(
      screen.getByTestId(NAMES).closest('label'),
    );
  });

  it('ticking one unticks the other', async () => {
    await renderOpen();
    await userEvent.click(screen.getByTestId(IDS));
    expect([latest.show_station_labels, latest.show_station_names]).toEqual([true, false]);
    await userEvent.click(screen.getByTestId(NAMES));
    expect([latest.show_station_labels, latest.show_station_names]).toEqual([false, true]);
    expect(screen.getByTestId(IDS)).not.toBeChecked();
    expect(screen.getByTestId(NAMES)).toBeChecked();
    await userEvent.click(screen.getByTestId(NAMES));
    expect([latest.show_station_labels, latest.show_station_names]).toEqual([false, false]);
  });

  it('Station names is disabled, saying why, when the project has no names', async () => {
    setNames([]);
    await renderOpen();
    const names = screen.getByTestId(NAMES);
    expect(names).toBeDisabled();
    expect(names.closest('label')!.getAttribute('title')).toBe('This project has no station names');
    expect(screen.getByTestId(IDS)).toBeEnabled();
  });

  it('a ticked Station names stays untickable even without names', async () => {
    setNames([]);
    await renderOpen({ ...DEFAULT_IMAGE_OPTIONS, show_station_names: true });
    const names = screen.getByTestId(NAMES);
    expect(names).toBeEnabled();
    await userEvent.click(names);
    expect(latest.show_station_names).toBe(false);
    expect(screen.getByTestId(NAMES)).toBeDisabled();
  });
});
