use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};

use crate::parsers::geo::non_empty;

use super::{
    apply_segment, apply_units_options, datum_recognized, is_anonymous, is_omitted,
    is_walls_separator, parse_angle, parse_depth, parse_dms, parse_length, set_prefix,
    split_first_token, strip_inline_comment, utm_to_wgs84, AngleUnit, Case, LengthUnit, LrudType,
    OrderElem, ParsedSurvey, VectorType, WallsFix, WallsRef, WallsUnits, WallsVector,
};

// ---------------------------------------------------------------------------
// .SRV parsing
// ---------------------------------------------------------------------------

/// Parse the contents of one `.SRV` file into normalized vectors.
pub(crate) fn parse_srv_contents(
    contents: &str,
    title: &str,
    name: &str,
    inherited_segment: &str,
    initial_units: &WallsUnits,
    initial_macros: &HashMap<String, String>,
    georef: Option<WallsRef>,
    grid_relative: bool,
    derive_decl: bool,
    warnings: &mut Vec<String>,
) -> ParsedSurvey {
    let mut units = initial_units.clone();
    let mut unit_stack: Vec<WallsUnits> = Vec::new();
    // Seed with macros inherited from the project/book/survey `.OPTIONS`; inline
    // `#units $name=value` directives add to this same table as parsing proceeds.
    let mut macros: HashMap<String, String> = initial_macros.clone();
    let mut segment = inherited_segment.to_string();
    let mut date: Option<NaiveDate> = None;
    let mut surveyor: Option<String> = None;
    let mut block_comment_depth: u32 = 0;

    let mut vectors: Vec<WallsVector> = Vec::new();
    let mut fixes: Vec<WallsFix> = Vec::new();
    // Whether any shot in this survey was measured in feet (LengthUnit::Feet at
    // the point it was recorded, not merely mentioned in a `#Units` directive
    // that was later overridden) - surfaces on the import receipt so a
    // feet-to-meters conversion is never silent (#391).
    let mut used_feet = false;

    for (line_no, raw) in contents.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        // `trimmed` only classifies the line (blank, comment, directive). The
        // text that gets parsed is trimmed of Walls separators only, so a
        // U+00A0 at either end of the line stays in the station name or value
        // it touches, as it does mid-line.
        let trimmed = line.trim();
        let fields = line.trim_matches(is_walls_separator);
        if trimmed.is_empty() {
            continue;
        }
        // Block comments (#[ ... #]).
        if trimmed.starts_with("#[") {
            block_comment_depth += 1;
            continue;
        }
        if trimmed.starts_with("#]") {
            block_comment_depth = block_comment_depth.saturating_sub(1);
            continue;
        }
        if block_comment_depth > 0 {
            continue;
        }
        // Whole-line comment.
        if trimmed.starts_with(';') {
            let body = trimmed[1..].trim();
            // Heuristic: capture a team/surveyor annotation.
            if surveyor.is_none() {
                if let Some(team) = extract_team(body) {
                    surveyor = Some(team);
                }
            }
            continue;
        }

        // Directive line.
        if trimmed.starts_with('#') {
            let expanded = match expand_macros(fields.trim_start(), &macros) {
                Ok(s) => s,
                Err(e) => {
                    warnings.push(format!("{name}:{}: {e}", line_no + 1));
                    continue;
                }
            };
            handle_directive(
                &expanded,
                &mut units,
                &mut unit_stack,
                &mut macros,
                &mut segment,
                &mut date,
                &georef,
                &mut fixes,
                name,
                line_no + 1,
                warnings,
            );
            continue;
        }

        // Vector / data line.
        let expanded = match expand_macros(fields, &macros) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!("{name}:{}: {e}", line_no + 1));
                continue;
            }
        };
        if let Some(vec) = parse_vector_line(&expanded, &units, date, grid_relative, derive_decl) {
            if units.d_unit == LengthUnit::Feet || units.s_unit == LengthUnit::Feet {
                used_feet = true;
            }
            vectors.push(vec);
        }
    }

    // If this survey projected any UTM `#Fix` through an unrecognized datum,
    // warn ONCE (the datum is a `.REF` property shared by every fix, so a
    // per-fix warning would repeat identically across thousands of rows).
    if let Some(r) = &georef {
        if !r.datum.trim().is_empty() && !datum_recognized(&r.datum) {
            let projected = fixes.iter().filter(|f| f.projected_from_utm).count();
            if projected > 0 {
                warnings.push(format!(
                    "{name}: datum '{}' is not recognized; {projected} fixed station(s) given \
                     as UTM coordinates were projected as WGS84 and may be off by tens of meters.",
                    r.datum
                ));
            }
        }
    }

    let survey_date = vectors.iter().find_map(|v| v.date).or(date);
    ParsedSurvey {
        title: title.to_string(),
        name: name.to_string(),
        segment,
        date: survey_date,
        surveyor,
        vectors,
        fixes,
        georef,
        // A parsed survey is resolved by construction; the unresolved
        // placeholder is built directly in `parse_wpj`.
        resolved: true,
        used_feet,
    }
}

