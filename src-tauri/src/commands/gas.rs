use tauri::Manager;

use crate::config::GasConfig;
use crate::core::state::dive_plan::{MAX_SEGMENTS, MAX_WAYPOINTS};
use crate::core::{AppState, CommandError};
use crate::models::{Cylinder, Diver, GasReachField, PlanSegment, StationRef};

/// Bound a plan's per-span segment list, mirroring the way-point cap in [`validate_plan_inputs`].
///
/// Kept separate from that function only because it is newer: both doors into the engine call it
/// (`compute_gas_reach_field` here, `render_profile_for` in `commands::export`), and the `.plnx`
/// write path caps segments on its own. Fold it into `validate_plan_inputs` when that signature is
/// next touched.
///
/// REJECT rather than truncate, for the same reason the way-point cap does: a segment is the diver's
/// statement about how a span is flown, and quietly dropping the last few would fly them at the plan
/// speed on no scooter instead - slower and cheaper on gas, so the reported reach could only grow.
///
/// The per-span bound is the same rule at the route's actual size: `resolve_segments` reads only
/// `waypoints + 1` spans, so rows past that would be silently inert at this door - while the
/// `.plnx` load path FOLDS an over-long tail into the last retained span, conservatively. A
/// request with more rows than the route has spans is describing a dive this plan cannot fly;
/// reject it rather than let the two doors quietly disagree about which rows count.
pub(crate) fn validate_plan_segments(
    segments: &[PlanSegment],
    waypoint_count: usize,
) -> Result<(), String> {
    if segments.len() > MAX_SEGMENTS {
        return Err(format!(
            "Too many plan segments: {} (max {MAX_SEGMENTS}).",
            segments.len()
        ));
    }
    if segments.len() > waypoint_count + 1 {
        return Err(format!(
            "More plan segments than route spans: {} for {waypoint_count} way point(s).",
            segments.len()
        ));
    }
    Ok(())
}

