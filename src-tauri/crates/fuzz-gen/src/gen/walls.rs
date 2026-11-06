//! Walls `.srv` / `.wpj` generator.
//!
//! A `.srv` is line-oriented: `#Units` and the other directives change how
//! every vector line after them reads, so the generator keeps a model of the
//! units state ([`Units`]) and writes each reading in the unit, column order
//! and notation the importer will read it in. That keeps most files
//! geometrically sane, so they reach conversion and the core import, while
//! the directives range over the whole grammar. A `.wpj` is a tree of
//! `.BOOK`s whose `.SURVEY` leaves name `.srv` files, resolved through the
//! `.PATH`s and `.OPTIONS` they inherit.
//!
//! Semantic knobs (`.srv`): `#Units` with every option the importer reads
//! (feet / metres, separate `D=` / `S=` units, CT and RECT column orders,
//! `A=` / `AB=` / `V=` / `VB=` in degrees, grads, mils and percent, `Decl=`,
//! `Grid=`, the `Inc*=` corrections, `TypeAB=` / `TypeVB=`, `Case=`, `LRUD=`,
//! `Tape=` including the depth-gauge `Order=DA Tape=SS` mode, `Prefix1..3=`,
//! `Flag=`, `UV*=`, `$name=` macros, `Save` / `Restore` / `Reset`),
//! `#Prefix1..3`, `#Segment` paths, `#Flag` / `#Note` / `#Symbol`, `#Fix` in
//! UTM and geographic form, `#Date` in each spelling Walls writes, `$( )`
//! expansion in names and readings, nested `#[ ... #]` block comments,
//! vectors with fs/bs `/` pairs, quadrant, D:M:S and unit-suffixed readings,
//! feet-and-inches lengths, LRUD as `<...>` and `*...*` with omitted `--`
//! values and unit-scaled sentinels, variance `( )` spans, anonymous splay
//! stations, inline `;` comments, huge vector counts and long `#Prefix`
//! chains. Hostile files add undefined macros, unbalanced block comments,
//! unparseable options and readings, edge-value numbers and station names
//! carrying more colons than there are prefix levels.
//!
//! Semantic knobs (`.wpj`): book trees (sometimes a hundred levels deep;
//! unbalanced `.ENDBOOK`s and depths past the importer's limit only in hostile
//! files), `.NAME` / `.STATUS` / `.OPTIONS` / `.PATH` / `.REF` entries, and
//! survey links that resolve, are missing, climb with `..`, are absolute or
//! refused (drive letter, UNC, DOS device), differ in case, name one file
//! twice or name the project itself.

use std::fmt::Write as _;

use crate::fileset::FileSet;
use crate::gen::{count, damage, line_endings, sentinel, spell, Encoding, Faults, FEET_TO_M};
use crate::source::ByteSource;

/// Most lines one rendered file may have.
const MAX_LINES: usize = 1450;

/// Most `.SURVEY` entries one `.wpj` lists.
const MAX_SURVEYS: usize = 10;

/// A text file being rendered, counted in lines.
#[derive(Debug, Default)]
struct Doc {
    text: String,
    lines: usize,
}

impl Doc {
    fn line(&mut self, s: &str) {
        self.text.push_str(s);
        self.text.push('\n');
        self.lines += 1;
    }

    /// True when fewer than `reserve` lines are left.
    fn full(&self, reserve: usize) -> bool {
        self.lines + reserve >= MAX_LINES
    }
}

/// Free text for a title or comment: one line, unless the file is hostile.
fn words(src: &mut ByteSource<'_>, f: Faults, max: usize) -> String {
    let t = src.text(max);
    if f.on() {
        t
    } else {
        t.replace(['\n', '\r'], " ")
    }
}

/// `v` as a reading: two decimals mostly, now and then another spelling the
/// importer still reads, and any spelling at all in a hostile file.
fn num(src: &mut ByteSource<'_>, f: Faults, v: f64) -> String {
    if f.on() && src.chance(96) {
        return spell(src, v);
    }
    if !src.chance(32) {
        return format!("{v:.2}");
    }
    match src.below(5) {
        0 => format!("{v}"),
        1 => format!("{v:e}"),
        2 if v >= 0.0 => format!("+{v:.1}"),
        3 => format!("{v:08.3}"),
        _ => format!("{v:.4}"),
    }
}

/// A plausible value in `lo..=hi`; in a hostile file sometimes an edge value.
fn val(src: &mut ByteSource<'_>, f: Faults, lo: f64, hi: f64) -> f64 {
    if f.on() {
        src.number(lo, hi)
    } else {
        src.f64_in(lo, hi)
    }
}

/// The `#Units` state the importer will be in, as far as the readings
/// written after it depend on it.
#[derive(Debug, Clone)]
struct Units {
    d_feet: bool,
    s_feet: bool,
    ct_order: Vec<char>,
    rect_order: Vec<char>,
    rect: bool,
    tape_ss: bool,
    a: char,
    ab: char,
    v: char,
    vb: char,
    prefix1: Option<String>,
}

impl Default for Units {
    fn default() -> Self {
        Self {
            d_feet: false,
            s_feet: false,
            ct_order: vec!['D', 'A', 'V'],
            rect_order: vec!['E', 'N', 'U'],
            rect: false,
            tape_ss: false,
            a: 'D',
            ab: 'D',
            v: 'D',
            vb: 'D',
            prefix1: None,
        }
    }
}

