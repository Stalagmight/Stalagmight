/**
 * #534: `ZeroDatumConfigDialog`'s OWN Back (to `zero-datum-selection`, not
 * the wizard's) discarded everything the user had typed - App.tsx relayed no
 * carry through that hop, and the forward pass from selection started blank.
 *
 * The fix carries this dialog's LIVE state (not just what it opened with) via
 * `onBack`, and App.tsx's routing decides whether a forward pass re-seeds it:
 * only a same-section re-pick does. This suite mounts the REAL
 * `ZeroDatumSelectionDialog` and `ZeroDatumConfigDialog`, driven by the REAL
 * `uiStore`, through a harness that calls the exact PRODUCTION functions
 * App.tsx calls (`carryZeroDatumConfig`, `readCarriedZeroDatumConfig`,
 * `ZERO_DATUM_CARRY_KEY` from `src/utils/zeroDatumCarry.ts`) rather than
 * re-implementing the carry decision locally - a harness that reimplements it
 * stays green when the production call site is deleted, which is exactly
 * what the first draft of this suite did.
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

import { useUIStore } from '../../../store/uiStore';
import { ZERO_DATUM_CARRY_KEY, readCarriedZeroDatumConfig, carryZeroDatumConfig } from '../../../utils/zeroDatumCarry';
import ZeroDatumSelectionDialog from '../ZeroDatumSelectionDialog';
import ZeroDatumConfigDialog from '../ZeroDatumConfigDialog';
import type { DmpFile, DmpSection } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function dmpSection(name: string, hash: string): DmpSection {
  return makeDmpSection({ name, shots: makeDmpShots(3), content_hash: hash });
}

function dmpFile(): DmpFile {
  return {
    version: 1,
    sections: [dmpSection('Main Passage', 'hash-a'), dmpSection('Side Tunnel', 'hash-b')],
  } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0, 1]),
  defaultSurveyor: 'DefaultSurveyor',
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

/** Mirrors App.tsx's `zero-datum-selection` / `zero-datum-config` routing
 *  (the `onBack` prop around App.tsx:855 and `onSelectSection` around
 *  App.tsx:825), calling the same production carry functions App.tsx does. */
function Harness() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const openDialog = useUIStore((s) => s.openDialog);

  if (activeDialog === 'zero-datum-selection') {
    return (
      <ZeroDatumSelectionDialog
        dmpFile={dialogData.dmpFile as DmpFile}
        selectionConfig={dialogData.selectionConfig as DmpSelectionConfig}
        filePath="/tmp/anchor.dmp"
        onCancel={() => {}}
        onBack={() => {}}
        onSkip={() => {}}
        onSelectSection={(sectionIndex) => {
          openDialog('zero-datum-config', {
            dmpFile: dialogData.dmpFile,
            selectionConfig: dialogData.selectionConfig,
            zeroDatumSectionIndex: sectionIndex,
            [ZERO_DATUM_CARRY_KEY]: carryZeroDatumConfig(readCarriedZeroDatumConfig(dialogData), sectionIndex),
          });
        }}
      />
    );
  }
  if (activeDialog === 'zero-datum-config') {
    return (
      <ZeroDatumConfigDialog
        dmpFile={dialogData.dmpFile as DmpFile}
        selectionConfig={dialogData.selectionConfig as DmpSelectionConfig}
        sectionIndex={dialogData.zeroDatumSectionIndex as number}
        filePath="/tmp/anchor.dmp"
        initialConfig={readCarriedZeroDatumConfig(dialogData)}
        onCancel={() => {}}
        onBack={(current) => {
          openDialog('zero-datum-selection', {
            dmpFile: dialogData.dmpFile,
            selectionConfig: dialogData.selectionConfig,
            [ZERO_DATUM_CARRY_KEY]: current,
          });
        }}
        onConfirm={() => {}}
      />
    );
  }
  return null;
}

/** Drive Back on the config dialog: dirty, so it goes through its own
 *  discard confirm first. */
function clickBack() {
  fireEvent.click(screen.getByTestId('zero-datum-config-back'));
  fireEvent.click(screen.getByTestId('zero-datum-config-back-confirm-discard'));
}

