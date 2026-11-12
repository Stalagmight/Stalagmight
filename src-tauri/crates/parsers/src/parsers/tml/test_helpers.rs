//! Shared XML-fixture builders for the `tml` parser's tests, factored out so
//! both `tml::tests` and `tml::decode::tests` can build synthetic TML/TMLU
//! documents without duplicating the boilerplate. A `test_helpers.rs` file is
//! excluded from the crate's file-size ratchet entirely (see
//! `scripts/check-rust-file-size.mjs`), so this also keeps fixture noise off
//! the production LOC of the modules that use it.

#![expect(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use super::{parse_tml, TmlFile, TmlShot, TmlShotType};

pub fn sample_tml_shot() -> TmlShot {
    TmlShot {
        id: 1,
        uuid: None,
        from_id: 0,
        shot_type: TmlShotType::Real,
        azimuth: 0.0,
        length: 10.0,
        depth: 0.0,
        depth_in: 0.0,
        inclination: 0.0,
        left: 0.0,
        right: 0.0,
        up: 0.0,
        down: 0.0,
        comment: None,
        explorer: None,
        date: None,
        color: String::new(),
        excluded: false,
        latitude: 0.0,
        longitude: 0.0,
        closure_to_id: -1,
        section: String::new(),
        credit: None,
    }
}

// Helper to build minimal XML for testing
pub fn wrap_xml(shots_xml: &str) -> String {
    wrap_xml_with("m", 100.0, shots_xml)
}

/// `wrap_xml` with the file-level unit and elevation spelled out. The
/// elevation is `Display` so a test can write text a `f64` cannot hold.
pub fn wrap_xml_with(unit: &str, elevation: impl std::fmt::Display, shots_xml: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>Test Cave</caveName>
    <unit>{unit}</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
    <firstStartAbsoluteElevation>{elevation}</firstStartAbsoluteElevation>
    <Data>
        {shots_xml}
    </Data>
</CaveFile>"#
    )
}

/// Run the real `parse_tml` over synthetic XML, via a throwaway `.tmlu` in a
/// temp dir (the entry point takes a path, and the extension picks the
/// reader). Nothing survives the call.
pub fn parse_synthetic_tmlu(xml: &str) -> TmlFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.tmlu");
    fs::write(&path, xml).unwrap();
    parse_tml(&path).unwrap()
}

/// A shot element with the depth fields spelled out, for the rebase tests.
pub fn tml_shot_xml_depth(
    id: i64,
    from_id: i64,
    shot_type: &str,
    section: &str,
    depth: f64,
    depth_in: f64,
) -> String {
    format!(
        r"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>
    <ClosureToID>-1</ClosureToID>
    <Type>{shot_type}</Type>
    <Section>{section}</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
    <Depth>{depth}</Depth>
    <DepthIn>{depth_in}</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
    )
}

/// A REAL shot with every field the #412 join tests need to vary.
///
/// The join refuses on trip metadata, on an excluded seam, and on a repeated
/// UUID, so its tests have to drive `<Explorer>`/`<Date>`/`<Color>`,
/// `<Excluded>` and `<UUID>` independently - more knobs than the positional
/// builders above carry, and a positional builder with nine parameters is
/// worse to read than a struct literal with defaults.
pub struct TripShot<'a> {
    pub id: i64,
    pub from_id: i64,
    pub section: &'a str,
    pub uuid: Option<&'a str>,
    pub excluded: bool,
    pub explorer: &'a str,
    pub date: &'a str,
    pub color: &'a str,
    pub depth: f64,
    /// `-1.0` is Ariane's "derive it from the previous station" sentinel.
    pub depth_in: f64,
}

impl Default for TripShot<'_> {
    fn default() -> Self {
        Self {
            id: 0,
            from_id: -1,
            section: "A",
            uuid: None,
            excluded: false,
            explorer: "Tester",
            date: "2024-06-15",
            color: "0x8f6112ff",
            depth: 5.0,
            depth_in: -1.0,
        }
    }
}

impl TripShot<'_> {
    pub fn xml(&self) -> String {
        let Self {
            id,
            from_id,
            section,
            uuid,
            excluded,
            explorer,
            date,
            color,
            depth,
            depth_in,
        } = self;
        let uuid_element = uuid.map_or_else(String::new, |u| format!("\n    <UUID>{u}</UUID>"));
        format!(
            r"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>{uuid_element}
    <ClosureToID>-1</ClosureToID>
    <Type>REAL</Type>
    <Section>{section}</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
    <Depth>{depth}</Depth>
    <DepthIn>{depth_in}</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>{explorer}</Explorer>
    <Date>{date}</Date>
    <Color>{color}</Color>
    <Excluded>{excluded}</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
        )
    }
}

pub fn tml_shot_xml(id: i64, from_id: i64, shot_type: &str, section: &str) -> String {
    format!(
        r#"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>
    <ClosureToID>-1</ClosureToID>
    <Type>{shot_type}</Type>
    <Section>{section}</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>2.0</Inclination>
    <Left>1.0</Left>
    <Right>2.0</Right>
    <Up>3.0</Up>
    <Down>4.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"#
    )
}

/// A REAL shot with explicit azimuth and length, for the #412
/// branch-continuation and re-join tests.
pub fn tml_shot_xml_az_len(
    id: i64,
    from_id: i64,
    section: &str,
    azimuth: f64,
    length: f64,
) -> String {
    format!(
        r"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>
    <ClosureToID>-1</ClosureToID>
    <Type>REAL</Type>
    <Section>{section}</Section>
    <Azimut>{azimuth}</Azimut>
    <Length>{length}</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
    )
}

pub fn tml_shot_xml_gps(
    id: i64,
    from_id: i64,
    shot_type: &str,
    section: &str,
    lat: f64,
    lng: f64,
) -> String {
    format!(
        r#"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>
    <ClosureToID>-1</ClosureToID>
    <Type>{shot_type}</Type>
    <Section>{section}</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>2.0</Inclination>
    <Left>1.0</Left>
    <Right>2.0</Right>
    <Up>3.0</Up>
    <Down>4.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>{lat}</Latitude>
    <Longitude>{lng}</Longitude>
</SurveyData>"#
    )
}

pub fn tmlu_shot_xml(id: i64, from_id: i64, shot_type: &str, section: &str) -> String {
    format!(
        r#"<SRVD>
    <ID>{id}</ID>
    <FRID>{from_id}</FRID>
    <CID>-1</CID>
    <TY>{shot_type}</TY>
    <SC>{section}</SC>
    <AZ>45.0</AZ>
    <LG>5.0</LG>
    <DP>3.0</DP>
    <DPI>-1.0</DPI>
    <INC>1.0</INC>
    <L>0.5</L>
    <R>0.5</R>
    <U>1.5</U>
    <D>0.5</D>
    <CM></CM>
    <EX>Explorer1</EX>
    <DT>2025-01-01</DT>
    <CL>0x44aa88ff</CL>
    <EXC>false</EXC>
    <LT>0.0</LT>
    <LGT>0.0</LGT>
</SRVD>"#
    )
}

pub fn write_tml_archive(path: &Path, entry: &str, body: &[u8]) {
    write_tml_archive_entries(path, &[(entry, body)]);
}

pub fn write_tml_archive_entries(path: &Path, entries: &[(&str, &[u8])]) {
    use std::io::Write as _;
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    for (name, body) in entries {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
}