impl Units {
    /// Depth-gauge vectors: distance and azimuth, then two station depths.
    fn dive(&self) -> bool {
        !self.rect
            && self.tape_ss
            && self.ct_order.contains(&'D')
            && self.ct_order.contains(&'A')
            && !self.ct_order.contains(&'V')
    }
}

/// `m` metres in feet when `feet`.
fn length_in(feet: bool, m: f64) -> f64 {
    if feet {
        m / FEET_TO_M
    } else {
        m
    }
}

/// `deg` degrees in Walls angle unit `unit`.
fn angle_in(unit: char, deg: f64) -> f64 {
    match unit {
        'G' => deg / 0.9,
        'M' => deg * 6400.0 / 360.0,
        'P' => deg.to_radians().tan() * 100.0,
        _ => deg,
    }
}

/// A non-negative angle as `D:M:S`.
fn dms(deg: f64) -> String {
    let a = deg.abs();
    let d = a.trunc();
    let m = ((a - d) * 60.0).trunc();
    let s = ((a - d) * 60.0 - m) * 60.0;
    let sign = if deg < 0.0 { "-" } else { "" };
    format!("{sign}{d:.0}:{m:.0}:{s:.2}")
}

/// An azimuth as a quadrant bearing (`N45.0E`).
fn quadrant(az: f64) -> String {
    let az = az.rem_euclid(360.0);
    let (a, b, m) = if az < 90.0 {
        ('N', 'E', az)
    } else if az < 180.0 {
        ('S', 'E', 180.0 - az)
    } else if az < 270.0 {
        ('S', 'W', az - 180.0)
    } else {
        ('N', 'W', 360.0 - az)
    };
    format!("{a}{m:.1}{b}")
}

/// One station shot so far: its name as typed and the `Prefix1` it was typed
/// under.
#[derive(Debug, Clone)]
struct Station {
    name: String,
    prefix: Option<String>,
}

/// Every station shot so far, across the files of a project, so later
/// vectors (and later files) tie into the earlier ones.
#[derive(Debug, Default)]
struct Net {
    stations: Vec<Station>,
    next: u32,
    tag: String,
}

impl Net {
    fn fresh(&mut self, u: &Units) -> String {
        self.next += 1;
        let name = format!("{}{}", self.tag, self.next);
        self.stations.push(Station {
            name: name.clone(),
            prefix: u.prefix1.clone(),
        });
        name
    }

    /// Station `i` as it must be typed under `u` to name the same station:
    /// an explicit `P:` (or a bare `:`) when the active `Prefix1` differs.
    fn refer(&self, i: usize, u: &Units) -> Option<String> {
        let s = self.stations.get(i)?;
        Some(if s.prefix == u.prefix1 {
            s.name.clone()
        } else {
            match &s.prefix {
                Some(p) => format!("{p}:{}", s.name),
                None => format!(":{}", s.name),
            }
        })
    }

    /// A station to shoot from: the newest one usually, any earlier one
    /// (a branch) sometimes.
    fn pick(&self, src: &mut ByteSource<'_>, u: &Units) -> Option<String> {
        let n = self.stations.len();
        if n == 0 {
            return None;
        }
        let i = if src.chance(40) { src.below(n) } else { n - 1 };
        self.refer(i, u)
    }
}

/// What the directives of one survey (and the `.OPTIONS` above it) leave
/// behind.
#[derive(Debug, Clone, Default)]
struct State {
    u: Units,
    saved: Vec<Units>,
    /// A macro whose value is this file's station tag (`$(t)12` is `A12`).
    tag_macro: Option<String>,
    /// A macro holding a distance.
    len_macro: Option<String>,
}

