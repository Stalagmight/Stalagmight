//! [#576 FZ-PARSE-3] A CDATA section is character data like any other text: a
//! value an XML writer wrapped in `<![CDATA[...]]>` reads exactly as the same
//! value written as plain text. Found by the `gen_tmlu` fuzz generator: the
//! reader handled only text events, so a CDATA length read as 0 and a CDATA
//! comment or cave name vanished, with no warning on the receipt.

use super::test_helpers::{parse_synthetic_tmlu, wrap_xml};

fn shot(id: i64, from_id: i64, shot_type: &str, length: &str, comment: &str) -> String {
    format!(
        "<SurveyData><ID>{id}</ID><FromID>{from_id}</FromID><ClosureToID>-1</ClosureToID>\
         <Type>{shot_type}</Type><Section>SectionA</Section><Azimut>90.0</Azimut>\
         <Length>{length}</Length><Depth>2.0</Depth><DepthIn>2.0</DepthIn>\
         <Inclination>0.0</Inclination><Left>0.0</Left><Right>0.0</Right><Up>0.0</Up>\
         <Down>0.0</Down><Comment>{comment}</Comment></SurveyData>"
    )
}

/// The REAL shot of `xml` as (length, comment).
fn real_shot(xml: &str) -> (f64, Option<String>) {
    let file = parse_synthetic_tmlu(xml);
    let shot = file
        .sections
        .iter()
        .flat_map(|s| &s.shots)
        .find(|s| s.id == 1)
        .expect("shot 1 survives the parse");
    (shot.length, shot.comment.clone())
}

#[test]
fn cdata_values_read_like_the_same_plain_text() {
    let plain = wrap_xml(&format!(
        "{}{}",
        shot(0, -1, "START", "0.0", ""),
        shot(1, 0, "REAL", "7.25", "note &amp; more")
    ));
    let cdata = wrap_xml(&format!(
        "{}{}",
        shot(0, -1, "START", "0.0", ""),
        shot(1, 0, "REAL", "<![CDATA[7.25]]>", "<![CDATA[note & more]]>")
    ))
    .replace(
        "<caveName>Test Cave</caveName>",
        "<caveName><![CDATA[Test Cave]]></caveName>",
    );

    let from_plain = real_shot(&plain);
    let from_cdata = real_shot(&cdata);
    println!("plain text: {from_plain:?}");
    println!("CDATA:      {from_cdata:?}");
    assert_eq!(
        from_cdata, from_plain,
        "a CDATA value reads like plain text"
    );
    assert_eq!(from_plain, (7.25, Some("note & more".to_string())));
    assert_eq!(parse_synthetic_tmlu(&cdata).cave_name, "Test Cave");
}
