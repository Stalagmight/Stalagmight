/**
 * #534: `ZeroDatumConfigDialog`'s OWN Back (to `zero-datum-selection`, not
 * the wizard's) discarded everything the user had typed - App.tsx relayed no
 * carry through that hop, and the forward pass from selection started blank.
 *
 * The fix carries this dialog's LIVE state (not just what it opened with) via
 * `onBack`, and App.tsx's routing decides whether a forward pass re-seeds it:
 * only a same-line re-pick does. This suite mounts the REAL
 * `ZeroDatumSelectionDialog` and `ZeroDatumConfigDialog`, driven by the REAL
 * `uiStore`, through a harness that calls the exact PRODUCTION functions
 * App.tsx calls (`carryZeroDatumConfig`, `readCarriedZeroDatumConfig`,
 * `ZERO_DATUM_CARRY_KEY` from `src/utils/zeroDatumCarry.ts`) rather than
 * re-implementing the carry decision locally - a harness that reimplements it
 * stays green when the production call site is deleted, which is exactly
 * what the first draft of this suite did.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: 33 + index * 33,
        size: 33,
      })),
    getTotalSize: () => count * 33,
    scrollToIndex: () => {},
  }),
}));
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../LinePreview', () => ({ default: () => null }));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({
    children,
    ...props
  }: {
    children: React.ReactNode | ((p: { requestClose: () => void }) => React.ReactNode);
    [k: string]: unknown;
  }) => {
    void props;
    return <div>{typeof children === 'function' ? children({ requestClose: () => {} }) : children}</div>;
  },
}));

import { useUIStore } from '../../../store/uiStore';
import { ZERO_DATUM_CARRY_KEY, readCarriedZeroDatumConfig, carryZeroDatumConfig } from '../../../utils/zeroDatumCarry';
import ZeroDatumSelectionDialog from '../ZeroDatumSelectionDialog';
import ZeroDatumConfigDialog from '../ZeroDatumConfigDialog';
import type { DmpFile, DmpSection, DmpShot } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

function dmpShot(): DmpShot {
  return {
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
  };
}

function dmpSection(name: string, hash: string): DmpSection {
  return {
    version: 1,
    name,
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: 3 }, dmpShot),
    content_hash: hash,
  };
}

function dmpFile(): DmpFile {
  return {
    version: 1,
    lines: [dmpSection('Main Passage', 'hash-a'), dmpSection('Side Tunnel', 'hash-b')],
  } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0, 1]),
  defaultSurveyor: 'DefaultSurveyor',
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

/** Mirrors App.tsx's `zero-datum-selection` / `zero-datum-config` routing
 *  (the `onBack` prop around App.tsx:855 and `onSelectLine` around
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
        onSelectLine={(lineIndex) => {
          openDialog('zero-datum-config', {
            dmpFile: dialogData.dmpFile,
            selectionConfig: dialogData.selectionConfig,
            zeroDatumLineIndex: lineIndex,
            [ZERO_DATUM_CARRY_KEY]: carryZeroDatumConfig(readCarriedZeroDatumConfig(dialogData), lineIndex),
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
        lineIndex={dialogData.zeroDatumLineIndex as number}
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
  it('Back-to-selection then same-line-forward shows the LIVE (not just carried-in) GPS and name', () => {
    useUIStore.getState().openDialog('zero-datum-config', {
      dmpFile: dmpFile(),
      selectionConfig,
      zeroDatumLineIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: '24.550123, -81.780456' },
    });
    fireEvent.change(screen.getByTestId('line-name-input'), { target: { value: 'Typed Name' } });

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-line-card-0'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('24.550123, -81.780456');
    expect(screen.getByTestId('line-name-input')).toHaveValue('Typed Name');
  });

  /**
   * Injection: in `carryZeroDatumConfig` (src/utils/zeroDatumCarry.ts), drop
   * the `carried?.lineIndex === lineIndex` check (always return
   * `carried`) - a different-line re-pick then still shows 'Typed Name'.
   */
  it('Back-to-selection then different-line-forward starts fresh', () => {
    useUIStore.getState().openDialog('zero-datum-config', {
      dmpFile: dmpFile(),
      selectionConfig,
      zeroDatumLineIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: '24.550123, -81.780456' },
    });
    fireEvent.change(screen.getByTestId('line-name-input'), { target: { value: 'Typed Name' } });

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-line-card-1'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('');
    expect(screen.getByTestId('line-name-input')).toHaveValue('Side Tunnel');
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
      zeroDatumLineIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: 'not a coordinate' },
    });
    expect(screen.queryByTestId('zero-datum-gps-parsed')).not.toBeInTheDocument();

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-line-card-0'));

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
      zeroDatumLineIndex: 0,
    });
    render(<Harness />);

    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
      target: { value: 'not a coordinate' },
    });
    fireEvent.change(screen.getByTestId('zero-datum-depth-reference-input'), {
      target: { value: '-12.5' },
    });

    clickBack();
    fireEvent.click(screen.getByTestId('zero-datum-line-card-0'));

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

    fireEvent.click(screen.getByTestId('zero-datum-line-card-0'));

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('');
    expect(screen.getByTestId('line-name-input')).toHaveValue('Main Passage');
  });
});