/// One `#Units` option (or `.OPTIONS` word), applied to `st` the way the
/// importer applies it. `header` allows the options that would rename the
/// stations already shot (`Case=`, the outer prefix levels, the tag macro);
/// `stack` allows `Save` / `Restore` / `Reset`.
fn option(
    src: &mut ByteSource<'_>,
    f: Faults,
    st: &mut State,
    tag: &str,
    header: bool,
    stack: bool,
) -> String {
    let u = &mut st.u;
    match src.below(22) {
        0 | 1 => {
            let feet = src.bool();
            u.d_feet = feet;
            u.s_feet = feet;
            let spellings: &[&str] = if feet {
                &["Feet", "FEET", "f", "feet"]
            } else {
                &["Meters", "meters", "m", "Meter"]
            };
            src.pick_or(spellings, "Meters").to_string()
        }
        2 => {
            u.d_feet = src.bool();
            u.s_feet = src.bool();
            let d = if u.d_feet { "Feet" } else { "Meters" };
            let s = if u.s_feet { "F" } else { "M" };
            format!("D={d} S={s}")
        }
        3 | 4 => {
            let order = src.pick_or(
                &["DAV", "DVA", "ADV", "AVD", "VDA", "VAD", "DA", "AD"],
                "DAV",
            );
            u.ct_order = order.chars().collect();
            if order.len() == 2 {
                // A dive survey: depth gauges, taped station to station.
                u.tape_ss = true;
                format!("Order={order} Tape=SS")
            } else {
                format!("Order={order}")
            }
        }
        5 => {
            if u.rect && src.bool() {
                u.rect = false;
                "CT".to_string()
            } else {
                u.rect = true;
                let order = src.pick_or(&["ENU", "NEU", "UEN", "EN", "NE"], "ENU");
                u.rect_order = order.chars().collect();
                if src.bool() {
                    format!("RECT Order={order}")
                } else {
                    let angle = val(src, f, -20.0, 20.0);
                    format!("RECT={} Order={order}", num(src, f, angle))
                }
            }
        }
        6 => {
            let (unit, word) = src.pick_or(
                &[
                    ('D', "Degrees"),
                    ('D', "d"),
                    ('G', "Grads"),
                    ('G', "g"),
                    ('M', "Mils"),
                ],
                ('D', "Degrees"),
            );
            match src.below(3) {
                0 => {
                    u.a = unit;
                    format!("A={word}")
                }
                1 => {
                    u.ab = unit;
                    format!("AB={word}")
                }
                _ => {
                    u.a = unit;
                    u.ab = unit;
                    format!("A/AB={word}")
                }
            }
        }
        7 => {
            let (unit, word) = src.pick_or(
                &[
                    ('D', "Degrees"),
                    ('G', "Grads"),
                    ('M', "mils"),
                    ('P', "Percent"),
                    ('P', "p"),
                ],
                ('D', "Degrees"),
            );
            match src.below(3) {
                0 => {
                    u.v = unit;
                    format!("V={word}")
                }
                1 => {
                    u.vb = unit;
                    format!("VB={word}")
                }
                _ => {
                    u.v = unit;
                    u.vb = unit;
                    format!("V/VB={word}")
                }
            }
        }
        8 => match src.below(4) {
            0 => "Decl=".to_string(),
            1 => {
                let g = val(src, f, -3.0, 3.0);
                format!("Grid={}", num(src, f, g))
            }
            _ => {
                let d = val(src, f, -20.0, 20.0);
                format!("Decl={}", num(src, f, d))
            }
        },
        9 => {
            let key = src.pick_or(
                &["IncD", "IncA", "IncAB", "IncV", "IncVB", "IncS", "IncH"],
                "IncD",
            );
            if src.chance(32) {
                format!("{key}=")
            } else {
                let c = val(src, f, -1.0, 1.0);
                format!("{key}={}", num(src, f, c))
            }
        }
        10 => {
            let key = src.pick_or(&["TypeAB", "TypeVB"], "TypeAB");
            let value = src.pick_or(&["C", "N", "C,2", "N,5,X", "C,2,X", "Corrected"], "N");
            format!("{key}={value}")
        }
        11 if header => src
            .pick_or(
                &["Case=Upper", "Case=Lower", "Case=Mixed", "Case=U"],
                "Case=Mixed",
            )
            .to_string(),
        12 => src
            .pick_or(
                &[
                    "LRUD=From",
                    "LRUD=To",
                    "LRUD=F:RLUD",
                    "LRUD=T:UDLR",
                    "LRUD=F:LRUD",
                    "LRUD=TB",
                ],
                "LRUD=From",
            )
            .to_string(),
        13 => {
            let tape = src.pick_or(&["IT", "SS", "IS", "ST", "TT", "it"], "IT");
            u.tape_ss = tape.eq_ignore_ascii_case("ss");
            format!("Tape={tape}")
        }
        14 => {
            let p = src.pick_or(&["CAVE", "SEC", "Q2", "X"], "CAVE");
            if header && src.chance(64) {
                // The outer levels: every name in the file gains them.
                let level = src.pick_or(&["Prefix2", "Prefix3"], "Prefix2");
                format!("{level}={p}")
            } else if src.chance(40) {
                u.prefix1 = None;
                "Prefix=".to_string()
            } else {
                u.prefix1 = Some(p.to_string());
                format!("Prefix={p}")
            }
        }
        15 => src
            .pick_or(
                &["Flag=SPLAY", "Flag=\"loose rock\"", "Flag=", "Flag=SURFACE"],
                "Flag=",
            )
            .to_string(),
        16 => src
            .pick_or(&["UVH=2", "UVV=1.5", "UV=1", "UVH=0"], "UV=1")
            .to_string(),
        17 if header && st.tag_macro.is_none() => {
            st.tag_macro = Some("t".to_string());
            format!("$t={tag}")
        }
        18 if st.len_macro.is_none() => {
            st.len_macro = Some("len".to_string());
            let d = src.f64_in(1.0, 20.0);
            format!("$len={d:.2}")
        }
        19 if stack => {
            if st.saved.len() < 10 {
                st.saved.push(st.u.clone());
            }
            "Save".to_string()
        }
        20 if stack => {
            if let Some(saved) = st.saved.pop() {
                st.u = saved;
            }
            "Restore".to_string()
        }
        21 if stack && src.chance(64) => {
            st.u = Units::default();
            "Reset".to_string()
        }
        _ if f.hit(src, 128) => src
            .pick_or(
                &[
                    "Order=XYZ",
                    "Decl=abc",
                    "LRUD=F:LLLL",
                    "Tape=Q",
                    "Frobnicate=1",
                    "=DAV",
                    "$",
                    "$(x)=1",
                    "IncD=nan",
                    "Prefix=\"unterminated",
                    "A=Q",
                ],
                "Order=XYZ",
            )
            .to_string(),
        _ => {
            u.d_feet = false;
            u.s_feet = false;
            "Meters".to_string()
        }
    }
}

/// A run of options for a `#Units` line or a `.OPTIONS` entry.
fn options(
    src: &mut ByteSource<'_>,
    f: Faults,
    st: &mut State,
    tag: &str,
    header: bool,
    stack: bool,
) -> String {
    let n = 1 + src.below(4);
    let mut parts = Vec::with_capacity(n);
    for _ in 0..n {
        parts.push(option(src, f, st, tag, header, stack));
    }
    let mut out = parts.join(" ");
    if src.chance(12) {
        // Stray spaces around `=`, which the importer glues back on.
        out = out.replace('=', " = ");
    }
    out
}

