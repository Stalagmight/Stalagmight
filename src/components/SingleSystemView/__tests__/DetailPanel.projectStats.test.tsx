/**
 * The Project stats panel counts stations as the model and the file summary
 * do: a junction is one station, and the panel shows the junction count, not
 * the wire's merge pairs (review finding UX-8: a Split Line Here, which adds
 * no station and merges nothing, raised "Stations" and "Station merges").
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import DetailPanel from '../DetailPanel';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Project, ProjectStats } from '../../../types/project';

function stats(over: Partial<ProjectStats>): ProjectStats {
  return {
    line_count: 4,
    shot_count: 15,
    total_length: 120,
    min_depth: 0,
    max_depth: 7,
    station_count: 16,
    junction_count: 3,
    loop_count: 1,
    penetration_distance: 60,
    ...over,
  };
}

/** Four wire merge pairs: what the panel used to show as "Station merges". */
function project(): Project {
  const pair = (a: number, b: number) => ({
    station_a: { line_id: a, station_index: 0 },
    station_b: { line_id: b, station_index: 0 },
  });
  return {
    name: 'Synthetic',
    file_path: null,
    lines: [],
    station_merges: [pair(1, 2), pair(1, 3), pair(1, 4), pair(1, 5)],
    zero_datums: [],
    loop_closures: [],
  } as unknown as Project;
}

/** The value printed under the stat card labelled `label`. */
function statValue(label: string): string | null {
  const labelEl = screen.getByText(label);
  return labelEl.nextElementSibling?.textContent ?? null;
}

beforeEach(() => {
  useUIStore.setState({ detailPanelTab: 'details', appMode: 'survey', geometryMode: 'measured', selection: null, multiSelection: [] });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('Project stats', () => {
  it('shows distinct stations and junctions from the backend stats', () => {
    useProjectStore.setState({ project: project(), stations: [], warnings: [], stats: stats({}), config: null });
    render(<DetailPanel />);
    expect(statValue('Stations')).toBe('16');
    expect(statValue('Junctions')).toBe('3');
    expect(screen.queryByText('Station merges')).toBeNull();
  });

  it('follows the junction count, not the merge pairs', () => {
    useProjectStore.setState({ project: project(), stations: [], warnings: [], stats: stats({ junction_count: 4 }), config: null });
    render(<DetailPanel />);
    expect(statValue('Junctions')).toBe('4');
  });
});
