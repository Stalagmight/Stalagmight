/**
 * Component test - the TML export unanchored-section notice (#411).
 *
 * The original defect: exporting a project with no zero datum raised an alarming modal
 * ("Some sections will be discarded") listing every section, with no way to act on it.
 *
 * A later review round found the premise itself was wrong: `build_export_shots`
 * (src-tauri/src/export/tml.rs) never discards a section. And the notice must not
 * claim entrance topology either: entrance-hood is merge-derived in the exporter
 * while this dialog classifies by zero-datum reachability - two predicates that
 * disagree in both directions (see the comment above the AlertDialog in
 * ExportControls.tsx). So the copy states only the true consequence - the export
 * is not georeferenced - and it must (a) name the real fix (Set Zero Datum, via
 * the station context menu), and (b) distinguish "nothing in this project is
 * anchored" from "some sections aren't" - from the ACTUAL unanchored-section
 * count, not from `zero_datums.length` (a datum whose station no longer resolves
 * anchors nothing, but still has `zero_datums.length > 0`).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// The text assertions below need live i18n; today it loads transitively through
// projectStore's own import, but that is nobody's contract - import it directly.
import '../../i18n';

vi.mock('../../utils/fileDialogs', () => ({
  saveDialog: vi.fn(),
  exportSaveFilters: vi.fn(() => []),
  ensureExtension: vi.fn((p: string) => p),
}));

vi.mock('../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
}));

import ExportControls from './ExportControls';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { saveDialog } from '../../utils/fileDialogs';
import { trackDialogComplete } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { DEFAULT_IMAGE_OPTIONS } from '../../store/exportStore';
import type { Project } from '../../types/ipc/Project';
import type { Section } from '../../types/ipc/Section';
import type { ZeroDatum } from '../../types/ipc/ZeroDatum';

const section = (id: number, name: string | null = null): Section =>
  ({ id, name, shots: [] }) as unknown as Section;

const ZERO_DATUM: ZeroDatum = {
  station_ref: { section_id: 1, station_index: 0 },
  gps: { latitude: 1, longitude: 2, depth_reference: 0 },
  role: 'Placement',
};

const buildProject = (sections: Section[], zero_datums: ZeroDatum[]): Project =>
  ({ sections, zero_datums } as unknown as Project);

let getAnchoredSectionIds: ReturnType<typeof vi.fn>;
let exportTml: ReturnType<typeof vi.fn>;

const baseProps = {
  variant: 'dialog' as const,
  exportType: 'data' as const,
  onExportTypeChange: vi.fn(),
  dataFormat: 'tml' as const,
  onDataFormatChange: vi.fn(),
  imageOptions: DEFAULT_IMAGE_OPTIONS,
  onImageOptionsChange: vi.fn(),
};

beforeEach(() => {
  vi.clearAllMocks();
  getAnchoredSectionIds = vi.fn();
  exportTml = vi.fn(async () => undefined);
  vi.mocked(saveDialog).mockResolvedValue('/tmp/out.tml');
});

describe('TML export unanchored-section notice', () => {
  it('names the fix and says every section is unanchored when the project has no zero datum at all', async () => {
    getAnchoredSectionIds.mockResolvedValue([]);
    useProjectStore.setState({
      project: buildProject([section(1), section(2)], []),
      getAnchoredSectionIds,
      exportTml,
    });

    render(<ExportControls {...baseProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    const dialog = await screen.findByTestId('tml-unanchored-warning-dialog');
    expect(dialog).toHaveTextContent('No section is connected to a zero datum');
    expect(dialog).toHaveTextContent('Set Zero Datum');
    expect(dialog).not.toHaveTextContent(/\d+ of \d+ sections/);
    // The two claims the copy must make / must not make: nothing is discarded,
    // and no entrance-topology promise (entrance-hood is merge-derived and this
    // dialog cannot know it - see the component comment).
    expect(dialog).toHaveTextContent('Nothing is discarded');
    expect(dialog).not.toHaveTextContent('entrance');
    expect(saveDialog).not.toHaveBeenCalled();
  });

  it('uses the all-unanchored wording when a zero datum exists but resolves nothing (#411 regression)', async () => {
    // zero_datums.length > 0, but its station no longer resolves: get_anchored_section_ids
    // returns []. The title must follow the unanchored COUNT, not zero_datums.length.
    getAnchoredSectionIds.mockResolvedValue([]);
    useProjectStore.setState({
      project: buildProject([section(1), section(2), section(3)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportTml,
    });

    render(<ExportControls {...baseProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    const dialog = await screen.findByTestId('tml-unanchored-warning-dialog');
    expect(dialog).toHaveTextContent('No section is connected to a zero datum');
    expect(dialog).not.toHaveTextContent(/\d+ of \d+ sections/);
  });

  it('names the fix and gives a count when only some sections are unanchored', async () => {
    getAnchoredSectionIds.mockResolvedValue([1]);
    useProjectStore.setState({
      project: buildProject([section(1), section(2), section(3)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportTml,
    });

    render(<ExportControls {...baseProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    const dialog = await screen.findByTestId('tml-unanchored-warning-dialog');
    expect(dialog).toHaveTextContent('2 of 3 sections');
    expect(dialog).toHaveTextContent('Set Zero Datum');
    expect(dialog).not.toHaveTextContent('No section is connected to a zero datum');
    expect(dialog).toHaveTextContent('Nothing is discarded');
    expect(dialog).not.toHaveTextContent('entrance');
    expect(saveDialog).not.toHaveBeenCalled();
  });

  it('uses singular wording for exactly one unanchored section', async () => {
    getAnchoredSectionIds.mockResolvedValue([1]);
    useProjectStore.setState({
      project: buildProject([section(1), section(2)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportTml,
    });

    render(<ExportControls {...baseProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    const dialog = await screen.findByTestId('tml-unanchored-warning-dialog');
    // A missing `_one` key renders as the raw `[key]` under vitest (DEV
    // `parseMissingKeyHandler`) and as '' in production - either way this
    // positive assertion on the actual singular text catches it, where a mere
    // absence check would not.
    expect(dialog).toHaveTextContent('1 of 2 sections is not connected');
  });

  it('does not show the notice when every section is anchored', async () => {
    getAnchoredSectionIds.mockResolvedValue([1, 2]);
    useProjectStore.setState({
      project: buildProject([section(1), section(2)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportTml,
    });

    render(<ExportControls {...baseProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(saveDialog).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('tml-unanchored-warning-dialog')).not.toBeInTheDocument();
  });
});

/**
 * A cancelled OS save picker must not complete the dialog (#560).
 *
 * `handleExport`'s completion block (`trackDialogComplete` + `onComplete`)
 * used to sit OUTSIDE both `if (selected)` guards, so it ran even when
 * `saveDialog` resolved `null` (the user cancelled). That closed the dialog
 * on a cancel and suppressed the `dialog-cancel.export` telemetry
 * `trackDialogComplete` is meant to gate.
 */