pub(crate) fn extract_team(comment: &str) -> Option<String> {
    let lower = comment.to_ascii_lowercase();
    for key in ["team:", "surveyors:", "surveyor:", "survey:"] {
        if let Some(pos) = lower.find(key) {
            let after = comment[pos + key.len()..].trim();
            if !after.is_empty() {
                return Some(after.to_string());
            }
        }
    }
    None
}

pub(crate) fn handle_directive(
    line: &str,
    units: &mut WallsUnits,
    unit_stack: &mut Vec<WallsUnits>,
    macros: &mut HashMap<String, String>,
    segment: &mut String,
    date: &mut Option<NaiveDate>,
    georef: &Option<WallsRef>,
    fixes: &mut Vec<WallsFix>,
    name: &str,
    line_no: usize,
    warnings: &mut Vec<String>,
) {
    let (directive, rest) = split_first_token(line);
    // Strip any trailing `;` comment from the directive arguments (quote-aware).
    // Walls allows an inline comment after a directive (e.g. `#Date 2008-12-01
    // ;TODO`); without this the date/segment/prefix value would carry the note.
    let rest = strip_inline_comment(rest);
    let key = directive.trim_start_matches('#').to_ascii_lowercase();
    match key.as_str() {
        "units" | "u" => {
            apply_units_options(units, rest, unit_stack, macros, warnings);
        }
        "date" => {
            let raw = rest.trim();
            match parse_walls_date(raw) {
                Some(parsed) => *date = Some(parsed),
                // A later malformed `#Date` must NOT clobber a previously-set
                // valid date - that would silently drop the survey date and
                // force 0° auto-declination for the shots that follow. Keep the
                // prior value and warn instead.
                None if !raw.is_empty() => {
                    warnings.push(format!("{name}:{line_no}: unrecognized #Date '{raw}'"));
                }
                None => {}
            }
        }
        "segment" | "seg" | "s" => {
            apply_segment(segment, rest.trim());
        }
        "prefix" | "prefix1" => set_prefix(units, 0, rest.trim_matches(is_walls_separator)),
        "prefix2" => set_prefix(units, 1, rest.trim_matches(is_walls_separator)),
        "prefix3" => set_prefix(units, 2, rest.trim_matches(is_walls_separator)),
        "fix" => {
            if let Some(fix) = parse_fix(rest, units, georef) {
                fixes.push(fix);
            }
        }
        "flag" | "f" | "note" | "n" | "symbol" | "sym" => {
            // Parsed but not modeled (flags/notes/symbols don't affect geometry).
        }
        _ => {
            // Unknown directive - ignore.
        }
    }
}

// ---------------------------------------------------------------------------
// Vector line parsing
// ---------------------------------------------------------------------------

