use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{bail, Result};

use crate::parsers::encoding::{
    cp1252_replacement_warning, describe_byte_limit, read_cp1252, Cp1252Text,
};
use crate::parsers::linked_files::{
    is_dos_device_name, outside_root, Admission, LinkBudget, MAX_LINKED_BYTES,
};

use super::{
    apply_units_options, is_walls_separator, parse_srv_contents, split_first_token, ParsedSurvey,
    ParsedWalls, WallsRef, WallsUnits, STATUS_DERIVE_DECL, STATUS_DETACHED,
    STATUS_DONT_DERIVE_DECL, STATUS_GRID_RELATIVE, STATUS_NOT_GRID_RELATIVE, STATUS_TYPE_OTHER,
};

/// Parse a bare `.SRV` file as a one-survey project (no `.WPJ` tree).
pub(crate) fn parse_single_srv(path: &Path) -> Result<ParsedWalls> {
    let Cp1252Text {
        text: contents,
        replaced,
    } = read_cp1252(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("survey")
        .to_string();
    // A bare `.SRV` is the one file the user picked, so the warning needs no name.
    let mut warnings: Vec<String> = cp1252_replacement_warning(None, "Walls", replaced)
        .into_iter()
        .collect();
    let survey = parse_srv_contents(
        &contents,
        &name,
        &name,
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        None,
        true,  // grid-relative inherit -> default on; harmless with no georef
        false, // no auto-declination for a bare .srv (no georef/status)
        &mut warnings,
    );
    Ok(ParsedWalls {
        project_name: name,
        book_count: 0,
        surveys: vec![survey],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// .WPJ parsing
// ---------------------------------------------------------------------------

/// One entry while walking the `.WPJ` tree.
#[derive(Clone)]
pub(crate) struct WpjEntry {
    pub(crate) is_book: bool,
    pub(crate) title: String,
    pub(crate) name: Option<String>,
    pub(crate) status: u32,
    pub(crate) options: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) reference: Option<WallsRef>,
}

/// The deepest a `.WPJ` may nest books (#576 B12, decision "share state,
/// refuse past 256"). Real projects nest a handful of levels; a file nesting
/// deeper is refused rather than walked, as the `#Units` Save stack is
/// bounded (`units.rs`).
pub(crate) const MAX_BOOK_DEPTH: usize = 256;

/// One link of a chain a book passes down to everything under it (#576 B12):
/// the value this level set and the chain its parent had. Children share
/// their ancestors' links instead of copying them, so N nested books hold N
/// links - not the N(N+1)/2 copies of every ancestor's `.OPTIONS` and
/// segment name they used to, which let a few hundred kilobytes of nested
/// books take gigabytes.
pub(crate) struct Inherited {
    value: String,
    parent: Option<Rc<Inherited>>,
}

/// `chain` with `value` appended.
fn extend_chain(chain: Option<&Rc<Inherited>>, value: String) -> Rc<Inherited> {
    Rc::new(Inherited {
        value,
        parent: chain.cloned(),
    })
}

/// The chain's values, root first.
fn chain_values(chain: Option<&Rc<Inherited>>) -> Vec<&str> {
    let mut values = Vec::new();
    let mut link = chain.map(Rc::as_ref);
    while let Some(l) = link {
        values.push(l.value.as_str());
        link = l.parent.as_deref();
    }
    values.reverse();
    values
}

/// Inherited context accumulated down the book tree. Cheap to clone: the
/// option and segment chains and the reference are shared.
#[derive(Clone)]
pub(crate) struct WpjContext {
    /// Where this entry's files resolve, or why its `.PATH` (or an
    /// ancestor's, which a relative `.PATH` builds on) was refused
    /// ([`resolve_path`]).
    pub(crate) dir: Result<PathBuf, &'static str>,
    /// The `.wpj`'s own directory - the immutable project root. A relative
    /// `.PATH` may not climb above it (traversal guard); an absolute `.PATH`
    /// resets `dir` but leaves `root` unchanged.
    pub(crate) root: PathBuf,
    /// The `#units`-option strings set from the root down to here.
    pub(crate) options: Option<Rc<Inherited>>,
    pub(crate) reference: Option<Rc<WallsRef>>,
    /// The segment names set from the root down to here.
    pub(crate) segment: Option<Rc<Inherited>>,
    pub(crate) derive_decl: bool,
    pub(crate) grid_relative: bool,
}

impl WpjContext {
    /// The segment path, `/`-joined from the root down.
    pub(crate) fn segment_path(&self) -> String {
        chain_values(self.segment.as_ref()).join("/")
    }
}

pub(crate) fn parse_wpj(path: &Path) -> Result<ParsedWalls> {
    parse_wpj_within(path, MAX_LINKED_BYTES)
}

/// [`parse_wpj`] with the byte budget its `.SRV` files share spelled out. A
/// `.SRV` is read once however many entries name it, and one that would take
/// the project past `linked_byte_limit` is skipped, both with a warning - the
/// rules a Compass `.MAK` follows for its `.DAT` files (#576 NF-FIX3R-2).
pub(crate) fn parse_wpj_within(path: &Path, linked_byte_limit: u64) -> Result<ParsedWalls> {
    let Cp1252Text {
        text: contents,
        replaced,
    } = read_cp1252(path)?;
    let root_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();

    // A project reads many files, so each warning about one names it.
    let mut warnings: Vec<String> = cp1252_replacement_warning(Some(path), "Walls", replaced)
        .into_iter()
        .collect();
    let mut surveys: Vec<ParsedSurvey> = Vec::new();
    let mut missing_files: Vec<String> = Vec::new();
    let mut skipped_attachments: u32 = 0;
    let mut book_count: u32 = 0;
    let mut project_name = String::new();
    let mut project_ref: Option<WallsRef> = None;
    let mut budget = LinkBudget::new(linked_byte_limit);

    // Stack of book contexts. The context on top of the stack is the
    // currently-open book's; its values are what children inherit.
    let mut stack: Vec<WpjContext> = Vec::new();
    // The entry currently being described by the property directives that
    // follow a `.BOOK`/`.SURVEY` line.
    let mut pending: Option<WpjEntry> = None;

    let root_context = WpjContext {
        dir: Ok(root_dir.clone()),
        root: root_dir.clone(),
        options: None,
        reference: None,
        segment: None,
        derive_decl: false,
        // Root default matches dewalls `WpjEntry::gridRelative()` (false when no
        // ancestor sets a grid STATUS bit) and our own `derive_decl: false`
        // default above. A `true` default would silently rotate every CT azimuth
        // by the `.REF` grid convergence on a project that carries a `.REF` but
        // sets no grid-relative bit.
        grid_relative: false,
    };

    // Flush `pending` (a fully-described entry) into the tree: push books onto
    // the stack, resolve+parse surveys.
    macro_rules! flush_pending {
        () => {
            if let Some(entry) = pending.take() {
                // Only `.ENDBOOK` closes a book, so a file missing those lines
                // nests every sibling book in the one before it and lands here
                // too (#576 RV3-R-NITS): the message names that cause as well.
                if entry.is_book && stack.len() >= MAX_BOOK_DEPTH {
                    bail!(
                        "This .WPJ nests books more than {MAX_BOOK_DEPTH} levels deep (book \
                         '{}' opens level {}), or is missing .ENDBOOK lines; no real project \
                         does, so it was not imported.",
                        crate::models::sanitize_display_text(&entry.title, 64),
                        stack.len() + 1
                    );
                }
                let parent_ctx = stack.last().unwrap_or(&root_context);
                let ctx = derive_context(parent_ctx, &entry);
                if entry.is_book {
                    book_count += 1;
                    if project_name.is_empty() {
                        project_name = entry.title.clone();
                    }
                    if project_ref.is_none() {
                        if let Some(r) = &ctx.reference {
                            project_ref = Some(WallsRef::clone(r));
                        }
                    }
                    stack.push(ctx);
                } else {
                    // Survey leaf.
                    if entry.status & STATUS_DETACHED != 0 {
                        // Detached: skipped during compilation.
                    } else if entry.status & STATUS_TYPE_OTHER != 0 {
                        skipped_attachments += 1;
                    } else {
                        let label = entry.name.clone().unwrap_or_else(|| entry.title.clone());
                        // The survey's file, or the name to list it under as
                        // not imported.
                        let found = match &ctx.dir {
                            // A refused `.PATH` (#576 SEC-1) is never handed to
                            // the file system, not even to ask whether it exists.
                            Err(reason) => {
                                warnings.push(format!(
                                    "survey '{label}' was skipped: its .PATH was refused \
                                     ({reason}); it was not imported."
                                ));
                                Err(label.clone())
                            }
                            Ok(dir) => match resolve_survey(&entry, dir) {
                                Some(file_path) if file_path.exists() => {
                                    // Defense-in-depth: an absolute local `.PATH`
                                    // is honored (genuine Walls does), but a file
                                    // read from outside the `.wpj`'s own tree -
                                    // through it, or through a symlink inside the
                                    // project (#576 RV3-R-10) - is said out loud,
                                    // once it has actually been read (below).
                                    // (The read is NOT blocked: an explicit path
                                    // is the author's deliberate choice.)
                                    let outside = outside_root(&file_path, &ctx.root);
                                    Ok((file_path, outside))
                                }
                                other => Err(other
                                    .map_or_else(|| label.clone(), |p| p.display().to_string())),
                            },
                        };
                        match found {
                            Ok((file_path, outside)) => match budget.admit(&file_path) {
                                Admission::AlreadyRead => warnings.push(format!(
                                    "survey '{label}': {} is listed more than once in the .WPJ; \
                                     the file was read once, and this listing was skipped so its \
                                     shots are not imported twice.",
                                    file_path.display()
                                )),
                                Admission::OverLimit { limit } => warnings.push(format!(
                                    "survey '{label}' was skipped: with it, the survey files this \
                                     .WPJ lists would add up to more than the {} limit for one \
                                     project; it was not imported.",
                                    describe_byte_limit(limit)
                                )),
                                Admission::Fits(ticket) => match read_cp1252(&file_path) {
                                    Ok(Cp1252Text {
                                        text: contents,
                                        replaced,
                                    }) => {
                                        budget.record(ticket);
                                        // Only a listing that was read is said
                                        // to be read from outside: a repeat or an
                                        // over-budget one is skipped, and says
                                        // so (#576 RV4-R-P2).
                                        if let Some(real) = outside {
                                            warnings.push(format!(
                                                "survey '{label}' is read from outside the \
                                                 project directory ({})",
                                                real.display()
                                            ));
                                        }
                                        warnings.extend(cp1252_replacement_warning(
                                            Some(&file_path),
                                            "Walls",
                                            replaced,
                                        ));
                                        let (initial, initial_macros) =
                                            build_initial_units(&ctx, &mut warnings);
                                        let survey = parse_srv_contents(
                                            &contents,
                                            &entry.title,
                                            &label,
                                            &ctx.segment_path(),
                                            &initial,
                                            &initial_macros,
                                            ctx.reference.as_deref().cloned(),
                                            ctx.grid_relative,
                                            ctx.derive_decl,
                                            &mut warnings,
                                        );
                                        surveys.push(survey);
                                    }
                                    Err(e) => {
                                        warnings.push(format!(
                                            "Could not read survey '{}': {e}",
                                            file_path.display()
                                        ));
                                    }
                                },
                            },
                            Err(shown) => {
                                missing_files.push(shown);
                                surveys.push(ParsedSurvey {
                                    title: entry.title.clone(),
                                    name: label,
                                    segment: ctx.segment_path(),
                                    date: None,
                                    surveyor: None,
                                    vectors: Vec::new(),
                                    fixes: Vec::new(),
                                    georef: ctx.reference.as_deref().cloned(),
                                    resolved: false,
                                    used_feet: false,
                                });
                            }
                        }
                    }
                }
            }
        };
    }

    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        // Split the line trimmed of Walls separators only, so an `.OPTIONS`
        // value ending in U+00A0 keeps it, as the same text does in a `.SRV`.
        let (directive, rest) = split_first_token(raw.trim_end_matches(is_walls_separator));
        let dir_up = directive.to_ascii_uppercase();
        match dir_up.as_str() {
            ".BOOK" => {
                flush_pending!();
                pending = Some(WpjEntry {
                    is_book: true,
                    title: rest.trim().to_string(),
                    name: None,
                    status: 0,
                    options: None,
                    path: None,
                    reference: None,
                });
            }
            ".SURVEY" => {
                flush_pending!();
                pending = Some(WpjEntry {
                    is_book: false,
                    title: rest.trim().to_string(),
                    name: None,
                    status: 0,
                    options: None,
                    path: None,
                    reference: None,
                });
            }
            ".ENDBOOK" => {
                flush_pending!();
                stack.pop();
            }
            ".NAME" => {
                if let Some(e) = pending.as_mut() {
                    e.name = Some(rest.trim().to_string());
                }
            }
            ".STATUS" => {
                if let Some(e) = pending.as_mut() {
                    let raw = rest.trim();
                    match raw.parse() {
                        Ok(status) => e.status = status,
                        // An unparseable .STATUS would otherwise default to 0,
                        // which clears the DETACHED/TYPE_OTHER bits and imports an
                        // entry the author excluded. Keep the default but surface it.
                        Err(_) => warnings.push(format!(
                            "Unparseable .STATUS '{raw}'; treating as 0 (entry may import unexpectedly)"
                        )),
                    }
                }
            }
            ".OPTIONS" => {
                if let Some(e) = pending.as_mut() {
                    e.options = Some(rest.trim_matches(is_walls_separator).to_string());
                }
            }
            ".PATH" => {
                if let Some(e) = pending.as_mut() {
                    e.path = Some(rest.trim().to_string());
                }
            }
            ".REF" => {
                if let Some(e) = pending.as_mut() {
                    match parse_ref(rest.trim()) {
                        Ok(r) => e.reference = Some(r),
                        Err(msg) => warnings.push(format!("Ignoring malformed .REF: {msg}")),
                    }
                }
            }
            _ => {
                // Unknown directive - ignore.
            }
        }
    }
    flush_pending!();

    Ok(ParsedWalls {
        project_name,
        book_count,
        surveys,
        georef: project_ref,
        missing_files,
        skipped_attachments,
        warnings,
    })
}

/// Derive a child entry's inherited context from its parent's. The parent's
/// option and segment chains are shared, not copied (#576 B12).
pub(crate) fn derive_context(parent: &WpjContext, entry: &WpjEntry) -> WpjContext {
    let mut ctx = parent.clone();
    // Resolve `.PATH`. A refused parent folder stays refused for a relative
    // `.PATH`, which builds on it; an absolute one replaces it.
    if let Some(p) = &entry.path {
        ctx.dir = match &parent.dir {
            Ok(base) => resolve_path(base, &parent.root, p),
            Err(_) if replaces_base(p) => resolve_path(&parent.root, &parent.root, p),
            Err(reason) => Err(*reason),
        };
    }
    // Accumulate `.OPTIONS`.
    if let Some(opts) = &entry.options {
        if !opts.trim().is_empty() {
            ctx.options = Some(extend_chain(parent.options.as_ref(), opts.clone()));
        }
    }
    // Nearest `.REF` wins. An entry that opts out of a reference (the
    // reference-unspecified status bit 6) and declares none of its own does
    // NOT inherit the ancestor reference.
    if let Some(r) = &entry.reference {
        ctx.reference = Some(Rc::new(r.clone()));
    } else if entry.status & (1 << 6) != 0 {
        ctx.reference = None;
    }
    // Segment: an entry whose NAME_DEFINES_SEGMENT bit (3) is set contributes
    // its own name as a segment level - for surveys as well as books
    // (dewalls `WpjEntry::segment()` semantics).
    if let Some(name) = &entry.name {
        if !name.trim().is_empty() && entry.status & (1 << 3) != 0 {
            ctx.segment = Some(extend_chain(
                parent.segment.as_ref(),
                name.trim().to_string(),
            ));
        }
    }
    // Tristate derive-declination / grid-relative (off/on/inherit).
    if entry.status & STATUS_DERIVE_DECL != 0 {
        ctx.derive_decl = true;
    } else if entry.status & STATUS_DONT_DERIVE_DECL != 0 {
        ctx.derive_decl = false;
    }
    if entry.status & STATUS_GRID_RELATIVE != 0 {
        ctx.grid_relative = true;
    } else if entry.status & STATUS_NOT_GRID_RELATIVE != 0 {
        ctx.grid_relative = false;
    }
    ctx
}

/// Resolve a `.SURVEY` entry to its `.SRV` file path in `dir`.
pub(crate) fn resolve_survey(entry: &WpjEntry, dir: &Path) -> Option<PathBuf> {
    let name = entry.name.as_ref()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // A `.NAME` is a node identifier that becomes a bare file name, never a
    // path. Reject one carrying a path separator or drive prefix (`/`, `\`,
    // `:`): valid `.wpj` names never contain them, and allowing them would let a
    // crafted project read files outside its directory (e.g.
    // `.NAME ../../../../etc/passwd`). Treated as unresolved (reported missing).
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return None;
    }
    // Nor a DOS device (`CON`, `NUL`): `NUL.SRV` opens the device on Windows,
    // so it is never looked up (#576 RV2-R-10).
    if is_dos_device_name(name) {
        return None;
    }
    // Only a `.SRV` suffix (case-insensitive) marks the name as already
    // extended; any OTHER trailing extension still gets `.SRV` appended
    // (dewalls/survex: a `.NAME FOO.OLD` resolves to `FOO.OLD.SRV`).
    let already_srv = name.to_ascii_lowercase().ends_with(".srv");
    let candidates: Vec<String> = if already_srv {
        vec![name.to_string()]
    } else {
        vec![format!("{name}.SRV"), format!("{name}.srv")]
    };
    for cand in &candidates {
        let p = dir.join(cand);
        if p.exists() {
            return Some(p);
        }
    }
    // Default to the first candidate even if missing (caller reports missing).
    Some(dir.join(&candidates[0]))
}

/// Whether a `.PATH` replaces the folder it is resolved against rather than
/// building on it: an absolute, drive-qualified or network/device path.
fn replaces_base(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.starts_with("//") || Path::new(&normalized).is_absolute() || normalized.contains(':')
}

// Resolve a `.PATH` against a parent directory: the rules the Compass `.MAK`
// reader follows for its linked files (#576 SEC-1, decision "same as .MAK"),
// kept in ONE place (RV3-R-18). An absolute local path REPLACES the base
// (genuine Walls honors it - `Walls32 prjhier.cpp` `GetPathPrefix` uses an
// absolute `.PATH` as the prefix), normalized; a relative one concatenates
// with `..` clamped at `root` (the `.wpj`'s own directory); network, device
// and unresolvable drive-letter paths are refused from the text.
pub(crate) use crate::parsers::linked_files::resolve_link as resolve_path;

/// Parse a `.REF` line: `northing easting zone conv elev flags latDMS lonDMS
/// datumIdx "datum"`.
pub(crate) fn parse_ref(line: &str) -> Result<WallsRef, String> {
    // Pull the trailing quoted datum name first.
    let (head, datum) = match line.rfind('"') {
        Some(end) => {
            let start = line[..end].rfind('"').ok_or("unterminated datum quote")?;
            let datum = line[start + 1..end].to_string();
            (line[..start].trim().to_string(), datum)
        }
        None => (line.trim().to_string(), String::new()),
    };
    let f: Vec<f64> = head
        .split_whitespace()
        .map(|t| t.parse::<f64>().unwrap_or(f64::NAN))
        .collect();
    if f.len() < 12 || f[..12].iter().any(|v| !v.is_finite()) {
        return Err(format!(
            "expected >=12 finite numeric fields, got {}",
            f.len()
        ));
    }
    // Fields: 0 N, 1 E, 2 zone, 3 convergence, 4 elev, 5 flags,
    // 6-8 lat d/m/s, 9-11 lon d/m/s, [12 datum index].
    // Validate the integer-coded fields BEFORE casting: an `f64 as i32/u32` cast
    // saturates (negative → 0, huge → MAX), which would silently flip the
    // hemisphere flags or pick the wrong UTM meridian. Reject out-of-range.
    let zone_raw = f[2];
    if zone_raw.fract() != 0.0 || !(1.0..=60.0).contains(&zone_raw.abs()) {
        return Err(format!("invalid UTM zone '{zone_raw}'"));
    }
    let flags_raw = f[5];
    if flags_raw.fract() != 0.0 || !(0.0..=65535.0).contains(&flags_raw) {
        return Err(format!("invalid .REF flags field '{flags_raw}'"));
    }
    let flags = flags_raw as u32;
    let lat = dms(f[6], f[7], f[8]) * if flags & 0x08 != 0 { -1.0 } else { 1.0 };
    let lon = dms(f[9], f[10], f[11]) * if flags & 0x04 != 0 { -1.0 } else { 1.0 };
    Ok(WallsRef {
        zone: zone_raw as i32,
        convergence: f[3],
        latitude: lat,
        longitude: lon,
        datum,
    })
}

pub(crate) fn dms(d: f64, m: f64, s: f64) -> f64 {
    let sign = if d < 0.0 { -1.0 } else { 1.0 };
    sign * (d.abs() + m / 60.0 + s / 3600.0)
}

/// Build the initial `#Units` state and macro table for a survey from inherited
/// `.OPTIONS`. Macros defined in a parent/book/survey `.OPTIONS` string (e.g.
/// `$cave=ABC`) must be visible to the child `.SRV` body, so they are returned
/// alongside the units rather than discarded - otherwise any `$(cave)` reference
/// in the survey fails to expand and those shots are silently dropped.
pub(crate) fn build_initial_units(
    ctx: &WpjContext,
    warnings: &mut Vec<String>,
) -> (WallsUnits, HashMap<String, String>) {
    let mut units = WallsUnits::default();
    let mut macros: HashMap<String, String> = HashMap::new();
    let mut stack: Vec<WallsUnits> = Vec::new();
    for opts in chain_values(ctx.options.as_ref()) {
        apply_units_options(&mut units, opts, &mut stack, &mut macros, warnings);
    }
    (units, macros)
}
