/**
 * The editor's warning column lists the project's warnings for each saved
 * shot. The backend still opens them with a positional `Shot <line>:<position>`
 * prefix, which is never shown: the shot is named by its stations instead
 * (SPEC 4.2).
 *
 * `.tsx` for jsdom, with the virtualizer shimmed like the sibling suites.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: () => {},
  }),
}));

import { render, screen } from '@testing-library/react';
import '../../../i18n';
import { ShotTableEditor } from '../ShotTableEditor';
import { useProjectStore } from '../../../store/projectStore';
import type { EditableShot } from '../editableShot';
import type { Project, Station } from '../../../types/project';
import type { ProjectWarning } from '../../../types/ipc/ProjectWarning';

const shot = (id: number): EditableShot => ({
  id,
  length: 10,
  azimuth_in: 90,
  azimuth_out: 90,
  depth_in: -1,
  depth_out: -2,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal',
});

const warning = (shotId: number, shotIndex: number, message: string): ProjectWarning => ({
  lineId: 3,
  shotId,
  shotIndex,
  code: 'ZERO_LENGTH',
  message,
  severity: 'warning',
  isResolved: false,
});

const station = (index: number, name: string) =>
  ({ station_ref: { line_id: 3, station_index: index }, name }) as unknown as Station;

beforeAll(() => {
  if (typeof ResizeObserver === 'undefined') {
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  useProjectStore.setState({
    project: {
      lines: [{ id: 3, name: 'Main', color: '#123456', shots: [{ id: 10 }, { id: 11 }] }],
    } as unknown as Project,
    surveyData: null,
    stations: [station(0, 'A0'), station(1, 'A1'), station(2, 'A2')],
    config: null,
  });
});

function renderEditor() {
  render(
    <ShotTableEditor
      shots={[shot(10), shot(11)]}
      onShotChange={vi.fn()}
      onAddShot={vi.fn()}
      onDeleteShot={vi.fn()}
      lineId={3}
    />
  );
}

describe('ShotTableEditor warnings', () => {
  it('names the shot by its stations, never by the positional prefix', () => {
    useProjectStore.setState({ warnings: [warning(11, 1, 'Shot 3:2 has zero length')] });
    renderEditor();
    const title = screen.getByTestId('shot-warning-1').getAttribute('title') ?? '';
    expect(title).toContain('Shot A1 → A2 has zero length');
    expect(title).not.toContain('3:2');
    expect(screen.queryByTestId('shot-warning-0')).toBeNull();
  });

  it('keeps a message that has no positional prefix as it is', () => {
    useProjectStore.setState({ warnings: [warning(10, 0, 'Depth gauge reads past its range')] });
    renderEditor();
    expect(screen.getByTestId('shot-warning-0').getAttribute('title')).toContain('Depth gauge reads past its range');
  });
});
