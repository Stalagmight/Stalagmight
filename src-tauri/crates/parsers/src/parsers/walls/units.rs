use std::collections::HashMap;

use crate::parsers::geo::FEET_TO_METERS;

use super::{
    dms, AngleUnit, Case, LengthUnit, LrudType, OrderElem, TapeEnd, VectorType, WallsUnits,
};

/// Whether `c` separates two fields of a Walls line. Walls is a Windows C
/// program whose field separators are the ASCII whitespace characters (space,
/// tab, LF, VT, FF, CR) - exactly the ASCII characters `char::is_whitespace`
/// accepts. Of the non-ASCII characters it also accepts, a Windows-1252 file
/// can hold only the no-break space (byte `0xA0`, U+00A0), and that belongs to
/// the station name or value it sits in. (`char::is_ascii_whitespace` is not
/// the same test: it leaves out VT.)
pub(crate) fn is_walls_separator(c: char) -> bool {
    c.is_ascii() && c.is_whitespace()
}

/// Split a `#Units` argument string into tokens on whitespace, treating a
/// double-quoted span as a single token (the quotes are retained so `unquote`
/// can strip them later). Walls permits quoted values with embedded spaces -
/// e.g. `Flag="loose rock"`, `Prefix="A B"`, `$name="meters decl"` - which a
/// plain `split_whitespace` would tear across tokens, corrupting the value and
/// emitting spurious "unknown option" warnings.
pub(crate) fn split_units_args(args: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in args.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            c if is_walls_separator(c) && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Apply a `#Units` argument string to the running units state.
pub(crate) fn apply_units_options(
    units: &mut WallsUnits,
    args: &str,
    unit_stack: &mut Vec<WallsUnits>,
    macros: &mut HashMap<String, String>,
    warnings: &mut Vec<String>,
) {
    // Drop an inline `;` comment and glue values back onto keys split by stray
    // spaces around `=` (`Order =DAV` -> `Order=DAV`) before tokenizing.
    let cleaned = collapse_eq_spaces(strip_inline_comment(args));
    for token in split_units_args(&cleaned) {
        if token.is_empty() {
            continue;
        }
        // Macro definition: $name or $name=value.
        if let Some(stripped) = token.strip_prefix('$') {
            let (mname, mval) = match stripped.split_once('=') {
                Some((k, v)) => (k.to_string(), unquote(v).to_string()),
                None => (stripped.to_string(), String::new()),
            };
            macros.insert(mname, mval);
            continue;
        }
        let (key, value) = match token.split_once('=') {
            Some((k, v)) => (k.to_ascii_lowercase(), Some(v)),
            None => (token.to_ascii_lowercase(), None),
        };
        // A token like `=DAV` (a leading `=`, from a stray space that
        // `collapse_eq_spaces` couldn't reattach) has no key - skip it silently
        // rather than warn "Unknown #units option ''".
        if key.is_empty() {
            continue;
        }
        apply_units_option(units, &key, value, unit_stack, warnings);
    }
}

/// Parse a numeric `#Units` correction argument. A bare/empty value resets the
/// correction to `0`; a non-empty value that fails to parse leaves the current
/// correction UNCHANGED and records a warning, rather than silently zeroing a
/// real instrument correction.
pub(crate) fn parse_correction(
    value: Option<&str>,
    current: f64,
    key: &str,
    warnings: &mut Vec<String>,
    parse: impl Fn(&str) -> Option<f64>,
) -> f64 {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => 0.0,
        Some(v) => match parse(v) {
            Some(x) => x,
            None => {
                warnings.push(format!(
                    "Unparseable #Units {key}='{v}'; correction left unchanged"
                ));
                current
            }
        },
    }
}