pub(crate) fn parse_vector_line(
    line: &str,
    units: &WallsUnits,
    date: Option<NaiveDate>,
    grid_relative: bool,
    derive_decl: bool,
) -> Option<WallsVector> {
    // Split off the comment (everything after the first ';').
    let (body, comment) = match line.find(';') {
        Some(pos) => (&line[..pos], non_empty(&line[pos + 1..])),
        None => (line, None),
    };

    // Extract delimited LRUD / variance spans and the leftover token stream.
    let extracted = extract_spans(body);
    let tokens = extracted.tokens;
    if tokens.len() < 2 {
        // Lone-station LRUD lines (passage dimensions) and stray fragments -
        // skip silently (we can't attach a station-only LRUD to a shot).
        return None;
    }

    let from_raw = &tokens[0];
    let to_raw = &tokens[1];
    let from = normalize_station(from_raw, units);
    let to = normalize_station(to_raw, units);

    // Anonymous wall point (`-`/`--`/`---`): a splay reading, not a connected leg.
    let from_anon = is_anonymous(from_raw);
    let to_anon = is_anonymous(to_raw);

    let meas = &tokens[2..];
    let mut cursor = 0usize;

    let mut length_m: Option<f64> = None;
    let mut az: FsBs = FsBs::default();
    let mut inc: FsBs = FsBs::default();
    let mut east = 0.0;
    let mut north = 0.0;
    let mut up_rect = 0.0;

    for elem in units.active_order() {
        let tok = meas.get(cursor);
        cursor += 1;
        let Some(tok) = tok else { break };
        match elem {
            OrderElem::Dist => {
                length_m = parse_length(tok, units.d_unit);
            }
            OrderElem::Azm => {
                az = parse_fsbs(tok, units.a_unit, units.ab_unit);
            }
            OrderElem::Inc => {
                inc = parse_fsbs(tok, units.v_unit, units.vb_unit);
            }
            OrderElem::East => east = parse_length(tok, units.d_unit).unwrap_or(0.0),
            OrderElem::North => north = parse_length(tok, units.d_unit).unwrap_or(0.0),
            OrderElem::Up => up_rect = parse_length(tok, units.d_unit).unwrap_or(0.0),
        }
    }

    // RECT vector: convert E/N/U deltas to a CT-style vector.
    if units.vector_type == VectorType::Rect {
        let horiz = (east * east + north * north).sqrt();
        let length = (horiz * horiz + up_rect * up_rect).sqrt();
        if length <= 0.0 {
            return None;
        }
        // `#Units RECT=<angle>` rotates the rectangular E/N frame clockwise from
        // true north; apply it to the computed bearing here (declination is then
        // skipped at conversion, while `.REF`/`#Units Grid=` convergence still
        // applies). A pure-vertical RECT shot (E=N=0) has no defined bearing, so
        // leave the azimuth unset rather than recording a spurious 0° (due north).
        let azimuth_deg = if horiz < 1e-9 {
            None
        } else {
            Some(crate::models::normalize_azimuth(
                east.atan2(north).to_degrees() + units.rect,
            ))
        };
        let inc_up = up_rect.atan2(horiz).to_degrees();
        // RECT vectors carry the same trailing `<L R U D>` passage dimensions as
        // CT vectors; the span is already extracted above, so parse it like the
        // CT path rather than dropping the wall data. (#241)
        let (left, right, up, down) = extracted
            .lrud
            .as_deref()
            .map(|l| parse_lrud(l, units))
            .unwrap_or((None, None, None, None));
        return Some(WallsVector {
            from,
            to,
            length_m: length,
            azimuth_in: azimuth_deg,
            azimuth_out: azimuth_deg,
            inc_deg: Some(inc_up),
            pitch_in: Some(-inc_up),
            pitch_out: None,
            from_depth: None,
            to_depth: None,
            is_depth_gauge: false,
            left,
            right,
            up,
            down,
            comment,
            is_reference: from_anon || to_anon,
            from_anon,
            to_anon,
            date,
            units_decl: units.decl,
            inch: 0.0,
            grid_relative,
            // `#Units Grid=` convergence rotates true→grid north for ALL vectors,
            // CT and RECT alike; the RECT frame angle (`units.rect`) is a separate
            // rotation already baked into the bearing above.
            grid_convergence: units.grid,
            derive_decl,
            is_rect: true,
            lrud_is_to: units.lrud_type == LrudType::To,
        });
    }

    let is_dive = units.is_depth_gauge();
    let mut from_depth = None;
    let mut to_depth = None;
    if is_dive {
        // Two trailing depth columns (station depths below surface). They scale
        // by the S ("short") unit, NOT the D unit - matching genuine Walls
        // `parse_height`, which reads depth/height columns via
        // `parse_length_arg(val, i, u->bFeetS)` (the S feet flag). The S unit
        // covers LRUD plus instrument/target heights and these station depths.
        if let Some(tok) = meas.get(cursor) {
            from_depth = parse_depth(tok, units.s_unit);
            cursor += 1;
        }
        if let Some(tok) = meas.get(cursor) {
            to_depth = parse_depth(tok, units.s_unit);
        }
    }

    // Apply the `IncD=` distance correction only when a NON-ZERO distance was
    // given - genuine Walls guards this as `if (u->incD != 0.0 && d != 0.0)`
    // (`wall-srv.cpp`). Adding IncD to an exact-`0` leg would turn a deliberate
    // coincident-station tie (`A B 0 …`) into a spurious IncD-long shot.
    let length_m = length_m
        .map(|d| if d != 0.0 { d + units.incd } else { 0.0 })
        .unwrap_or(0.0);

    // Combine foresight/backsight azimuth into two forward readings.
    let (azimuth_in, azimuth_out) = combine_azimuth(&az, units);
    // Combine inclination.
    let (inc_deg, pitch_in, pitch_out) = if is_dive {
        (None, None, None)
    } else {
        combine_inclination(&inc, units)
    };

    // LRUD.
    let (left, right, up, down) = extracted
        .lrud
        .as_deref()
        .map(|l| parse_lrud(l, units))
        .unwrap_or((None, None, None, None));

    // A floated/`(?)` variance is a network-adjustment hint on a REAL measured
    // leg - it does not make the shot a splay, so it must not be dropped by the
    // splay filter. Only anonymous (`-`/`--`) wall endpoints are splays.
    let is_reference = from_anon || to_anon;

    // Pure station-equate (no distance, no depth, no bearing) - skip. A
    // zero-length leg that still carries a bearing (fore- OR back-sight) is a
    // coincident-station tie worth keeping, matching `is_importable_shot`.
    if length_m <= 0.0
        && from_depth.is_none()
        && to_depth.is_none()
        && az.fs.is_none()
        && az.bs.is_none()
    {
        if left.is_none() && right.is_none() && up.is_none() && down.is_none() {
            return None;
        }
    }

    Some(WallsVector {
        from,
        to,
        length_m,
        azimuth_in,
        azimuth_out,
        inc_deg,
        pitch_in,
        pitch_out,
        from_depth,
        to_depth,
        is_depth_gauge: is_dive,
        left,
        right,
        up,
        down,
        comment,
        is_reference,
        from_anon,
        to_anon,
        date,
        units_decl: units.decl,
        // IncH only adjusts the vertical of non-plumb CT shots; dive shots take
        // their vertical from the depth gauges, so it never applies there.
        inch: if is_dive { 0.0 } else { units.inch },
        grid_relative,
        // `#Units Grid=` convergence applies to CT azimuths (dive shots take it
        // too); RECT vectors apply it the same way on top of their frame rotation.
        grid_convergence: units.grid,
        derive_decl,
        is_rect: false,
        lrud_is_to: units.lrud_type == LrudType::To,
    })
}

