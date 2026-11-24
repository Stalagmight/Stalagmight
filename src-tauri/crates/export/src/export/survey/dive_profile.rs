//! The dive profile of a station-graph plan: the renderer
//! (`super::super::dive_profile`) on the plan's wire form. The route has no
//! merge hops (plan D18), so a junction is one profile point, not a
//! zero-length pair.

use anyhow::Result;

use crate::models::survey::{GasReach, Project};
use crate::models::StationRef;

use super::super::dive_profile::{
    render_dive_profile as render_wire, ProfileContext, ProfileFormat,
};

/// Render the planned route of `reach` (computed on `project`, with the way
/// points the plan was asked with) as a dive log.
pub fn render_dive_profile(
    project: &Project,
    reach: &GasReach,
    waypoints: &[StationRef],
    format: ProfileFormat,
    ctx: &ProfileContext<'_>,
) -> Result<String> {
    render_wire(&reach.to_wire(project, waypoints), format, ctx)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::render_dive_profile;
    use crate::export::survey::test_helpers::cave;
    use crate::export::{ProfileContext, ProfileFormat};
    use crate::models::config::{GasConfig, GasUnits};
    use crate::models::{CylRole, Cylinder, Diver, StationRef, TankMaterial};

    /// A route through a junction has one profile point there: the outbound
    /// distances strictly increase (no zero-length merge hop), and both
    /// renderers accept it.
    #[test]
    fn a_route_through_a_junction_has_no_zero_length_point() {
        let cave = cave();
        let diver = Diver::new(20.0, None).unwrap();
        let cylinders = [Cylinder::new(
            11.1,
            207.0,
            200.0,
            TankMaterial::Aluminium,
            CylRole::BackGas,
            None,
        )
        .unwrap()];
        let reach = cave.project.gas_reach(
            Some(cave.t[0]),
            Some(cave.n[3]),
            &[],
            &[],
            &diver,
            &cylinders,
            &GasConfig::default(),
            &[],
        );
        assert!(reach.route.contains(&cave.t[2]), "{:?}", reach.route);
        assert_eq!(
            reach.route.iter().filter(|&&s| s == cave.t[2]).count(),
            1,
            "the junction is one route node"
        );
        let field = reach.to_wire(&cave.project, &[]);
        let outbound: Vec<f64> = field
            .route_profile
            .iter()
            .filter(|p| p.outbound)
            .map(|p| p.distance_m)
            .collect();
        assert!(outbound.windows(2).all(|w| w[1] > w[0]), "{outbound:?}");
        let ctx = ProfileContext {
            project_name: "Synthetic cave",
            units: GasUnits::default(),
            endpoint: Some(StationRef::new(cave.north, 3)),
        };
        for format in [ProfileFormat::PlainText, ProfileFormat::SubsurfaceXml] {
            let text = render_dive_profile(&cave.project, &reach, &[], format, &ctx).unwrap();
            assert!(!text.is_empty());
        }
    }
}
