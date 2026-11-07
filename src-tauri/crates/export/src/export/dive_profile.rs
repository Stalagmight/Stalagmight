//! Dive-profile export (#299): the planned round-trip profile as something a diver can carry out
//! of the app.
//!
//! Two renderers over the same `GasReachField.route_profile` series:
//!
//! - **Subsurface XML** - `<divelog><dives><dive><divecomputer><sample time= depth=>`, written to
//!   match the shape Subsurface's own writer emits so its `File > Import` can read it.
//!   ⚠ The grammar is modelled on Subsurface's format, but this has NOT been round-tripped through
//!   a real Subsurface build - treat "importable" as intent, not a verified fact, until someone
//!   does. `subsurface_xml_matches_the_documented_grammar` pins the shape we believe is required.
//! - **Plain text** - a fixed-width table, because the common case is a diver copying the numbers
//!   onto a slate or retyping them into another planner.
//!
//! Both lead with a header saying what this is NOT. The app has no decompression model at all: no
//! ascent, no stops, no tissue loading. A file that merely *looked* like a dive log, opened in a
//! deco planner months later, is exactly the sort of thing that gets read as a schedule - so the
//! disclaimer is a correctness requirement, not politeness.

use std::fmt::Write as _;
use std::io::Cursor;

use anyhow::Result;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;

use crate::models::config::GasUnits;
use crate::models::{GasReachField, RouteProfilePoint, StationRef};
use crate::parsers::geo::FEET_TO_METERS;

/// What this file is, on every export, in both formats.
const DISCLAIMER: &str =
    "Planned penetration profile from Stalagmight. NOT a decompression schedule: \
no ascent, stops or tissue loading are modelled. Depths and times are the planned \
route only.";

/// Which serialization the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFormat {
    /// Subsurface-importable XML.
    SubsurfaceXml,
    /// Fixed-width text table.
    PlainText,
}

impl ProfileFormat {
    /// Parse the wire value the frontend sends. Unknown values are rejected rather than defaulted:
    /// silently writing XML into a `.txt` the diver asked for is worse than a visible error.
    pub fn from_wire(value: &str) -> Result<Self> {
        match value {
            "subsurface-xml" => Ok(Self::SubsurfaceXml),
            "text" => Ok(Self::PlainText),
            other => Err(anyhow::anyhow!("unknown dive-profile format: {other}")),
        }
    }

    /// Stable identifier for logs and telemetry.
    ///
    /// `pub` (was `pub(crate)`): the root crate's export commands log it, and
    /// that call now crosses the crate boundary.
    pub fn label(self) -> &'static str {
        match self {
            Self::SubsurfaceXml => "subsurface-xml",
            Self::PlainText => "text",
        }
    }
}

/// A character that must never survive into an exported document.
///
/// `char::is_control()` alone is NOT enough, and believing otherwise is how the hole below stayed
/// open: it matches the **Cc** category only, so U+2028 (LINE SEPARATOR, category `Zl`) and U+2029
/// (PARAGRAPH SEPARATOR, `Zp`) pass straight through it. Both are mandatory line breaks under
/// UAX #14 - editors, `white-space: pre`, and most languages' line iterators split on them - so
/// either one forges a line just as effectively as `\n`. Rust's own `str::lines()` splits on `\n`
/// alone, which is precisely why a test written with it cannot see the gap.
///
/// The bidi and zero-width controls (`Cf`) go for the Trojan-Source reason: U+202E reverses the
/// visual order of everything after it, in a document whose entire purpose is to be read literally.
fn is_forbidden(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}'   // zero-width space/joiners, LTR/RTL marks
            | '\u{2028}' | '\u{2029}' // line and paragraph separators
            | '\u{202A}'..='\u{202E}' // bidi embeddings and overrides
            | '\u{2066}'..='\u{2069}' // bidi isolates
            | '\u{FEFF}'              // BOM / zero-width no-break space
        )
}