#[derive(Default)]
pub(crate) struct FsBs {
    pub(crate) fs: Option<f64>,
    pub(crate) bs: Option<f64>,
}

pub(crate) struct ExtractedSpans {
    pub(crate) tokens: Vec<String>,
    pub(crate) lrud: Option<String>,
}

/// The first `close` at or after `start` in `chars` that *properly terminates*
/// a span, as `(closer index, index to resume scanning at)`. Genuine Walls
/// only treats a token-initial `<`/`*` as an LRUD when the matching `>`/`*` is
/// followed by whitespace or end-of-line (`wall-srv.cpp`: `!pNote[1] ||
/// isspace(pNote[1])`); otherwise the delimiter is part of a station name.
///
/// "Whitespace" here is a Walls separator ([`is_walls_separator`]), the rule
/// the tokenizer splits fields by - with one exception (#576 WALLS-LRUD,
/// decision "only trailing NBSP closes"): a closer followed by nothing but
/// no-break spaces (U+00A0, often pasted from a word processor) and
/// separators up to the end of the line still closes, and that blank tail is
/// consumed with it. Anywhere else a U+00A0 joins the fields it sits
/// between, so `*A*<NBSP>B` is one station name, not LRUD `A` plus station
/// `<NBSP>B`; at the end of the line there is nothing left to join, so an
/// LRUD followed by one is not lost.
pub(crate) fn find_span_end(chars: &[char], start: usize, close: char) -> Option<(usize, usize)> {
    (start..chars.len()).find_map(|j| {
        if chars[j] != close {
            return None;
        }
        match chars.get(j + 1) {
            None => Some((j, j + 1)),
            Some(&c) if is_walls_separator(c) => Some((j, j + 1)),
            Some('\u{a0}')
                if chars[j + 1..]
                    .iter()
                    .all(|&c| c == '\u{a0}' || is_walls_separator(c)) =>
            {
                Some((j, chars.len()))
            }
            Some(_) => None,
        }
    })
}

