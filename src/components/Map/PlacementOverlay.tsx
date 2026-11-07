import { useMemo } from 'react';
import type { SectionData } from '../../types/project';
import type { AzimuthReference } from '../../types/ipc/AzimuthReference';
import { getThemeColor } from '../../utils/themeColors';
import { azimuthReferenceFor, walkStations, type WalkedStation } from '../../utils/stationWalk';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore, type AdditionalPlacementSection } from '../../store/uiStore';
import type { PixelsPerMeter } from './placementScale';

interface PlacementOverlayProps {
  /** Section data to preview */
  sectionData: SectionData;
  /** Which station index aligns to cursor (0 = first, N = last) */
  alignStationIndex: number;
  /** Current cursor position in screen pixels */
  cursorPosition: { x: number; y: number } | null;
  /** Whether snapped to an existing station */
  isSnapped: boolean;
  /** Pixels per meter east and north at current map zoom (for to-scale rendering) */
  pixelsPerMeter: PixelsPerMeter;
  /** Additional sections to render alongside primary (for segment placement) */
  additionalSections?: AdditionalPlacementSection[] | undefined;
  /** Callback to cancel placement (shown as button on touch devices) */
  onCancel?: () => void;
  /**
   * The north reference the section(s) being placed WILL be recorded with, if
   * the import sets one (a DMP import: magnetic, no correction). Omitted, the
   * section records none and the backend walks it with the project default.
   */
  azimuthReference?: AzimuthReference | null | undefined;
}

/**
 * Overlay component that renders a section preview following the cursor
 * during manual placement mode. The preview is rendered to-scale with the map.
 */
