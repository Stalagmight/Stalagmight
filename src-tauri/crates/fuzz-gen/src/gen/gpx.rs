//! GPX generator: a `<gpx>` document with metadata, waypoints, tracks (with
//! segments) and routes.
//!
//! Semantic knobs: coordinates at and past their ranges (and in other number
//! spellings), missing or duplicated `lat`/`lon`, elevation missing on some
//! points (the converter fills those in from neighbours), points that repeat
//! (zero-length shots), namespaced tags, unknown and deeply nested extension
//! elements, CDATA and entity-escaped text, a document cut off mid-element,
//! and the encodings / byte-order marks a GPS export might use.

use std::fmt::Write as _;

use crate::fileset::FileSet;
use crate::gen::{count, damage, line_endings, spell, Encoding, Faults};
use crate::source::ByteSource;

fn escaped_text(src: &mut ByteSource<'_>, f: Faults, max: usize) -> String {
    let raw = src.text(max);
    match src.below(6) {
        0 if !raw.contains("]]>") => format!("<![CDATA[{raw}]]>"),
        1 if f.on() => raw, // unescaped: may well break the XML, on purpose
        _ => raw
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;"),
    }
}

fn coord(src: &mut ByteSource<'_>, f: Faults, limit: f64, base: f64, step: f64) -> String {
    let v = if f.hit(src, 16) {
        src.number(-limit * 2.0, limit * 2.0)
    } else {
        base + src.f64_in(-step, step)
    };
    if f.on() {
        spell(src, v)
    } else {
        format!("{v:.7}")
    }
}

fn point_attrs(src: &mut ByteSource<'_>, f: Faults, base: (f64, f64)) -> String {
    let lat = coord(src, f, 90.0, base.0, 0.001);
    let lon = coord(src, f, 180.0, base.1, 0.001);
    match if f.on() { src.below(24) } else { 24 } {
        0 => format!(r#"lon="{lon}""#),
        1 => format!(r#"lat="{lat}""#),
        2 => format!(r#"lat="{lat}" lat="{lat}" lon="{lon}""#),
        3 => format!(r"lat={lat} lon={lon}"),
        4 | 5 => format!(r#"lon="{lon}" lat='{lat}' extra="&amp;""#),
        _ => format!(r#"lat="{lat}" lon="{lon}""#),
    }
}

fn point_children(src: &mut ByteSource<'_>, f: Faults, out: &mut String, ele_base: f64) {
    if !src.chance(40) {
        let ele = if f.hit(src, 16) {
            src.number(-11_000.0, 9_000.0)
        } else {
            ele_base + src.f64_in(-5.0, 5.0)
        };
        let ele = if f.on() {
            spell(src, ele)
        } else {
            format!("{ele:.2}")
        };
        let _ = write!(out, "<ele>{ele}</ele>");
    }
    if src.chance(64) {
        let bad = f.hit(src, 128);
        let time = if bad {
            src.pick_or(&["not a time", "", "2024-13-40T99:99:99"], "")
        } else {
            "2024-01-02T03:04:05Z"
        };
        let _ = write!(out, "<time>{time}</time>");
    }
    if src.chance(64) {
        let _ = write!(out, "<name>{}</name>", escaped_text(src, f, 20));
    }
    if src.chance(32) {
        let cmt = escaped_text(src, f, 30);
        let desc = escaped_text(src, f, 30);
        let _ = write!(out, "<cmt>{cmt}</cmt><desc>{desc}</desc>");
    }
    if src.chance(16) {
        nested_extensions(src, f, out);
    }
}

/// Unknown elements the reader must skip, sometimes nested very deep.
fn nested_extensions(src: &mut ByteSource<'_>, f: Faults, out: &mut String) {
    let depth = if src.chance(16) {
        200 + src.below(2000)
    } else {
        1 + src.below(4)
    };
    out.push_str("<extensions>");
    for i in 0..depth {
        let _ = write!(out, "<x:e{} a=\"{i}\">", i % 7);
    }
    out.push_str(&escaped_text(src, f, 8));
    for i in (0..depth).rev() {
        if f.hit(src, 2) {
            continue; // an unbalanced close, rarely
        }
        let _ = write!(out, "</x:e{}>", i % 7);
    }
    out.push_str("</extensions>");
}

fn points(
    src: &mut ByteSource<'_>,
    f: Faults,
    out: &mut String,
    tag: &str,
    n: usize,
    base: (f64, f64),
) {
    let ele_base = src.f64_in(-50.0, 500.0);
    let mut last = String::new();
    for _ in 0..n {
        // A repeated point is a zero-length shot.
        let attrs = if !last.is_empty() && src.chance(16) {
            last.clone()
        } else {
            point_attrs(src, f, base)
        };
        last.clone_from(&attrs);
        if src.chance(24) {
            let _ = write!(out, "<{tag} {attrs}/>");
        } else {
            let _ = write!(out, "<{tag} {attrs}>");
            point_children(src, f, out, ele_base);
            let _ = write!(out, "</{tag}>");
        }
    }
}

/// Render one GPX import.
pub fn generate(src: &mut ByteSource<'_>) -> FileSet {
    let f = Faults::pick(src);
    let base = (src.f64_in(-80.0, 80.0), src.f64_in(-179.0, 179.0));
    let prefix = if src.chance(24) { "gpx:" } else { "" };
    let mut out = String::new();
    if !src.chance(16) {
        out.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        out.push('\n');
    }
    if f.hit(src, 8) {
        out.push_str("<!DOCTYPE gpx [<!ENTITY e \"&#x26;&#38;\">]>\n");
    }
    let _ = write!(
        out,
        r#"<{prefix}gpx version="1.1" creator="fuzz" xmlns="http://www.topografix.com/GPX/1/1" xmlns:gpx="http://www.topografix.com/GPX/1/1" xmlns:x="urn:x">"#
    );
    if src.bool() {
        let name = escaped_text(src, f, 24);
        let _ = write!(out, "<metadata><name>{name}</name></metadata>");
    }
    let waypoints = count(src, 6, 2000);
    points(src, f, &mut out, &format!("{prefix}wpt"), waypoints, base);
    for _ in 0..count(src, 3, 50) {
        let name = escaped_text(src, f, 16);
        let _ = write!(out, "<trk><name>{name}</name>");
        for _ in 0..=count(src, 2, 30) {
            out.push_str("<trkseg>");
            let n = count(src, 10, 5000);
            points(src, f, &mut out, "trkpt", n, base);
            out.push_str("</trkseg>");
        }
        out.push_str("</trk>");
    }
    for _ in 0..count(src, 2, 20) {
        let name = escaped_text(src, f, 16);
        let _ = write!(out, "<rte><name>{name}</name>");
        let n = count(src, 8, 3000);
        points(src, f, &mut out, "rtept", n, base);
        out.push_str("</rte>");
    }
    if src.chance(8) {
        nested_extensions(src, f, &mut out);
    }
    let _ = write!(out, "</{prefix}gpx>");
    let text = line_endings(src, &out);
    let bytes = Encoding::pick(src).encode(&text);
    FileSet::single("track.gpx", damage(src, bytes))
}