/// Walk a vector line body, pulling out `<...>`/`*...*` (LRUD) and `(...)`
/// (variance) delimited spans, returning the remaining whitespace tokens.
pub(crate) fn extract_spans(body: &str) -> ExtractedSpans {
    let chars: Vec<char> = body.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut lrud: Option<String> = None;
    let mut i = 0;

    let flush = |current: &mut String, tokens: &mut Vec<String>| {
        if !current.is_empty() {
            tokens.push(std::mem::take(current));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        match c {
            c if is_walls_separator(c) => flush(&mut current, &mut tokens),
            '#' => {
                // Inline directive (segment) - stop collecting measurement
                // tokens; the rest of the line doesn't carry geometry.
                flush(&mut current, &mut tokens);
                break;
            }
            // LRUD/variance delimiters open a span ONLY at a token boundary
            // (`current` empty) AND only when properly terminated (closer at
            // end-of-body or before whitespace). Walls lays a shot out as
            // whitespace-separated fields `from to <measures…> [lrud] [(var)]`,
            // so a delimiter glued inside a token - e.g. the literal `*` in
            // `W-*/2/46` - or an UNTERMINATED `<`/`*` - e.g. a station named
            // `*8` - is a literal name character, not a span. Without this a
            // mid-name `*` tore `W-*/2/46 W-*/2/50` into a phantom `W-` hub, and
            // an unterminated `*8` swallowed the rest of the line as a bogus LRUD.
            '<' | '*' if current.is_empty() => {
                let close = if c == '<' { '>' } else { '*' };
                if let Some((end, resume)) = find_span_end(&chars, i + 1, close) {
                    lrud = Some(chars[i + 1..end].iter().collect());
                    i = resume;
                    continue;
                }
                current.push(c); // unterminated → literal name character
            }
            '(' if current.is_empty() => {
                // Variance override `(...)` - consume through the first `)` (or
                // to end of body if unterminated) and discard. We don't model
                // per-shot variance, and a floated `(?)` does NOT turn a real
                // measured leg into a splay.
                i = (i + 1..chars.len())
                    .find(|&j| chars[j] == ')')
                    .map_or(chars.len(), |e| e + 1);
                continue;
            }
            other => current.push(other),
        }
        i += 1;
    }
    flush(&mut current, &mut tokens);

    ExtractedSpans { tokens, lrud }
}

/// Combine an azimuth fs/bs pair into two forward-direction degree readings.
fn combine_azimuth(az: &FsBs, units: &WallsUnits) -> (Option<f64>, Option<f64>) {
    let fs = az
        .fs
        .map(|v| crate::models::normalize_azimuth(v + units.inca));
    let bs_forward = az.bs.map(|v| {
        let corrected = v + units.incab;
        if units.type_ab_corrected {
            crate::models::normalize_azimuth(corrected)
        } else {
            crate::models::normalize_azimuth(corrected + 180.0)
        }
    });
    if units.type_ab_no_avg {
        // Use foresight only.
        return (fs.or(bs_forward), fs.or(bs_forward));
    }
    match (fs, bs_forward) {
        (Some(f), Some(b)) => (Some(f), Some(b)),
        (Some(f), None) => (Some(f), Some(f)),
        (None, Some(b)) => (Some(b), Some(b)),
        (None, None) => (None, None),
    }
}

/// Combine an inclination fs/bs pair: returns (avg `+up` deg, pitch_in `+down`,
/// pitch_out `+down`).
fn combine_inclination(inc: &FsBs, units: &WallsUnits) -> (Option<f64>, Option<f64>, Option<f64>) {
    let fs = inc.fs.map(|v| v + units.incv);
    let bs_forward = inc.bs.map(|v| {
        let corrected = v + units.incvb;
        if units.type_vb_corrected {
            corrected
        } else {
            -corrected
        }
    });
    if units.type_vb_no_avg {
        let chosen = fs.or(bs_forward);
        return (chosen, chosen.map(|v| -v), None);
    }
    match (fs, bs_forward) {
        (Some(f), Some(b)) => (Some((f + b) / 2.0), Some(-f), Some(-b)),
        (Some(f), None) => (Some(f), Some(-f), None),
        (None, Some(b)) => (Some(b), Some(-b), None),
        (None, None) => (None, None, None),
    }
}

fn parse_lrud(
    span: &str,
    units: &WallsUnits,
) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let parts: Vec<&str> = span
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    let mut vals: [Option<f64>; 4] = [None; 4];
    for (i, slot) in units.lrud_order.iter().enumerate() {
        let Some(tok) = parts.get(i) else { break };
        if is_omitted(tok) {
            continue;
        }
        // Trailing facing/C-flag tokens are ignored (positions >= 4).
        // IncS= corrects the "short" measurements (LRUDs + inst/target heights).
        let v = parse_length(tok, units.s_unit).map(|x| x + units.incs);
        let idx = match slot {
            'L' => 0,
            'R' => 1,
            'U' => 2,
            'D' => 3,
            _ => continue,
        };
        vals[idx] = v.filter(|x| *x >= 0.0);
    }
    (vals[0], vals[1], vals[2], vals[3])
}

