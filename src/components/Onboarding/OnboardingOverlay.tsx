import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import type { ActiveOnboardingStep } from '../../hooks/useOnboarding';
import type { OnboardingIllustration, OnboardingPlacement } from '../../onboarding/flows';
import {
  FitToViewIcon,
  MinusIcon,
  PlusIcon,
} from '../common/icons';
import { ToolGlyph } from '../common/toolIcons';
import { clamp } from '../../utils/clamp';
import { publishOnboardingStep } from '../../hooks/useOnboardingStep';
import { useProjectStore } from '../../store/projectStore';
import { navigationToolsEnabled } from '../../utils/toolUtils';

interface OnboardingOverlayProps {
  step: ActiveOnboardingStep;
  stepIndex: number;
  stepCount: number;
  canGoBack: boolean;
  onBack: () => void;
  onNext: () => void;
  onSkipCurrent: () => void;
  onSkipAll: () => void;
}

interface SpotlightRect {
  top: number;
  left: number;
  width: number;
  height: number;
}

interface RectBounds {
  top: number;
  left: number;
  right: number;
  bottom: number;
}

const SPOTLIGHT_PADDING = 10;
const SPOTLIGHT_RADIUS = 14;
const TOOLTIP_GAP = 18;
const TOOLTIP_DEFAULT_MAX_WIDTH = 360;
const VIEWPORT_MARGIN = 16;
const TOOLTIP_ESTIMATED_HEIGHT = 250;
const TOOLTIP_SETTLE_DELAY_MS = 220;
/**
 * How long to wait for a step's target before giving up on anchoring to it.
 * A step can out-live its target (the anchoring dialog closed, the control moved
 * behind a mode switch). Waiting forever leaves a spotlight-less dim with the
 * card hidden and `locked` swallowing every event - including the Escape that
 * would dismiss it, since the skip confirmation renders inside that same hidden
 * card. Give up and center the step instead, so Next/Skip stay reachable (#331).
 */
const TARGET_GIVE_UP_DELAY_MS = 1200;

function boundsFromDomRect(rect: DOMRect): RectBounds {
  return {
    top: rect.top,
    left: rect.left,
    right: rect.right,
    bottom: rect.bottom,
  };
}

function isNonEmptyBounds(bounds: RectBounds): boolean {
  return bounds.right > bounds.left && bounds.bottom > bounds.top;
}

function intersectBounds(a: RectBounds, b: RectBounds): RectBounds | null {
  const next: RectBounds = {
    top: Math.max(a.top, b.top),
    left: Math.max(a.left, b.left),
    right: Math.min(a.right, b.right),
    bottom: Math.min(a.bottom, b.bottom),
  };
  return isNonEmptyBounds(next) ? next : null;
}

function toSpotlightRect(bounds: RectBounds): SpotlightRect {
  return {
    top: bounds.top,
    left: bounds.left,
    width: bounds.right - bounds.left,
    height: bounds.bottom - bounds.top,
  };
}

function getElementClippingBounds(element: HTMLElement): RectBounds | null {
  if (typeof window === 'undefined') {
    return null;
  }

  let clippingBounds: RectBounds = {
    top: 0,
    left: 0,
    right: window.innerWidth,
    bottom: window.innerHeight,
  };

  let ancestor = element.parentElement;
  while (ancestor) {
    const style = window.getComputedStyle(ancestor);
    const clipsX = style.overflowX !== 'visible';
    const clipsY = style.overflowY !== 'visible';

    if (clipsX || clipsY) {
      const ancestorRect = ancestor.getBoundingClientRect();
      const ancestorBounds: RectBounds = {
        top: clipsY ? ancestorRect.top : Number.NEGATIVE_INFINITY,
        left: clipsX ? ancestorRect.left : Number.NEGATIVE_INFINITY,
        right: clipsX ? ancestorRect.right : Number.POSITIVE_INFINITY,
        bottom: clipsY ? ancestorRect.bottom : Number.POSITIVE_INFINITY,
      };
      const nextBounds = intersectBounds(clippingBounds, ancestorBounds);
      if (!nextBounds) {
        return null;
      }
      clippingBounds = nextBounds;
    }

    ancestor = ancestor.parentElement;
  }

  return clippingBounds;
}

function getVisibleSpotlightRect(element: HTMLElement): SpotlightRect | null {
  const elementBounds = boundsFromDomRect(element.getBoundingClientRect());
  if (!isNonEmptyBounds(elementBounds)) {
    return null;
  }

  const clippingBounds = getElementClippingBounds(element);
  if (!clippingBounds) {
    return null;
  }

  const visibleBounds = intersectBounds(elementBounds, clippingBounds);
  if (!visibleBounds) {
    return null;
  }

  const paddedBounds: RectBounds = {
    top: visibleBounds.top - SPOTLIGHT_PADDING,
    left: visibleBounds.left - SPOTLIGHT_PADDING,
    right: visibleBounds.right + SPOTLIGHT_PADDING,
    bottom: visibleBounds.bottom + SPOTLIGHT_PADDING,
  };

  const spotlightBounds = intersectBounds(paddedBounds, clippingBounds);
  return spotlightBounds ? toSpotlightRect(spotlightBounds) : null;
}

