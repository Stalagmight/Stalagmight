/**
 * EntityBlob - the entity "pill" used for ids in the data table. Same style as
 * the detail-panel link pills (entity symbol + tinted background + dim
 * entity-colored border), but with just the id text (`LineID:ID`) - no
 * divider and no separate suffix.
 *
 * Renders the pill only when the highlighting level is "full"; otherwise it
 * renders its children as plain text, so callers can wrap an id unconditionally.
 */
import type { CSSProperties } from 'react';
import { useUIStore } from '../../store/uiStore';
import type { BlobEntity } from '../../theme/syntacticPalette';
import { syntacticVarName, syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { EntitySymbol } from './toolIcons';

export interface EntityBlobProps {
  /** Which geometry entity this represents (drives the color + symbol). */
  entity: BlobEntity;
  /** The id text rendered in the pill. */
  children: React.ReactNode;
  /** Optional tooltip. */
  title?: string;
  /** Extra classes for the pill. */
  className?: string;
  testId?: string;
}

export function EntityBlob({ entity, children, title, className = '', testId }: EntityBlobProps) {
  const level = useUIStore((s) => s.syntacticLevel);

  // Only the Full level paints pills; otherwise keep the plain text rendering.
  if (!syntacticLevelAtLeast(level, 'full')) {
    return <>{children}</>;
  }

  return (
    <span
      className={`sh-pill inline-flex items-center gap-1 align-middle rounded-full px-2 py-0.5 text-xs leading-none text-text-primary tabular-nums ${className}`}
      style={{ '--o': `var(${syntacticVarName(entity)})` } as CSSProperties}
      data-entity={entity}
      data-testid={testId}
      title={title}
    >
      <span className="flex shrink-0 items-center">
        <EntitySymbol entity={entity} size={14} />
      </span>
      {children}
    </span>
  );
}

export default EntityBlob;