// ---------------------------------------------------------------------------
// #Fix parsing
// ---------------------------------------------------------------------------

fn parse_fix(rest: &str, units: &WallsUnits, georef: &Option<WallsRef>) -> Option<WallsFix> {
    // #Fix <name> <E> <N> <U> [(var)] [/note] [#seg]
    // Strip a trailing `;` comment before tokenizing (the vector path does the
    // same); otherwise `467427 2258942;datum` swallows the northing token.
    let body = match rest.find(';') {
        Some(pos) => &rest[..pos],
        None => rest,
    };
    let extracted = extract_spans(body);
    let tokens = extracted.tokens;
    if tokens.is_empty() {
        return None;
    }
    let name = normalize_station(&tokens[0], units);
    let coords = &tokens[1..];
    // `#Fix` columns are E/N/U coordinates, independent of the vector
    // measurement order (a dive survey's CT `Order=DA` must NOT truncate the
    // elevation column). The rectangular `rect_order` is exactly that - an
    // explicit `Order=ENU…` if one was set, otherwise the E/N/U default. Each
    // column is a length or an N/S/E/W-prefixed geographic angle.
    let coord_order: &[OrderElem] = &units.rect_order;
    let mut east_val: Option<f64> = None;
    let mut north_val: Option<f64> = None;
    let mut up_val: Option<f64> = None;
    let mut lat: Option<f64> = None;
    let mut lon: Option<f64> = None;
    let mut idx = 0usize;
    for elem in coord_order {
        let Some(tok) = coords.get(idx) else { break };
        idx += 1;
        match elem {
            OrderElem::East | OrderElem::Dist => {
                // Place a geographic coordinate by the N/S/E/W prefix it carries,
                // not by the column, so a fix written latitude-first still
                // resolves correctly. A plain number is a UTM easting.
                if let Some((axis, geo)) = parse_geo_coord(tok) {
                    match axis {
                        GeoAxis::Lat => lat = Some(geo),
                        GeoAxis::Lon => lon = Some(geo),
                    }
                } else {
                    east_val = parse_length(tok, units.d_unit);
                }
            }
            OrderElem::North | OrderElem::Azm => {
                if let Some((axis, geo)) = parse_geo_coord(tok) {
                    match axis {
                        GeoAxis::Lat => lat = Some(geo),
                        GeoAxis::Lon => lon = Some(geo),
                    }
                } else {
                    north_val = parse_length(tok, units.d_unit);
                }
            }
            OrderElem::Up | OrderElem::Inc => {
                up_val = parse_length(tok, units.d_unit);
            }
        }
    }

    // The datum a UTM fix projects under is a property of the inherited `.REF`,
    // not of the individual fix, so any "unrecognized datum" warning is emitted
    // ONCE per survey (see `parse_srv_contents`) rather than here - otherwise a
    // surface file's thousands of `#Fix` rows each repeat the same message.
    let mut projected_from_utm = false;
    let lat_long = match (lat, lon) {
        (Some(la), Some(lo)) => Some((la, lo)),
        _ => match (east_val, north_val, georef) {
            (Some(e), Some(n), Some(r)) => {
                let projected = utm_to_wgs84(e, n, r.zone, &r.datum);
                projected_from_utm = projected.is_some();
                projected
            }
            _ => None,
        },
    };

    Some(WallsFix {
        name,
        lat_long,
        up: up_val,
        projected_from_utm,
    })
}