pub(crate) fn apply_units_option(
    units: &mut WallsUnits,
    key: &str,
    value: Option<&str>,
    unit_stack: &mut Vec<WallsUnits>,
    warnings: &mut Vec<String>,
) {
    match key {
        "reset" => {
            // RESET reverts units/calibration/prefixes to defaults (but not the
            // active segment, which lives outside WallsUnits).
            *units = WallsUnits::default();
        }
        "save" => {
            // Push current units (max depth 10, matching Walls).
            if unit_stack.len() < 10 {
                unit_stack.push(units.clone());
            }
        }
        "restore" => {
            if let Some(saved) = unit_stack.pop() {
                *units = saved;
            }
        }
        "meters" | "meter" | "m" => {
            units.d_unit = LengthUnit::Meters;
            units.s_unit = LengthUnit::Meters;
        }
        "feet" | "f" => {
            units.d_unit = LengthUnit::Feet;
            units.s_unit = LengthUnit::Feet;
        }
        // CT/RECT only flip the active vector type - the per-type column orders
        // (`ct_order` / `rect_order`) are independent and untouched, matching
        // dewalls. This is what lets a bare `#Units CT` after a `#Units RECT`
        // block read shots with the CT order again instead of dropping them.
        "ct" => units.vector_type = VectorType::Ct,
        "rect" => {
            // `Rect` switches to rectangular E/N/U vectors; `Rect=<azimuth>` sets
            // the bearing of the +N (Y) axis (the grid rotation applied to the
            // computed bearing at conversion). Either way the rectangular column
            // order lives in `rect_order`, not here.
            units.vector_type = VectorType::Rect;
            if let Some(v) = value {
                units.rect = parse_signed_angle(v).unwrap_or(units.rect);
            }
        }
        "order" | "o" => {
            if let Some(v) = value {
                match parse_order(v) {
                    // Route by the element letters, not the current vector type
                    // (dewalls `order()` -> ctOrder/rectOrder): E/N/U is a
                    // rectangular order, anything else (D/A/V) is compass-and-tape.
                    Some(order) => {
                        if order.iter().any(|e| {
                            matches!(e, OrderElem::East | OrderElem::North | OrderElem::Up)
                        }) {
                            units.rect_order = order;
                        } else {
                            units.ct_order = order;
                        }
                    }
                    None => warnings.push(format!(
                        "Ignoring unparseable #Units Order='{v}'; keeping the previous order"
                    )),
                }
            }
        }
        "d" => {
            if let Some(v) = value {
                units.d_unit = parse_length_unit(v).unwrap_or(units.d_unit);
            }
        }
        "s" => {
            if let Some(v) = value {
                units.s_unit = parse_length_unit(v).unwrap_or(units.s_unit);
            }
        }
        "a" => {
            if let Some(v) = value {
                let u = parse_angle_unit(v).unwrap_or(units.a_unit);
                units.a_unit = u;
            }
        }
        "ab" => {
            if let Some(v) = value {
                units.ab_unit = parse_angle_unit(v).unwrap_or(units.ab_unit);
            }
        }
        "a/ab" => {
            if let Some(v) = value {
                let u = parse_angle_unit(v).unwrap_or(units.a_unit);
                units.a_unit = u;
                units.ab_unit = u;
            }
        }
        "v" => {
            if let Some(v) = value {
                units.v_unit = parse_angle_unit_incl(v).unwrap_or(units.v_unit);
            }
        }
        "vb" => {
            if let Some(v) = value {
                units.vb_unit = parse_angle_unit_incl(v).unwrap_or(units.vb_unit);
            }
        }
        "v/vb" => {
            if let Some(v) = value {
                let u = parse_angle_unit_incl(v).unwrap_or(units.v_unit);
                units.v_unit = u;
                units.vb_unit = u;
            }
        }
        // `Decl=<angle>` sets an explicit declination - including an explicit
        // `Decl=0`, which declares the azimuths already true/grid and suppresses
        // auto-derivation. A bare `Decl=` / `Decl` clears it back to the default
        // (derive when the .STATUS flag is on).
        "decl" => match value.map(str::trim).filter(|v| !v.is_empty()) {
            None => units.decl = None,
            Some(v) => match parse_signed_angle(v) {
                Some(d) => units.decl = Some(d),
                // Do NOT fabricate an explicit `Decl=0` on a parse error: that
                // would wrongly suppress IGRF auto-derivation. Warn and keep the
                // current declination state instead.
                None => warnings.push(format!(
                    "Unparseable #Units Decl='{v}'; declination left unchanged"
                )),
            },
        },
        "grid" => {
            units.grid = parse_correction(value, units.grid, "Grid", warnings, parse_signed_angle);
        }
        "incd" => {
            let u = units.d_unit;
            units.incd =
                parse_correction(value, units.incd, "IncD", warnings, |v| parse_length(v, u));
        }
        // Azimuth/inclination corrections carry the current instrument's angle
        // unit (a bare `IncV=2` under `V=Grads` means 2 grads).
        "inca" => {
            let u = units.a_unit;
            units.inca =
                parse_correction(value, units.inca, "IncA", warnings, |v| parse_angle(v, u));
        }
        "incab" => {
            let u = units.ab_unit;
            units.incab =
                parse_correction(value, units.incab, "IncAB", warnings, |v| parse_angle(v, u));
        }
        "incv" => {
            let u = units.v_unit;
            units.incv =
                parse_correction(value, units.incv, "IncV", warnings, |v| parse_angle(v, u));
        }
        "incvb" => {
            let u = units.vb_unit;
            units.incvb =
                parse_correction(value, units.incvb, "IncVB", warnings, |v| parse_angle(v, u));
        }
        "incs" => {
            let u = units.s_unit;
            units.incs =
                parse_correction(value, units.incs, "IncS", warnings, |v| parse_length(v, u));
        }
        "inch" => {
            let u = units.d_unit;
            units.inch =
                parse_correction(value, units.inch, "IncH", warnings, |v| parse_length(v, u));
        }
        "typeab" => apply_type_backsight(
            value,
            &mut units.type_ab_corrected,
            &mut units.type_ab_no_avg,
        ),
        "typevb" => apply_type_backsight(
            value,
            &mut units.type_vb_corrected,
            &mut units.type_vb_no_avg,
        ),
        "case" => {
            units.case = match value.map(|v| v.chars().next().unwrap_or(' ').to_ascii_uppercase()) {
                Some('U') => Case::Upper,
                Some('L') => Case::Lower,
                _ => Case::Mixed,
            }
        }
        "lrud" => apply_lrud_option(units, value, warnings),
        "tape" => apply_tape_option(units, value, warnings),
        "prefix" | "prefix1" => set_prefix(units, 0, value.unwrap_or("")),
        "prefix2" => set_prefix(units, 1, value.unwrap_or("")),
        "prefix3" => set_prefix(units, 2, value.unwrap_or("")),
        "flag" => {
            units.flag = value
                .map(|v| unquote(v).to_string())
                .filter(|s| !s.is_empty());
        }
        "uvh" | "uvv" | "uv" => {
            // Variance scaling - not modeled (we don't carry per-shot variance).
        }
        _ => {
            warnings.push(format!("Unknown #units option '{key}'"));
        }
    }
}