/// One `.srv` being rendered.
struct Srv<'n> {
    doc: Doc,
    f: Faults,
    st: State,
    net: &'n mut Net,
    /// Latitude / longitude the file's geographic `#Fix`es sit around.
    geo: (f64, f64),
}

impl Srv<'_> {
    /// A station name as typed, sometimes through the tag macro.
    fn typed(&self, src: &mut ByteSource<'_>, name: String) -> String {
        if let Some(m) = &self.st.tag_macro {
            if let Some(rest) = name.strip_prefix(self.net.tag.as_str()) {
                if src.chance(96) {
                    return format!("$({m}){rest}");
                }
            }
        }
        name
    }

    fn distance(&self, src: &mut ByteSource<'_>, m: f64) -> String {
        let f = self.f;
        match src.below(24) {
            20 => format!("{}m", num(src, f, m)),
            21 => format!("{}f", num(src, f, m / FEET_TO_M)),
            22 => {
                let inches = m / FEET_TO_M * 12.0;
                let ft = (inches / 12.0).trunc();
                format!("{ft:.0}i{:.1}", inches - ft * 12.0)
            }
            23 if self.st.len_macro.is_some() => {
                format!("$({})", self.st.len_macro.as_deref().unwrap_or("len"))
            }
            _ => num(src, f, length_in(self.st.u.d_feet, m)),
        }
    }

    /// One angle reading in `unit`, in one of the notations Walls reads.
    fn angle(&self, src: &mut ByteSource<'_>, unit: char, deg: f64, azimuth: bool) -> String {
        let f = self.f;
        match src.below(20) {
            15 if azimuth && unit == 'D' => quadrant(deg),
            // D:M:S is degrees whatever the unit.
            16 => dms(deg),
            17 => format!("{}d", num(src, f, deg)),
            18 if azimuth => format!("{}g", num(src, f, deg / 0.9)),
            19 if !azimuth && deg.abs() < 80.0 => {
                format!("{}p", num(src, f, angle_in('P', deg)))
            }
            _ => num(src, f, angle_in(unit, deg)),
        }
    }

    fn azimuth(&self, src: &mut ByteSource<'_>, deg: f64) -> String {
        let u = &self.st.u;
        if src.chance(8) {
            return "--".to_string();
        }
        let fs = self.angle(src, u.a, deg, true);
        if !src.chance(48) {
            return fs;
        }
        let back = (deg + 180.0 + src.f64_in(-1.5, 1.5)).rem_euclid(360.0);
        let bs = self.angle(src, u.ab, back, true);
        match src.below(4) {
            0 => format!("{fs}/--"),
            1 => format!("/{bs}"),
            2 => format!("--/{bs}"),
            _ => format!("{fs}/{bs}"),
        }
    }

    fn inclination(&self, src: &mut ByteSource<'_>, deg: f64) -> String {
        let u = &self.st.u;
        if src.chance(8) {
            return "--".to_string();
        }
        let fs = self.angle(src, u.v, deg, false);
        if !src.chance(40) {
            return fs;
        }
        let back = -deg + src.f64_in(-1.0, 1.0);
        let bs = self.angle(src, u.vb, back, false);
        match src.below(3) {
            0 => format!("{fs}/--"),
            1 => format!("--/{bs}"),
            _ => format!("{fs}/{bs}"),
        }
    }

    /// A "short" measurement (LRUD, depth): a plausible value in the S unit,
    /// an omitted `--`, or one of the unit-scaled "no value" markers.
    fn short(&self, src: &mut ByteSource<'_>, lo: f64, hi: f64) -> String {
        let f = self.f;
        match src.below(24) {
            20 => "--".to_string(),
            21 | 22 => {
                let s = sentinel(src);
                num(src, f, s)
            }
            23 => {
                let m = src.f64_in(lo, hi);
                format!("{}m", num(src, f, m))
            }
            _ => {
                let m = val(src, f, lo, hi);
                num(src, f, length_in(self.st.u.s_feet, m))
            }
        }
    }

    fn lrud(&self, src: &mut ByteSource<'_>) -> String {
        let mut vals: Vec<String> = (0..4).map(|_| self.short(src, 0.0, 6.0)).collect();
        if src.chance(16) {
            // A trailing facing / C flag the importer skips.
            vals.push(src.pick_or(&["C", "90", "N45E"], "C").to_string());
        }
        let body = vals.join(if src.bool() { "," } else { " " });
        if src.chance(64) {
            format!("*{body}*")
        } else {
            format!("<{body}>")
        }
    }

    fn vector(&mut self, src: &mut ByteSource<'_>) {
        let f = self.f;
        let u = self.st.u.clone();
        let from = match self.net.pick(src, &u) {
            Some(s) => s,
            None => self.net.fresh(&u),
        };
        let splay = !u.rect && src.chance(20);
        let to = if splay {
            src.pick_or(&["-", "--"], "-").to_string()
        } else if self.net.stations.len() > 2 && src.chance(12) {
            let i = src.below(self.net.stations.len());
            self.net.refer(i, &u).unwrap_or_default()
        } else {
            self.net.fresh(&u)
        };
        let (from, to) = if splay && src.chance(48) {
            (to, from)
        } else {
            (from, to)
        };
        let mut toks = vec![self.typed(src, from), self.typed(src, to)];
        if f.hit(src, 16) {
            // More colons than there are prefix levels, or a bad macro.
            let bad = src.pick_or(
                &["A:B:C:D:E:1", "$(nope)7", "$(", "::", "<1", "*", "(oops"],
                "::",
            );
            toks.push(bad.to_string());
        }
        let length = if splay {
            val(src, f, 0.2, 8.0)
        } else if src.chance(4) {
            0.0
        } else {
            val(src, f, 0.3, 40.0)
        };
        let az = val(src, f, 0.0, 360.0);
        let inc = val(src, f, -60.0, 60.0);
        if u.rect {
            let (a, i) = (az.to_radians(), inc.to_radians());
            let e = length * i.cos() * a.sin();
            let n = length * i.cos() * a.cos();
            let up = length * i.sin();
            for c in &u.rect_order {
                let v = match c {
                    'E' => e,
                    'N' => n,
                    _ => up,
                };
                toks.push(num(src, f, length_in(u.d_feet, v)));
            }
        } else {
            for c in &u.ct_order {
                let tok = match c {
                    'D' => self.distance(src, length),
                    'A' => self.azimuth(src, az),
                    _ => self.inclination(src, inc),
                };
                toks.push(tok);
            }
            if u.dive() {
                toks.push(self.short(src, 0.0, 60.0));
                toks.push(self.short(src, 0.0, 60.0));
            }
        }
        if src.chance(120) {
            toks.push(self.lrud(src));
        }
        if src.chance(12) {
            let var = src.pick_or(&["(?)", "(R5,3)", "(0.5)", "(*)", "(*,?)"], "(?)");
            toks.push(var.to_string());
        }
        let sep = match src.below(16) {
            14 => "\t",
            15 => "   ",
            _ => " ",
        };
        let mut line = String::new();
        if src.chance(32) {
            line.push_str("  ");
        }
        line.push_str(&toks.join(sep));
        if src.chance(40) {
            let _ = write!(line, " ;{}", words(src, f, 24));
        } else if src.chance(8) {
            line.push_str(" #s sub");
        }
        self.doc.line(&line);
    }

    fn date(&mut self, src: &mut ByteSource<'_>) {
        let (y, m, d) = (1960 + src.below(67), 1 + src.below(12), 1 + src.below(28));
        let text = if self.f.hit(src, 64) {
            src.pick_or(
                &[
                    "2001-02-30",
                    "13/45/99",
                    "02-29-00",
                    "yesterday",
                    "99999-1-1",
                    "",
                    "2001-1",
                ],
                "",
            )
            .to_string()
        } else {
            match src.below(5) {
                1 => format!("{y}/{m:02}/{d:02}"),
                2 => format!("{m:02}-{d:02}-{y}"),
                3 => format!("{m}/{d}/{:02}", y % 100),
                4 => format!("{y}-{m}-{d}"),
                _ => format!("{y}-{m:02}-{d:02}"),
            }
        };
        let mut line = format!("#Date {text}");
        if src.chance(24) {
            line.push_str(" ;TODO");
        }
        self.doc.line(&line);
    }

    fn fix(&mut self, src: &mut ByteSource<'_>) {
        let f = self.f;
        let u = self.st.u.clone();
        let name = if self.net.stations.is_empty() {
            self.net.fresh(&u)
        } else {
            let i = src.below(self.net.stations.len());
            self.net.refer(i, &u).unwrap_or_default()
        };
        let up = length_in(u.d_feet, val(src, f, -50.0, 3000.0));
        let up = num(src, f, up);
        let coords = if src.bool() {
            let e = length_in(u.d_feet, val(src, f, 200_000.0, 800_000.0));
            let n = length_in(u.d_feet, val(src, f, 1_000_000.0, 9_000_000.0));
            format!("{} {} {up}", num(src, f, e), num(src, f, n))
        } else {
            let lat = self.geo.0 + val(src, f, -0.01, 0.01);
            let lon = self.geo.1 + val(src, f, -0.01, 0.01);
            let lat = format!("{}{}", if lat < 0.0 { 'S' } else { 'N' }, dms(lat.abs()));
            let lon = format!("{}{}", if lon < 0.0 { 'W' } else { 'E' }, dms(lon.abs()));
            if src.bool() {
                format!("{lon} {lat} {up}")
            } else {
                format!("{lat} {lon} {up}")
            }
        };
        let mut line = format!("#Fix {} {coords}", self.typed(src, name));
        if src.chance(32) {
            line.push_str(" (0.5)");
        }
        if src.chance(32) {
            let _ = write!(line, " /{}", words(src, f, 12));
        }
        if src.chance(32) {
            line.push_str(";datum");
        }
        self.doc.line(&line);
    }

    fn block_comment(&mut self, src: &mut ByteSource<'_>, depth: usize) {
        let f = self.f;
        self.doc
            .line(src.pick_or(&["#[", "#[ old data", "  #["], "#["));
        for _ in 0..=src.below(4) {
            if depth < 3 && src.chance(32) {
                self.block_comment(src, depth + 1);
            } else if src.bool() {
                // A vector the importer must not read.
                self.doc.line("Z1 Z2 1e9 0 0 <1,2,3,4>");
            } else {
                let w = words(src, f, 30);
                self.doc.line(&w);
            }
        }
        if !f.hit(src, 32) {
            self.doc.line("#]");
        }
    }

    fn directive(&mut self, src: &mut ByteSource<'_>) {
        let f = self.f;
        match src.below(16) {
            0..=2 => {
                let tag = self.net.tag.clone();
                let opts = options(src, f, &mut self.st, &tag, false, true);
                let kw = src.pick_or(&["#Units", "#units", "#UNITS", "#u", "#U"], "#Units");
                let mut line = format!("{kw} {opts}");
                if src.chance(24) {
                    let _ = write!(line, " ;{}", words(src, f, 16));
                }
                self.doc.line(&line);
            }
            3 => self.date(src),
            4 => {
                let seg = src.pick_or(
                    &[
                        "/CAVE/SEC",
                        "..",
                        "./sub",
                        "\\a\\b",
                        "sub",
                        ".",
                        "/",
                        "../../..",
                    ],
                    "sub",
                );
                let kw = src.pick_or(&["#Segment", "#Seg", "#s"], "#Segment");
                self.doc.line(&format!("{kw} {seg}"));
            }
            5 => {
                let p = src.pick_or(&["CAVE", "SEC", "Q2", "B"], "SEC");
                if src.chance(48) {
                    self.st.u.prefix1 = None;
                    self.doc.line("#Prefix");
                } else {
                    self.st.u.prefix1 = Some(p.to_string());
                    self.doc.line(&format!("#Prefix {p}"));
                }
            }
            6 => {
                let line = match src.below(4) {
                    0 => "#Flag SPLAY".to_string(),
                    1 => match self.net.stations.last() {
                        Some(s) => format!("#Flag {} /SURFACE", s.name),
                        None => "#Flag".to_string(),
                    },
                    2 => format!("#Note {}", words(src, f, 24)),
                    _ => "#Sym A1 X".to_string(),
                };
                self.doc.line(&line);
            }
            7 => self.fix(src),
            8 => self.block_comment(src, 0),
            9 => self.doc.line(""),
            10 => {
                // A lone-station LRUD line (passage dimensions only).
                if let Some(s) = self.net.stations.last() {
                    let line = format!("{} <1,2,3,4>", s.name);
                    self.doc.line(&line);
                }
            }
            11 if f.on() => {
                let junk = src.pick_or(
                    &[
                        "#]",
                        "#Prefix A:B:C:D",
                        "#Units $(undefined)",
                        "#Fix",
                        "#Segment",
                        "#",
                        "$(t)",
                    ],
                    "#",
                );
                self.doc.line(junk);
            }
            _ => {
                let line = format!(";{}", words(src, f, 40));
                self.doc.line(&line);
            }
        }
    }

    fn header(&mut self, src: &mut ByteSource<'_>) {
        let f = self.f;
        let title = format!("; {}", words(src, f, 30));
        self.doc.line(&title);
        if src.chance(96) {
            let team = format!(";team: {}", words(src, f, 20));
            self.doc.line(&team);
        }
        if src.chance(192) {
            let tag = self.net.tag.clone();
            let opts = options(src, f, &mut self.st, &tag, true, true);
            self.doc.line(&format!("#Units {opts}"));
        }
        if src.chance(160) {
            self.date(src);
        }
        if src.chance(64) {
            self.doc.line("#Segment /CAVE");
        }
        if src.chance(16) {
            // A long run of prefix changes, the last one winning.
            for i in 0..src.below(200) {
                self.doc.line(&format!("#Prefix P{i}"));
            }
            self.st.u.prefix1 = None;
            self.doc.line("#Prefix");
        }
    }

    fn body(&mut self, src: &mut ByteSource<'_>, vectors: usize) {
        for _ in 0..vectors {
            if self.doc.full(16) {
                break;
            }
            if src.chance(30) {
                self.directive(src);
            }
            self.vector(src);
        }
    }
}