/// Which geographic axis an `N/S/E/W`-prefixed `#Fix` coordinate names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GeoAxis {
    Lat,
    Lon,
}

/// Parse a geographic coordinate token like `W97:43:52.5` / `N31:16:45`,
/// returning the axis (lat/long) named by the prefix so the caller can place it
/// correctly regardless of column order.
fn parse_geo_coord(tok: &str) -> Option<(GeoAxis, f64)> {
    let first = tok.chars().next()?;
    let (axis, sign) = match first.to_ascii_uppercase() {
        'N' => (GeoAxis::Lat, 1.0),
        'S' => (GeoAxis::Lat, -1.0),
        'E' => (GeoAxis::Lon, 1.0),
        'W' => (GeoAxis::Lon, -1.0),
        _ => return None,
    };
    // The matched prefix is ASCII, so byte index 1 is a char boundary even for
    // Latin-1-decoded (multibyte) text.
    let deg = parse_dms(&tok[1..])?;
    Some((axis, sign * deg.abs()))
}

pub(crate) fn parse_fsbs(tok: &str, fs_unit: AngleUnit, bs_unit: AngleUnit) -> FsBs {
    match tok.split_once('/') {
        Some((l, r)) => FsBs {
            fs: if l.is_empty() || is_omitted(l) {
                None
            } else {
                parse_angle(l, fs_unit)
            },
            bs: if r.is_empty() || is_omitted(r) {
                None
            } else {
                parse_angle(r, bs_unit)
            },
        },
        None => FsBs {
            fs: if is_omitted(tok) {
                None
            } else {
                parse_angle(tok, fs_unit)
            },
            bs: None,
        },
    }
}

