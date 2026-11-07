//! The north reference an Ariane file declares (#438).
//!
//! Split out of `mod.rs` only to keep that file under the size ratchet; it is
//! one decision and its receipt line.

use chrono::Datelike;

use crate::models::{AzimuthReference, CorrectionSource};

use super::TmlFile;

/// The north reference to record on every section of this file (#438).
///
/// One reference per FILE, not per section: `useMagneticAzimuth` is a file-level
/// flag in this format, unlike Compass, where the declination is per survey.
///
/// The magnetic case wants a declination, and the reproducible source is the
/// IGRF model already bundled for the Compass "auto" mode
/// ([`crate::parsers::geo::igrf_declination`]), evaluated at the file's
/// georeference on the earliest date the file records. Anything missing - no
/// `<GeoCoding>`, no survey date - gets a ZERO correction and `Unknown`
/// provenance, never a guess.
///
/// ## What goes in the receipt, and what only goes in the log
///
/// `warnings` is the IMPORT RECEIPT, which the user reads. Only a shortfall
/// they can act on goes there: a file that IS georeferenced and IS dated and
/// still got no declination, because that is surprising and the cause (a date
/// outside the model's 1900-2025 span) is worth naming. The ordinary outcomes -
/// a declination computed, a true-north declaration honoured, a file that
/// simply does not say where or when it was surveyed - are `tracing::info`:
/// nothing was degraded and there is nothing to do, and a receipt line on every
/// single import is a line nobody reads.
pub(super) fn tml_azimuth_reference(tml: &TmlFile, warnings: &mut Vec<String>) -> AzimuthReference {
    if !tml.use_magnetic_azimuth {
        tracing::info!(
            purpose = "record the file's north reference",
            "The file declares true-north azimuths (useMagneticAzimuth=false); recorded a \
             true-north reference with no declination correction."
        );
        return AzimuthReference::true_north(CorrectionSource::FileDeclared);
    }

    let Some(geo) = tml.geo_coding.as_ref() else {
        tracing::info!(
            purpose = "record the file's north reference",
            "The file declares magnetic azimuths (useMagneticAzimuth=true) and carries no \
             georeference, so no declination could be computed. Azimuths were imported exactly \
             as measured, with a 0.0 degree correction."
        );
        return AzimuthReference::magnetic(0.0, CorrectionSource::Unknown);
    };

    // The EARLIEST dated section: the earliest survey date this format records,
    // and the one the declination should be evaluated at.
    //
    // An undated file gets no model call at all. Substituting today's date made
    // the import wall-clock-dependent - the bundled model stops at 2025-12-31,
    // so from 2026 on every undated file took the failure branch and printed
    // whatever today happened to be into its receipt. The same file has to
    // import to the same project on any two days.
    let Some(epoch_date) = tml.sections.iter().filter_map(|section| section.date).min() else {
        tracing::info!(
            purpose = "record the file's north reference",
            "The file declares magnetic azimuths and is georeferenced, but carries no survey \
             date, so there is no epoch to evaluate a declination at. Azimuths were imported \
             exactly as measured, with a 0.0 degree correction."
        );
        return AzimuthReference::magnetic(0.0, CorrectionSource::Unknown);
    };
    let epoch_year = epoch_date.year();

    match crate::parsers::geo::igrf_declination(geo.latitude, geo.longitude, epoch_date) {
        Ok(declination) if declination.is_finite() => {
            tracing::info!(
                declination,
                epoch = %epoch_date,
                purpose = "record the file's north reference",
                "The file declares magnetic azimuths; recorded an IGRF declination. Stored \
                 azimuths are unchanged - the correction is applied when geometry is computed."
            );
            AzimuthReference::magnetic(declination, CorrectionSource::IgrfAuto { epoch_year })
        }
        // Both remaining arms ARE receipt-worthy: the file gave everything the
        // model needs and the model still could not answer.
        Ok(_) => {
            warnings.push(
                "The file declares magnetic azimuths, but the geomagnetic model returned a \
                 non-finite declination; azimuths were imported exactly as measured, with a 0.0 \
                 degree correction."
                    .to_string(),
            );
            AzimuthReference::magnetic(0.0, CorrectionSource::Unknown)
        }
        Err(reason) => {
            warnings.push(format!(
                "The file declares magnetic azimuths and is georeferenced ({lat:.4}, {lon:.4}), \
                 but the declination could not be computed for its earliest survey date \
                 ({epoch_date}): {reason}. Azimuths were imported exactly as measured, with a \
                 0.0 degree correction.",
                lat = geo.latitude,
                lon = geo.longitude
            ));
            AzimuthReference::magnetic(0.0, CorrectionSource::Unknown)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::tml::test_helpers::parse_synthetic_tmlu;

    /// A magnetic, GEOREFERENCED file with no survey date on any shot.
    fn undated_georeferenced_magnetic() -> String {
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>Undated</caveName>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
    <firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>
    <Data>
        <SurveyData>
            <ID>0</ID><FromID>-1</FromID><ClosureToID>-1</ClosureToID>
            <Type>START</Type><Section>S</Section>
            <Azimut>0.0</Azimut><Length>0.0</Length><Depth>0.0</Depth>
            <DepthIn>-1.0</DepthIn><Inclination>0.0</Inclination>
            <Left>0.0</Left><Right>0.0</Right><Up>0.0</Up><Down>0.0</Down>
            <Comment></Comment><Explorer></Explorer><Date></Date>
            <Color>0x8f6112ff</Color><Excluded>false</Excluded>
            <Latitude>20.25</Latitude><Longitude>-87.5</Longitude>
        </SurveyData>
        <SurveyData>
            <ID>1</ID><FromID>0</FromID><ClosureToID>-1</ClosureToID>
            <Type>REAL</Type><Section>S</Section>
            <Azimut>90.0</Azimut><Length>10.0</Length><Depth>5.0</Depth>
            <DepthIn>-1.0</DepthIn><Inclination>0.0</Inclination>
            <Left>0.0</Left><Right>0.0</Right><Up>0.0</Up><Down>0.0</Down>
            <Comment></Comment><Explorer></Explorer><Date></Date>
            <Color>0x8f6112ff</Color><Excluded>false</Excluded>
            <Latitude>0.0</Latitude><Longitude>0.0</Longitude>
        </SurveyData>
    </Data>
</CaveFile>"#
            .to_string()
    }

    /// #438: the same file has to import to the same project on any two days.
    ///
    /// Substituting `Utc::now()` for a missing survey date made this
    /// wall-clock-dependent: the bundled IGRF model stops at 2025-12-31, so from
    /// 2026 the call deterministically FAILED and printed whatever today
    /// happened to be into the receipt - a different receipt every day, and a
    /// different one again if the model is ever extended. There is no epoch, so
    /// there is no model call.
    #[test]
    fn an_undated_file_asks_the_model_nothing_and_says_nothing_about_today() {
        let tml = parse_synthetic_tmlu(&undated_georeferenced_magnetic());
        assert!(
            tml.geo_coding.is_some(),
            "premise: the fixture must be georeferenced, or the date is never reached"
        );
        assert!(
            tml.sections.iter().all(|s| s.date.is_none()),
            "premise: the fixture must carry no survey date"
        );

        let mut warnings = Vec::new();
        let reference = tml_azimuth_reference(&tml, &mut warnings);

        assert_eq!(
            reference,
            AzimuthReference::magnetic(0.0, CorrectionSource::Unknown),
            "no epoch means no correction and no claimed provenance"
        );
        assert!(
            warnings.is_empty(),
            "a file that simply does not say when it was surveyed is not a defect: {warnings:?}"
        );
    }

    /// The success paths say nothing in the RECEIPT. A user who imports a clean
    /// file and is shown a warning learns to skip the warnings.
    #[test]
    fn an_ordinary_import_puts_nothing_in_the_receipt() {
        for xml in [
            undated_georeferenced_magnetic(),
            undated_georeferenced_magnetic().replace(
                "<useMagneticAzimuth>true</useMagneticAzimuth>",
                "<useMagneticAzimuth>false</useMagneticAzimuth>",
            ),
            undated_georeferenced_magnetic()
                .replace("<Latitude>20.25</Latitude>", "<Latitude>0.0</Latitude>")
                .replace("<Longitude>-87.5</Longitude>", "<Longitude>0.0</Longitude>"),
        ] {
            let tml = parse_synthetic_tmlu(&xml);
            let mut warnings = Vec::new();
            let _ = tml_azimuth_reference(&tml, &mut warnings);
            assert!(warnings.is_empty(), "clean file warned: {warnings:?}");
        }
    }

    /// The one case that IS receipt-worthy: everything the model needs is
    /// present and it still could not answer, because the date is outside the
    /// bundled model's 1900-2025 span.
    #[test]
    fn a_dated_georeferenced_file_the_model_cannot_serve_says_so_in_the_receipt() {
        let xml =
            undated_georeferenced_magnetic().replace("<Date></Date>", "<Date>2030-04-01</Date>");
        let tml = parse_synthetic_tmlu(&xml);
        assert!(
            tml.sections.iter().any(|s| s.date.is_some()),
            "premise: the fixture must carry a survey date"
        );

        let mut warnings = Vec::new();
        let reference = tml_azimuth_reference(&tml, &mut warnings);
        assert_eq!(
            reference,
            AzimuthReference::magnetic(0.0, CorrectionSource::Unknown)
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("2030-04-01"),
            "the receipt must name the date it could not serve: {warnings:?}"
        );
    }
}
