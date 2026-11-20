//! What a map-image export renders from (C14): one snapshot of the open
//! project together with its two derived views, both memoised per revision
//! (`app_state`'s derived cache), so an export solves nothing the map has
//! already solved and lays the survey out at most once.

use std::sync::Arc;

use crate::models::survey::{AdjustedPositions, Project, RawPositions};

use super::AppState;

/// One project state and the views of it that an image export draws: the
/// Closures Computed View (what the map shows, and so what the image shows)
/// and the measured layout the export's own derivations read (closure
/// alignment, the distance ramp).
pub struct ExportViews {
    /// A clone of the open project, taken with the views.
    pub project: Project,
    /// The measured (raw) layout of [`Self::project`].
    pub raw: Arc<RawPositions>,
    /// The Closures Computed View of [`Self::project`] under the current
    /// closure-adjustment config (the raw layout where nothing is adjusted).
    pub adjusted: Arc<AdjustedPositions>,
}

impl AppState {
    /// The open project with its raw layout and its Computed View, or `None`
    /// when no project is open.
    ///
    /// The Computed View comes from the cache (solved outside the project
    /// lock on a miss). The raw layout is the cached one when no edit landed
    /// while the view was fetched (the revision did not move, so the cache
    /// describes exactly the cloned state); otherwise it is laid out from the
    /// clone, so the three always describe one state.
    pub fn export_views(&self) -> Option<ExportViews> {
        let before = self.current_revision();
        let (project, adjusted) = self.adjusted_positions()?;
        let raw = {
            let guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match guard.as_ref() {
                Some(live) if self.current_revision() == before => self.raw_positions_of(live),
                _ => Arc::new(project.raw_positions()),
            }
        };
        Some(ExportViews {
            project,
            raw,
            adjusted,
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use crate::core::state::AppState;
    use crate::models::survey::{ClosureStrategy, LineMeta, Project, StartAt};
    use crate::models::ShotData;

    fn shot(length: f64, azimuth: f64) -> ShotData {
        ShotData {
            length,
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            depth_in: 1.0,
            depth_out: 1.0,
            ..ShotData::default()
        }
    }

    /// A square loop surveyed with a misclosure, closed by a Batch closure.
    fn looped() -> Project {
        let mut p = Project::new("Loop", None);
        let line = p
            .add_line(
                LineMeta::default(),
                StartAt::New,
                vec![
                    shot(10.0, 0.0),
                    shot(10.0, 90.0),
                    shot(10.0, 180.0),
                    shot(9.0, 270.0),
                ],
            )
            .unwrap();
        let chain: Vec<_> = p.line(line).unwrap().stations().collect();
        p.set_placement(chain[0], 0.0, 0.0).unwrap();
        p.push_closure(
            chain[4],
            Some(line),
            chain[0],
            Some(line),
            ClosureStrategy::Batch,
        );
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        p
    }

    /// The views describe the open project: the raw layout is the project's,
    /// the Computed View is its solve (which moves the misclosed loop), and a
    /// second fetch of the same revision hands back the same memoised views.
    #[test]
    fn the_views_are_the_open_projects_and_are_memoised() {
        let state = AppState::new().unwrap();
        assert!(state.export_views().is_none(), "no project open");
        let project = looped();
        state.set_project_for_tests(project.clone());

        let views = state.export_views().unwrap();
        assert_eq!(views.project, project);
        assert_eq!(*views.raw, project.raw_positions());
        let solved = project.adjusted_positions(&state.get_config().closure_adjustment);
        assert_eq!(*views.adjusted, solved);
        let end = project.lines()[0].stations().last().unwrap();
        assert_ne!(
            views.adjusted.xy(end),
            views.raw.xy(end),
            "the loop is adjusted"
        );

        let again = state.export_views().unwrap();
        assert!(std::sync::Arc::ptr_eq(&views.raw, &again.raw));
        assert!(std::sync::Arc::ptr_eq(&views.adjusted, &again.adjusted));
    }
}
