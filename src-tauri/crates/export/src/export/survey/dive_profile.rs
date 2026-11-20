//! The dive profile of a station-graph plan: the legacy renderer
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
