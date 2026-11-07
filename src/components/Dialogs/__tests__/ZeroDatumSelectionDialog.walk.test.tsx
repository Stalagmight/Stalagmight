/**
 * The zero-datum section cards draw each section's shape with the backend's
 * walk (#576 D15): a shot's PLAN length is its tape projected by its depth
 * change, not the tape itself. The card used to walk the slope length, so a
 * steep leg drew up to its full tape length and the shape the user picks a
 * section by was not the shape the import builds.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ config: null }),
}));

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ testId: true }),
);

import ZeroDatumSelectionDialog from '../ZeroDatumSelectionDialog';
import type { DmpFile, DmpShot } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { makeDmpShot } from '../../../test/factories/dmp';

function dmpShot(heading: number, length: number, depthIn: number, depthOut: number): DmpShot {
  return makeDmpShot({ heading_in: heading, heading_out: heading, length, depth_in: depthIn, depth_out: depthOut });
}

/** The extent of the card's drawn centreline, in SVG pixels. */
function drawnExtent(shots: DmpShot[]): { width: number; height: number } {
  const dmpFile = {
    version: 1,
    sections: [
      { version: 1, name: 'CaveA', direction: 'in', date: '2026-01-01', shots, content_hash: 'h' },
    ],
  } as unknown as DmpFile;
  const { container } = render(
    <ZeroDatumSelectionDialog
      dmpFile={dmpFile}
      selectionConfig={{ selectedIndices: new Set([0]) } as unknown as DmpSelectionConfig}
      filePath="/tmp/CaveA.dmp"
      onCancel={vi.fn()}
      onBack={vi.fn()}
      onSkip={vi.fn()}
      onSelectSection={vi.fn()}
    />
  );
  const d = container.querySelector('[data-testid="zero-datum-section-card-0"] path')?.getAttribute('d');
  if (!d) throw new Error('the card drew no centreline');
  const points = [...d.matchAll(/[ML](-?[\d.]+),(-?[\d.]+)/g)].map((m) => [Number(m[1]), Number(m[2])]);
  const xs = points.map(([x]) => x!);
  const ys = points.map(([, y]) => y!);
  return { width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys) };
}

describe('ZeroDatumSelectionDialog section card shape', () => {
  it('draws a steep leg by its plan length', () => {
    // 10 m east dropping 6 m (8 m in plan), then 8 m north on the flat: in plan
    // a right angle with two EQUAL legs, so the drawing is as tall as it is
    // wide. Walked by the tape it is 10 x 8.
    const { width, height } = drawnExtent([dmpShot(90, 10, 0, 6), dmpShot(0, 8, 6, 6)]);
    expect(width).toBeGreaterThan(0);
    expect(height).toBeCloseTo(width, 0);
  });
});
