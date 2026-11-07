/**
 * [#576 F32-EDIT] Orphan placement's overlap treatment sends only the target
 * readings that change.
 *
 * "Use imported data" and "Interpolate" rewrote the target overlap shot from a
 * whole row built from the store's copies, and the store holds every reading
 * rounded through an f32 (the binary envelope): a reading the treatment left
 * equal was re-sent as its rounding and saved over the backend's exact value.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// jsdom does not implement scrollIntoView; the Select component calls it on open.
Element.prototype.scrollIntoView = vi.fn();
vi.mock('../SectionPreview', () => ({ default: () => null }));
vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryWith());

import OrphanPlacementDialog from '../OrphanPlacementDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { OrphanPlacementSuggestion } from '../../../types/commands';
import { accepted, type CommandOutcome } from '../../../store/commandResult';
import type { EditOutcome } from '../../../types/ipc/EditOutcome';
import type { Project, Shot } from '../../../types/project';
import { makeShot } from '../../../test/factories/survey';

const f32 = (v: number) => Math.fround(v);
const ORPHAN_ID = 1;
const TARGET_ID = 2;

/** A shot as the store holds it after a binary load or refresh. */
const storeShot = (id: number, length: number): Shot =>
  makeShot({
    id,
    azimuth_in: f32(123.4),
    azimuth_out: f32(303.3),
    length: f32(length),
    depth_in: f32(10.1),
    depth_out: f32(11.7),
    pitch_in: f32(-3.3),
    pitch_out: f32(3.1),
    left: f32(0.7),
    right: f32(1.3),
    up: f32(0.2),
    down: f32(2.9),
  });

const section = (id: number, shots: Shot[]) => ({
  id,
  name: `Section${id}`,
  surveyor: null,
  date: null,
  color: '#ffffff',
  direction: 'in',
  quality: 1,
  next_shot_id: shots.length,
  reverse_parity: false,
  gauge_shot_uuid: null,
  gauge_neighbour_uuid: null,
  azimuth_reference: null,
  shots,
});

const PROJECT = {
  name: 'CaveA',
  sections: [section(ORPHAN_ID, [storeShot(0, 7)]), section(TARGET_ID, [storeShot(0, 1.2), storeShot(1, 4)])],
  station_merges: [],
  zero_datums: [],
  loop_closures: [],
} as unknown as Project;

/** The orphan's first shot overlaps the target's first shot. */
const SUGGESTION: OrphanPlacementSuggestion = {
  orphanSectionId: ORPHAN_ID,
  orphanSectionName: 'Section1',
  orphanStationIndex: 0,
  targetSectionId: TARGET_ID,
  targetSectionName: 'Section2',
  targetStationIndex: 0,
  score: 0.9,
  matchedShots: 1,
  existingMatchedShots: 1,
  preferredMergeOffset: 1,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
};

let updateShot: ReturnType<typeof vi.fn>;

beforeEach(() => {
  const ok = async (): Promise<CommandOutcome<EditOutcome>> => accepted<EditOutcome>({ warnings: [], remote_link: null });
  updateShot = vi.fn(ok);
  useProjectStore.setState({
    project: PROJECT,
    mergeStations: vi.fn(ok),
    updateShot,
    setShotKind: vi.fn(ok),
    refreshProjectState: vi.fn(async () => {}),
    saveAfterImport: vi.fn(async () => true),
  });
  useUIStore.setState({ closeDialog: vi.fn(), setView: vi.fn(), triggerFitToBounds: vi.fn() });
});

describe('OrphanPlacementDialog - overlap treatment sends only the changed readings', () => {
  it('"use imported data" sends the one reading the orphan changes', async () => {
    render(<OrphanPlacementDialog suggestions={[SUGGESTION]} />);
    await userEvent.click(screen.getByTestId('overlap-mode-select'));
    await userEvent.click(screen.getByTestId('overlap-mode-select-option-import_data'));
    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() => expect(updateShot).toHaveBeenCalled());
    const [sectionId, shotId, updates] = updateShot.mock.calls[0]!;
    console.log('orphan import_data update_shot patch:', JSON.stringify(updates));
    expect([sectionId, shotId]).toEqual([TARGET_ID, 0]);
    expect(updates).toEqual({ length: f32(7) });
  });
});