function combineSpotlightRects(rects: SpotlightRect[]): SpotlightRect | null {
  if (rects.length === 0) {
    return null;
  }
  const top = Math.min(...rects.map((rect) => rect.top));
  const left = Math.min(...rects.map((rect) => rect.left));
  const right = Math.max(...rects.map((rect) => rect.left + rect.width));
  const bottom = Math.max(...rects.map((rect) => rect.top + rect.height));
  return {
    top,
    left,
    width: right - left,
    height: bottom - top,
  };
}

function calculateTooltipStyle(
  targetRect: SpotlightRect | null,
  placement: OnboardingPlacement,
  viewportVersion: number,
  maxWidth: number = TOOLTIP_DEFAULT_MAX_WIDTH,
  offsetX: number = 0,
  tooltipHeight: number = TOOLTIP_ESTIMATED_HEIGHT
): CSSProperties {
  void viewportVersion;

  const viewportWidth = typeof window !== 'undefined' ? window.innerWidth : 1280;
  const viewportHeight = typeof window !== 'undefined' ? window.innerHeight : 720;
  const maxLeft = Math.max(VIEWPORT_MARGIN, viewportWidth - maxWidth - VIEWPORT_MARGIN);
  const maxTop = Math.max(VIEWPORT_MARGIN, viewportHeight - tooltipHeight - VIEWPORT_MARGIN);

  if (!targetRect || placement === 'center') {
    return {
      width: `min(${maxWidth}px, calc(100vw - ${VIEWPORT_MARGIN * 2}px))`,
      left: '50%',
      top: '50%',
      transform: 'translate(-50%, -50%)',
    };
  }

  const centerX = targetRect.left + targetRect.width / 2;
  const centerY = targetRect.top + targetRect.height / 2;
  const targetTop = targetRect.top;
  const targetBottom = targetRect.top + targetRect.height;

  let left = centerX - maxWidth / 2;
  let top = targetBottom + TOOLTIP_GAP;

  if (placement === 'top') {
    top = targetTop - tooltipHeight - TOOLTIP_GAP;
  } else if (placement === 'left') {
    left = targetRect.left - maxWidth - TOOLTIP_GAP;
    top = centerY - tooltipHeight / 2;
  } else if (placement === 'right') {
    left = targetRect.left + targetRect.width + TOOLTIP_GAP;
    top = centerY - tooltipHeight / 2;
  }

  if (placement === 'bottom' && top > maxTop) {
    top = targetRect.top - tooltipHeight - TOOLTIP_GAP;
  }
  if (placement === 'top' && top < VIEWPORT_MARGIN) {
    top = targetRect.top + targetRect.height + TOOLTIP_GAP;
  }
  if (placement === 'left' && left < VIEWPORT_MARGIN) {
    left = targetRect.left + targetRect.width + TOOLTIP_GAP;
  }
  if (placement === 'right' && left > maxLeft) {
    left = targetRect.left - maxWidth - TOOLTIP_GAP;
  }

  left += offsetX;

  return {
    width: `min(${maxWidth}px, calc(100vw - ${VIEWPORT_MARGIN * 2}px))`,
    left: clamp(left, VIEWPORT_MARGIN, maxLeft),
    top: clamp(top, VIEWPORT_MARGIN, maxTop),
  };
}

function renderMouseIcon(className: string, highlight: 'left' | 'wheel' | 'right'): JSX.Element {
  return (
    <svg viewBox="0 0 44 54" className={className} aria-hidden="true">
      <defs>
        <clipPath id="mouse-body-clip">
          <rect x="2.5" y="2.5" width="39" height="49" rx="18" />
        </clipPath>
      </defs>
      <rect x="2.5" y="2.5" width="39" height="49" rx="18" fill="#f8fafc" stroke="#111827" strokeWidth="2" />
      {highlight === 'left' ? (
        <path
          d="M21 21.5H3.5V3.5H21V21.5Z"
          fill="#ef4444"
          clipPath="url(#mouse-body-clip)"
        />
      ) : null}
      {highlight === 'right' ? (
        <path
          d="M40.5 21.5H22V3.5h18.5v18Z"
          fill="#ef4444"
          clipPath="url(#mouse-body-clip)"
        />
      ) : null}
      <line x1="22" y1="5.5" x2="22" y2="20.5" stroke="#9ca3af" strokeWidth="1.6" />
      <line x1="5.5" y1="22" x2="38.5" y2="22" stroke="#9ca3af" strokeWidth="1.4" />
      <rect
        x="18"
        y="7"
        width="8"
        height="12"
        rx="4"
        fill={highlight === 'wheel' ? '#ef4444' : '#9ca3af'}
        fillOpacity={highlight === 'wheel' ? 1 : 1}
      />
    </svg>
  );
}

