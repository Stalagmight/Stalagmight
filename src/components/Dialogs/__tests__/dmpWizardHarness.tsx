/**
 * Shared fixtures for the DMP wizard's component tests (#576 W4).
 *
 * Fixtures only - no `vi.mock` here: mocks are hoisted per test file, so every
 * file that renders the wizard still declares its own (SectionPreview, the map
 * panel, maplibre-gl, react-i18next, telemetry, utils/ipc) before importing this.
 * All data is synthetic.
 */
import { render } from '@testing-library/react';
import { vi } from 'vitest';
import type { DmpFile } from '../../../types/commands';
import type { ImportReceipt } from '../../../types/ipc/ImportReceipt';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import DmpNewImportDialog, { type SavedManualPlacingState } from '../DmpNewImportDialog';
import type { ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { convertToEditableShots } from '../../../utils/shotConversion';
import { useUIStore } from '../../../store/uiStore';

export const receipt = (overrides: Partial<ImportReceipt> = {}): ImportReceipt => ({
  source_format: 'mNemo DMP',
  file_name: 'dive.dmp',
  sections_found: 2,
  sections_imported: 1,
  shot_count: 3,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: [],
  info: [],
  zero_datum_created: null,
  ...overrides,
});

/** In the real `DmpShot` shape: `shot_type: 'Standard'` and a non-unit length
 *  are what `isImportableDmpShot` keeps. */
export const dmpSection = (name: string) => ({
  version: 1,
  name,
  direction: 'in',
  date: '2026-01-01',
  content_hash: `hash_${name}`,
  shots: Array.from({ length: 2 }, () => ({
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
    temperature: 20,
    hour: 12,
    minute: 0,
    second: 0,
    marker_index: 0,
  })),
});

export const dmpFile = {
  version: 1,
  sections: [dmpSection('A'), dmpSection('B'), dmpSection('C')],
} as unknown as DmpFile;

export const selectionConfig = (selected: number[]): DmpSelectionConfig => ({
  selectedIndices: new Set(selected),
  defaultSurveyor: 'S',
  defaultDate: null,
  fileHash: 'file-hash',
  sectionHashes: selected.map((i) => `hash_${'ABC'[i]}`),
});

export const savedState = (overrides: Partial<SavedManualPlacingState>): SavedManualPlacingState => ({
  connections: [],
  skippedIndices: [],
  segmentQueue: [],
  segmentAnchor: { sectionIndex: 0, stationIndex: 0 },
  directPlacements: [],
  skippedSuggestions: [],
  segmentSkipCounts: [],
  sectionEdits: [],
  ...overrides,
});

/** Section 0 as the zero datum: the mount effect imports it first. */
export const zeroDatumOn0 = (): ZeroDatumConfigResult => ({
  sectionIndex: 0,
  stationIndex: 0,
  gps: { latitude: 20, longitude: -87, depth_reference: 0 },
  metadata: {
    name: 'A',
    surveyor: 'S',
    date: '2026-01-01',
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    reverse: false,
  },
  shots: convertToEditableShots(dmpFile.sections[0]!.shots, dmpFile.sections[0]!.version),
});

export interface WizardProps {
  onComplete?: (ids?: number[], r?: ImportReceipt) => void;
  onCancel?: () => void;
  onBack?: () => void;
  selected?: number[];
  zeroDatumConfig?: ZeroDatumConfigResult;
  autoStartManualPlacement?: boolean;
}

export const renderWizard = (saved: SavedManualPlacingState | undefined, props: WizardProps = {}) =>
  render(
    <DmpNewImportDialog
      dmpFile={dmpFile}
      selectionConfig={selectionConfig(props.selected ?? [0])}
      project={null}
      stations={[]}
      onCancel={props.onCancel ?? vi.fn()}
      onBack={props.onBack ?? vi.fn()}
      onComplete={props.onComplete ?? vi.fn()}
      savedManualPlacingState={saved}
      zeroDatumConfig={props.zeroDatumConfig}
      autoStartManualPlacement={props.autoStartManualPlacement ?? false}
    />,
  );

/**
 * Render the instance App.tsx would mount next: the wizard as `openDialog` left
 * it in `dialogData`. A map placement CLOSES the wizard and reopens a fresh one,
 * so a test that drives `placementMode.onComplete` has to do the same.
 */
export const renderReopenedWizard = (props: Pick<WizardProps, 'onComplete' | 'onCancel' | 'onBack'> = {}) => {
  const data = useUIStore.getState().dialogData as {
    dmpFile: DmpFile;
    selectionConfig: DmpSelectionConfig;
    zeroDatumConfig?: ZeroDatumConfigResult;
    savedManualPlacingState?: SavedManualPlacingState;
    autoStartManualPlacement?: boolean;
  };
  return render(
    <DmpNewImportDialog
      dmpFile={data.dmpFile}
      selectionConfig={data.selectionConfig}
      project={null}
      stations={[]}
      onCancel={props.onCancel ?? vi.fn()}
      onBack={props.onBack ?? vi.fn()}
      onComplete={props.onComplete ?? vi.fn()}
      savedManualPlacingState={data.savedManualPlacingState}
      zeroDatumConfig={data.zeroDatumConfig}
      autoStartManualPlacement={Boolean(data.autoStartManualPlacement)}
    />,
  );
};
