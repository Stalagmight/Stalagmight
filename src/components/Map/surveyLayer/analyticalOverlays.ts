/**
 * Analytical-overlay mode state (#576 D3 split): the distance gradient (#34), the
 * gas reachable frontier (#38) and the Plan-mode idle tint (#395 revision), plus
 * what derives from it alone - whether the idle tint paints, the custom
 * renderer's overlay input, and the ramp paint expressions the stock interact
 * overlay and drag preview color with. SurveyLayer's setters write the fields
 * and then refresh every consumer.
 */
import type { ExpressionSpecification } from 'maplibre-gl';
import {
  distanceColorExpression,
  DISTANCE_NEAR,
  DISTANCE_FAR,
  type DistanceFeatureState,
} from '../../../utils/distanceGradient';
import {
  gasReachColorExpression,
  GAS_NEAR,
  GAS_FAR,
  type GasReachFeatureState,
} from '../../../utils/gasReach';
import type { MapThemeColors } from '../../../utils/themeColors';
import { overlayInputFrom } from './customRender/overlayColor';

export class AnalyticalOverlays {
  // Distance-gradient overlay (#34). feature-state based so re-root needs no
  // feature rebuild; re-applied after source rebuilds because endApplyFeatures
  // clears state before tile IDs can be reused for different merged groups.
  distanceMode: boolean = false;
  distanceState: DistanceFeatureState | null = null;
  // Gas reachable-frontier overlay (#38). Same feature-state mechanics as the
  // distance gradient (own `gasReach` key); mutually exclusive with it (both own
  // the shots/stations color paint), enforced upstream in uiStore.
  gasReachMode: boolean = false;
  gasReachState: GasReachFeatureState | null = null;
  // Plan-mode idle tint (#395 revision): true while Plan is active but no gas
  // origin has been chosen yet - the whole survey renders the gas ramp's dim far
  // color instead of leaking section colors under the gas scheme. Driven by
  // MapCanvas (setPlanIdleTint); yields to a real gas field via isPlanIdleActive().
  planIdleTint: boolean = false;

  /** True when the plan-idle tint should actually paint: the flag is set AND no
   *  real gas field is active - a real field always wins (#395 revision). */
  isPlanIdleActive(): boolean {
    return this.planIdleTint && !(this.gasReachMode && this.gasReachState);
  }

  /** The analytical overlay ramp input for the custom renderer and drag ghost. */
  overlayInput(): ReturnType<typeof overlayInputFrom> {
    return overlayInputFrom(this.distanceMode, this.distanceState, this.gasReachMode, this.gasReachState);
  }

  /** The interact overlay's shot / station color paint (applyInteractPaint): the
   *  resolved per-feature `color`, or the active ramp over the carried value. */
  interactColorExpression(themeColors: MapThemeColors): ExpressionSpecification {
    const colorProp: ExpressionSpecification = ['get', 'color'] as ExpressionSpecification;
    const shotColor = this.distanceMode
      ? distanceColorExpression(
          DISTANCE_NEAR,
          DISTANCE_FAR,
          this.distanceState?.max ?? 0,
          themeColors.grayLine,
          ['get', 'dist'] as ExpressionSpecification,
        )
      : this.gasReachMode
        ? gasReachColorExpression(
            GAS_NEAR,
            GAS_FAR,
            this.gasReachState?.max ?? 0,
            themeColors.grayLine,
            ['get', 'gasReach'] as ExpressionSpecification,
          )
        : colorProp;
    return shotColor;
  }

  /** The drag preview's shot/station color paint. Plain mode reads the resolved
   *  per-feature `color` property (baked at capture time, like the interact
   *  overlay); distance / gas-reach modes read the carried `dist` / `gasReach`
   *  through the SAME ramp expressions the bulk layers use, so colors match. */
  dragPreviewColorExpr(themeColors: MapThemeColors): ExpressionSpecification {
    if (this.distanceMode) {
      return distanceColorExpression(
        DISTANCE_NEAR,
        DISTANCE_FAR,
        this.distanceState?.max ?? 0,
        themeColors.grayLine,
        ['get', 'dist'] as ExpressionSpecification,
      );
    }
    if (this.gasReachMode) {
      return gasReachColorExpression(
        GAS_NEAR,
        GAS_FAR,
        this.gasReachState?.max ?? 0,
        themeColors.grayLine,
        ['get', 'gasReach'] as ExpressionSpecification,
      );
    }
    return ['get', 'color'] as ExpressionSpecification;
  }
}
