//! Compass `.dat` / `.mak` generator.
//!
//! A `.dat` holds survey records, each closed by a form feed: a cave-name
//! line, `SURVEY NAME:`, `SURVEY DATE:` (with an optional `COMMENT:`),
//! `SURVEY TEAM:` and the team on the line after it, one parameter line
//! (`DECLINATION:`, `FORMAT:`, `CORRECTIONS:`, `CORRECTIONS2:`), a column
//! header, and the shots. The numbers are stored in decimal feet and degrees
//! whatever the FORMAT says the notebook used. A `.mak` is a list of
//! `;`-terminated records: the base location (`@`), datum (`&`), UTM zone
//! (`$`), file convergence (`%` / `*`), project flags (`!`), folders (`[` /
//! `]`) and the `#` links to `.dat` files, each with optional fixed stations.
//!
//! Semantic knobs (`.dat`): several surveys per file tied together by shared
//! station names, FORMAT strings of every length the reader takes (11, 12,
//! 13, 15) with every unit, order, backsight and LRUD-association code, no
//! FORMAT at all, corrections, backsights (including the -999 "not taken"
//! marker), LRUDs with the -9.90 / -999 / 999 "not measured" markers and
//! their metre-scaled forms, zero-length shots, flags (`#|...#`) and
//! comments, dates with leading zeros, two-digit years and odd spacing, team
//! lines with commas and non-ASCII text, a missing final form feed, a DOS EOF
//! byte, odd but readable number spellings, huge shot counts, Windows-1252
//! text and CRLF line endings mostly. Hostile files add impossible dates,
//! broken FORMAT strings, empty names, lowercase column headers, edge-value
//! and unreadable numbers, unterminated flags and other line endings.
//!
//! Semantic knobs (`.mak`): base location, datum (every name the reader
//! knows), zone, convergence and flag records in varying order and
//! repetition, `/` comments and project ids, folders, and links that
//! resolve, are missing, climb with `..`, are absolute or refused, differ in
//! case, are listed twice, point at the project itself, or carry fixed
//! stations in feet or metres. Hostile files add `;` comment lines, unknown
//! datums and flags, repeated flags, missing state, missing semicolons, bad
//! numbers and malformed fixes.

use std::fmt::Write as _;

use crate::fileset::FileSet;
use crate::gen::{count, damage, line_endings, sentinel, spell, Encoding, Faults, FEET_TO_M};
use crate::source::ByteSource;

/// Most lines one rendered file may have.
const MAX_LINES: usize = 1450;

/// Most `.dat` files one `.mak` links.
const MAX_LINKS: usize = 8;

/// Every datum name the `.mak` reader knows.
const DATUMS: &[&str] = &[
    "Adindan",
    "Arc 1950",
    "Arc 1960",
    "Australian 1966",
    "Australian 1984",
    "Camp Area Astro",
    "Cape",
    "European 1950",
    "European 1979",
    "Geodetic 1949",
    "HongKong 1963",
    "HuTzuShan",
    "Indian",
    "North American 1927",
    "North American 1983",
    "Oman",
    "Ordinance Survey 1936",
    "Pulkovo 1942",
    "South American 1956",
    "South American 1969",
    "Tokyo",
    "WGS 1972",
    "WGS 1984",
];

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

/// Free text: one line, unless the file is hostile.
fn words(src: &mut ByteSource<'_>, f: Faults, max: usize) -> String {
    let t = src.text(max);
    if f.on() {
        t
    } else {
        t.replace(['\n', '\r', '\u{c}'], " ")
    }
}