pub(crate) fn parse_walls_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Walls writes ISO `YYYY-MM-DD` (and `/`) plus US `MM-DD-YYYY` and 2-digit
    // `MM-DD-YY`. chrono's `%Y` greedily matches a 2-digit leading field, so the
    // ISO formats can NOT be tried first: `05/07/17` (May 7 2017) would wrongly
    // parse as year 0005. Disambiguate by field widths: a 4-digit FIRST field is
    // year-first (ISO); otherwise it is month-first, and a 4- vs 2-digit LAST
    // field selects `%Y` vs `%y` (chrono maps `%y` 00-69 → 2000-2069, 70-99 →
    // 1970-1999).
    let parts: Vec<&str> = s.split(['-', '/']).map(str::trim).collect();
    let first_len = parts.first().map_or(0, |f| f.len());
    let last_len = parts.last().map_or(0, |f| f.len());
    if first_len == 4 || last_len == 4 {
        let formats: &[&str] = if first_len == 4 {
            &["%Y-%m-%d", "%Y/%m/%d"]
        } else {
            &["%m-%d-%Y", "%m/%d/%Y"]
        };
        for fmt in formats {
            if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
                return Some(d);
            }
        }
        return None;
    }
    // 2-digit year. Genuine Walls maps EVERY 2-digit year into 1900-1999 with no
    // pivot - `if (y < 100) y += 1900;` in `wall-srv.cpp` `ParseDate`. We mirror
    // that exactly so the date (and any IGRF auto-declination Walls derived from
    // it) reproduces Walls' own interpretation. chrono's `%y` pivots 00-69 →
    // 2000s, so roll any 2000s result back a century. (This also keeps a 1960s
    // legacy survey, e.g. `64` → 1964, inside the IGRF range. Trade-off: a survey
    // actually done in 2025 but written `…-25` reads as 1925 - exactly as Walls
    // reads it; Walls itself warns that `yyyy-mm-dd` is preferred.)
    //
    // Leap-day exception: `02-29-00` parses (via `%y`) to 2000-02-29, but 1900
    // was NOT a leap year, so `with_year(1900)` fails and `unwrap_or(d)` keeps
    // 2000-02-29. That is the only valid date the two digits can denote (Feb 29
    // 1900 does not exist), so we deliberately leave it at 2000 rather than force
    // a parse failure - the single date that escapes the "every 2-digit year →
    // 1900s" rule, by arithmetic necessity.
    for fmt in &["%m-%d-%y", "%m/%d/%y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(if d.year() >= 2000 {
                d.with_year(d.year() - 100).unwrap_or(d)
            } else {
                d
            });
        }
    }
    None
}

/// Normalize a station name: apply `Case=` folding and prefix levels.
pub(crate) fn normalize_station(raw: &str, units: &WallsUnits) -> String {
    let cased = match units.case {
        Case::Upper => raw.to_uppercase(),
        Case::Lower => raw.to_lowercase(),
        Case::Mixed => raw.to_string(),
    };
    // Combine the entered name with the active prefix levels exactly as Walls
    // does (dewalls `WallsUnits::processStationName`): the count of explicit
    // `:`-separated segments already typed suppresses that many of the LOWEST
    // inherited prefix levels, and each remaining level is prepended so the
    // outermost ends up at the front (Prefix3:Prefix2:Prefix1:name); leading
    // `:` (from unset/suppressed levels) are then stripped. `units.prefix` is
    // indexed [Prefix1, Prefix2, Prefix3] (innermost → outermost). A leading
    // colon therefore cancels the inherited Prefix1, e.g. `:A1` under
    // `Prefix=CAVE` resolves to bare `A1`, not `CAVE:A1`.
    let explicit_count = cased.matches(':').count();
    let mut name = cased;
    for level in explicit_count..units.prefix.len() {
        let p = units.prefix[level].as_deref().unwrap_or("");
        name = format!("{p}:{name}");
    }
    name.trim_start_matches(':').to_string()
}

/// Expand `$(name)` macro references in a line. Operates on `&str` slices
/// (`$(`, `)` are ASCII) so multibyte UTF-8 (decoded Latin-1) text is preserved
/// verbatim.
pub(crate) fn expand_macros(
    line: &str,
    macros: &HashMap<String, String>,
) -> Result<String, String> {
    if !line.contains("$(") {
        return Ok(line.to_string());
    }
    let mut out = String::new();
    let mut rest = line;
    while let Some(pos) = rest.find("$(") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        match after.find(')') {
            Some(close) => {
                let name = &after[..close];
                match macros.get(name) {
                    Some(val) => out.push_str(val),
                    None => return Err(format!("undefined macro $({name})")),
                }
                rest = &after[close + 1..];
            }
            None => {
                // Unterminated `$(` - emit the remainder literally.
                out.push_str(&rest[pos..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}