/// Collapse anything that could forge a line or break the output format.
///
/// The project name is the one caller-controlled string that reaches these documents, and it is
/// loaded straight out of the project database with no validation. A name containing a line break
/// would inject arbitrary lines into a file whose whole purpose is to be read as plan data - a
/// forged `Turn: 400 m in, ...` above the real one, in a `.stalx` shared between cavers. C0 control
/// characters are also forbidden outright by XML 1.0, so leaving them in produces a file Subsurface
/// rejects. See [`is_forbidden`] for why "a line break" is a wider set than `\n`. Truncated because
/// the name is unbounded.
fn sanitize_name(name: &str) -> String {
    const MAX: usize = 120;
    let cleaned: String = name
        .chars()
        .map(|c| if is_forbidden(c) { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    match trimmed.char_indices().nth(MAX) {
        Some((cut, _)) => format!("{}...", &trimmed[..cut]),
        None => trimmed.to_string(),
    }
}

/// Everything the renderers need that is not in the reach field itself.
pub struct ProfileContext<'a> {
    /// Project name, as loaded - sanitized here, not by the caller.
    pub project_name: &'a str,
    /// The diver's length unit. Gas stays litres and runtime stays minutes in both systems.
    pub units: GasUnits,
    /// The endpoint the diver asked for, if any. Needed to say whether the turnaround is the place
    /// they aimed at, somewhere short of it, or somewhere else entirely - see [`TurnContext`].
    pub endpoint: Option<StationRef>,
}

/// How the turnaround relates to the endpoint the diver asked for.
///
/// Mirrors what `GasPlanPanel` puts on screen beside the chart. The exported file travels alone, so
/// it has to carry the same qualifier or it states a turnaround as unconditional fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnContext {
    /// No endpoint set: the turnaround is simply the farthest point in budget, as asked.
    NoEndpoint,
    /// The turnaround IS the requested endpoint - the plan does what the diver asked.
    AtEndpoint,
    /// Gas runs out on the way there. The turn is still ON the path to the endpoint.
    ShortOfEndpoint,
    /// The endpoint cannot be reached from here at all, so the turn is the deepest point in budget
    /// and need not be on the way to anything.
    EndpointUnusable,
}

fn turn_context(field: &GasReachField, endpoint: Option<StationRef>) -> TurnContext {
    match endpoint {
        None => TurnContext::NoEndpoint,
        Some(want) if field.final_turn_ref == Some(want) => TurnContext::AtEndpoint,
        Some(_) if field.turn_toward_endpoint => TurnContext::ShortOfEndpoint,
        Some(_) => TurnContext::EndpointUnusable,
    }
}

impl TurnContext {
    /// The sentence to carry into the document, if this state needs one.
    fn caveat(self) -> Option<&'static str> {
        match self {
            Self::NoEndpoint | Self::AtEndpoint => None,
            Self::ShortOfEndpoint => Some(
                "WARNING: your endpoint is out of reach on this gas. The turnaround below is as far \
along the way to it as the budget allows - it is NOT the endpoint you chose.",
            ),
            Self::EndpointUnusable => Some(
                "WARNING: your endpoint cannot be reached from here at all. The turnaround below is \
the deepest point within budget and IS NOT ON THE WAY to your endpoint - it may lie in an unrelated \
passage.",
            ),
        }
    }
}

/// Render the profile in `format`. Errors when the plan has no route to export.
pub fn render_dive_profile(
    field: &GasReachField,
    format: ProfileFormat,
    ctx: &ProfileContext<'_>,
) -> Result<String> {
    // A profile needs a turnaround AND somewhere to swim to it. The engine already refuses to
    // publish either degenerate case (see `GasReachField::route_profile`), so in-app this is
    // unreachable - but this is the boundary a compromised renderer or a stale field arrives at, and
    // a zero-duration "dive" is exactly the artefact the module header says must not exist.
    //
    // Both tests are needed: a one-point series has no leg, and a multi-point series joined only by
    // merge/closure ties has legs but spans no metres.
    let swum_m = field
        .route_profile
        .iter()
        .fold(0.0_f64, |acc, p| acc.max(p.distance_m));
    if field.route_profile.len() < 2 || swum_m <= 0.0 {
        return Err(anyhow::anyhow!(
            "This plan has no route to export - set an origin and a reachable endpoint first."
        ));
    }
    let name = sanitize_name(ctx.project_name);
    let turn = turn_context(field, ctx.endpoint);
    match format {
        ProfileFormat::SubsurfaceXml => render_subsurface_xml(field, &name, turn),
        ProfileFormat::PlainText => Ok(render_plain_text(field, &name, ctx.units, turn)),
    }
}

/// `mm:ss` from fractional minutes - Subsurface's sample-time format.
///
/// Rounds to whole seconds and carries, so 1.9999 min renders as `2:00`, never `1:60`.
fn mmss(minutes: f64) -> String {
    let total = if minutes.is_finite() && minutes > 0.0 {
        (minutes * 60.0).round() as i64
    } else {
        0
    };
    format!("{}:{:02}", total / 60, total % 60)
}

/// The depth a document states for a route point: a station above the datum is AT the surface, and
/// so is one with no usable depth. The gas engine already reads it that way (`physics::ata` uses
/// `max(depth, 0)`), and BOTH formats go through this, so the slate and the Subsurface file of one
/// plan cannot disagree - the slate used to print `-5.0` (and `-0.0` for a hair above) where the
/// XML said `0.0 m` (#576 B27(c)).
fn surface_clamped(depth_m: f64) -> f64 {
    if depth_m.is_finite() && depth_m > 0.0 {
        depth_m
    } else {
        0.0
    }
}

