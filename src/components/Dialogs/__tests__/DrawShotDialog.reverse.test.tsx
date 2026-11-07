/**
 * The Draw Shot dialog's azimuth frame: what you type is what gets stored.
 *
 * This dialog used to reciprocate on the prepend path - it prefilled the copied
 * bearing turned by 180 and turned whatever you typed by another 180 on the way
 * out. The two cancelled for an untouched prefill, so the *default* stored the
 * right value and nothing downstream ever noticed; only a hand-typed bearing
 * came out 180 degrees from the number on screen. Nothing in the tree could
 * catch that: no vitest file and no Playwright spec opened this dialog, so
 * reverting the fix left the whole suite green.
 *
 * Hence these tests assert the two halves separately - the prefill and the
 * value handed to `createShot` - because a double reversal is invisible to a
 * test that only checks the stored end.
 *
 * Turning a leg around is now an explicit checkbox, and it must go through
 * `reverseTargetShot`: `azimuth_in`/`azimuth_out` are two readings of the same
 * FORWARD line (`src-tauri/src/models/shot.rs`), so a reversal swaps them as
 * well as adding 180, and swaps the ends of depth/pitch/LRUD with them. A
 * bare `+180` on each field - the rule this dialog used to carry - is a
 * different transform, and test 3 fails on it.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const createShot = vi.fn().mockResolvedValue({ success: true, shot_id: 99 });
const closeDialog = vi.fn();
const setSelection = vi.fn();

const SECTION_ID = 1;
const FIRST_SHOT_ID = 10;

/** Source shot the context menu copies geometry from: a bent, sloping leg. */
const SOURCE = {
  azimuth_in: 90,
  azimuth_out: 95,
  depth_in: 1,
  depth_out: 3,
  pitch_in: 5,
  pitch_out: 7,
};

const dialogData: Record<string, unknown> = {};

const project = {
  sections: [
    { id: SECTION_ID, name: 'Mainline', shots: [{ id: FIRST_SHOT_ID }] },
  ],
};
const stations = [
  { station_ref: { section_id: SECTION_ID, station_index: 0 }, coordinates: { z: 0 } },
];

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData, setSelection }),
}));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ createShot, project, stations }),
}));

vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryStubs());

vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

vi.mock('../../common/EntityBlob', () => ({
  EntityBlob: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
}));

import DrawShotDialog from '../DrawShotDialog';

/** Open in prepend mode, the path that used to reciprocate. */
function openPrepend() {
  for (const k of Object.keys(dialogData)) delete dialogData[k];
  Object.assign(dialogData, {
    fromStation: { section_id: SECTION_ID, station_index: 0 },
    prependMode: true,
    allowZeroLength: true,
    sourceGeometry: { ...SOURCE },
  });
}

/** The geometry `createShot` was handed (arg 1 of the last call). */
function storedShot(): Record<string, number | null> {
  expect(createShot).toHaveBeenCalledTimes(1);
  return createShot.mock.calls[0]![1] as Record<string, number | null>;
}

function setInput(testid: string, value: string) {
  fireEvent.change(screen.getByTestId(testid), { target: { value } });
}

describe('DrawShotDialog azimuth frame', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    openPrepend();
  });

  it('prefills the source bearings unreversed', () => {
    render(<DrawShotDialog />);

    // Injecting `normalizeAzimuth(base + 180)` back into the prefill turns
    // these into 270 / 275.
    expect(screen.getByTestId('draw-shot-azimuth-in-input')).toHaveValue(SOURCE.azimuth_in);
    expect(screen.getByTestId('draw-shot-azimuth-out-input')).toHaveValue(SOURCE.azimuth_out);
  });

  it('defaults the reverse checkbox to off and stores a typed bearing verbatim', async () => {
    render(<DrawShotDialog />);

    expect(screen.getByTestId('draw-shot-reverse-direction')).not.toBeChecked();

    // A bearing that is NOT the prefill: the old double reversal cancelled out
    // for the prefill, so only a typed value can catch a single reversal.
    setInput('draw-shot-azimuth-in-input', '20');
    setInput('draw-shot-azimuth-out-input', '20');
    setInput('draw-shot-length-input', '5');
    fireEvent.click(screen.getByTestId('draw-shot-create'));

    // Restoring `rawAzimuthIn + 180` on either the entry or the exit side
    // makes this 200.
    await waitFor(() => expect(storedShot()).toMatchObject({ azimuth_in: 20, azimuth_out: 20 }));
  });

  it('turns the whole shot around when the box is checked', async () => {
    render(<DrawShotDialog />);

    setInput('draw-shot-left-input', '2');
    setInput('draw-shot-right-input', '8');
    setInput('draw-shot-up-input', '4');
    setInput('draw-shot-down-input', '6');
    setInput('draw-shot-length-input', '5');
    fireEvent.click(screen.getByTestId('draw-shot-reverse-direction'));
    fireEvent.click(screen.getByTestId('draw-shot-create'));

    await waitFor(() =>
      expect(storedShot()).toMatchObject({
        // Swapped AND reciprocated. A bare +180 per field - the rule this
        // dialog used to carry - would give 270 / 275 and fail here.
        azimuth_in: 275,
        azimuth_out: 270,
        // The ends of the leg trade places with the bearings, which is the
        // whole reason this is one checkbox and not an azimuth-only one.
        depth_in: SOURCE.depth_out,
        depth_out: SOURCE.depth_in,
        pitch_in: -SOURCE.pitch_out,
        pitch_out: -SOURCE.pitch_in,
        left: 8,
        right: 2,
        // Facing the other way swaps left/right; up is still up.
        up: 4,
        down: 6,
        length: 5,
      }),
    );
  });

  it('leaves the shot untouched when the box is checked and then unchecked', async () => {
    render(<DrawShotDialog />);

    setInput('draw-shot-length-input', '5');
    const box = screen.getByTestId('draw-shot-reverse-direction');
    fireEvent.click(box);
    fireEvent.click(box);
    expect(box).not.toBeChecked();
    fireEvent.click(screen.getByTestId('draw-shot-create'));

    await waitFor(() =>
      expect(storedShot()).toMatchObject({
        azimuth_in: SOURCE.azimuth_in,
        azimuth_out: SOURCE.azimuth_out,
        depth_in: SOURCE.depth_in,
        depth_out: SOURCE.depth_out,
      }),
    );
  });

  it('offers the reverse checkbox outside prepend mode too', () => {
    for (const k of Object.keys(dialogData)) delete dialogData[k];
    Object.assign(dialogData, {
      fromStation: { section_id: SECTION_ID, station_index: 1 },
      appendMode: true,
    });

    render(<DrawShotDialog />);

    expect(screen.getByTestId('draw-shot-reverse-direction')).not.toBeChecked();
  });
});