/// Render one `.srv`, starting from `st` (what a `.wpj` hands down) and
/// drawing station names from `net`.
fn srv_text(
    src: &mut ByteSource<'_>,
    f: Faults,
    st: State,
    net: &mut Net,
    vectors: usize,
) -> String {
    let geo = (src.f64_in(-60.0, 70.0), src.f64_in(-170.0, 170.0));
    let mut srv = Srv {
        doc: Doc::default(),
        f,
        st,
        net,
        geo,
    };
    srv.header(src);
    srv.body(src, vectors);
    srv.doc.text
}

/// Line endings, encoding and damage for one rendered file. Walls files are
/// Windows files: CRLF and Windows-1252 mostly.
fn finish(src: &mut ByteSource<'_>, f: Faults, text: &str) -> Vec<u8> {
    let text = if f.on() {
        line_endings(src, text)
    } else if src.chance(64) {
        text.to_string()
    } else {
        text.replace('\n', "\r\n")
    };
    let encoding = if f.on() {
        Encoding::pick(src)
    } else {
        match src.below(8) {
            0 => Encoding::Utf8,
            1 => Encoding::Utf8Bom,
            _ => Encoding::Cp1252,
        }
    };
    damage(src, encoding.encode(&text))
}

/// Render one Walls survey opened on its own.
pub fn generate_srv(src: &mut ByteSource<'_>) -> FileSet {
    let f = Faults::pick(src);
    let mut net = Net {
        tag: "A".to_string(),
        ..Net::default()
    };
    let vectors = count(src, 60, 1300);
    let text = srv_text(src, f, State::default(), &mut net, vectors);
    FileSet::single("survey.srv", finish(src, f, &text))
}