/// `v` as a `.dat` number: two decimals mostly, now and then another
/// spelling the reader still takes, and any spelling at all in a hostile
/// file.
fn num(src: &mut ByteSource<'_>, f: Faults, v: f64) -> String {
    if f.on() && src.chance(48) {
        return spell(src, v);
    }
    if !src.chance(24) {
        return format!("{v:.2}");
    }
    match src.below(4) {
        0 => format!("{v}"),
        1 => format!("{v:e}"),
        2 => format!("{v:08.3}"),
        _ => format!("{v:.3}"),
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

/// Every station shot so far, across the files of a project, so later
/// surveys (and later files) tie into the earlier ones.
#[derive(Debug, Default)]
struct Net {
    names: Vec<String>,
    next: u32,
}

impl Net {
    fn fresh(&mut self, src: &mut ByteSource<'_>, tag: &str) -> String {
        self.next += 1;
        let name = match src.below(32) {
            28 => format!("{tag}'{}", self.next),
            29 => format!("{tag}-{}", self.next),
            30 => format!("{tag}_{}", self.next),
            31 => format!("*{tag}{}", self.next),
            _ => format!("{tag}{}", self.next),
        };
        self.names.push(name.clone());
        name
    }

    fn any(&self, src: &mut ByteSource<'_>) -> Option<String> {
        src.pick(&self.names).cloned()
    }
}

/// A FORMAT string, and whether the shot lines carry backsight columns.
struct Format {
    text: Option<String>,
    backsights: bool,
}

fn format_code(src: &mut ByteSource<'_>, f: Faults) -> Format {
    if src.chance(40) {
        return Format {
            text: None,
            backsights: false,
        };
    }
    let bearing = src.pick_or(&['D', 'Q', 'R'], 'D');
    let length = src.pick_or(&['D', 'I', 'M'], 'D');
    let passage = src.pick_or(&['D', 'I', 'M'], 'D');
    let incl = src.pick_or(&['D', 'G', 'M', 'R', 'W'], 'D');
    let lrud = src.pick_or(&["LRUD", "LUDR", "UDLR", "RLUD", "UDRL", "DULR"], "LUDR");
    let order3 = src.pick_or(&["LAD", "LDA", "ADL", "ALD", "DAL", "DLA"], "LAD");
    let order5 = src.pick_or(&["LADad", "LAaDd", "ADLad", "LADda", "adLAD"], "LADad");
    let back = if src.chance(120) { 'B' } else { 'N' };
    let assoc = if src.bool() { 'F' } else { 'T' };
    let len = src.pick_or(&[11usize, 12, 12, 13, 13, 15], 13);
    let mut s = format!("{bearing}{length}{passage}{incl}{lrud}");
    match len {
        11 => s.push_str(order3),
        12 => {
            s.push_str(order3);
            s.push(back);
        }
        13 => {
            s.push_str(order3);
            s.push(back);
            s.push(assoc);
        }
        _ => {
            s.push_str(order5);
            s.push(back);
            s.push(assoc);
        }
    }
    let backsights = len >= 12 && back == 'B';
    if f.hit(src, 24) {
        // A length the reader has no layout for, or a code it does not know.
        match src.below(4) {
            0 => s.push('Z'),
            1 => s.truncate(9),
            2 => s.replace_range(0..1, "X"),
            _ => s = s.to_lowercase(),
        }
    }
    Format {
        text: Some(s),
        backsights,
    }
}

/// A Compass LRUD: feet mostly, the "not measured" markers (and their
/// metre-scaled forms) sometimes.
fn lrud(src: &mut ByteSource<'_>, f: Faults) -> f64 {
    match src.below(24) {
        17 | 18 => -9.9,
        19 => -9.9 * FEET_TO_M,
        20 => -999.0,
        21 => 999.0,
        22 => sentinel(src),
        23 => 0.0,
        _ => val(src, f, 0.0, 25.0),
    }
}

/// An angle reading, or the -999 "not taken" marker now and then.
fn angle(src: &mut ByteSource<'_>, f: Faults, v: f64, missing: u8) -> f64 {
    if src.chance(missing) {
        src.pick_or(&[-999.0, -999.0 * FEET_TO_M, -9999.0], -999.0)
    } else if f.on() && src.chance(16) {
        src.number(-720.0, 720.0)
    } else {
        v
    }
}

/// One survey record of a `.dat`.
fn survey(src: &mut ByteSource<'_>, f: Faults, net: &mut Net, doc: &mut Doc, tag: &str) {
    let fmt = format_code(src, f);
    let cave = if src.chance(64) {
        words(src, f, 24)
    } else {
        src.pick_or(&["SYNTHETIC CAVE", "Cueva Ficticia", "Caf\u{e9} Cave"], "X")
            .to_string()
    };
    doc.line(&cave);
    let name = if f.hit(src, 8) {
        src.pick_or(&["", " ", "#", "A B"], "").to_string()
    } else {
        format!("{tag}{}", src.below(1000))
    };
    doc.line(&format!("SURVEY NAME: {name}"));
    doc.line(&date_line(src, f));
    doc.line("SURVEY TEAM:");
    let team = if f.hit(src, 16) {
        String::new()
    } else {
        match src.below(4) {
            0 => "Ann Smith, Bob Jones".to_string(),
            1 => words(src, f, 30).trim().to_string(),
            _ => "J. Doe, M. M\u{fc}ller".to_string(),
        }
    };
    let team = if team.is_empty() && !f.on() {
        "Nobody".to_string()
    } else {
        team
    };
    doc.line(&team);
    let decl = val(src, f, -20.0, 20.0);
    let mut params = format!("DECLINATION: {}", num(src, f, decl));
    if let Some(code) = &fmt.text {
        let _ = write!(params, "  FORMAT: {code}");
    }
    if src.chance(80) {
        let (a, i, l) = (
            val(src, f, -2.0, 2.0),
            val(src, f, -2.0, 2.0),
            val(src, f, -0.5, 0.5),
        );
        let (a, i, l) = (num(src, f, a), num(src, f, i), num(src, f, l));
        let _ = write!(params, "  CORRECTIONS: {a} {i} {l}");
    }
    if src.chance(48) {
        let (a, i) = (val(src, f, -2.0, 2.0), val(src, f, -2.0, 2.0));
        let (a, i) = (num(src, f, a), num(src, f, i));
        let _ = write!(params, "  CORRECTIONS2: {a} {i}");
    }
    doc.line(&params);
    doc.line("");
    let mut header =
        "        FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT"
            .to_string();
    if fmt.backsights {
        header.push_str("     AZM2     INC2");
    }
    header.push_str("   FLAGS  COMMENTS");
    if f.hit(src, 8) {
        header = header.to_lowercase();
    }
    doc.line(&header);
    doc.line("");
    let shots = count(src, 30, 600);
    let mut last: Option<String> = net.any(src);
    for _ in 0..shots {
        if doc.full(4) {
            break;
        }
        let from = match &last {
            Some(s) if !src.chance(32) => s.clone(),
            _ => match net.any(src) {
                Some(s) => s,
                None => net.fresh(src, tag),
            },
        };
        // A shot to an existing station closes a loop; it does not move the
        // traverse on.
        let to = if net.names.len() > 2 && src.chance(10) {
            net.any(src).unwrap_or_default()
        } else {
            let to = net.fresh(src, tag);
            last = Some(to.clone());
            to
        };
        // Zero-length shots and the lengths Compass reads as "missing".
        let length = match src.below(64) {
            62 => 0.0,
            63 => src.pick_or(&[999.0, 990.0, -1.0, 1e-6], 999.0),
            _ => val(src, f, 1.0, 100.0),
        };
        let az = val(src, f, 0.0, 359.99);
        let inc = val(src, f, -60.0, 60.0);
        let (fore_az, fore_inc) = (angle(src, f, az, 6), angle(src, f, inc, 6));
        let mut line = format!(
            "{from:>12} {to:>12} {} {} {}",
            num(src, f, length),
            num(src, f, fore_az),
            num(src, f, fore_inc),
        );
        for _ in 0..4 {
            let v = lrud(src, f);
            let _ = write!(line, " {}", num(src, f, v));
        }
        if fmt.backsights {
            // A backsight reads the reverse bearing and the negated
            // inclination, give or take the instrument.
            let back_az = (az + 180.0 + src.f64_in(-1.0, 1.0)).rem_euclid(360.0);
            let back_inc = -inc + src.f64_in(-1.0, 1.0);
            let back_az = angle(src, f, back_az, 24);
            let back_inc = angle(src, f, back_inc, 24);
            let _ = write!(line, " {} {}", num(src, f, back_az), num(src, f, back_inc));
        }
        if src.chance(40) {
            let flags = src.pick_or(&["L", "P", "X", "C", "LP", "PX", "LPXC", "S", ""], "L");
            if f.hit(src, 32) {
                let _ = write!(line, " #|{flags}");
            } else {
                let _ = write!(line, " #|{flags}#");
            }
        }
        if src.chance(48) {
            let _ = write!(line, " {}", words(src, f, 30));
        }
        doc.line(&line);
    }
}

/// A `SURVEY DATE:` line in one of the forms written in the wild.
fn date_line(src: &mut ByteSource<'_>, f: Faults) -> String {
    let (m, d, y) = (1 + src.below(12), 1 + src.below(28), 1960 + src.below(67));
    let date = if f.hit(src, 24) {
        src.pick_or(
            &[
                "2 30 1990",
                "13 1 2000",
                "0 0 0",
                "99999999999 1 1",
                "1 1 70000",
                "a b c",
                "",
                "7 10",
            ],
            "",
        )
        .to_string()
    } else {
        match src.below(8) {
            5 => format!("{m:02} {d:02} {y}"),
            6 => format!("{m} {d} {:02}", y % 100),
            7 => format!("{m}  {d}\t{y}"),
            _ => format!("{m} {d} {y}"),
        }
    };
    let mut line = format!("SURVEY DATE: {date}");
    if src.chance(64) {
        let _ = write!(line, "  COMMENT:{}", words(src, f, 30));
    }
    line
}

/// Render the surveys of one `.dat` (station names from `net`, new ones
/// starting with `tag`).
fn dat_text(
    src: &mut ByteSource<'_>,
    f: Faults,
    net: &mut Net,
    tag: &str,
    surveys: usize,
) -> String {
    let mut doc = Doc::default();
    for i in 0..surveys {
        if doc.full(20) {
            break;
        }
        survey(src, f, net, &mut doc, tag);
        // Compass closes every survey with a form feed; the last one is
        // sometimes missing, or followed by a DOS end-of-file byte.
        let last = i + 1 == surveys;
        if !(last && src.chance(32)) {
            doc.line("\u{c}");
        }
        if last && src.chance(24) {
            doc.text.push('\u{1a}');
        }
    }
    doc.text
}

/// Line endings, encoding and (when `may_damage`) damage for one rendered
/// file. Compass files are Windows files: CRLF and Windows-1252 mostly.
fn finish(src: &mut ByteSource<'_>, f: Faults, text: &str, may_damage: bool) -> Vec<u8> {
    let text = if f.on() && src.chance(96) {
        line_endings(src, text)
    } else if src.chance(48) {
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
    let bytes = encoding.encode(&text);
    if may_damage {
        damage(src, bytes)
    } else {
        bytes
    }
}

/// Render one Compass survey opened on its own.
pub fn generate_dat(src: &mut ByteSource<'_>) -> FileSet {
    let f = Faults::pick(src);
    let mut net = Net::default();
    let surveys = 1 + count(src, 4, 30);
    let text = dat_text(src, f, &mut net, "A", surveys);
    FileSet::single("survey.dat", finish(src, f, &text, true))
}

/// The `@` base location: easting, northing, elevation, zone, convergence.
fn base_location(src: &mut ByteSource<'_>, f: Faults) -> String {
    if f.hit(src, 48) {
        return src
            .pick_or(
                &[
                    "@500000.0,4000000.0,100.0;",
                    "@1e5,2,3,4,5;",
                    "@500000.0,4000000.0,100.0,300,0.0;",
                    "@500000.0,4000000.0,100.0,16,0.0",
                    "@,,,,;",
                    "@-.5,1,1,1,1;",
                ],
                "@;",
            )
            .to_string();
    }
    let east = src.f64_in(200_000.0, 800_000.0);
    let north = src.f64_in(0.0, 9_000_000.0);
    let elev = src.f64_in(-100.0, 3000.0);
    let zone = 1 + src.below(60);
    let conv = src.f64_in(-3.0, 3.0);
    format!("@{east:.3},{north:.3},{elev:.3},{zone},{conv:.3};")
}

/// A `!` record: one flag of each group, in any order, either case.
fn flags(src: &mut ByteSource<'_>, f: Faults) -> String {
    const GROUPS: &[[char; 2]] = &[
        ['G', 'g'],
        ['V', 'v'],
        ['O', 'o'],
        ['T', 't'],
        ['S', 's'],
        ['X', 'x'],
        ['P', 'p'],
        ['L', 'l'],
        ['C', 'c'],
    ];
    let mut out = String::new();
    for g in GROUPS {
        if src.chance(200) {
            out.push(g[usize::from(src.bool())]);
        }
    }
    if src.chance(200) {
        out.push(src.pick_or(&['I', 'E', 'A'], 'E'));
    }
    // Shuffle by rotating: the reader takes the flags in any order.
    let n = out.chars().count();
    if n > 1 {
        let k = src.below(n);
        let chars: Vec<char> = out.chars().collect();
        out = chars[k..].iter().chain(&chars[..k]).collect();
    }
    if f.hit(src, 96) {
        out.push(src.pick_or(&['Z', 'G', 'A', '1', ' '], 'Z'));
    }
    format!("!{out};")
}

/// A `,NAME[unit,e,n,elev]` list of fixed stations for a link.
fn fixes(src: &mut ByteSource<'_>, f: Faults, net: &Net, base: (f64, f64)) -> String {
    let mut out = String::new();
    for _ in 0..=src.below(3) {
        let Some(name) = src.pick(&net.names).cloned() else {
            break;
        };
        if !src.chance(200) {
            let _ = write!(out, ",{name}");
            continue;
        }
        let feet = src.bool();
        let unit = if feet { 'f' } else { 'm' };
        let scale = if feet { 1.0 / FEET_TO_M } else { 1.0 };
        let (e, n, z) = (
            (base.0 + src.f64_in(-500.0, 500.0)) * scale,
            (base.1 + src.f64_in(-500.0, 500.0)) * scale,
            src.f64_in(-50.0, 2000.0) * scale,
        );
        if f.hit(src, 32) {
            let bad = src.pick_or(
                &[
                    "[500,1,2]",
                    "[q,1,2,3]",
                    "[m,1e3,2,3]",
                    "[m,1,2,3",
                    "[,1,2,3]",
                ],
                "[",
            );
            let _ = write!(out, ",{name}{bad}");
        } else {
            let sep = if src.chance(32) { ", " } else { "," };
            let _ = write!(out, ",{name}[{unit}{sep}{e:.3}{sep}{n:.3}{sep}{z:.3}]");
        }
        if src.chance(24) {
            out.push_str("\n /fix comment\n");
        }
    }
    out
}

/// A `.mak` being rendered, with the `.dat` files its links resolve to.
struct Mak {
    doc: Doc,
    f: Faults,
    net: Net,
    files: Vec<(String, Vec<u8>)>,
    /// Links written so far (as written), so one can be listed again.
    links: Vec<String>,
    base: (f64, f64),
}

impl Mak {
    /// Write a `.dat` at `path` unless one is there.
    fn add_dat(&mut self, src: &mut ByteSource<'_>, path: &str) {
        if self.files.iter().any(|(p, _)| p == path) {
            return;
        }
        let tag = tag_for(self.files.len());
        let surveys = 1 + count(src, 2, 12);
        let text = dat_text(src, self.f, &mut self.net, &tag, surveys);
        let bytes = finish(src, self.f, &text, true);
        self.files.push((path.to_string(), bytes));
    }

    /// One `#` link; `folder` is the `[` folder it sits in, if any.
    fn link(&mut self, src: &mut ByteSource<'_>, folder: Option<&str>, first: bool) {
        let f = self.f;
        let k = self.files.len() + self.links.len();
        let here = |name: &str| match folder {
            Some(d) => format!("{d}/{name}"),
            None => name.to_string(),
        };
        // (link text, file written for it)
        let (text, file) = if first {
            (format!("s{k}.dat"), Some(here(&format!("s{k}.dat"))))
        } else {
            match src.below(16) {
                7 => (format!("gone{k}.dat"), None),
                8 => (format!("S{k}.DAT"), Some(here(&format!("s{k}.dat")))),
                9 if !self.links.is_empty() => {
                    let again = src.pick(&self.links).cloned().unwrap_or_default();
                    (again, None)
                }
                10 => ("project.mak".to_string(), None),
                11 if folder.is_none() => (format!("..\\s{k}.dat"), Some(format!("s{k}.dat"))),
                12 if folder.is_none() => (format!("sub\\s{k}.dat"), Some(format!("sub/s{k}.dat"))),
                13 => (format!("/stalfuzz-nowhere/s{k}.dat"), None),
                14 => (
                    src.pick_or(
                        &[
                            "C:\\Caves\\x.dat",
                            "\\\\server\\share\\x.dat",
                            "NUL.dat",
                            "CON",
                        ],
                        "CON",
                    )
                    .to_string(),
                    None,
                ),
                15 => (format!("s{k}"), Some(here(&format!("s{k}")))),
                _ => (format!("s{k}.dat"), Some(here(&format!("s{k}.dat")))),
            }
        };
        if let Some(path) = &file {
            self.add_dat(src, path);
        }
        let stations = if src.chance(64) {
            fixes(src, f, &self.net, self.base)
        } else {
            String::new()
        };
        self.doc.line(&format!("#{text}{stations};"));
        self.links.push(text);
    }

    /// The state records that may come before (or between) links.
    fn settings(&mut self, src: &mut ByteSource<'_>) {
        let f = self.f;
        if src.chance(64) {
            let zone = 1 + src.below(60);
            self.doc.line(&format!("${zone};"));
        }
        if src.chance(96) {
            let conv = src.f64_in(-3.0, 3.0);
            let kind = if src.bool() { '%' } else { '*' };
            self.doc.line(&format!("{kind}{conv:.2};"));
        }
        if src.chance(128) {
            let fl = flags(src, f);
            self.doc.line(&fl);
        }
    }
}

/// The name tag of a project's `k`th file: `A`..`Z`, then `AA`...
fn tag_for(k: usize) -> String {
    let letter = |i: usize| char::from(b'A' + (i % 26) as u8);
    if k < 26 {
        letter(k).to_string()
    } else {
        format!("{}{}", letter(k / 26 - 1), letter(k))
    }
}

/// Render one Compass project with its linked surveys.
pub fn generate_mak(src: &mut ByteSource<'_>) -> FileSet {
    let f = Faults::pick(src);
    let base = (
        src.f64_in(200_000.0, 800_000.0),
        src.f64_in(0.0, 9_000_000.0),
    );
    let mut m = Mak {
        doc: Doc::default(),
        f,
        net: Net::default(),
        files: Vec::new(),
        links: Vec::new(),
        base,
    };
    if src.chance(128) {
        m.doc.line("/ Compass project file");
    }
    if src.chance(32) {
        m.doc.line("/0f8fad5b-d9cb-469f-a165-70867728950e;");
    }
    if f.hit(src, 32) {
        let bad = src.pick_or(
            &[
                "; a semicolon comment",
                "/0f8fad5b-d9cb-469f-a165-70867728950e0;",
                "&Bogus Datum 2000;",
                "$300;",
                "%abc;",
                "!GG;",
                "^",
                "&WGS 1984",
                "[;",
                "];",
            ],
            ";",
        );
        m.doc.line(bad);
    }
    // The base location and datum must come before the first link (a
    // hostile file sometimes leaves one out).
    let skip = if f.hit(src, 48) { 1 + src.below(2) } else { 0 };
    let loc = base_location(src, f);
    let datum = format!("&{};", src.pick_or(DATUMS, "WGS 1984"));
    if src.bool() {
        if skip != 1 {
            m.doc.line(&loc);
        }
        if skip != 2 {
            m.doc.line(&datum);
        }
    } else {
        if skip != 2 {
            m.doc.line(&datum);
        }
        if skip != 1 {
            m.doc.line(&loc);
        }
    }
    m.settings(src);
    let links = 1 + count(src, 3, MAX_LINKS - 1);
    let mut folder: Option<&str> = None;
    for i in 0..links {
        if m.doc.full(8) {
            break;
        }
        if folder.is_none() && i > 0 && src.chance(32) {
            let name = src.pick_or(&["sub", "Data", "more caves"], "sub");
            m.doc.line(&format!("[{name};"));
            folder = Some(name);
        } else if folder.is_some() && src.chance(64) {
            m.doc.line("];");
            folder = None;
        }
        if src.chance(24) {
            // State changes between links apply to the links after them.
            let loc = base_location(src, f);
            m.doc.line(&loc);
            let datum = format!("&{};", src.pick_or(DATUMS, "WGS 1984"));
            m.doc.line(&datum);
            m.settings(src);
        }
        if src.chance(24) {
            m.doc.line("/ comment between links");
        }
        m.link(src, folder, i == 0);
    }
    if folder.is_some() && !f.hit(src, 64) {
        m.doc.line("];");
    }
    let text = m.doc.text.clone();
    let mut set = FileSet::single("project.mak", finish(src, f, &text, f.on()));
    for (path, bytes) in m.files {
        set.push(&path, bytes);
    }
    set
}