/**
 * `showNavigationTools` mirrors `map.controls.show_navigation_tools` (#409): the
 * Pan/Zoom tools are off the toolbar unless the user opts in, so the tour must
 * not point at a group that is not on screen.
 */
function renderStepIllustration(
  illustration: OnboardingIllustration | undefined,
  t: (key: string) => string,
  showNavigationTools: boolean
): JSX.Element | null {
  if (illustration === 'map-with-pan-zoom') {
    return (
      <>
        {renderStepIllustration('mouse-pan-zoom', t, showNavigationTools)}
        {renderStepIllustration('map-controls', t, showNavigationTools)}
      </>
    );
  }

  if (illustration === 'tools-overview') {
    const toolItems = {
      select: {
        label: t('toolbar.tools.select'),
        description: t('onboarding.illustrations.tools.items.select'),
        icon: <ToolGlyph tool="select" size={16} />,
      },
      pan: {
        label: t('toolbar.tools.pan'),
        description: t('onboarding.illustrations.tools.items.pan'),
        icon: <ToolGlyph tool="pan" size={16} />,
      },
      zoom: {
        label: t('toolbar.tools.zoom'),
        description: t('onboarding.illustrations.tools.items.zoom'),
        icon: <ToolGlyph tool="zoom" size={16} />,
      },
      merge: {
        label: t('toolbar.tools.merge'),
        description: t('onboarding.illustrations.tools.items.merge'),
        icon: <ToolGlyph tool="merge" size={16} />,
      },
      measure: {
        label: t('toolbar.tools.measure'),
        description: t('onboarding.illustrations.tools.items.measure'),
        icon: <ToolGlyph tool="measure" size={16} />,
      },
      comment: {
        label: t('toolbar.tools.comment'),
        description: t('onboarding.illustrations.tools.items.comment'),
        icon: <ToolGlyph tool="comment" size={16} />,
      },
      moveComment: {
        label: t('toolbar.tools.moveComment'),
        description: t('onboarding.illustrations.tools.items.moveComment'),
        icon: <ToolGlyph tool="move_comment" size={16} />,
      },
      moveStation: {
        label: t('toolbar.tools.moveStation'),
        description: t('onboarding.illustrations.tools.items.moveStation'),
        icon: <ToolGlyph tool="move_station" size={16} />,
      },
      moveShot: {
        label: t('toolbar.tools.moveShot'),
        description: t('onboarding.illustrations.tools.items.moveShot'),
        icon: <ToolGlyph tool="move_shot" size={16} />,
      },
      moveSegment: {
        label: t('toolbar.tools.moveSegment'),
        description: t('onboarding.illustrations.tools.items.moveSegment'),
        icon: <ToolGlyph tool="move_segment" size={16} />,
      },
    };

    const allGroups: { id: string; title: string; items: Array<keyof typeof toolItems> }[] = [
      {
        id: 'select',
        title: t('onboarding.illustrations.tools.groups.select'),
        items: ['select'],
      },
      {
        id: 'navigate',
        title: t('toolbar.groups.navigate'),
        items: ['pan', 'zoom'],
      },
      {
        id: 'edit',
        title: t('onboarding.illustrations.tools.groups.edit'),
        items: ['merge', 'measure'],
      },
      {
        id: 'comment',
        title: t('toolbar.groups.comment'),
        items: ['comment', 'moveComment'],
      },
      {
        id: 'move',
        title: t('toolbar.groups.move'),
        items: ['moveStation', 'moveShot', 'moveSegment'],
      },
    ];
    // Drop the navigate group when the toolbar drops it (#409). `groupColumns`
    // still names it; the lookup below already skips a column entry with no
    // group, so the first column degrades to Select alone rather than breaking.
    const groups = showNavigationTools
      ? allGroups
      : allGroups.filter((group) => group.id !== 'navigate');

    const groupById = Object.fromEntries(groups.map((group) => [group.id, group]));
    const groupIcons: Record<string, JSX.Element | null> = {
      select: <ToolGlyph tool="select" size={14} />,
      navigate: <ToolGlyph tool="pan" size={14} />,
      comment: <ToolGlyph tool="comment" size={14} />,
      move: <ToolGlyph tool="move_segment" size={14} />,
      edit: null,
    };
    const groupColumns = [
      ['select', 'navigate'],
      ['edit'],
      ['comment', 'move'],
    ] as const;

    return (
      <div
        className="mb-2 rounded-md border border-border bg-background p-2"
        data-testid="onboarding-illustration-tools-overview"
      >
        <div className="mb-2 text-xs font-semibold text-text-primary">
          {t('onboarding.illustrations.tools.title')}
        </div>
        <div className="grid grid-cols-3 gap-2">
          {groupColumns.map((column, columnIndex) => (
            <div key={`group-column-${columnIndex}`} className="space-y-2">
              {column.map((groupId) => {
                const group = groupById[groupId];
                if (!group) {
                  return null;
                }
                return (
                  <div key={group.id} className="rounded border border-border bg-background-30 px-2 py-1.5">
                    <div className="mb-1.5 border-b border-border pb-1 text-xs font-semibold uppercase tracking-wide text-text-secondary">
                      <span className="inline-flex items-center gap-1">
                        {groupIcons[group.id] ? (
                          <span className="text-accent">{groupIcons[group.id]}</span>
                        ) : null}
                        <span>{group.title}</span>
                      </span>
                    </div>
                    <div className="space-y-1.5">
                      {group.items.map((itemKey) => {
                        const item = toolItems[itemKey];
                        const descriptionKey = `onboarding.illustrations.tools.items.${itemKey}`;
                        const hasDescription =
                          item.description.trim().length > 0 && item.description !== descriptionKey;
                        return (
                          <div key={`${group.id}-${itemKey}`}>
                            <div className="flex items-center gap-1.5 text-sm font-semibold leading-none text-text-primary">
                              <span className="text-accent">{item.icon}</span>
                              <span>{item.label}</span>
                            </div>
                            {hasDescription ? (
                              <p className="mt-0.5 pl-6 text-sm leading-tight text-text-secondary">{item.description}</p>
                            ) : null}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
          ))}
              </div>
      </div>
    );
  }

  if (illustration === 'map-controls') {
    return (
      <div
        className="mb-2 grid grid-cols-1 gap-2"
        data-testid="onboarding-illustration-map-controls"
      >
        <div className="rounded-md border border-border bg-background px-3 py-2">
          <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
            <FitToViewIcon size={16} />
            <span>{t('onboarding.illustrations.map.fitTitle')}</span>
          </div>
          <p className="mt-1 text-sm leading-relaxed text-text-secondary">
            {t('onboarding.illustrations.map.fitDescription')}
          </p>
        </div>
        <div className="grid grid-cols-[1.25fr_1fr] gap-2">
          <div className="rounded-md border border-border bg-background px-3 py-2">
            <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
              <PlusIcon size={14} />
              <MinusIcon size={14} />
              <span className="whitespace-nowrap">{t('onboarding.illustrations.map.zoomTitle')}</span>
            </div>
          </div>
          <div className="rounded-md border border-border bg-background px-3 py-2">
            <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
              <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
                <path d="M2 8h12" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
                <path d="M2 5v6M14 5v6" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
              </svg>
              <span className="whitespace-nowrap">{t('onboarding.illustrations.map.scaleTitle')}</span>
            </div>
          </div>
        </div>
        <div className="rounded-md border border-border bg-background px-3 py-2">
          <div className="grid grid-cols-[minmax(0,1fr)_auto] items-end gap-4">
            <div className="min-w-0">
              <div className="text-sm font-semibold text-text-primary">
                <span className="whitespace-nowrap">{t('onboarding.illustrations.map.contextTitle')}</span>
              </div>
              <p className="mt-1 text-sm leading-relaxed text-text-secondary">
                {t('onboarding.illustrations.map.contextDescription')}
              </p>
            </div>
            <div className="flex flex-shrink-0 flex-col items-center">
              <span className="mb-1.5 text-xs font-medium leading-none text-text-secondary">{t('onboarding.illustrations.map.rightClick')}</span>
              {renderMouseIcon('h-8 w-7', 'right')}
            </div>
          </div>
        </div>
      </div>
    );
  }


  if (illustration !== 'mouse-pan-zoom') {
    return null;
  }

  return (
    <div className="mb-2 grid grid-cols-1 gap-1" data-testid="onboarding-illustration-mouse-pan-zoom">
      <div className="relative rounded-md border border-border bg-background px-2 py-1.5">
        <div className="absolute left-2 top-1.5 inline-flex items-center gap-1 text-xs font-semibold leading-none text-text-primary">
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" aria-hidden="true">
            <path d="M8 1v14M1 8h14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <path d="M8 1L6.5 2.5M8 1L9.5 2.5M8 15l-1.5-1.5M8 15l1.5-1.5M1 8l1.5-1.5M1 8l1.5 1.5M15 8l-1.5-1.5M15 8l-1.5 1.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          {t('onboarding.illustrations.mouse.panTitle')}
        </div>
        <div className="pt-2 text-xs text-text-secondary">
          {/*
            Only rendered during placement, where a plain drag does not pan
            (mapGestureMode hands the press to placement): Ctrl+drag and
            middle-drag are the gestures that pan there.
          */}
          <div className="grid grid-cols-[1fr_auto_1fr] items-end">
            <div className="col-start-1 flex items-end justify-center gap-2">
              <span className="inline-flex h-7 min-w-[2.7rem] items-center justify-center rounded-md border border-border bg-background-50 px-2 font-semibold text-text-primary">
                CTRL
              </span>
              <span className="mb-2">+</span>
              <div className="flex w-6 flex-col items-center">
                <span className="mb-1.5 whitespace-nowrap text-center text-[11px] leading-none">Left click</span>
                {renderMouseIcon('h-8 w-7', 'left')}
              </div>
            </div>
            <span className="col-start-2 mb-2 px-1 text-text-secondary">or</span>
            <div className="col-start-3 flex items-end justify-start gap-2 translate-x-5">
              <div className="flex w-6 flex-col items-center">
                <span className="mb-1.5 whitespace-nowrap text-center text-[11px] leading-none">Middle click</span>
                {renderMouseIcon('h-8 w-7', 'wheel')}
              </div>
            </div>
          </div>
        </div>
      </div>
      <div className="relative rounded-md border border-border bg-background px-2 py-1.5">
        <div className="absolute left-2 top-1.5 inline-flex items-center gap-1 text-xs font-semibold leading-none text-text-primary">
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" aria-hidden="true">
            <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="M10.5 10.5L14 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <path d="M7 5.3v3.4M5.3 7h3.4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          {t('onboarding.illustrations.mouse.zoomTitle')}
        </div>
        <div className="grid grid-cols-[1fr_auto_1fr] items-end pt-2 text-xs text-text-secondary">
          <div className="col-start-1 flex items-end justify-end gap-1 pr-2">
            <div className="flex flex-col items-center">
              <span className="mb-1.5 text-center text-[11px] leading-none">Scroll</span>
              {renderMouseIcon('h-8 w-7', 'wheel')}
            </div>
            <svg viewBox="0 0 14 40" className="h-7 w-3 self-center" aria-hidden="true">
              <path d="M7 18v5" stroke="#ef4444" strokeWidth="2" strokeLinecap="round" />
              <path d="M7 12L3 18h8z" fill="#ef4444" />
              <path d="M7 29l-4-6h8z" fill="#ef4444" />
            </svg>
          </div>
          <span className="col-start-2 mb-2 px-1 text-text-secondary">or</span>
          <div className="col-start-3 flex items-end justify-start gap-1 pl-2">
            <svg
              viewBox="28 21 119 177"
              preserveAspectRatio="xMidYMax meet"
              className="h-14 w-12 self-end"
              aria-hidden="true"
            >
              <defs>
                <clipPath id="touch-gesture-left-cutout" clipPathUnits="userSpaceOnUse">
                  <path
                    clipRule="evenodd"
                    d="
                      M 0 0 H 220 V 200 H 0 Z
                      M 65 55
                      A 15 15 0 0 1 80 40
                      A 15 15 0 0 1 95 55
                      L 95 200
                      A 15 15 0 0 1 80 215
                      A 15 15 0 0 1 65 200
                      Z
                    "
                  />
                </clipPath>
                <clipPath id="touch-gesture-right-cutout" clipPathUnits="userSpaceOnUse">
                  <path
                    clipRule="evenodd"
                    d="
                      M 0 0 H 220 V 200 H 0 Z
                      M 84 45
                      A 15 15 0 0 1 99 30
                      A 15 15 0 0 1 114 45
                      L 114 200
                      A 15 15 0 0 1 99 215
                      A 15 15 0 0 1 84 200
                      Z
                    "
                  />
                </clipPath>
              </defs>
              <g clipPath="url(#touch-gesture-left-cutout)">
                <g clipPath="url(#touch-gesture-right-cutout)">
                  <circle cx="80" cy="55" r="24" fill="#ef4444" />
                  <circle cx="99" cy="45" r="24" fill="#ef4444" />
                </g>
              </g>
              <path
                d="
                  M 42 158
                  Q 47 132 42 105
                  Q 42 101 47 101
                  H 46
                  Q 62 101 62 124
                  Q 64 142 64 159
                  Z
                  M 42 158
                  H 82
                  V 199
                  H 42
                  Z
                  M 52 138
                  H 72
                  V 158
                  H 52
                  Z
                "
                fill="#fff"
                transform="rotate(-40 52 129)"
              />
              <rect x="85" y="135" width="58" height="63" fill="#fff" />
              <g fill="none" stroke="#fff" strokeWidth="20" strokeLinecap="round" strokeLinejoin="round">
                <line x1="80" y1="55" x2="80" y2="135" />
                <line x1="99" y1="45" x2="99" y2="145" />
                <line x1="117" y1="95" x2="117" y2="145" strokeWidth="20" />
                <line x1="136.5" y1="102" x2="136.5" y2="157" strokeWidth="20" />
              </g>
            </svg>
            <svg viewBox="0 0 14 40" className="h-7 w-3 self-center" aria-hidden="true">
              <path d="M7 18v5" stroke="#ef4444" strokeWidth="2" strokeLinecap="round" />
              <path d="M7 12L3 18h8z" fill="#ef4444" />
              <path d="M7 29l-4-6h8z" fill="#ef4444" />
            </svg>
          </div>
        </div>
      </div>
    </div>
  );
}

export default function OnboardingOverlay({
  step,
  stepIndex,
  stepCount,
  canGoBack,
  onBack,
  onNext,
  onSkipCurrent,
  onSkipAll,
}: OnboardingOverlayProps) {
  const { t } = useTranslation();
  const cardRef = useRef<HTMLDivElement>(null);
  const maskIdRef = useRef(`onboarding-mask-${Math.random().toString(36).slice(2)}`);
  const [targetRects, setTargetRects] = useState<SpotlightRect[]>([]);
  const [viewportVersion, setViewportVersion] = useState(0);
  const [tooltipHeight, setTooltipHeight] = useState(TOOLTIP_ESTIMATED_HEIGHT);
  const [isTooltipSettled, setIsTooltipSettled] = useState(true);
  // True once we have stopped waiting for a target that never appeared.
  const [targetUnresolvable, setTargetUnresolvable] = useState(false);
  const [showSkipConfirm, setShowSkipConfirm] = useState(false);

  const isLastStep = stepIndex >= stepCount - 1;
  const interaction = step.interaction ?? 'locked';
  const placement = step.placement ?? 'bottom';
  const description = t(step.descriptionKey);
  const hasDescription = description.trim().length > 0 && description !== step.descriptionKey;
  const descriptionItems: string[] = (() => {
    if (!step.descriptionItemsKey) return [];
    const raw = t(step.descriptionItemsKey, { returnObjects: true }) as unknown;
    return Array.isArray(raw) ? raw.filter((item): item is string => typeof item === 'string') : [];
  })();
  // The tool reference must show the toolbar the user actually has: the
  // navigate group is hidden unless `map.controls.show_navigation_tools` is on
  // (#409), and `navigationToolsEnabled(null)` is false, which is the right
  // answer while config is still loading - the tour cannot run before it is.
  const config = useProjectStore((state) => state.config);
  const illustration = renderStepIllustration(
    step.illustration,
    t,
    navigationToolsEnabled(config)
  );
  const targetSelectorList = useMemo(() => {
    if (step.targetSelectors && step.targetSelectors.length > 0) {
      return step.targetSelectors;
    }
    if (step.targetSelector) {
      return [step.targetSelector];
    }
    return [];
  }, [step.targetSelector, step.targetSelectors]);
  const selectorKey = targetSelectorList.join('|');
  const hasTargetSelectors = targetSelectorList.length > 0;

  const requestSkip = useCallback(() => {
    setShowSkipConfirm(true);
  }, []);

  const handleSkipCurrent = useCallback(() => {
    setShowSkipConfirm(false);
    onSkipCurrent();
  }, [onSkipCurrent]);

  const handleSkipAll = useCallback(() => {
    setShowSkipConfirm(false);
    onSkipAll();
  }, [onSkipAll]);

  useEffect(() => {
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = previousOverflow;
    };
  }, []);

  // Publish the active step to the onboarding-step store for consumers outside
  // this overlay's tree (DisplayMenu, SingleSystemView). The matching DOM
  // attributes on the overlay element below now have no in-app consumer - the
  // observer that read them is gone (#331) - and are kept only as a test/debug
  // handle.
  useEffect(() => {
    publishOnboardingStep(step.flowId, step.id);
  }, [step.flowId, step.id]);
  useEffect(
    () => () => {
      publishOnboardingStep(null, null);
    },
    [],
  );

  useEffect(() => {
    const updateRect = () => {
      if (targetSelectorList.length === 0) {
        setTargetRects([]);
        return;
      }

      const nextRects: SpotlightRect[] = [];
      for (const selector of targetSelectorList) {
        const element = document.querySelector(selector) as HTMLElement | null;
        if (!element) {
          continue;
        }

        const spotlightRect = getVisibleSpotlightRect(element);
        if (!spotlightRect) {
          continue;
        }

        nextRects.push(spotlightRect);
      }

      setTargetRects(nextRects);
    };

    updateRect();
    const intervalId = window.setInterval(updateRect, 200);

    const handleViewportChange = () => {
      setViewportVersion((current) => current + 1);
      updateRect();
    };

    window.addEventListener('resize', handleViewportChange);
    window.addEventListener('scroll', updateRect, true);

    return () => {
      window.clearInterval(intervalId);
      window.removeEventListener('resize', handleViewportChange);
      window.removeEventListener('scroll', updateRect, true);
    };
  }, [selectorKey, step.id, targetSelectorList]);

  useEffect(() => {
    cardRef.current?.focus();
  }, [step.id]);

  useEffect(() => {
    setShowSkipConfirm(false);
  }, [step.flowId, stepIndex]);

  useEffect(() => {
    const card = cardRef.current;
    if (!card) {
      return;
    }

    const updateHeight = () => {
      const nextHeight = Math.ceil(card.getBoundingClientRect().height);
      setTooltipHeight((current) => (Math.abs(current - nextHeight) > 1 ? nextHeight : current));
    };

    updateHeight();

    if (typeof ResizeObserver === 'undefined') {
      return;
    }

    const observer = new ResizeObserver(updateHeight);
    observer.observe(card);

    return () => {
      observer.disconnect();
    };
  }, [step.id, stepIndex]);

  useEffect(() => {
    const isAllowedTarget = (node: Node | null): boolean => {
      if (!node) return false;
      if (cardRef.current?.contains(node)) return true;
      if (interaction === 'free') return true;
      if (interaction === 'target' && targetSelectorList.length > 0) {
        for (const selector of targetSelectorList) {
          const target = document.querySelector(selector);
          if (target?.contains(node)) {
            return true;
          }
        }
      }
      return false;
    };

    const handlePointer = (event: PointerEvent | MouseEvent) => {
      if (isAllowedTarget(event.target as Node | null)) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
    };

    const handleWheel = (event: WheelEvent) => {
      if (isAllowedTarget(event.target as Node | null)) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
    };

    const handleKeyboard = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        if (showSkipConfirm) {
          setShowSkipConfirm(false);
        } else {
          requestSkip();
        }
        return;
      }

      if (showSkipConfirm) {
        return;
      }

      if (event.key === 'ArrowLeft' && canGoBack) {
        event.preventDefault();
        event.stopPropagation();
        onBack();
        return;
      }

      if (event.key === 'ArrowRight') {
        event.preventDefault();
        event.stopPropagation();
        onNext();
      }
    };

    document.addEventListener('pointerdown', handlePointer, true);
    document.addEventListener('click', handlePointer, true);
    document.addEventListener('wheel', handleWheel, { capture: true, passive: false });
    document.addEventListener('keydown', handleKeyboard, true);

    return () => {
      document.removeEventListener('pointerdown', handlePointer, true);
      document.removeEventListener('click', handlePointer, true);
      document.removeEventListener('wheel', handleWheel, true);
      document.removeEventListener('keydown', handleKeyboard, true);
    };
  }, [canGoBack, interaction, onBack, onNext, requestSkip, showSkipConfirm, targetSelectorList]);

  const combinedTargetRect = useMemo(() => combineSpotlightRects(targetRects), [targetRects]);
  const spotlightRects = useMemo(() => {
    if (step.mergeTargetSpotlights && combinedTargetRect) {
      return [combinedTargetRect];
    }
    return targetRects;
  }, [combinedTargetRect, step.mergeTargetSpotlights, targetRects]);
  const tooltipTargetRect = useMemo(() => {
    if (!step.tooltipTargetSelector) {
      return combinedTargetRect;
    }
    const tooltipTarget = document.querySelector(step.tooltipTargetSelector) as HTMLElement | null;
    if (!tooltipTarget) {
      return combinedTargetRect;
    }
    const rect = getVisibleSpotlightRect(tooltipTarget);
    return rect ?? combinedTargetRect;
  }, [combinedTargetRect, step.tooltipTargetSelector, viewportVersion]);
  const tooltipTargetSignature = useMemo(() => {
    if (!tooltipTargetRect) {
      return 'none';
    }
    return [
      Math.round(tooltipTargetRect.left),
      Math.round(tooltipTargetRect.top),
      Math.round(tooltipTargetRect.width),
      Math.round(tooltipTargetRect.height),
    ].join(':');
  }, [
    tooltipTargetRect?.height,
    tooltipTargetRect?.left,
    tooltipTargetRect?.top,
    tooltipTargetRect?.width,
  ]);

  useEffect(() => {
    if (!hasTargetSelectors) {
      setIsTooltipSettled(true);
      return;
    }

    if (tooltipTargetSignature === 'none') {
      setIsTooltipSettled(false);
      setTargetUnresolvable(false);
      const giveUpId = window.setTimeout(() => {
        setTargetUnresolvable(true);
      }, TARGET_GIVE_UP_DELAY_MS);
      return () => {
        window.clearTimeout(giveUpId);
      };
    }

    setTargetUnresolvable(false);
    setIsTooltipSettled(false);
    const timeoutId = window.setTimeout(() => {
      setIsTooltipSettled(true);
    }, TOOLTIP_SETTLE_DELAY_MS);

    return () => {
      window.clearTimeout(timeoutId);
    };
  }, [hasTargetSelectors, step.id, tooltipTargetSignature]);

  const tooltipStyle = useMemo(
    () =>
      calculateTooltipStyle(
        tooltipTargetRect,
        placement,
        viewportVersion,
        step.tooltipMaxWidth ?? TOOLTIP_DEFAULT_MAX_WIDTH,
        step.tooltipOffsetX ?? 0,
        tooltipHeight
      ),
    [
      placement,
      step.tooltipMaxWidth,
      step.tooltipOffsetX,
      tooltipHeight,
      tooltipTargetRect,
      viewportVersion,
    ]
  );
  // `targetUnresolvable` short-circuits the anchor wait: the step renders
  // centered (computeTooltipStyle already centers a null rect) rather than
  // staying hidden behind a dim the user cannot dismiss.
  const isTooltipAnchorReady =
    targetUnresolvable ||
    ((!hasTargetSelectors || tooltipTargetRect !== null) && isTooltipSettled);

  const overlay = (
    <div
      className="fixed inset-0 z-[100000] pointer-events-none"
      data-testid="onboarding-overlay"
      data-onboarding-flow-id={step.flowId}
      data-onboarding-step-id={step.id}
    >
      {spotlightRects.length > 0 ? (
        <>
          <svg
            className="fixed inset-0 h-full w-full pointer-events-none"
            aria-hidden="true"
            preserveAspectRatio="none"
          >
            <defs>
              <mask id={maskIdRef.current}>
                <rect x="0" y="0" width="100%" height="100%" fill="white" />
                {spotlightRects.map((rect, idx) => (
                  <rect
                    key={`hole-${idx}`}
                    x={rect.left}
                    y={rect.top}
                    width={rect.width}
                    height={rect.height}
                    rx={SPOTLIGHT_RADIUS}
                    ry={SPOTLIGHT_RADIUS}
                    fill="black"
                  />
                ))}
              </mask>
            </defs>
            <rect
              x="0"
              y="0"
              width="100%"
              height="100%"
              fill="rgba(0, 0, 0, 0.66)"
              mask={`url(#${maskIdRef.current})`}
            />
          </svg>
          {spotlightRects.map((rect, idx) => (
            <div
              key={`outline-${idx}`}
              className="fixed pointer-events-none border-2 border-accent/90 shadow-[0_0_0_3px_rgba(56,189,248,0.18)] transition-all duration-200"
              style={{
                left: rect.left,
                top: rect.top,
                width: rect.width,
                height: rect.height,
                borderRadius: SPOTLIGHT_RADIUS,
              }}
            />
          ))}
        </>
      ) : (
        <div className="fixed inset-0 bg-black/70 pointer-events-none" />
      )}

      <div
        ref={cardRef}
        role="dialog"
        aria-modal="true"
        aria-label={t(step.titleKey)}
        tabIndex={-1}
        className="fixed pointer-events-auto max-h-[calc(100vh-2rem)] overflow-y-auto rounded-xl border border-border bg-panel px-4 py-4 shadow-[0_0_42px_14px_rgba(0,0,0,0.72)] focus:outline-none"
        style={{
          ...tooltipStyle,
          visibility: isTooltipAnchorReady ? 'visible' : 'hidden',
          pointerEvents: isTooltipAnchorReady ? 'auto' : 'none',
        }}
      >
        <div className="mb-1 text-xs uppercase tracking-wide text-text-secondary">
          {t(step.flowTitleKey)}
        </div>
        <div className="mb-2 text-xs text-text-secondary">
          {t('onboarding.progress', { current: stepIndex + 1, total: stepCount })}
        </div>
        <h2 className="mb-2 text-lg font-semibold text-text-primary">{t(step.titleKey)}</h2>
        <div className="mb-4">
          {showSkipConfirm ? (
            <>
              <p className="mb-2 text-sm leading-relaxed text-text-secondary">
                {t('onboarding.skipConfirm.description')}
              </p>
            </>
          ) : (
            <>
              {hasDescription && <p className="mb-2 text-sm leading-relaxed text-text-secondary">{description}</p>}
              {illustration}
              {descriptionItems.length > 0 && (
                <ul className="mt-2 list-disc pl-5 text-sm leading-relaxed text-text-secondary">
                  {descriptionItems.map((item, index) => (
                    <li key={`${step.id}-desc-item-${index}`}>{item}</li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
        <div className="flex items-center justify-between gap-2">
          {showSkipConfirm ? (
            <>
              <button
                className="rounded-md border border-border px-3 py-1.5 text-sm text-text-primary transition-colors hover:bg-background-50"
                onClick={() => setShowSkipConfirm(false)}
                type="button"
                data-testid="onboarding-skip-confirm-cancel"
              >
                {t('common.actions.cancel')}
              </button>
              <div className="flex items-center gap-2">
                <button
                  className="rounded-md border border-border px-3 py-1.5 text-sm text-text-primary transition-colors hover:bg-background-50"
                  onClick={handleSkipCurrent}
                  type="button"
                  data-testid="onboarding-skip-current"
                >
                  {t('onboarding.skipConfirm.skipCurrent')}
                </button>
                <button
                  className="rounded-md bg-error px-3 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                  onClick={handleSkipAll}
                  type="button"
                  data-testid="onboarding-skip-all"
                >
                  {t('onboarding.skipConfirm.skipAll')}
                </button>
              </div>
            </>
          ) : (
            <>
              <button
                className="rounded-md px-3 py-1.5 text-sm text-text-secondary transition-colors hover:bg-background-50 hover:text-text-primary"
                onClick={requestSkip}
                type="button"
                data-testid="onboarding-skip"
              >
                {t('onboarding.actions.skip')}
              </button>
              <div className="flex items-center gap-2">
                {canGoBack && (
                  <button
                    className="rounded-md border border-border px-3 py-1.5 text-sm text-text-primary transition-colors hover:bg-background-50"
                    onClick={onBack}
                    type="button"
                    data-testid="onboarding-back"
                  >
                    {t('common.actions.back')}
                  </button>
                )}
                <button
                  className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                  onClick={onNext}
                  type="button"
                  data-testid="onboarding-next"
                >
                  {isLastStep ? t('common.actions.done') : t('common.actions.next')}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );

  return createPortal(overlay, document.body);
}