describe('export save-picker cancel (#560)', () => {
  beforeEach(() => {
    vi.mocked(saveDialog).mockResolvedValue(null);
  });

  it('data export: leaves the dialog open and does not track completion on cancel', async () => {
    const exportCsv = vi.fn(async () => undefined);
    const onComplete = vi.fn();
    useProjectStore.setState({
      project: buildProject([section(1)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportCsv,
    });

    render(<ExportControls {...baseProps} dataFormat="csv" onComplete={onComplete} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(saveDialog).toHaveBeenCalledTimes(1));
    expect(exportCsv).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
    expect(trackDialogComplete).not.toHaveBeenCalled();
  });

  it('image export: leaves the dialog open and does not track completion on cancel', async () => {
    const exportImage = vi.fn(async () => undefined);
    const onComplete = vi.fn();
    useProjectStore.setState({
      project: buildProject([section(1)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportImage,
    });

    render(<ExportControls {...baseProps} exportType="image" onComplete={onComplete} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(saveDialog).toHaveBeenCalledTimes(1));
    expect(exportImage).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
    expect(trackDialogComplete).not.toHaveBeenCalled();
  });

  it('TML "continue anyway" re-entry: still does not complete on a cancelled picker', async () => {
    // The unanchored-warning "Continue" path re-enters handleExport with
    // skipTmlAnchorCheck - it must obey the same cancel guard as the direct path.
    getAnchoredSectionIds.mockResolvedValue([]);
    const exportTmlFn = vi.fn(async () => undefined);
    const onComplete = vi.fn();
    useProjectStore.setState({
      project: buildProject([section(1), section(2)], []),
      getAnchoredSectionIds,
      exportTml: exportTmlFn,
    });

    render(<ExportControls {...baseProps} onComplete={onComplete} />);
    await userEvent.click(screen.getByTestId('export-button'));

    const dialog = await screen.findByTestId('tml-unanchored-warning-dialog');
    expect(dialog).toBeInTheDocument();
    await userEvent.click(screen.getByTestId('tml-unanchored-warning-continue'));

    await waitFor(() => expect(saveDialog).toHaveBeenCalledTimes(1));
    expect(exportTmlFn).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
    expect(trackDialogComplete).not.toHaveBeenCalled();
  });
});

/**
 * A failed export must reach a surface.
 *
 * Before this, `handleExport`'s catch was a bare `console.error`: the dialog
 * stayed open with its button un-greyed and the panel did nothing at all, which
 * is indistinguishable from "the file was written". Every reachable refusal ends
 * up in that catch - the backend's "Cannot export over the open project file",
 * `export_png`'s DPI/size bails, and any I/O error - and none of them is visible
 * anywhere else, because the four `projectStore` export actions do NOT go through
 * `executeCommand` and so never reach its `COMMAND_FAILED` catch-all.
 */
describe('export failure surface', () => {
  const dataProps = { ...baseProps, dataFormat: 'csv' as const, onComplete: vi.fn() };

  beforeEach(() => {
    useUIStore.setState({ toasts: [] });
  });

  it('raises an error toast carrying the backend message when the export command rejects', async () => {
    const exportCsv = vi.fn(async () => {
      throw new Error('Cannot export over the open project file');
    });
    useProjectStore.setState({
      project: buildProject([section(1)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportCsv,
    });

    render(<ExportControls {...dataProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(useUIStore.getState().toasts).toHaveLength(1));
    const toast = useUIStore.getState().toasts[0]!;
    expect(toast.key).toBe(TOAST_KEYS.EXPORT_FAILED);
    expect(toast.kind).toBe('error');
    // The backend message, not the generic fallback: it names WHICH refusal
    // happened, and "Cannot export over the open project file" is already the
    // one-line "what happened, then what to do" the copy rules ask for.
    expect(toast.message).toBe('Cannot export over the open project file');
    // A failed export is not a completed one.
    expect(dataProps.onComplete).not.toHaveBeenCalled();
  });

  it('falls back to the localized string when the rejection carries no message', async () => {
    const exportCsv = vi.fn(async () => {
      throw undefined;
    });
    useProjectStore.setState({
      project: buildProject([section(1)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportCsv,
    });

    render(<ExportControls {...dataProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(useUIStore.getState().toasts).toHaveLength(1));
    expect(useUIStore.getState().toasts[0]!.message).toBe(
      'Export failed - try a different folder or file name',
    );
  });

  it('stays silent when the export succeeds', async () => {
    const exportCsv = vi.fn(async () => undefined);
    useProjectStore.setState({
      project: buildProject([section(1)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportCsv,
    });

    render(<ExportControls {...dataProps} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(dataProps.onComplete).toHaveBeenCalled());
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });

  it('does not toast when the pre-export anchor check throws - that check fails OPEN', async () => {
    // The check is informational and the export proceeds, so reporting a failure
    // here would report one that did not happen. See the comment on the catch in
    // ExportControls.tsx.
    const exportTmlOk = vi.fn(async () => undefined);
    getAnchoredSectionIds.mockRejectedValue(new Error('anchored ids IPC exploded'));
    useProjectStore.setState({
      project: buildProject([section(1)], [ZERO_DATUM]),
      getAnchoredSectionIds,
      exportTml: exportTmlOk,
    });

    render(<ExportControls {...baseProps} onComplete={vi.fn()} />);
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(exportTmlOk).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('tml-unanchored-warning-dialog')).not.toBeInTheDocument();
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });
});
