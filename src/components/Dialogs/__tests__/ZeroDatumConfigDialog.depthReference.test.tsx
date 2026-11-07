/**
 * The zero-datum depth/elevation reference input (#524).
 *
 * Every GPS coordinate constructor in `src/utils/gps.ts` hardcoded
 * `depth_reference: 0`, so a zero datum could never carry a known depth below
 * the surface even when the surveyor knew it - `GpsCoordinates.depth_reference`
 * is +down, meters below the surface (docs/inclination.md; NOT the same sign
 * as #465's project-level "entrance elevation", which is +up from sea level).
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';

vi.mock('@tanstack/react-virtual', () => import('../../../test/factories/virtualizer'));
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

vi.mock('../../common/SectionMetadataForm', () => ({ SectionMetadataForm: () => null }));

import ZeroDatumConfigDialog, {
  type ZeroDatumConfigResult,
  type ZeroDatumConfigDraft,
} from '../ZeroDatumConfigDialog';
import type { DmpFile, DmpSection } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function dmpFile(): DmpFile {
  const section: DmpSection = makeDmpSection({ shots: makeDmpShots(3) });
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyor: 'S',
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

const onConfirm = vi.fn<(result: ZeroDatumConfigResult) => void>();

function renderDialog(initialConfig?: ZeroDatumConfigDraft, onBack = vi.fn()) {
  render(
    <ZeroDatumConfigDialog
      dmpFile={dmpFile()}
      selectionConfig={selectionConfig}
      sectionIndex={0}
      filePath="/tmp/anchor.dmp"
      onCancel={vi.fn()}
      onBack={onBack}
      onConfirm={onConfirm}
      initialConfig={initialConfig}
    />,
  );
}

function enterGps(value = '20.2, -87.4') {
  fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value } });
}

function enterDepthReference(value: string) {
  fireEvent.change(screen.getByTestId('zero-datum-depth-reference-input'), {
    target: { value },
  });
}

describe('ZeroDatumConfigDialog depth reference (#524)', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('blank input keeps today\'s behaviour: depth_reference 0', () => {
    renderDialog();
    enterGps();

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm.mock.calls[0]![0].gps.depth_reference).toBe(0);
  });

  it('threads an entered depth into the confirmed gps.depth_reference, unflipped', () => {
    // Sign-convention pin: entering a positive depth (below the surface, the
    // app-wide +down frame) must produce the SAME positive number, not a
    // negated one. A regression that flips the sign would still pass a mere
    // "is it non-zero" assertion, so this checks the exact value.
    renderDialog();
    enterGps();
    enterDepthReference('12.5');

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm.mock.calls[0]![0].gps.depth_reference).toBe(12.5);
  });

  it('threads the depth through when it is entered BEFORE the GPS text', () => {
    renderDialog();
    enterDepthReference('8');
    enterGps();

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm.mock.calls[0]![0].gps.depth_reference).toBe(8);
  });

  it('accepts a negative depth reference (above the surface) unmodified', () => {
    renderDialog();
    enterGps();
    enterDepthReference('-4');

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm.mock.calls[0]![0].gps.depth_reference).toBe(-4);
  });

  it('blocks Confirm on an unparsable depth reference', () => {
    renderDialog();
    enterGps();
    enterDepthReference('not-a-number');

    expect(screen.getByTestId('zero-datum-config-confirm')).toBeDisabled();
    expect(screen.getByTestId('zero-datum-depth-reference-error')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));
    expect(onConfirm).not.toHaveBeenCalled();
  });

  // #524 review: `Number.isFinite` was the only gate, and finiteness is not a
  // range - 1e300 is finite and travelled through the confirm, into the
  // project's geometry, and into every distance derived from it. The backend
  // (`GpsCoordinates::validate`) refuses it now, so accepting it here would
  // only produce a Confirm that fails at the far end.
  it('blocks Confirm on a depth reference outside any earthly range', () => {
    renderDialog();
    enterGps();

    for (const value of ['1e300', '-1e300', '11001']) {
      enterDepthReference(value);
      expect(Number.isFinite(Number(value))).toBe(true);
      expect(screen.getByTestId('zero-datum-config-confirm')).toBeDisabled();
      expect(screen.getByTestId('zero-datum-depth-reference-error')).toBeInTheDocument();
    }

    // The bound itself is still fine, and so is a datum above the surface.
    for (const value of ['11000', '-11000', '-4']) {
      enterDepthReference(value);
      expect(screen.getByTestId('zero-datum-config-confirm')).toBeEnabled();
    }

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm.mock.calls[0]![0].gps.depth_reference).toBe(-4);
  });

  it('is dirty once a depth reference is typed, and the guard is field-specific', () => {
    renderDialog();
    enterGps();
    expect(screen.getByTestId('zero-datum-depth-reference-input')).toHaveValue('');

    enterDepthReference('3');
    expect(screen.getByTestId('zero-datum-depth-reference-input')).toHaveValue('3');
  });
});

/**
 * #566 review 3: every other in-progress field survives this dialog's own Back
 * (#534) - the GPS box even carries its RAW TEXT so an unparsed entry comes
 * back. The depth reference did not: `buildCurrentDraft` never carried it, and
 * the re-entry could only reconstruct it from `initialConfig.gps.depth_reference`,
 * which does not exist until the GPS box parses AND the user confirms.
 */