pub(crate) fn apply_type_backsight(value: Option<&str>, corrected: &mut bool, no_avg: &mut bool) {
    *corrected = false;
    *no_avg = false;
    let Some(v) = value else { return };
    let mut parts = v.split(',');
    if let Some(kind) = parts.next() {
        if kind
            .chars()
            .next()
            .map(|c| c.eq_ignore_ascii_case(&'c'))
            .unwrap_or(false)
        {
            *corrected = true;
        }
    }
    // Skip tolerance; last part may be X (use foresight only).
    for p in parts {
        if p.eq_ignore_ascii_case("x") {
            *no_avg = true;
        }
    }
}

pub(crate) fn apply_lrud_option(
    units: &mut WallsUnits,
    value: Option<&str>,
    warnings: &mut Vec<String>,
) {
    let Some(v) = value else { return };
    let (kind, order) = match v.split_once(':') {
        Some((k, o)) => (k, Some(o)),
        None => (v, None),
    };
    units.lrud_type = match kind.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('T') => LrudType::To,
        _ => LrudType::From,
    };
    if let Some(o) = order {
        let chars: Vec<char> = o.chars().map(|c| c.to_ascii_uppercase()).collect();
        if chars.len() == 4 && chars.iter().all(|c| "LRUD".contains(*c)) {
            units.lrud_order = [chars[0], chars[1], chars[2], chars[3]];
        } else {
            warnings.push(format!(
                "Ignoring #Units LRUD order '{o}' (expected 4 of L, R, U, D); keeping LRUD"
            ));
        }
    }
}

