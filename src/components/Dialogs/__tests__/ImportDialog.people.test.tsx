/**
 * New Section's Surveyors / Explorers pills: prefilled from the session's last
 * used people (and not "dirty" for that), applied to EVERY shot entered, and
 * remembered for the next section.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, act } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

vi.mock('../../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
  setSource: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import ImportDialog from '../ImportDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { SECTION_COLORS } from '../../../utils/colors';
import type { SectionData, ShotData } from '../../../types/project';
import type { ShotDraft } from '../../SingleSystemView/dataTableDrafts';

const byTestId = (id: string): HTMLElement => {
  const el = document.body.querySelector(`[data-testid="${id}"]`);
  if (!el) throw new Error(`${id} not found`);
  return el as HTMLElement;
};
const queryTestId = (id: string) => document.body.querySelector(`[data-testid="${id}"]`);

const complete: Partial<ShotData> = { length: 10, azimuth_in: 90, azimuth_out: 90, depth_in: 0, depth_out: 1 };
const draft = (key: number, data: Partial<ShotData>): ShotDraft => ({ key, anchor: 'end', data });

function renderWithShots(count: number) {
  const drafts = Array.from({ length: count }, (_, i) => draft(i + 1, complete));
  useUIStore.setState({
    dialogData: {
      savedFormState: {
        name: '',
        people: { surveyors: [], explorers: [] },
        date: '',
        color: SECTION_COLORS[0]!,
        drafts,
        nextKey: count + 1,
      },
    },
  });
  render(<ImportDialog />);
}

function typePerson(field: 'surveyors' | 'explorers', name: string) {
  const input = byTestId(`import-section-${field}-input`) as HTMLInputElement;
  fireEvent.change(input, { target: { value: name } });
  fireEvent.keyDown(input, { key: 'Enter' });
}

let placed: SectionData | null = null;

beforeEach(() => {
  placed = null;
  useUIStore.setState({
    dialogData: {},
    resolvedTheme: 'dark',
    appMode: 'survey',
    geometryMode: 'measured',
    lastUsedCredit: { surveyors: [], explorers: [] },
    startPlacementMode: ((data: SectionData) => {
      placed = data;
    }) as never,
    closeDialog: vi.fn(),
  });
  useProjectStore.setState({ project: null, config: null });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('New Section people', () => {
  it('prefills the last used surveyors and explorers without counting them as a change', () => {
    useUIStore.setState({ lastUsedCredit: { surveyors: [{ name: 'Ann Diver' }], explorers: [{ name: 'Bo Lead' }] } });
    render(<ImportDialog />);
    expect(byTestId('import-section-surveyors-pill-0-label')).toHaveTextContent('Ann Diver');
    expect(byTestId('import-section-explorers-pill-0-label')).toHaveTextContent('Bo Lead');
    // Closing asks nothing: the dialog is not dirty.
    fireEvent.click(byTestId('import-section-dialog-close'));
    expect(queryTestId('dirty-dialog-discard')).toBeNull();
    expect(useUIStore.getState().closeDialog).toHaveBeenCalled();
  });

  it('counts a name still being typed as a change', () => {
    render(<ImportDialog />);
    fireEvent.change(byTestId('import-section-surveyors-input'), { target: { value: 'Half typ' } });
    fireEvent.click(byTestId('import-section-dialog-close'));
    expect(queryTestId('dirty-dialog-discard')).not.toBeNull();
  });

  it('puts the pills and the date on every shot, and remembers the people', () => {
    renderWithShots(3);
    typePerson('surveyors', 'Ann Diver');
    typePerson('surveyors', 'Cy Survey');
    typePerson('explorers', 'Bo Lead');
    fireEvent.click(byTestId('import-section-date-input-empty'));
    fireEvent.change(byTestId('import-section-date-input'), { target: { value: '2026-03-02' } });
    act(() => {
      byTestId('import-section-create').click();
    });
    expect(placed).not.toBeNull();
    expect(placed!.surveyor).toBeNull();
    expect(placed!.shots).toHaveLength(3);
    for (const shot of placed!.shots) {
      expect(shot.credit).toEqual({
        surveyors: [{ name: 'Ann Diver' }, { name: 'Cy Survey' }],
        explorers: [{ name: 'Bo Lead' }],
        date: '2026-03-02',
      });
    }
    expect(useUIStore.getState().lastUsedCredit).toEqual({
      surveyors: [{ name: 'Ann Diver' }, { name: 'Cy Survey' }],
      explorers: [{ name: 'Bo Lead' }],
    });
  });

  it('leaves the shots uncredited when no people and no date were given', () => {
    renderWithShots(2);
    act(() => {
      byTestId('import-section-create').click();
    });
    expect(placed!.shots.every((shot) => shot.credit === undefined)).toBe(true);
  });

  it('removes a pill with its x', () => {
    renderWithShots(1);
    typePerson('surveyors', 'Ann Diver');
    typePerson('surveyors', 'Cy Survey');
    fireEvent.click(byTestId('import-section-surveyors-pill-0-remove'));
    expect(byTestId('import-section-surveyors-pill-0-label')).toHaveTextContent('Cy Survey');
    expect(queryTestId('import-section-surveyors-pill-1')).toBeNull();
  });
});