describe('ZeroDatumConfigDialog depth reference survives Back (#566)', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('carries a depth typed while the GPS box has not parsed', () => {
    const onBack = vi.fn<(draft: ZeroDatumConfigDraft) => void>();
    renderDialog(undefined, onBack);

    // GPS deliberately unparsable, so nothing can reconstruct the depth from
    // `gps.depth_reference` on the way back in.
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: 'not coordinates' },
    });
    enterDepthReference('-12.5');

    fireEvent.click(screen.getByTestId('zero-datum-config-back'));
    // The dirty guard is up (fields were touched), so Back asks first.
    fireEvent.click(screen.getByTestId('zero-datum-config-back-confirm-discard'));

    expect(onBack).toHaveBeenCalledTimes(1);
    const draft = onBack.mock.calls[0]![0];
    expect(draft.gps).toBeNull();
    expect(draft.depthReferenceText).toBe('-12.5');
  });

  it('re-seeds the typed depth on a same-section forward pass', () => {
    const draft: ZeroDatumConfigDraft = {
      sectionIndex: 0,
      stationIndex: 0,
      gps: null,
      gpsInputText: 'not coordinates',
      depthReferenceText: '-12.5',
      metadata: {
        name: 'S',
        surveyor: null,
        date: null,
        color: '#3b82f6',
        direction: 'in',
        quality: 1.0,
        reverse: false,
      },
      shots: [],
    } as unknown as ZeroDatumConfigDraft;

    renderDialog(draft);
    expect(screen.getByTestId('zero-datum-depth-reference-input')).toHaveValue('-12.5');
  });

  /**
   * The gps-derived fallback stays for the WIZARD's one-hop carry, which hands
   * back a confirmed `ZeroDatumConfigResult` with no raw text at all. It also
   * takes over the falsy-check quirk deliberately: a stored depth of exactly 0
   * is "recorded as the surface", not "blank".
   */
  it('falls back to the confirmed depth when there is no carried text', () => {
    const confirmed = {
      sectionIndex: 0,
      stationIndex: 0,
      gps: { latitude: 20.2, longitude: -87.4, depth_reference: -4 },
      metadata: {
        name: 'S',
        surveyor: null,
        date: null,
        color: '#3b82f6',
        direction: 'in',
        quality: 1.0,
        reverse: false,
      },
      shots: [],
    } as unknown as ZeroDatumConfigDraft;

    renderDialog(confirmed);
    expect(screen.getByTestId('zero-datum-depth-reference-input')).toHaveValue('-4');
  });

  it('shows an explicit zero rather than a blank box', () => {
    const atSurface = {
      sectionIndex: 0,
      stationIndex: 0,
      gps: { latitude: 20.2, longitude: -87.4, depth_reference: 0 },
      metadata: {
        name: 'S',
        surveyor: null,
        date: null,
        color: '#3b82f6',
        direction: 'in',
        quality: 1.0,
        reverse: false,
      },
      shots: [],
    } as unknown as ZeroDatumConfigDraft;

    renderDialog(atSurface);
    expect(screen.getByTestId('zero-datum-depth-reference-input')).toHaveValue('0');
  });
});