/// Where a `.wpj` entry's files resolve, inside the project folder
/// (`Some(folders)`), or `None` when its `.PATH` leads outside it or was
/// refused; plus the units its `.OPTIONS` chain sets up.
#[derive(Debug, Clone, Default)]
struct Ctx {
    dir: Option<Vec<String>>,
    st: State,
}

/// A `.PATH` for an entry whose parent resolves in `dir`, and where the
/// entry resolves with it: the importer's rules (backslashes separate, `..`
/// stops at the project folder, an absolute path replaces the folder, drive
/// letters, UNC paths and DOS devices are refused).
fn path_line(src: &mut ByteSource<'_>, dir: Option<&[String]>) -> (String, Option<Vec<String>>) {
    let mut d = dir.map(<[String]>::to_vec);
    let room = d.as_ref().map_or(0, |v| 2usize.saturating_sub(v.len()));
    let folder = src.pick_or(&["data", "Data", "srv", "cave 2", "x.y"], "data");
    let text = match src.below(16) {
        0..=2 if room >= 1 => {
            if let Some(v) = d.as_mut() {
                v.push(folder.to_string());
            }
            folder.to_string()
        }
        3 if room >= 1 => {
            if let Some(v) = d.as_mut() {
                v.push(folder.to_string());
            }
            format!(".\\{folder}\\")
        }
        4 if room >= 2 => {
            if let Some(v) = d.as_mut() {
                v.push(folder.to_string());
                v.push("deep".to_string());
            }
            format!("{folder}/deep")
        }
        5 | 6 => {
            if let Some(v) = d.as_mut() {
                v.pop();
            }
            "..".to_string()
        }
        7 => {
            if let Some(v) = d.as_mut() {
                v.pop();
                v.pop();
                v.push(folder.to_string());
            }
            format!("..\\..\\..\\{folder}")
        }
        8 => {
            d = None;
            "/stalfuzz-nowhere/caves".to_string()
        }
        9 => {
            d = None;
            "C:\\Caves\\Data".to_string()
        }
        10 => {
            d = None;
            "\\\\server\\share\\caves".to_string()
        }
        11 => {
            d = None;
            src.pick_or(&["CON", "data\\NUL", "COM1.srv"], "CON")
                .to_string()
        }
        12 => String::new(),
        _ => ".".to_string(),
    };
    (text, d)
}