pub(crate) fn apply_tape_option(
    units: &mut WallsUnits,
    value: Option<&str>,
    warnings: &mut Vec<String>,
) {
    let Some(v) = value else { return };
    let chars: Vec<char> = v.chars().map(|c| c.to_ascii_uppercase()).collect();
    let end = |c: char| match c {
        'I' => Some(TapeEnd::Instrument),
        'S' => Some(TapeEnd::Station),
        'T' => Some(TapeEnd::Target),
        _ => None,
    };
    match (
        chars.first().copied().and_then(end),
        chars.get(1).copied().and_then(end),
    ) {
        (Some(from), Some(to)) => {
            units.tape_from = from;
            units.tape_to = to;
        }
        _ => warnings.push(format!(
            "Ignoring #Units Tape='{v}' (expected two of I/S/T, e.g. Tape=SS)"
        )),
    }
}

pub(crate) fn set_prefix(units: &mut WallsUnits, level: usize, value: &str) {
    let v = unquote(value);
    units.prefix[level] = if v.trim_matches(is_walls_separator).is_empty() {
        None
    } else {
        Some(v.trim_matches(is_walls_separator).to_string())
    };
}

pub(crate) fn apply_segment(segment: &mut String, arg: &str) {
    let arg = arg.trim();
    if arg.is_empty() {
        return;
    }
    let normalized = arg.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix('/') {
        *segment = stripped.to_string();
        return;
    }
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if let Some(pos) = segment.rfind('/') {
                    segment.truncate(pos);
                } else {
                    segment.clear();
                }
            }
            other => {
                if segment.is_empty() {
                    *segment = other.to_string();
                } else {
                    segment.push('/');
                    segment.push_str(other);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Numeric token parsers
// ---------------------------------------------------------------------------

pub(crate) fn is_omitted(tok: &str) -> bool {
    tok.len() >= 2 && tok.chars().all(|c| c == '-')
}

/// A from/to station of exactly one or two dashes (`-` or `--`) is an anonymous
/// wall/splay endpoint, not a real named station - matching genuine Walls'
/// `name_isnull` (`wall-srv.cpp`: `*p=='-'` and the next char is a space, or
/// `--` then a space). Three-or-more dashes (`---`, `----`) are NOT null in
/// Walls: they are stored as a real station name, so we must keep them too.
/// (dewalls' `optionalStationRx = "-+"` treats any dash run as anonymous, which
/// is the reverse-engineering being broader than McKenzie's source.) Each
/// anonymous endpoint is later given a globally-unique synthetic name so
/// unrelated splays never collapse onto a single phantom `--` node.
pub(crate) fn is_anonymous(tok: &str) -> bool {
    matches!(tok, "-" | "--")
}

pub(crate) fn unquote(s: &str) -> &str {
    let s = s.trim_matches(is_walls_separator);
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
}

/// Split a directive line into its keyword and its argument. The keyword ends
/// at the first whitespace character, U+00A0 included (no keyword holds one).
/// The argument starts past the Walls separators after that, so a U+00A0
/// opening it stays in the station name or value it begins, as it would in a
/// vector line.
pub(crate) fn split_first_token(line: &str) -> (&str, &str) {
    let trimmed = line.trim_start();
    match trimmed.char_indices().find(|&(_, c)| c.is_whitespace()) {
        Some((pos, c)) => (
            &trimmed[..pos],
            trimmed[pos + c.len_utf8()..].trim_start_matches(is_walls_separator),
        ),
        None => (trimmed, ""),
    }
}

/// Strip a trailing `;` line-comment from a directive argument, honoring
/// double-quoted spans so a quoted value containing `;` is preserved. Walls
/// treats `;` as a comment start anywhere outside quotes, so e.g.
/// `#Date 2008-12-01 ;TODO` and `#Date 2000-06-06; oraz 2000-06-30` carry the
/// date plus a trailing note - without stripping, the date fails to parse.
pub(crate) fn strip_inline_comment(s: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => return &s[..i],
            _ => {}
        }
    }
    s
}

