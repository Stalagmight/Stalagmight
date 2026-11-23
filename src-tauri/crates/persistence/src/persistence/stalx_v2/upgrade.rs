//! The load-time upgrade of a version 2 file an earlier build of version 2
//! wrote (plan 2026-11-23 RS 8.2): a file whose `requires` does not list
//! [`super::ARCHIVED_STATIONS`]. Such a build showed a junction on each line
//! by the name that line knew it by (U1 per-line aliases) and every loop
//! closure end by its own name; this build shows one kept station on every
//! line of a junction, and one end of a loop closure as the other.
//!
//! The decoder reads U1 alias rows as records naming their lines (step 1:
//! the latest alias naming a line names it, the earlier ones are history, a
//! nameless one takes the name the line showed, the station's own), which is
//! all the upgrade needs to know about them. Decode never changes the model:
//! it runs the upgrade on a copy, checks the result, and leaves it for
//! [`super::Decoded::upgrade`] to apply, which every site that hands a
//! decoded project to the app calls. The other steps are the shared
//! kept-stations rules ([`crate::persistence::kept`]):
//!
//! 2. Each junction shows the identity on its longest line (rule K over own
//!    lines among the station's own identity and its records). When a record
//!    wins, the node takes its id, name and external id, and the old identity
//!    becomes a record naming the lines that showed it, with its external id;
//!    the U1 joined comment stays with the shown identity, so records get none
//!    (U-P2-13c).
//! 3. Each closure archives the end rule K does not keep, in list order, when
//!    C1 and C2 allow it, else nothing.
//! 4. N8 by rule L, "already shown on the line" being what the file showed.
//! 5. What lines show differently: every name, every comment now hidden,
//!    every letter and every closure left with no archive.

use crate::models::survey::{Project, Violation};

use super::DecodeError;
use crate::models::StationId;

use crate::persistence::kept::{self, Before, Comments, IdentityChanges};

/// What the upgrade of an earlier version 2 file did (RS 8.2). Counts and
/// ids only, but for [`Self::changes`], which is survey content (for the
/// conversion notice, never logged).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpgradeReport {
    /// Records the file held (U1 alias rows, read as records).
    pub records: usize,
    /// Junctions now shown by one of their records (rule K), as `(old
    /// Station ID, new Station ID)`: the node took the record's id, so an
    /// export keyed by ids (Ariane) numbers it differently once (declared).
    pub swapped: Vec<(StationId, StationId)>,
    /// Closures given an archived end.
    pub closures_archived: usize,
    /// Stations given a letter suffix (rule L).
    pub lettered: usize,
    /// What lines show differently.
    pub changes: IdentityChanges,
}

impl UpgradeReport {
    /// Whether the upgrade changed nothing in the survey: the file held no
    /// record and no closure, so it reads the same under both rules.
    pub fn is_empty(&self) -> bool {
        self.records == 0
            && self.swapped.is_empty()
            && self.closures_archived == 0
            && self.lettered == 0
            && self.changes.is_empty()
    }
}

/// The violations the upgrade itself settles, which an earlier file may
/// hold before it: N8 (a line showed the junction by another name than one
/// of its own stations' under this build's rule), A4 (a junction whose every
/// line knew it by an alias), and C1 / C2 (step 3 normalises the closures).
/// Every other violation is the file's own damage.
pub(super) fn settles(violation: &Violation) -> bool {
    matches!(
        violation,
        Violation::DuplicateStationName { .. }
            | Violation::NoNativeLine { .. }
            | Violation::StationArchivedTwice { .. }
            | Violation::ClosureArchiveCycle { .. }
    )
}

/// Run the upgrade on `project` (an earlier version 2 file as decoded, whose
/// only violations are ones [`settles`] lists). `in_time` is asked between
/// its steps (a reader's deadline, [`super::DecodeOptions::deadline`]).
pub(super) fn run(
    project: &mut Project,
    in_time: &dyn Fn() -> Result<(), DecodeError>,
) -> Result<UpgradeReport, DecodeError> {
    let records = project.stations.values().map(|s| s.archived.len()).sum();
    let before = Before::of(project, Comments::Shared);
    let swapped = kept::keep_longest(project, Comments::Shared);
    in_time()?;
    let settled = kept::settle(project, &before);
    Ok(UpgradeReport {
        records,
        swapped,
        closures_archived: settled.closures_archived,
        lettered: settled.lettered.len(),
        changes: settled.changes,
    })
}