/// One decimal, and never `-0.0` - a station a hair above the datum would otherwise export a
/// negative depth, which some importers reject outright.
fn depth_str(depth_m: f64) -> String {
    format!("{:.1} m", surface_clamped(depth_m))
}

fn max_depth(points: &[RouteProfilePoint]) -> f64 {
    points.iter().map(|p| p.depth_m).fold(0.0_f64, f64::max)
}

/// Subsurface's native dive XML: one `<sample>` per route station, over the whole round trip so the
/// dive returns to the entrance rather than ending at the turnaround.
///
/// The `<divelog>` wrapper and the ` min` suffix on every time are what Subsurface's own writer
/// emits; matching it is the closest we can get to importable without a round-trip through a real
/// build (see the module header - this is unverified).
///
/// Otherwise deliberately minimal - no cylinders, no gas mixes, no date. Emitting a fabricated dive
/// date or a gas mix the diver has not confirmed would make a *plan* indistinguishable from a logged
/// dive on import, which is precisely the confusion the disclaimer exists to prevent.
fn render_subsurface_xml(
    field: &GasReachField,
    project_name: &str,
    turn: TurnContext,
) -> Result<String> {
    let points = &field.route_profile;
    let mut buf = Vec::new();
    let mut writer = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);

    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    writer.write_event(Event::Comment(BytesText::new(DISCLAIMER)))?;

    let mut divelog = BytesStart::new("divelog");
    divelog.push_attribute(("program", "stalagmight"));
    divelog.push_attribute(("version", "3"));
    writer.write_event(Event::Start(divelog))?;
    writer.write_event(Event::Start(BytesStart::new("dives")))?;

    let duration = points.last().map_or(0.0, |p| p.elapsed_min);
    let mut dive = BytesStart::new("dive");
    dive.push_attribute(("number", "1"));
    dive.push_attribute(("duration", minutes_attr(duration).as_str()));
    writer.write_event(Event::Start(dive))?;

    write_text_element(&mut writer, "location", project_name)?;
    // The turn caveat rides in `<notes>` beside the disclaimer: it is the same class of statement
    // (what this file is not), and `<notes>` is the one free-text element an importer will show.
    let notes = match turn.caveat() {
        Some(caveat) => format!("{DISCLAIMER}\n\n{caveat}"),
        None => DISCLAIMER.to_string(),
    };
    write_text_element(&mut writer, "notes", &notes)?;

    let mut computer = BytesStart::new("divecomputer");
    computer.push_attribute(("model", "Stalagmight dive plan"));
    writer.write_event(Event::Start(computer))?;

    let mut depth = BytesStart::new("depth");
    depth.push_attribute(("max", depth_str(max_depth(points)).as_str()));
    writer.write_event(Event::Empty(depth))?;

    for point in points {
        let mut sample = BytesStart::new("sample");
        sample.push_attribute(("time", minutes_attr(point.elapsed_min).as_str()));
        sample.push_attribute(("depth", depth_str(point.depth_m).as_str()));
        writer.write_event(Event::Empty(sample))?;
    }

    writer.write_event(Event::End(BytesEnd::new("divecomputer")))?;
    writer.write_event(Event::End(BytesEnd::new("dive")))?;
    writer.write_event(Event::End(BytesEnd::new("dives")))?;
    writer.write_event(Event::End(BytesEnd::new("divelog")))?;

    drop(writer);
    Ok(String::from_utf8(buf)?)
}

/// `mm:ss min` - the unit-suffixed duration form Subsurface writes into time attributes.
fn minutes_attr(minutes: f64) -> String {
    format!("{} min", mmss(minutes))
}

fn write_text_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    tag: &str,
    text: &str,
) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

/// Length in the diver's unit. The field is canonical SI throughout; only what a human reads is
/// converted.
fn length(metres: f64, units: GasUnits) -> f64 {
    match units {
        GasUnits::Metric => metres,
        GasUnits::Imperial => metres / FEET_TO_METERS,
    }
}

/// Cubic feet per litre - the same factor `formatGas.ts`'s `formatVolume` uses on screen.
const CUFT_PER_LITRE: f64 = 0.035_314_7;

/// Gas volume in the diver's unit.
///
/// An imperial plan shows gas in **cubic feet** in the panel (`formatVolume`), so a litres-only
/// slate would disagree with the screen by a factor of ~28 for the one number a diver is most likely
/// to sanity-check against their gauges.
fn volume(litres: f64, units: GasUnits) -> f64 {
    match units {
        GasUnits::Metric => litres,
        GasUnits::Imperial => litres * CUFT_PER_LITRE,
    }
}