/// Validate a plan's inputs before they reach the engine, and normalize its config.
///
/// Shared by every command that computes a reach field. `AppState::gas_reach_field` validates
/// nothing - it clones the project and delegates - and its contract says so explicitly: "the diver /
/// cylinders are assumed already validated and `gas_config` normalized **by the command**". So each
/// entry point owes this, and a second one that skipped it would reintroduce exactly the holes this
/// closes: a NaN/zero RMV or cylinder poisoning the field, and an unbounded way-point list walking
/// past the cap that exists "so the limit cannot be walked past by a caller that never touches a
/// file".
pub(crate) fn validate_plan_inputs(
    waypoints: &[StationRef],
    diver: Diver,
    cylinders: Vec<Cylinder>,
    mut gas_config: GasConfig,
) -> Result<(Diver, Vec<Cylinder>, GasConfig), String> {
    // Every way point costs the engine a full extra Dijkstra, so an over-long list is a compute
    // amplifier on the hottest path there is. The `.plnx` write path already caps it for that
    // reason; enforce the same bound for any caller that never touches a file.
    //
    // REJECT rather than truncate. A way point is a constraint the dive must satisfy, and the
    // engine's whole contract is that an unusable one stalls the plan instead of being quietly
    // skipped - silently dropping the 65th would hand back a route that ignores it, which is the
    // unsafe direction. The UI caps at the same number, so this is unreachable from normal use.
    if waypoints.len() > MAX_WAYPOINTS {
        return Err(format!(
            "Too many way points: {} (max {MAX_WAYPOINTS}).",
            waypoints.len()
        ));
    }
    // Re-validate through the guarding constructors - a NaN/zero RMV or cylinder from the
    // frontend would otherwise reach the engine and poison the field.
    let diver = Diver::new(diver.rmv_l_per_min, diver.label)?;
    let cylinders = cylinders
        .into_iter()
        .map(|c| {
            // `new` re-validates the numerics and resets the non-constructor fields, so carry
            // the frontend's `enabled` + `usable_gas_bar` across (serde already defaulted any
            // missing ones). `usable_gas_bar` is otherwise unbounded here - the engine clamps it
            // to `[0, basis]` (the conservative-capped fill) per cylinder before budgeting AND
            // before display, so a stray value can't over-state reach or the readout.
            let (enabled, usable_gas_bar) = (c.enabled, c.usable_gas_bar);
            Cylinder::new(
                c.water_volume_l,
                c.rated_pressure_bar,
                c.fill_bar,
                c.material,
                c.role,
                c.label,
            )
            .map(|mut cyl| {
                cyl.enabled = enabled;
                cyl.usable_gas_bar = usable_gas_bar;
                cyl
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    gas_config.normalize();
    Ok((diver, cylinders, gas_config))
}

/// Single-diver open-circuit gas reachability field (#38, Phase 1 headline).
///
/// Validates the diver + cylinders (serde deserialization builds the structs directly,
/// bypassing the model guards) and normalizes the gas config, then computes the reachable
/// frontier off the UI thread via `spawn_blocking` (mirrors `get_station_distances_from`; the
/// engine reads the instant MEASURED coords, never the slow closure-adjusted solve). Returns
/// `Ok(None)` when no project is open; `Err(msg)` on invalid input.
#[tauri::command]
pub async fn compute_gas_reach_field(
    app_handle: tauri::AppHandle,
    origin: Option<StationRef>,
    endpoint: Option<StationRef>,
    // Ordered way points (#298) the dive must visit, in order, between the origin and the endpoint.
    // The frontend always sends it (empty when the plan has none). Each is re-resolved by the
    // engine; an entry that no longer resolves stalls the plan rather than being skipped.
    waypoints: Vec<StationRef>,
    // Per-span speed + propulsion (#317), one entry per route span (`waypoints.len() + 1`). The
    // frontend always sends it; an empty or mis-sized list is reconciled by the engine to the
    // config speed with no scooter, which is the pre-#317 field exactly.
    segments: Vec<PlanSegment>,
    diver: Diver,
    cylinders: Vec<Cylinder>,
    gas_config: GasConfig,
    // Per-stage drop overrides (#246), aligned 1:1 with the field's `stage_drop_refs`: the station
    // the diver dragged each ▼ marker to (`None` = keep the strategy default). The frontend always
    // sends it (empty when nothing is dragged); the engine clamps each pinned drop before it can
    // move the budget, so a stray ref can only fall through to the default.
    stage_drop_overrides: Vec<Option<StationRef>>,
) -> Result<Option<GasReachField>, CommandError> {
    // This command re-runs on every gas-input keystroke, so the way-point cap inside is a compute
    // amplifier guard on the hottest path there is.
    let (diver, cylinders, gas_config) =
        validate_plan_inputs(&waypoints, diver, cylinders, gas_config)?;
    validate_plan_segments(&segments, waypoints.len())?;

    let field = tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        state.inner().gas_reach_field(
            origin.as_ref(),
            endpoint.as_ref(),
            &waypoints,
            &segments,
            &diver,
            &cylinders,
            &gas_config,
            &stage_drop_overrides,
        )
    })
    .await
    // A JoinError (engine panic / cancellation) is a real backend bug, so surface it as
    // Err rather than collapsing it to Ok(None) - which is reserved for "no project open".
    .map_err(|e| CommandError::internal(format!("gas reach computation failed: {e}")))?;
    Ok(field)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests assert on a known-Err result; a failed unwrap should fail the test"
)]
mod tests {
    use super::{validate_plan_segments, MAX_SEGMENTS};
    use crate::models::PlanSegment;

    fn span() -> PlanSegment {
        PlanSegment {
            speed_m_per_min: Some(55.0),
            on_scooter: true,
        }
    }

    #[test]
    fn the_segment_list_is_capped_at_the_ipc_boundary() {
        // The way-point cap exists "so the limit cannot be walked past by a caller that never
        // touches a file"; the segment list arrived at the same door with no such bound, even
        // though the `.plnx` write path caps it.
        assert!(validate_plan_segments(&[], 0).is_ok());
        assert!(validate_plan_segments(&vec![span(); MAX_SEGMENTS], MAX_SEGMENTS - 1).is_ok());

        let err =
            validate_plan_segments(&vec![span(); MAX_SEGMENTS + 1], MAX_SEGMENTS).unwrap_err();
        assert!(err.contains("Too many plan segments"), "{err}");
    }

    #[test]
    fn segments_beyond_the_route_span_count_are_rejected() {
        // An in-cap list can still outrun the route: `resolve_segments` reads only
        // `waypoints + 1` spans, so before this bound a request with no way points and a slow or
        // scootered SECOND row sailed through validation with that row silently inert - while the
        // `.plnx` load path folds the same tail conservatively. Reject the disagreement at the
        // door.
        assert!(validate_plan_segments(&vec![span(); 2], 1).is_ok());

        let err = validate_plan_segments(&vec![span(); 2], 0).unwrap_err();
        assert!(err.contains("More plan segments than route spans"), "{err}");
    }
}
