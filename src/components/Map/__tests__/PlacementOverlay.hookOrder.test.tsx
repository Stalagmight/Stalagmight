/**
 * PlacementOverlay called its theme hook AFTER an early return (#576 D1, the
 * one react-hooks/rules-of-hooks error ESLint found on adoption). A render that
 * returned early ran one hook fewer than a render that did not, so the first
 * cursor position after a null one changed the hook order and React threw.
 * MapCanvas only mounts it with a cursor today, so this pins the component's
 * own contract: the cursor may come and go without a crash.
 */
import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/react';
import PlacementOverlay from '../PlacementOverlay';
import type { SectionData } from '../../../types/project';

const section = {
  color: '#4ecdc4',
  shots: [{ azimuth_in: 90, length: 10, depth_in: 0, depth_out: 1 }],
} as unknown as SectionData;

function overlay(cursor: { x: number; y: number } | null) {
  return (
    <PlacementOverlay
      sectionData={section}
      alignStationIndex={0}
      cursorPosition={cursor}
      isSnapped={false}
      pixelsPerMeter={{ east: 2, north: 2 }}
    />
  );
}

describe('PlacementOverlay hook order', () => {
  it('renders the preview when the cursor arrives after a render without one', () => {
    const { rerender, queryByTestId } = render(overlay(null));
    expect(queryByTestId('placement-preview')).toBeNull();

    rerender(overlay({ x: 100, y: 100 }));
    expect(queryByTestId('placement-preview')).not.toBeNull();

    rerender(overlay(null));
    expect(queryByTestId('placement-preview')).toBeNull();
  });
});
