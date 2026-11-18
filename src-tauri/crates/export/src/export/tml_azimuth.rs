//! Which north an Ariane document declares, and what that costs (#438).
//!
//! Split out of `tml.rs` to keep that file under the size ratchet; it is one
//! decision, and the export's only choice that can change a NUMBER rather than
//! a label.

use crate::models::{AzimuthReference, CorrectionSource, Line, NorthReference, Project};

/// How this document declares its azimuths (#438).
///
/// The flag `useMagneticAzimuth` is file-level and the project's references are
/// per line, so the three answers are: the lines agree on magnetic (write
/// the readings, flag true), they agree on true north (write them, flag false),
/// or they disagree - and then the only honest single-flag document is one where
/// every bearing has been rotated into true north on the way out.
///
/// ## An UNKNOWN correction is not a competing declaration
///
/// [`CorrectionSource::Unknown`] means "no correction is on record", not "a
/// correction of exactly zero" - it is what a `.dmp` and a georeference-less
/// `.tml` get, because the device is a compass and nothing in the file says how
/// far off north it read. A line like that is raw magnetic, which is exactly
/// what `useMagneticAzimuth=true` says, so it constrains the document to be
/// magnetic and otherwise votes on nothing.
///
/// Counting it as "correction 0" instead made the common case mixed: import an
/// IGRF-corrected `.tml` alongside `.dmp` files from the same cave and every
/// bearing in the export would be rewritten and the file declared true north -
/// baking OUR zero into the `.dmp` lines permanently, and asserting about
/// them the one thing we know is false. Handing Ariane the readings and letting
/// it apply its own declination is both the honest document and the stable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AzimuthExport {
    /// Every DECLARED line is magnetic with one correction, and any
    /// undeclared line is raw magnetic too: emit the readings untouched and
    /// let Ariane apply its own declination.
    Magnetic,
    /// Every line is already true north: emit untouched, flag false.
    TrueAsStored,
    /// The lines disagree and EVERY one of them has a correction on record,
    /// so each can be rotated into true north. Apply them and flag false.
    MixedToTrue,
    /// The lines disagree and at least one has NO correction on record, so
    /// the rewrite is impossible: a line whose offset from true north is
    /// unknown cannot be put into true north. Emit the readings untouched and
    /// flag magnetic - the only claim that is true of the unknown lines -
    /// and say in the export note that the declared ones are misdescribed by it.
    ///
    /// This exists because the alternative was silently wrong: the branch used
    /// to fall into [`Self::MixedToTrue`], where an undeclared line's
    /// reference resolves to identity, so its bearings went out UNROTATED under
    /// `useMagneticAzimuth=false` - the exact false claim the branch is for.
    MagneticUnresolved,
}

impl AzimuthExport {
    pub(super) fn decide(project: &Project) -> Self {
        let references: Vec<AzimuthReference> = project
            .lines
            .iter()
            .filter(|line| !line.shots.is_empty())
            .map(|line| project.azimuth_reference_for(line))
            .collect();

        // A line with no correction on record cannot be rotated anywhere.
        let any_unknown = references
            .iter()
            .any(|r| r.source == CorrectionSource::Unknown);
        let mut declared = references
            .iter()
            .filter(|r| r.source != CorrectionSource::Unknown);

        // Nothing declared at all - every existing project, and the historical
        // declaration this must keep producing.
        let Some(first) = declared.next() else {
            return AzimuthExport::Magnetic;
        };
        // Corrections come from the same file-level source, so this is an
        // identity test, not a tolerance: the epsilon only keeps a last-bit
        // difference from routing an otherwise uniform project down the
        // rewrite-every-azimuth path.
        let uniform = declared.all(|r| {
            r.north == first.north
                && (r.applied_correction() - first.applied_correction()).abs() < 1e-12
        });

        // Every rewrite below needs EVERY line's correction, not just the
        // declared ones - the document is rotated as a whole or not at all.
        let can_rewrite = !any_unknown;
        if !uniform {
            return if can_rewrite {
                AzimuthExport::MixedToTrue
            } else {
                AzimuthExport::MagneticUnresolved
            };
        }
        match first.north {
            NorthReference::Magnetic => AzimuthExport::Magnetic,
            // A true-north declaration and a raw magnetic line cannot share
            // one flag: whichever it names is a false claim about the other.
            NorthReference::True if any_unknown => AzimuthExport::MagneticUnresolved,
            NorthReference::True => AzimuthExport::TrueAsStored,
            // Grid bearings are neither: declaring them magnetic would have
            // Ariane add a declination to a grid azimuth, and declaring them
            // true would keep the meridian convergence in. Rotate them out -
            // when every line can be.
            NorthReference::Grid if can_rewrite => AzimuthExport::MixedToTrue,
            NorthReference::Grid => AzimuthExport::MagneticUnresolved,
        }
    }

    pub(super) fn is_magnetic(self) -> bool {
        matches!(
            self,
            AzimuthExport::Magnetic | AzimuthExport::MagneticUnresolved
        )
    }

    /// What the user has to be told about this document's azimuths, or `None`
    /// when there is nothing worth saying.
    ///
    /// A stable CODE, not prose: the toast copy is the frontend's and goes
    /// through i18n (`docs/notifications.md` - hardcoded English in a
    /// `showToast` call is a defect), and a note the backend spelled out in
    /// English could never be translated. The prose lives in
    /// [`Self::log_message`], which is for the log and for a run with no user.
    ///
    /// Only the two compromises speak. The plain cases are the format working
    /// as intended, and a note on every export is a note nobody reads.
    pub(super) fn note_code(self) -> Option<&'static str> {
        match self {
            AzimuthExport::Magnetic | AzimuthExport::TrueAsStored => None,
            AzimuthExport::MixedToTrue => Some("azimuth-rotated-to-true"),
            AzimuthExport::MagneticUnresolved => Some("azimuth-mixed-unresolved"),
        }
    }

    /// The long form of [`Self::note_code`], for the log.
    pub(super) fn log_message(self) -> &'static str {
        match self {
            AzimuthExport::Magnetic => {
                "Lines agree on magnetic; wrote the readings as stored under \
                 useMagneticAzimuth=true."
            }
            AzimuthExport::TrueAsStored => {
                "Lines agree on true north; wrote the readings as stored under \
                 useMagneticAzimuth=false."
            }
            AzimuthExport::MixedToTrue => {
                "Lines declare different north references, which one Ariane file cannot \
                 express: every bearing was rotated to true north on the way out and the file is \
                 marked useMagneticAzimuth=false. The project's own readings are untouched."
            }
            AzimuthExport::MagneticUnresolved => {
                "Lines declare different north references AND at least one has no declination \
                 on record, so the document could not be rotated into one frame: the readings \
                 were written as stored under useMagneticAzimuth=true, which misdescribes the \
                 lines that ARE corrected. Ariane will apply its own declination to all of \
                 them."
            }
        }
    }

    /// The reference to push each of `line`'s azimuths through on the way
    /// out. Identity except on the mixed path, so an ordinary export is
    /// byte-for-byte what it always was.
    pub(super) fn emit_reference(self, project: &Project, line: &Line) -> AzimuthReference {
        match self {
            AzimuthExport::MixedToTrue => project.azimuth_reference_for(line),
            _ => AzimuthReference::default(),
        }
    }
}