/// A `.REF` line: northing, easting, zone, convergence, elevation, flags,
/// latitude and longitude D M S, datum index and name.
fn ref_line(src: &mut ByteSource<'_>, f: Faults) -> String {
    if f.hit(src, 64) {
        return src
            .pick_or(
                &[
                    ".REF 1 2 3",
                    ".REF 2258942 467427 0 0 300 0 20 25 30 97 18 45 27 \"WGS 1984\"",
                    ".REF 2258942 467427 61 0 300 0 20 25 30 97 18 45 27 \"WGS 1984\"",
                    ".REF 2258942 467427 14 0 300 -1 20 25 30 97 18 45 27 \"WGS 1984\"",
                    ".REF 2258942 467427 14 nan 300 0 20 25 30 97 18 45 27 \"WGS 1984",
                    ".REF 2258942 467427 14.5 0 300 70000 20 25 30 97 18 45 27 \"",
                ],
                ".REF",
            )
            .to_string();
    }
    let zone = 1 + src.below(60);
    let north = src.f64_in(1_000_000.0, 9_000_000.0);
    let east = src.f64_in(200_000.0, 800_000.0);
    let conv = src.f64_in(-3.0, 3.0);
    let elev = src.below(3000);
    let flags = src.pick_or(&[0u32, 4, 8, 12, 1, 2], 0);
    let (lat_d, lat_m, lat_s) = (src.below(80), src.below(60), src.f64_in(0.0, 59.9));
    let (lon_d, lon_m, lon_s) = (src.below(180), src.below(60), src.f64_in(0.0, 59.9));
    let (idx, datum) = src.pick_or(
        &[
            (27, "WGS 1984"),
            (19, "North American 1983"),
            (18, "North American 1927"),
            (9, "European 1950"),
            (3, "Fictional Datum"),
        ],
        (27, "WGS 1984"),
    );
    format!(
        ".REF {north:.3} {east:.3} {zone} {conv:.3} {elev} {flags} {lat_d} {lat_m} {lat_s:.3} \
         {lon_d} {lon_m} {lon_s:.3} {idx} \"{datum}\""
    )
}

/// A `.wpj` being rendered, with the survey files its links resolve to.
struct Wpj {
    doc: Doc,
    f: Faults,
    net: Net,
    /// Linked files written so far, by their path in the file set.
    files: Vec<(String, Vec<u8>)>,
    surveys: usize,
}

impl Wpj {
    /// Write a `.srv` at `path`, starting from `st`, unless one is there.
    fn add_srv(&mut self, src: &mut ByteSource<'_>, path: String, st: State) {
        if self.files.iter().any(|(p, _)| *p == path) {
            return;
        }
        let k = self.surveys;
        self.net.tag = tag_for(k);
        let st = State {
            tag_macro: None,
            saved: Vec::new(),
            ..st
        };
        let vectors = count(src, 30, 400);
        let text = srv_text(src, self.f, st, &mut self.net, vectors);
        let bytes = finish(src, self.f, &text);
        self.files.push((path, bytes));
    }