/// Fixed-width table for a slate or a notes app.
///
/// Rendered in the diver's own units and labelled with them on every column header plus a spelt-out
/// line above the table. A slate that says 30 where the app said 98 is a unit-confusion accident
/// waiting to happen, and so is an unlabelled number - both halves matter.
///
/// That means LENGTH **and** GAS: an imperial plan shows cubic feet in the panel (`formatVolume`),
/// so a litres-only slate would disagree with the screen by ~28x on the number a diver is most
/// likely to check against their gauges. Only runtime is genuinely unit-free - minutes in both
/// systems - so it alone passes through untouched.
fn render_plain_text(
    field: &GasReachField,
    project_name: &str,
    units: GasUnits,
    turn_ctx: TurnContext,
) -> String {
    let (unit, unit_long, gas_unit, gas_unit_long) = match units {
        GasUnits::Metric => ("m", "metres", "L", "litres"),
        GasUnits::Imperial => ("ft", "feet", "cuft", "cubic feet"),
    };
    let len = |metres: f64| length(metres, units);
    let depth = |metres: f64| length(surface_clamped(metres), units);
    let vol = |litres: f64| volume(litres, units);
    // Litres are whole numbers; cubic feet are one to two orders smaller, so the same `{:.0}` would
    // round a 600 L turn volume to "21" and a small stage to "0".
    let gas_dp = usize::from(units == GasUnits::Imperial);
    let points = &field.route_profile;
    let mut out = String::new();
    let _ = writeln!(out, "{project_name} - planned dive profile");
    out.push_str(DISCLAIMER);
    // The turn caveat goes ABOVE the numbers, not in a footnote: it changes what the `Turn:` line
    // below means, and a slate is read top-down.
    if let Some(caveat) = turn_ctx.caveat() {
        let _ = write!(out, "\n\n{caveat}");
    }
    let _ = write!(
        out,
        "\n\nDistance/depth in {unit_long}, runtime in minutes, gas in {gas_unit_long}.\n"
    );

    // Turn / drop callouts, keyed on route index so a route that doubles back marks the right pass.
    // `rev().find()` rather than `filter().last()`: the turnaround is the LAST outbound point, and
    // walking forward through the whole round trip to find it is the `double_ended_iterator_last`
    // lint's exact complaint.
    let turn = points.iter().rev().find(|p| p.outbound);
    let turn_index = turn.map(|p| p.route_index);
    let mut notes: Vec<String> = Vec::new();
    for (i, &route_index) in field.stage_drop_route_indices.iter().enumerate() {
        if let Some(p) = points
            .iter()
            .find(|p| p.outbound && p.route_index == route_index)
        {
            let flag = if field.stage_drop_over_reserve.get(i).copied() == Some(true) {
                "  [OVER RESERVE]"
            } else {
                ""
            };
            notes.push(format!(
                "Stage {} drop: {:.0} {unit} in, {:.1} {unit} deep, {} runtime{flag}",
                i + 1,
                len(p.distance_m),
                depth(p.depth_m),
                mmss(p.elapsed_min)
            ));
        }
    }
    if let Some(turn) = turn {
        notes.push(format!(
            "Turn: {:.0} {unit} in, {:.1} {unit} deep, {} runtime, {:.*} {gas_unit} used",
            len(turn.distance_m),
            depth(turn.depth_m),
            mmss(turn.elapsed_min),
            gas_dp,
            vol(turn.gas_l)
        ));
    }
    if !notes.is_empty() {
        out.push('\n');
        for note in &notes {
            out.push_str(note);
            out.push('\n');
        }
    }

    // Column stops, matching the row format below exactly (10, +1+10, +1+10, +1+11): 10, 21, 32, 44.
    // A slate is copied by eye, so a header sitting a character off its column is a real defect.
    // Widths are held by the format spec rather than by hand-counted spaces, so `Dist(ft)` lines up
    // as exactly as `Dist(m)` does.
    let _ = writeln!(
        out,
        "\n{:>10} {:>10} {:>10} {:>11}  Leg",
        format!("Dist({unit})"),
        format!("Depth({unit})"),
        "Runtime",
        format!("Gas({gas_unit})")
    );
    for p in points {
        let leg = if Some(p.route_index) == turn_index && p.outbound {
            "turn"
        } else if p.outbound {
            "in"
        } else {
            "out"
        };
        let _ = writeln!(
            out,
            "{:10.1} {:10.1} {:>10} {:11.*}  {leg}",
            len(p.distance_m),
            depth(p.depth_m),
            mmss(p.elapsed_min),
            gas_dp,
            vol(p.gas_l)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
    )]

    use super::*;
    use crate::models::StationRef;

    fn point(
        route_index: u32,
        distance_m: f64,
        depth_m: f64,
        min: f64,
        outbound: bool,
    ) -> RouteProfilePoint {
        RouteProfilePoint {
            route_index,
            station_ref: StationRef::new(1, route_index),
            distance_m,
            depth_m,
            gas_l: distance_m * 3.0,
            elapsed_min: min,
            outbound,
        }
    }

    fn field() -> GasReachField {
        GasReachField {
            station_refs: Vec::new(),
            gas_to_reach: Vec::new(),
            reachable: Vec::new(),
            turn_frontier: Vec::new(),
            max_reachable_gas: 0.0,
            budget_litres: 2000.0,
            route: Vec::new(),
            stage_drop_refs: Vec::new(),
            final_turn_ref: None,
            turn_toward_endpoint: false,
            backgas_turn_bar: None,
            stage_drop_bars: Vec::new(),
            stage_drop_over_reserve: Vec::new(),
            stage_drop_distances_m: Vec::new(),
            cylinder_usable_bars: Vec::new(),
            route_line_length_m: 200.0,
            route_max_depth_m: Some(24.0),
            route_avg_depth_m: Some(12.0),
            waypoint_gas_litres: Vec::new(),
            scooter_distance_m: 0.0,
            scooter_range_limit_m: None,
            scooter_drop_ref: None,
            route_runtime_min: None,
            turn_on_scooter_range: false,
            route_profile: vec![
                point(0, 0.0, 0.0, 0.0, true),
                point(1, 100.0, 12.0, 20.0, true),
                point(2, 200.0, 24.0, 40.0, true),
                point(1, 300.0, 12.0, 50.0, false),
                point(0, 400.0, 0.0, 60.0, false),
            ],
            stage_drop_route_indices: Vec::new(),
            waypoint_route_indices: Vec::new(),
        }
    }

    /// Metric, no endpoint set - so no turn caveat, which is the plain case most tests want.
    fn ctx(name: &str) -> ProfileContext<'_> {
        ProfileContext {
            project_name: name,
            units: GasUnits::Metric,
            endpoint: None,
        }
    }

    #[test]
    fn xml_emits_one_sample_per_point_over_the_whole_round_trip() {
        let xml =
            render_dive_profile(&field(), ProfileFormat::SubsurfaceXml, &ctx("Cave")).unwrap();
        assert_eq!(xml.matches("<sample ").count(), 5);
        // The last sample is back at the entrance, not at the turnaround - a profile that stopped
        // at the turn would import as a dive the diver never surfaced from.
        assert!(xml.contains(r#"time="60:00 min" depth="0.0 m""#));
        assert!(xml.contains(r#"time="40:00 min" depth="24.0 m""#));
        assert!(xml.contains(r#"<dive number="1" duration="60:00 min">"#));
        assert!(xml.contains(r#"max="24.0 m""#));
        assert!(xml.contains("<location>Cave</location>"));
    }

    #[test]
    fn subsurface_xml_matches_the_documented_grammar() {
        // Pins the shape we believe Subsurface's importer needs, since nothing here can round-trip
        // through a real Subsurface build. If the interop claim is ever tested for real, THIS is the
        // test that should change - not the renderer, silently.
        let xml =
            render_dive_profile(&field(), ProfileFormat::SubsurfaceXml, &ctx("Cave")).unwrap();
        assert!(xml.contains(r#"<divelog program="stalagmight" version="3">"#));
        // `<dives>` nests inside `<divelog>`, and every time attribute carries the unit suffix
        // Subsurface's own writer emits - a bare "60:00" is the form it does NOT write.
        let divelog = xml.find("<divelog").expect("divelog");
        let dives = xml.find("<dives>").expect("dives");
        assert!(divelog < dives);
        assert!(xml.trim_end().ends_with("</divelog>"));
        assert!(
            !xml.contains(r#"time="60:00""#),
            "time attr lost its unit suffix"
        );
    }

    #[test]
    fn a_hostile_project_name_cannot_forge_lines_or_break_the_xml() {
        // `.stalx` files are shared between cavers, and the name is stored unvalidated. A line break
        // in it would inject a second, forged "Turn:" line into a document read as plan data.
        //
        // ⚠ Every separator is checked, not just `\n`. `str::lines()` splits on `\n` alone, so a
        // test written with it passes for an implementation that handles the Cc category and nothing
        // else - which is exactly what `char::is_control()` does, and exactly the hole this test
        // used to leave open. U+2028/U+2029 are line breaks to editors, browsers and most other
        // languages' line iterators, so they forge just as effectively.
        for sep in ['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'] {
            let hostile = format!("Cave{sep}Turn: 9999 m in, 1.0 m deep, 0:01 runtime, 1 L used");
            let text =
                render_dive_profile(&field(), ProfileFormat::PlainText, &ctx(&hostile)).unwrap();
            assert!(
                !text.contains(sep) || sep == '\n',
                "U+{:04X} survived sanitising",
                sep as u32
            );
            // Split on the FULL separator set, not Rust's `\n`-only notion of a line.
            let forged = text
                .split(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'])
                .filter(|l| l.starts_with("Turn:"))
                .count();
            assert_eq!(forged, 1, "U+{:04X} forged a second turn line", sep as u32);
            assert!(text.starts_with("Cave Turn: 9999 m in"));
        }

        // Bidi overrides are stripped for the Trojan-Source reason: U+202E reverses the visual order
        // of everything after it, in a document whose whole point is to be read literally.
        for hidden in ['\u{202E}', '\u{200F}', '\u{200B}', '\u{FEFF}', '\u{2069}'] {
            let name = format!("Cave{hidden}X");
            let text =
                render_dive_profile(&field(), ProfileFormat::PlainText, &ctx(&name)).unwrap();
            assert!(
                !text.contains(hidden),
                "U+{:04X} survived sanitising",
                hidden as u32
            );
        }

        // XML: markup is escaped by quick-xml, and control characters (which XML 1.0 forbids
        // outright, so they would make the file unparseable) are gone.
        let xml = render_dive_profile(&field(), ProfileFormat::SubsurfaceXml, &ctx("A<b>&\u{7}c"))
            .unwrap();
        assert!(xml.contains("<location>A&lt;b&gt;&amp; c</location>"));
        assert!(!xml.contains("<b>"));

        // And the name is bounded rather than pasted in at arbitrary length.
        let long = "x".repeat(500);
        let text = render_dive_profile(&field(), ProfileFormat::PlainText, &ctx(&long)).unwrap();
        assert!(text.lines().next().unwrap().len() < 200);
    }

    #[test]
    fn the_export_carries_the_endpoint_qualifier_the_panel_shows() {
        // The chart sits beside the panel's warning; the exported file travels alone, so it has to
        // state for itself that the turnaround is not the place the diver asked for. Without this a
        // slate reads "Turn: 200 m in" as unconditional fact for a turn in an unrelated passage.
        let mut f = field();
        f.final_turn_ref = Some(StationRef::new(1, 2));
        let want = Some(StationRef::new(9, 9));

        f.turn_toward_endpoint = false;
        let unusable = ProfileContext {
            project_name: "Cave",
            units: GasUnits::Metric,
            endpoint: want,
        };
        let text = render_dive_profile(&f, ProfileFormat::PlainText, &unusable).unwrap();
        assert!(text.contains("IS NOT ON THE WAY"), "{text}");
        let xml = render_dive_profile(&f, ProfileFormat::SubsurfaceXml, &unusable).unwrap();
        assert!(xml.contains("IS NOT ON THE WAY"));

        // Short of the endpoint but still on the way there is a weaker claim, and a different one.
        f.turn_toward_endpoint = true;
        let short = render_dive_profile(&f, ProfileFormat::PlainText, &unusable).unwrap();
        assert!(short.contains("out of reach on this gas"), "{short}");
        assert!(!short.contains("IS NOT ON THE WAY"));

        // Reaching the endpoint the diver chose earns no caveat at all - otherwise the warning is
        // noise on every ordinary plan and stops being read.
        let at_endpoint = ProfileContext {
            project_name: "Cave",
            units: GasUnits::Metric,
            endpoint: f.final_turn_ref,
        };
        let clean = render_dive_profile(&f, ProfileFormat::PlainText, &at_endpoint).unwrap();
        assert!(!clean.contains("WARNING"), "{clean}");
        // ...and neither does a plan with no endpoint set, which is the explore-as-far-as-gas-allows
        // case rather than a failure to arrive.
        let none = render_dive_profile(&f, ProfileFormat::PlainText, &ctx("Cave")).unwrap();
        assert!(!none.contains("WARNING"), "{none}");
    }

    #[test]
    fn the_text_slate_is_rendered_in_the_divers_own_units() {
        // An imperial diver copying a metric slate is a unit-confusion accident, and the renderer
        // has the units toggle available - it just was not passing it through.
        let imperial = ProfileContext {
            project_name: "Cave",
            units: GasUnits::Imperial,
            endpoint: None,
        };
        let text = render_dive_profile(&field(), ProfileFormat::PlainText, &imperial).unwrap();
        assert!(text.contains("Distance/depth in feet"));
        assert!(text.contains("gas in cubic feet"));
        assert!(text.contains("Dist(ft)"));
        assert!(text.contains("Depth(ft)"));
        // 200 m = 656.2 ft, 24 m = 78.7 ft. Hand-computed, not recomputed with the same expression.
        assert!(text.contains("Turn: 656 ft in, 78.7 ft deep"), "{text}");
        assert!(!text.contains(" m in,"));
        // GAS converts too: the panel shows cuft for an imperial plan, so litres here would disagree
        // with the screen by ~28x. 600 L x 0.0353147 = 21.2 cuft.
        assert!(text.contains("21.2 cuft used"), "{text}");
        assert!(!text.contains(" L used"));
        // Runtime is the one genuinely unit-free column.
        assert!(text.contains("40:00 runtime"));

        // Header columns still line up once the units are two and four characters wide.
        let header = text
            .lines()
            .find(|l| l.contains("Dist(ft)"))
            .expect("header");
        assert_eq!(header.find("Dist(ft)"), Some(2));
        assert_eq!(header.find("Depth(ft)"), Some(12));
        assert_eq!(header.find("Runtime"), Some(25));
        assert_eq!(header.find("Gas(cuft)"), Some(35));
        // Metric keeps whole litres; cubic feet would round a 600 L turn to "21" and a stage to "0".
        let metric = render_dive_profile(&field(), ProfileFormat::PlainText, &ctx("Cave")).unwrap();
        assert!(metric.contains("600 L used"));
        assert!(metric.contains("gas in litres"));
    }

    #[test]
    fn a_route_that_spans_no_swum_distance_is_refused() {
        // Legs are not enough - a route joined only by merge / loop-closure ties has plenty of
        // points and spans no metres, so it is still a dive of zero distance over zero duration.
        let mut f = field();
        for p in &mut f.route_profile {
            p.distance_m = 0.0;
            p.elapsed_min = 0.0;
        }
        for format in [ProfileFormat::SubsurfaceXml, ProfileFormat::PlainText] {
            let err = render_dive_profile(&f, format, &ctx("Cave")).unwrap_err();
            assert!(err.to_string().contains("no route to export"), "{format:?}");
        }
    }

    #[test]
    fn both_formats_say_this_is_not_a_deco_schedule() {
        // The single most important property of this export. A file that reads as a dive log in a
        // deco planner, months later, must carry its own contradiction.
        let xml =
            render_dive_profile(&field(), ProfileFormat::SubsurfaceXml, &ctx("Cave")).unwrap();
        let text = render_dive_profile(&field(), ProfileFormat::PlainText, &ctx("Cave")).unwrap();
        assert!(xml.contains("NOT a decompression schedule"));
        assert!(text.contains("NOT a decompression schedule"));
    }

    #[test]
    fn text_names_its_units_and_flags_an_over_reserve_drop() {
        let mut f = field();
        f.stage_drop_route_indices = vec![1];
        f.stage_drop_over_reserve = vec![true];
        let text = render_dive_profile(&f, ProfileFormat::PlainText, &ctx("Cave")).unwrap();
        assert!(text.contains("Distance/depth in metres"));
        // Drawn at the OUTBOUND pass of route index 1 (100 m), not the return one (300 m).
        assert!(text.contains("Stage 1 drop: 100 m in, 12.0 m deep, 20:00 runtime  [OVER RESERVE]"));
        assert!(text.contains("Turn: 200 m in, 24.0 m deep, 40:00 runtime, 600 L used"));
        assert_eq!(text.lines().filter(|l| l.ends_with("  in")).count(), 2);
        assert_eq!(text.lines().filter(|l| l.ends_with("  out")).count(), 2);
        assert_eq!(text.lines().filter(|l| l.ends_with("  turn")).count(), 1);
    }

    #[test]
    fn an_empty_plan_is_an_error_not_an_empty_file() {
        // Writing a zero-sample dive would import cleanly and silently as a dive that never
        // happened, so the caller has to be told instead.
        let mut f = field();
        f.route_profile.clear();
        assert!(render_dive_profile(&f, ProfileFormat::SubsurfaceXml, &ctx("Cave")).is_err());
        assert!(render_dive_profile(&f, ProfileFormat::PlainText, &ctx("Cave")).is_err());
    }

    #[test]
    fn a_single_point_plan_is_refused_too_not_written_as_a_zero_length_dive() {
        // One point is a dive of zero distance over zero duration. The engine will not publish one
        // (it needs a leg, not just a turnaround), so reaching this branch means a stale or forged
        // field - which is exactly what an IPC boundary has to assume it might get. `is_empty()`
        // would wave it through and write `<dive duration="0:00 min">` with a single sample.
        let mut f = field();
        f.route_profile.truncate(1);
        assert_eq!(f.route_profile.len(), 1);
        for format in [ProfileFormat::SubsurfaceXml, ProfileFormat::PlainText] {
            let err = render_dive_profile(&f, format, &ctx("Cave")).unwrap_err();
            assert!(err.to_string().contains("no route to export"), "{format:?}");
        }
    }

    #[test]
    fn mmss_carries_instead_of_emitting_a_sixtieth_second() {
        assert_eq!(mmss(0.0), "0:00");
        assert_eq!(mmss(1.5), "1:30");
        // 1.9999 min is 119.994 s; rounding the SECONDS alone would print "1:60".
        assert_eq!(mmss(1.9999), "2:00");
        assert_eq!(mmss(-3.0), "0:00");
        assert_eq!(mmss(f64::NAN), "0:00");
    }

    #[test]
    fn depth_never_exports_negative_or_minus_zero() {
        // A station a hair above the datum has a slightly negative z; some importers reject it.
        assert_eq!(depth_str(-0.001), "0.0 m");
        assert_eq!(depth_str(-5.0), "0.0 m");
        assert_eq!(depth_str(f64::NAN), "0.0 m");
        assert_eq!(depth_str(12.34), "12.3 m");
    }

    #[test]
    fn the_text_slate_never_prints_a_negative_depth_either() {
        // #576 B27(c): a station above the datum (or a hair above it) has a negative z. The XML
        // already clamps it to the surface, and the gas engine already treats it as the surface
        // (`physics::ata` uses max(depth, 0)); the slate printed it raw - `-5.0` in one file and
        // `0.0 m` in the other for the same plan, and `-0.0` for a hair above.
        let mut f = field();
        f.route_profile[1].depth_m = -0.04;
        f.route_profile[2].depth_m = -5.0;
        f.route_profile[3].depth_m = f64::NAN;
        f.stage_drop_route_indices = vec![1];
        f.stage_drop_over_reserve = vec![false];
        for units in [GasUnits::Metric, GasUnits::Imperial] {
            let ctx = ProfileContext {
                project_name: "CaveA",
                units,
                endpoint: None,
            };
            let text = render_dive_profile(&f, ProfileFormat::PlainText, &ctx).unwrap();
            let rows: Vec<&str> = text
                .lines()
                .filter(|l| l.ends_with("  in") || l.ends_with("  out") || l.ends_with("  turn"))
                .collect();
            assert_eq!(rows.len(), 5, "{units:?}:\n{text}");
            // Second column of every table row is the depth.
            let depths: Vec<&str> = rows
                .iter()
                .map(|r| r.split_whitespace().nth(1).unwrap())
                .collect();
            assert_eq!(
                depths,
                ["0.0", "0.0", "0.0", "0.0", "0.0"],
                "{units:?}:\n{text}"
            );
            let unit = if units == GasUnits::Metric { "m" } else { "ft" };
            let stage = format!(
                "Stage 1 drop: {:.0} {unit} in, 0.0 {unit} deep",
                length(100.0, units)
            );
            let turn = format!(
                "Turn: {:.0} {unit} in, 0.0 {unit} deep",
                length(200.0, units)
            );
            assert!(text.contains(&stage), "{units:?}: no `{stage}` in\n{text}");
            assert!(text.contains(&turn), "{units:?}: no `{turn}` in\n{text}");
            assert!(
                !text.contains("-0.0") && !text.contains("NaN"),
                "{units:?}:\n{text}"
            );
        }
    }

    #[test]
    fn format_wire_values_round_trip_and_reject_the_unknown() {
        assert_eq!(
            ProfileFormat::from_wire("subsurface-xml").unwrap(),
            ProfileFormat::SubsurfaceXml
        );
        assert_eq!(
            ProfileFormat::from_wire("text").unwrap(),
            ProfileFormat::PlainText
        );
        assert!(ProfileFormat::from_wire("uddf").is_err());
        assert!(ProfileFormat::from_wire("").is_err());
    }

    #[test]
    fn plain_text_golden() {
        // Replaces a byte-stability check that could not fail: `render_dive_profile` is a pure
        // function over a Vec, with no map iteration, timestamp or RNG, so calling it twice was
        // never able to differ - the assertion held for any implementation, correct or not.
        //
        // A golden actually pins the thing that matters for a fixed-width table meant to be copied
        // onto a slate by eye: the column stops. Header stops are 10 / 21 / 32 / 44, and every row
        // must land on them.
        let text = render_dive_profile(&field(), ProfileFormat::PlainText, &ctx("Cave")).unwrap();
        let expected = "\
Cave - planned dive profile
Planned penetration profile from Stalagmight. NOT a decompression schedule: no ascent, stops or tissue loading are modelled. Depths and times are the planned route only.

Distance/depth in metres, runtime in minutes, gas in litres.

Turn: 200 m in, 24.0 m deep, 40:00 runtime, 600 L used

   Dist(m)   Depth(m)    Runtime      Gas(L)  Leg
       0.0        0.0       0:00           0  in
     100.0       12.0      20:00         300  in
     200.0       24.0      40:00         600  turn
     300.0       12.0      50:00         900  out
     400.0        0.0      60:00        1200  out
";
        assert_eq!(text, expected);

        // The header's own column stops, asserted rather than eyeballed.
        let header = text
            .lines()
            .find(|l| l.contains("Dist(m)"))
            .expect("header row");
        assert_eq!(header.find("Dist(m)").unwrap() + "Dist(m)".len(), 10);
        assert_eq!(header.find("Depth(m)").unwrap() + "Depth(m)".len(), 21);
        assert_eq!(header.find("Runtime").unwrap() + "Runtime".len(), 32);
        assert_eq!(header.find("Gas(L)").unwrap() + "Gas(L)".len(), 44);
    }
}