describe('zero-datum-config Back-to-selection carry (#534)', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    useUIStore.setState({ activeDialog: null, dialogData: {} });
  });

  /**
   * Injection: in App.tsx's `onBack` for `zero-datum-config`, pass `{}`
   * instead of `[ZERO_DATUM_CARRY_KEY]: current` - the selection dialog then
   * has nothing to hand back, and the reopened config dialog shows the file
   * defaults instead of what was typed.
   */
  it('Back-to-selection then same-section-forward shows the LIVE (not just carried-in) GPS and name', () => {
    useUIStore.getState().openDialog('zero-datum-config', {
      dmpFile: dmpFile(),
      selectionConfig,
      zeroDatumSectionIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: '24.550123, -81.780456' },
    });
    fireEvent.change(screen.getByTestId('section-name-input'), { target: { value: 'Typed Name' } });

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-section-card-0'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('24.550123, -81.780456');
    expect(screen.getByTestId('section-name-input')).toHaveValue('Typed Name');
  });

  /**
   * Injection: in `carryZeroDatumConfig` (src/utils/zeroDatumCarry.ts), drop
   * the `carried?.sectionIndex === sectionIndex` check (always return
   * `carried`) - a different-section re-pick then still shows 'Typed Name'.
   */
  it('Back-to-selection then different-section-forward starts fresh', () => {
    useUIStore.getState().openDialog('zero-datum-config', {
      dmpFile: dmpFile(),
      selectionConfig,
      zeroDatumSectionIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: '24.550123, -81.780456' },
    });
    fireEvent.change(screen.getByTestId('section-name-input'), { target: { value: 'Typed Name' } });

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-section-card-1'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('');
    expect(screen.getByTestId('section-name-input')).toHaveValue('Side Tunnel');
  });

  /**
   * The GPS box never parsed - `gpsCoords` stayed null - so the carry has to
   * ride on the raw text (`gpsInputText`), not the parsed pair.
   *
   * Injection: in `buildCurrentDraft` (ZeroDatumConfigDialog.tsx), carry
   * `gps` alone and drop `gpsInputText` - the reopened box comes back empty
   * instead of holding the unparsed text.
   */
  it('carries the raw GPS text through Back even when it never parsed', () => {
    useUIStore.getState().openDialog('zero-datum-config', {
      dmpFile: dmpFile(),
      selectionConfig,
      zeroDatumSectionIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: 'not a coordinate' },
    });
    expect(screen.queryByTestId('zero-datum-gps-parsed')).not.toBeInTheDocument();

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-section-card-0'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('not a coordinate');
  });

  /**
   * #566 review 3: the depth/elevation box (#524) is the only in-progress
   * field this hop used to drop. It exists nowhere but the raw text until a
   * confirm mints `gps.depth_reference`, so a depth typed beside an unparsed
   * GPS entry had nothing to be reconstructed from.
   *
   * Driven through the SAME production carry functions as the GPS text above,
   * so it also pins that App.tsx relays the new field untouched: the carry is
   * opaque (`readCarriedZeroDatumConfig` casts the stored object whole), and a
   * relay that rebuilt the draft field-by-field would drop this and go red.
   */
  it('carries the raw depth text through Back even when the GPS never parsed', () => {
    useUIStore.getState().openDialog('zero-datum-config', {
      dmpFile: dmpFile(),
      selectionConfig,
      zeroDatumSectionIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: 'not a coordinate' },
    });
    fireEvent.change(screen.getByTestId('zero-datum-depth-reference-input'), {
      target: { value: '-12.5' },
    });

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-section-card-0'));

    expect(screen.getByTestId('zero-datum-depth-reference-input')).toHaveValue('-12.5');
    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('not a coordinate');
  });

  /**
   * Injection: seed `dialogData` with a leftover carry on `openDialog`'s
   * first-visit call (i.e. drop `openDialog`'s wholesale `dialogData` reset
   * in `uiStore.ts`) - a genuine first visit would then show stale values.
   */
  it('a genuine first visit into the selection dialog carries nothing', () => {
    useUIStore.getState().openDialog('zero-datum-selection', { dmpFile: dmpFile(), selectionConfig });
    render(<Harness />);

    fireEvent.click(screen.getByTestId('zero-datum-section-card-0'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('');
    expect(screen.getByTestId('section-name-input')).toHaveValue('Main Passage');
  });
});