export default function PlacementOverlay({
  sectionData,
  alignStationIndex,
  cursorPosition,
  isSnapped,
  pixelsPerMeter,
  additionalSections,
  onCancel,
  azimuthReference,
}: PlacementOverlayProps) {
  // Walked the way the backend will place the section (#576 D15): the shared
  // walk, with the reference it will be recorded with, else the project default.
  // This preview used to keep its own walk - `azimuth_in` alone, the slope
  // length, no reference - so the section landed off where it was aimed.
  const projectDefault = useProjectStore((s) => s.project?.default_azimuth_reference);
  const reference = azimuthReferenceFor(azimuthReference, projectDefault);
  const stationPositions = useMemo(
    () => walkStations(sectionData.shots, reference),
    [sectionData.shots, reference]
  );

  // Clamp align station index to valid range
  const anchorIndex = Math.max(0, Math.min(alignStationIndex, stationPositions.length - 1));
  const anchorStation = stationPositions[anchorIndex];

  // Subscribe to theme for reactive color updates (value triggers re-render).
  // Above the early return: a hook after it would run on some renders and not
  // others, which React rejects as a changed hook order (#576 D1).
  useUIStore((s) => s.resolvedTheme);

  if (!cursorPosition || !anchorStation) {
    return null;
  }

  // Use the map's actual scale (pixels per meter, per axis) for to-scale
  // rendering: east and north differ on a GPS-anchored survey (#576 RV4-FE-MF2).
  const { east: scaleX, north: scaleY } = pixelsPerMeter;

  // Transform station to screen coordinates, centered on cursor
  const toScreen = (station: WalkedStation): { x: number; y: number } => {
    const relX = (station.x - anchorStation.x) * scaleX;
    const relY = -(station.y - anchorStation.y) * scaleY; // Flip Y for screen coordinates
    return {
      x: cursorPosition.x + relX,
      y: cursorPosition.y + relY,
    };
  };

  // Build SVG path for shots
  const pathData = stationPositions
    .map((station, i) => {
      const screen = toScreen(station);
      return i === 0 ? `M ${screen.x} ${screen.y}` : `L ${screen.x} ${screen.y}`;
    })
    .join(' ');

  const snapColor = getThemeColor('--cc-snap-color');
  const crosshairColor = getThemeColor('--cc-compass-color');
  const stationStrokeColor = getThemeColor('--cc-stroke-color');

  // Color for preview (use section color with transparency)
  const strokeColor = sectionData.color;
  const fillColor = isSnapped ? snapColor : sectionData.color;

  return (
    <>
    <svg
      className="absolute inset-0 pointer-events-none"
      style={{ width: '100%', height: '100%', overflow: 'visible' }}
      data-testid="placement-preview"
    >
      {/* Shot lines */}
      <path
        d={pathData}
        fill="none"
        stroke={strokeColor}
        strokeWidth={2}
        strokeOpacity={0.8}
        strokeDasharray="4 2"
      />

      {/* Station markers */}
      {stationPositions.map((station, i) => {
        const screen = toScreen(station);
        const isAnchor = i === anchorIndex;
        return (
          <circle
            key={i}
            cx={screen.x}
            cy={screen.y}
            r={isAnchor ? 8 : 4}
            fill={isAnchor ? fillColor : strokeColor}
            fillOpacity={0.8}
            stroke={isSnapped && isAnchor ? snapColor : stationStrokeColor}
            strokeWidth={isAnchor ? 2 : 1}
          />
        );
      })}

      {/* Additional sections (segment context) */}
      {additionalSections?.map((addSection, secIdx) => {
        // A segment's sections come from one import, recorded with one reference.
        const addPositions = walkStations(addSection.sectionData.shots, reference);
        // Transform with offset relative to primary section's anchor station
        const toScreenAdd = (station: WalkedStation): { x: number; y: number } => {
          const relX = (station.x + addSection.offsetX - anchorStation.x) * scaleX;
          const relY = -(station.y + addSection.offsetY - anchorStation.y) * scaleY;
          return {
            x: cursorPosition.x + relX,
            y: cursorPosition.y + relY,
          };
        };

        const addPathData = addPositions
          .map((station, i) => {
            const screen = toScreenAdd(station);
            return i === 0 ? `M ${screen.x} ${screen.y}` : `L ${screen.x} ${screen.y}`;
          })
          .join(' ');

        return (
          <g key={secIdx}>
            <path
              d={addPathData}
              fill="none"
              stroke={addSection.color}
              strokeWidth={1.5}
              strokeOpacity={0.6}
              strokeDasharray="4 2"
            />
            {addPositions.map((station, i) => {
              const screen = toScreenAdd(station);
              return (
                <circle
                  key={i}
                  cx={screen.x}
                  cy={screen.y}
                  r={3}
                  fill={addSection.color}
                  fillOpacity={0.6}
                  stroke={stationStrokeColor}
                  strokeWidth={0.5}
                />
              );
            })}
          </g>
        );
      })}

      {/* Anchor indicator (crosshair) */}
      <g>
        <line
          x1={cursorPosition.x - 12}
          y1={cursorPosition.y}
          x2={cursorPosition.x - 5}
          y2={cursorPosition.y}
          stroke={isSnapped ? snapColor : crosshairColor}
          strokeWidth={2}
        />
        <line
          x1={cursorPosition.x + 5}
          y1={cursorPosition.y}
          x2={cursorPosition.x + 12}
          y2={cursorPosition.y}
          stroke={isSnapped ? snapColor : crosshairColor}
          strokeWidth={2}
        />
        <line
          x1={cursorPosition.x}
          y1={cursorPosition.y - 12}
          x2={cursorPosition.x}
          y2={cursorPosition.y - 5}
          stroke={isSnapped ? snapColor : crosshairColor}
          strokeWidth={2}
        />
        <line
          x1={cursorPosition.x}
          y1={cursorPosition.y + 5}
          x2={cursorPosition.x}
          y2={cursorPosition.y + 12}
          stroke={isSnapped ? snapColor : crosshairColor}
          strokeWidth={2}
        />
      </g>
    </svg>
    {onCancel && navigator.maxTouchPoints > 0 && (
      <button
        className="absolute bottom-4 left-1/2 -translate-x-1/2 pointer-events-auto bg-error text-accent-on px-4 py-2 rounded shadow-lg text-sm font-medium"
        onClick={onCancel}
        data-testid="placement-cancel-touch"
      >
        Cancel
      </button>
    )}
    </>
  );
}