/// Collapse whitespace immediately around `=` (outside quotes) in a `#Units`
/// argument string, so a hand-edited `Order = DAV` / `Order =DAV` / `Order= DAV`
/// is read as `Order=DAV`. Walls keys bind tightly to `=`; stray spaces would
/// otherwise split the value into its own token and emit spurious
/// "unknown option" / empty-option warnings.
pub(crate) fn collapse_eq_spaces(args: &str) -> String {
    let mut out = String::with_capacity(args.len());
    let mut in_quotes = false;
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                out.push(c);
            }
            '=' if !in_quotes => {
                while out.ends_with([' ', '\t']) {
                    out.pop();
                }
                out.push('=');
                while matches!(chars.peek(), Some(' ' | '\t')) {
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn split_unit_suffix<'a>(t: &'a str, suffixes: &[char]) -> (&'a str, Option<char>) {
    if let Some(last) = t.chars().last() {
        if suffixes.contains(&last) {
            return (
                &t[..t.len() - last.len_utf8()],
                Some(last.to_ascii_lowercase()),
            );
        }
    }
    (t, None)
}

pub(crate) fn parse_length_unit(v: &str) -> Option<LengthUnit> {
    match v.chars().next()?.to_ascii_uppercase() {
        'M' => Some(LengthUnit::Meters),
        'F' => Some(LengthUnit::Feet),
        _ => None,
    }
}

pub(crate) fn parse_angle_unit(v: &str) -> Option<AngleUnit> {
    match v.chars().next()?.to_ascii_uppercase() {
        'D' => Some(AngleUnit::Degrees),
        'G' => Some(AngleUnit::Grads),
        'M' => Some(AngleUnit::Mils),
        _ => None,
    }
}

pub(crate) fn parse_angle_unit_incl(v: &str) -> Option<AngleUnit> {
    match v.chars().next()?.to_ascii_uppercase() {
        'D' => Some(AngleUnit::Degrees),
        'G' => Some(AngleUnit::Grads),
        'M' => Some(AngleUnit::Mils),
        'P' => Some(AngleUnit::Percent),
        _ => None,
    }
}

pub(crate) fn parse_order(v: &str) -> Option<Vec<OrderElem>> {
    let mut order = Vec::new();
    for c in v.chars() {
        let elem = match c.to_ascii_uppercase() {
            'D' => OrderElem::Dist,
            'A' => OrderElem::Azm,
            'V' => OrderElem::Inc,
            'E' => OrderElem::East,
            'N' => OrderElem::North,
            'U' => OrderElem::Up,
            _ => return None,
        };
        order.push(elem);
    }
    if order.is_empty() {
        None
    } else {
        Some(order)
    }
}

pub(crate) fn parse_signed_angle(v: &str) -> Option<f64> {
    parse_angle(v, AngleUnit::Degrees)
}

/// Parse a length token (with optional `m`/`f`/`i` suffix or feet-inches form)
/// into meters.
pub(crate) fn parse_length(tok: &str, default: LengthUnit) -> Option<f64> {
    let t = tok.trim();
    if t.is_empty() || is_omitted(t) {
        return None;
    }
    // Feet-and-inches: `<ft>i<in>`, `<ft>i`, or `i<in>` - 'i' separates feet
    // from inches. (Measurement columns are numeric, so a lone 'i' only ever
    // appears in this notation or in junk like "inf", which fails to parse.)
    let lower = t.to_ascii_lowercase();
    if let Some(ipos) = lower.find('i') {
        let fpart = &t[..ipos];
        let inches_str = &t[ipos + 1..];
        if fpart.is_empty() && inches_str.is_empty() {
            return None; // bare "i" is not a measurement
        }
        let feet: f64 = if fpart.is_empty() {
            0.0
        } else {
            fpart.parse().ok()?
        };
        let inches: f64 = if inches_str.is_empty() {
            0.0
        } else {
            inches_str.parse().ok()?
        };
        // A leading '-' applies to the whole feet-and-inches measurement, so
        // the inches must inherit the feet token's sign (`-2i6` = -2 ft 6 in,
        // `-0i7` = -7 in). Adding `inches` with a fixed positive sign would
        // flip `-0iN` and under-count `-FiI`.
        let sign = if t.starts_with('-') { -1.0 } else { 1.0 };
        let m = sign * (feet.abs() + inches / 12.0) * FEET_TO_METERS;
        return m.is_finite().then_some(m);
    }
    let (num_str, unit) = split_unit_suffix(t, &['m', 'M', 'f', 'F']);
    let v: f64 = num_str.parse().ok()?;
    if !v.is_finite() {
        return None; // reject NaN / inf / overflow literals
    }
    let u = match unit {
        Some('m') => LengthUnit::Meters,
        Some('f') => LengthUnit::Feet,
        _ => default,
    };
    Some(v * u.factor())
}

/// Parse a depth gauge reading (positive = below surface) into meters.
pub(crate) fn parse_depth(tok: &str, default: LengthUnit) -> Option<f64> {
    if is_omitted(tok) {
        return None;
    }
    parse_length(tok, default)
}

/// Parse a Walls quadrant bearing (`N79E`, `S30W`, `N12:30E`) into an absolute
/// azimuth in degrees. The magnitude is measured off the N/S meridian toward
/// E/W and carries the azimuth `unit`. Returns `None` for non-quadrant tokens.
pub(crate) fn parse_quadrant(t: &str, unit: AngleUnit) -> Option<f64> {
    let first = t.chars().next()?.to_ascii_uppercase();
    let last = t.chars().next_back()?.to_ascii_uppercase();
    if !matches!(first, 'N' | 'S') || !matches!(last, 'E' | 'W') {
        return None;
    }
    // N/S/E/W are single-byte ASCII, so trimming the ends by byte index is safe.
    let mid = t[1..t.len() - 1].trim();
    if mid.is_empty() {
        return None;
    }
    let mag = if mid.contains(':') {
        parse_dms(mid)?
    } else {
        unit.to_degrees(mid.parse::<f64>().ok()?)
    };
    if !mag.is_finite() {
        return None;
    }
    let az = match (first, last) {
        ('N', 'E') => mag,
        ('N', 'W') => 360.0 - mag,
        ('S', 'E') => 180.0 - mag,
        ('S', 'W') => 180.0 + mag,
        _ => return None,
    };
    Some(crate::models::normalize_azimuth(az))
}

/// Parse an angle token (degrees) with optional `d`/`g`/`m`/`p` suffix, a
/// quadrant bearing, or `D:M:S` form.
pub(crate) fn parse_angle(tok: &str, default: AngleUnit) -> Option<f64> {
    let t = tok.trim();
    if t.is_empty() || is_omitted(t) {
        return None;
    }
    // Quadrant bearings (`N79E`) must be tried before the `:` / numeric paths
    // so `N12:30E` isn't misread as a bare D:M:S value.
    if let Some(az) = parse_quadrant(t, default) {
        return Some(az);
    }
    if t.contains(':') {
        return parse_dms(t);
    }
    let (num_str, unit) = split_unit_suffix(t, &['d', 'D', 'g', 'G', 'm', 'M', 'p', 'P']);
    let v: f64 = num_str.parse().ok()?;
    if !v.is_finite() {
        return None; // reject NaN / inf / overflow literals
    }
    let u = match unit {
        Some('d') => AngleUnit::Degrees,
        Some('g') => AngleUnit::Grads,
        Some('m') => AngleUnit::Mils,
        Some('p') => AngleUnit::Percent,
        _ => default,
    };
    let deg = u.to_degrees(v);
    deg.is_finite().then_some(deg)
}

/// Parse a `D:M:S` / `D:M` / `D` angle into decimal degrees.
pub(crate) fn parse_dms(t: &str) -> Option<f64> {
    let neg = t.starts_with('-');
    let body = t.trim_start_matches(['-', '+']);
    let mut parts = body.split(':');
    let d: f64 = parts.next()?.trim().parse().ok()?;
    let m: f64 = match parts.next() {
        Some(s) if !s.is_empty() => s.trim().parse().ok()?,
        _ => 0.0,
    };
    let s: f64 = match parts.next() {
        Some(x) if !x.is_empty() => x.trim().parse().ok()?,
        _ => 0.0,
    };
    if !d.is_finite() || !m.is_finite() || !s.is_finite() {
        return None;
    }
    // `body` is sign-stripped, so `dms` sees non-negative degrees and returns the
    // magnitude; apply the leading-sign here. Shares the one D:M:S formula.
    let val = dms(d, m, s);
    Some(if neg { -val } else { val })
}