    fn survey(&mut self, src: &mut ByteSource<'_>, ctx: &Ctx) {
        let f = self.f;
        self.surveys += 1;
        let k = self.surveys;
        let title = words(src, f, 20);
        self.doc.line(&format!(".SURVEY {title}"));
        let mut dir = ctx.dir.clone();
        let mut st = ctx.st.clone();
        let mut status = src.pick_or(&[0u32, 0, 8, 8, 8 | 512, 8 | 2048, 64, 256], 0);
        // (.NAME, file written for it); `None` writes no file.
        let (name, file) = match src.below(32) {
            20 | 21 => (format!("S{k}"), None),
            22 => (format!("s{k}"), Some(format!("S{k}.srv"))),
            23 => (
                src.pick_or(&["project", "project.wpj", "PROJECT"], "project")
                    .to_string(),
                None,
            ),
            24 => {
                status |= 2;
                (format!("S{k}"), Some(format!("S{k}.srv")))
            }
            25 => {
                status = 1 << 16;
                (
                    src.pick_or(&["notes.txt", "map.svg"], "map.svg")
                        .to_string(),
                    None,
                )
            }
            26 => (format!("S{k}.srv"), Some(format!("S{k}.srv"))),
            27 => (format!("S{k}.SRV"), Some(format!("S{k}.SRV"))),
            28 => (format!("S{k}.old"), Some(format!("S{k}.old.srv"))),
            29 => (format!("S{k}"), Some(format!("S{k}.SRV"))),
            30 => (format!("S {k}"), Some(format!("S {k}.srv"))),
            31 if f.on() => (
                src.pick_or(&["../S1", "a/b", "C:x", "NUL", "", "con.srv", "S1\\S2"], "")
                    .to_string(),
                None,
            ),
            _ => (format!("S{k}"), Some(format!("S{k}.srv"))),
        };
        let mut lines = vec![format!(".NAME {name}"), format!(".STATUS {status}")];
        if f.hit(src, 16) {
            lines.push(
                src.pick_or(&[".STATUS abc", ".STATUS -1", ".STATUS 99999999999"], "")
                    .to_string(),
            );
        }
        if src.chance(40) {
            let opts = options(src, f, &mut st, "", false, false);
            lines.push(format!(".OPTIONS {opts}"));
        }
        if src.chance(48) {
            let (text, d) = path_line(src, dir.as_deref());
            dir = d;
            lines.push(format!(".PATH {text}"));
        }
        if src.chance(64) {
            lines.reverse();
        }
        for l in &lines {
            self.doc.line(l);
        }
        if let (Some(file), Some(dir)) = (file, &dir) {
            let mut path = dir.clone();
            path.push(file);
            self.add_srv(src, path.join("/"), st);
        }
        if src.chance(12) {
            // The same file listed again.
            self.doc.line(".SURVEY again");
            for l in &lines {
                self.doc.line(l);
            }
        }
    }

    fn book(&mut self, src: &mut ByteSource<'_>, ctx: &Ctx, depth: usize) {
        let f = self.f;
        let title = words(src, f, 20);
        self.doc.line(&format!(".BOOK {title}"));
        self.doc.line(&format!(".NAME B{depth}x{}", self.surveys));
        let status = src.pick_or(&[0u32, 8, 8 | 512, 8 | 2048, 1024, 64, 8 | 256], 0);
        self.doc.line(&format!(".STATUS {status}"));
        let mut child = ctx.clone();
        if src.chance(64) {
            let opts = options(src, f, &mut child.st, "", false, false);
            self.doc.line(&format!(".OPTIONS {opts}"));
        }
        if src.chance(48) {
            let r = ref_line(src, f);
            self.doc.line(&r);
        }
        if src.chance(48) {
            let (text, d) = path_line(src, child.dir.as_deref());
            child.dir = d;
            self.doc.line(&format!(".PATH {text}"));
        }
        let n = if depth == 0 { 2 } else { 1 } + count(src, 4, 8);
        for _ in 0..n {
            if self.doc.full(60) || self.surveys >= MAX_SURVEYS {
                break;
            }
            if depth < 3 && src.chance(56) {
                self.book(src, &child, depth + 1);
            } else {
                self.survey(src, &child);
            }
        }
        if !f.hit(src, 24) {
            self.doc.line(".ENDBOOK");
        }
        if f.hit(src, 8) {
            self.doc.line(".ENDBOOK");
        }
    }
}

/// The station-name tag of a project's `k`th file: `A`..`Z`, then `AA`...
fn tag_for(k: usize) -> String {
    let letter = |i: usize| char::from(b'A' + (i % 26) as u8);
    if k < 26 {
        letter(k).to_string()
    } else {
        format!("{}{}", letter(k / 26 - 1), letter(k))
    }
}

/// Render one Walls project with its linked surveys.
pub fn generate_wpj(src: &mut ByteSource<'_>) -> FileSet {
    let f = Faults::pick(src);
    let mut w = Wpj {
        doc: Doc::default(),
        f,
        net: Net::default(),
        files: Vec::new(),
        surveys: 0,
    };
    w.doc.line(";WALLS Project file");
    // Books wrapped around the tree: a hundred levels at most normally; past
    // the importer's limit (256) only in a hostile file.
    let deep = if f.hit(src, 32) {
        200 + src.below(120)
    } else if src.chance(6) {
        8 + src.below(100)
    } else {
        0
    };
    for i in 0..deep {
        w.doc.line(&format!(".BOOK L{i}"));
        w.doc.line(&format!(".NAME L{i}"));
    }
    let root = Ctx {
        dir: Some(Vec::new()),
        st: State::default(),
    };
    w.book(src, &root, 0);
    for _ in 0..deep {
        if !f.hit(src, 16) {
            w.doc.line(".ENDBOOK");
        }
    }
    let text = w.doc.text.clone();
    let mut set = FileSet::single("project.wpj", finish(src, f, &text));
    for (path, bytes) in w.files {
        set.push(&path, bytes);
    }
    set
}
