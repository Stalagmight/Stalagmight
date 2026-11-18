/**
 * The segment placement cards name every station by its name - the one an
 * incoming DMP station will get, the one an existing project station has -
 * never by its position (SPEC 4.2).
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import '../../../i18n';
import { SegmentPlacementModes, type SegmentPlacementModesProps } from '../SegmentPlacementModes';
import { useProjectStore } from '../../../store/projectStore';
import { convertToEditableShots } from '../../../hooks/useImportState';
import type { CrossLineSuggestion, DmpShot } from '../../../types/commands';
import type { DmpSection } from '../../../types/ipc/DmpSection';
import type { Line } from '../../../types/project';
import type { LineEditState } from '../../../utils/batchImportHelpers';

const dmpShot = (): DmpShot => ({
  shot_type: 'Standard',
  heading_in: 10,
  heading_out: 10,
  length: 5,
  depth_in: 0,
  depth_out: 0,
  pitch_in: 0,
  pitch_out: 0,
  left: 0,
  right: 0,
  up: 0,
  down: 0,
  temperature: 0,
  hour: 0,
  minute: 0,
  second: 0,
  marker_index: 0,
});

const SECTION: DmpSection = {
  version: 1,
  name: 'Upstream',
  direction: 'in',
  date: '2026-01-01',
  shots: [dmpShot(), dmpShot(), dmpShot()],
  content_hash: '',
};

const PROJECT_LINE = { id: 5, name: 'Main Line', shots: [] } as unknown as Line;

const baseProps = (overrides: Partial<SegmentPlacementModesProps> = {}): SegmentPlacementModesProps => ({
  segmentSkippedSuggestion: null,
  placementMode: 'manual',
  onPlacementModeChange: vi.fn(),
  currentSegment: [0],
  segmentAnchor: { lineIndex: 0, stationIndex: 0 },
  onCommitSegmentAnchor: vi.fn(),
  lineEdits: new Map(),
  dmpSections: [SECTION],
  onAnchorLineHover: vi.fn(),
  onAnchorStationHover: vi.fn(),
  projectLines: [PROJECT_LINE],
  fromExistingLineId: null,
  onFromExistingLineChange: vi.fn(),
  fromExistingStationIdx: 0,
  onFromExistingStationChange: vi.fn(),
  fromExistingStationCount: 0,
  onPlaceOnMap: vi.fn(),
  ...overrides,
});

const optionText = (testId: string, value: number) =>
  screen.getByTestId(`${testId}-option-${value}`).textContent;

beforeAll(() => {
  // jsdom does not implement scrollIntoView; the Select calls it on open.
  Element.prototype.scrollIntoView = vi.fn();
});

beforeEach(() => {
  // The project station at index 1 of Main Line is named A7.
  useProjectStore.setState({
    project: { lines: [PROJECT_LINE] },
    surveyData: null,
    stations: [
      { station_ref: { line_id: 5, station_index: 0 }, name: 'A6' },
      { station_ref: { line_id: 5, station_index: 1 }, name: 'A7' },
    ],
  } as never);
});

describe('SegmentPlacementModes station names', () => {
  it('names an unedited incoming line by its MNemo numbers', () => {
    render(<SegmentPlacementModes {...baseProps()} />);
    fireEvent.click(screen.getByTestId('manual-import-station-select'));
    expect(optionText('manual-import-station-select', 0)).toBe('Station 0');
    expect(optionText('manual-import-station-select', 3)).toBe('Station 3');
  });

  it('names an edited line by the names its surviving stations keep', () => {
    const shots = convertToEditableShots(SECTION.shots);
    shots[0] = { ...shots[0]!, isDeleted: true };
    const edits = new Map<number, LineEditState>([
      [0, { name: 'Upstream', date: '', color: '#fff', direction: 'in', quality: 1, reverse: false, shots }],
    ]);
    render(<SegmentPlacementModes {...baseProps({ lineEdits: edits })} />);
    fireEvent.click(screen.getByTestId('manual-import-station-select'));
    // The first shot is gone: the line starts at MNemo station 1.
    expect(optionText('manual-import-station-select', 0)).toBe('Station 1');
    expect(optionText('manual-import-station-select', 2)).toBe('Station 3');
  });

  it('names the stations of an existing project line by the project', () => {
    render(
      <SegmentPlacementModes
        {...baseProps({ placementMode: 'fromExisting', fromExistingLineId: 5, fromExistingStationCount: 2 })}
      />
    );
    fireEvent.click(screen.getByTestId('from-existing-station-select'));
    expect(optionText('from-existing-station-select', 0)).toBe('Station A6');
    expect(optionText('from-existing-station-select', 1)).toBe('Station A7');
  });

  it('names the skipped suggestion target by line and station name', () => {
    const suggestion = {
      targetType: 'existing',
      existingLineId: 5,
      existingLineName: 'Main Line',
      matchedShots: 2,
    } as CrossLineSuggestion;
    render(
      <SegmentPlacementModes
        {...baseProps({
          placementMode: 'suggestion',
          segmentSkippedSuggestion: { suggestion, mergeImportStation: 0, mergeExistingStation: 1 },
        })}
      />
    );
    expect(screen.getByText(/Main Line \/ A7/)).toBeInTheDocument();
    expect(screen.queryByText(/stn/)).toBeNull();
  });
});
