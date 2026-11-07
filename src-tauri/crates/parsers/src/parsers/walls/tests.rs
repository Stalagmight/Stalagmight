#![expect(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Datelike;

use crate::models::section::{ImportClosure, ImportPlacement, ImportSectionConfig};
use crate::models::test_support::require_env_file;
use crate::parsers::encoding::read_cp1252;

use super::*;

// ----- helpers ---------------------------------------------------------

// `parse_srv` / `convert_one` moved to `walls::test_support` (also compiled
// under the `test-support` feature) so the root crate's tests can reach them.
use super::test_support::{convert_one, parse_srv};

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// RAII temp directory: created on construction, removed on `Drop` (so a
/// panicking test can't leak it). Isolation is by PID + tag + a process-wide
/// atomic nonce, so two tests sharing a tag - or repeated calls in one test -
/// never collide on the same path.
struct TempDir {
    path: PathBuf,
}

impl Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let n = NONCE.fetch_add(1, Ordering::Relaxed);
    let mut d = std::env::temp_dir();
    d.push(format!("walls_test_{tag}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    TempDir { path: d }
}

// ----- datum recognition / projection ---------------------------------

#[test]
fn datum_family_classifies_canonical_walls_names() {
    // The authoritative Walls datum table (wallcss.cpp `sDatum[]`) writes
    // these display names into `.REF`; the long NIMA names also appear.
    for name in ["WGS 1984", "WGS1984", "wgs 84", "WGS84"] {
        assert_eq!(datum_family(name), DatumFamily::Wgs84, "{name}");
    }
    for name in ["WGS 1972", "WGS72"] {
        assert_eq!(datum_family(name), DatumFamily::Wgs72, "{name}");
    }
    for name in ["NAD83", "North American 1983"] {
        assert_eq!(datum_family(name), DatumFamily::Nad83, "{name}");
    }
    for name in [
        "NAD27 Alaska",
        "NAD27 CONUS",
        "NAD27 Mexico",
        "North American 1927",
    ] {
        assert_eq!(datum_family(name), DatumFamily::Nad27, "{name}");
    }
    // Obscure regional datums fall back to a WGS84 approximation.
    for name in ["Camp Area Astro", "European 1950", "Tokyo", ""] {
        assert_eq!(datum_family(name), DatumFamily::OtherApproxWgs84, "{name}");
    }
}

#[test]
fn datum_proj_token_and_recognition_agree() {
    // The classifier is the single source of truth, so projection and
    // recognition can never disagree.
    assert_eq!(datum_proj_token("WGS 1984"), "datum=WGS84");
    assert_eq!(datum_proj_token("WGS 1972"), "ellps=WGS72");
    assert_eq!(datum_proj_token("North American 1983"), "datum=NAD83");
    assert_eq!(datum_proj_token("NAD27 Alaska"), "datum=NAD27");
    assert_eq!(datum_proj_token("Camp Area Astro"), "datum=WGS84");
    assert!(datum_recognized("WGS 1984"));
    assert!(datum_recognized("NAD27 Alaska"));
    assert!(!datum_recognized("Camp Area Astro"));
}

#[test]
fn wgs1984_utm_fix_does_not_warn() {
    // Regression: "WGS 1984" is the standard Walls name for WGS84, so a
    // surface file of many UTM `#Fix` points under it must produce ZERO
    // datum warnings (previously one per fix - hundreds of them).
    let georef = Some(WallsRef {
        zone: 34,
        convergence: 0.0,
        latitude: 49.0,
        longitude: 20.0,
        datum: "WGS 1984".to_string(),
    });
    let srv = "#units rect order=ENU\n#units meters\n#prefix UTM3\n\
               #fix 1 417727.726 5456614.566 970\n\
               #fix 72 417753.428 5456614.566 980\n\
               #fix 242 413813.448 5456614.566 990\n";
    let mut warnings = Vec::new();
    let survey = parse_srv_contents(
        srv,
        "t",
        "t",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        georef,
        true,
        false,
        &mut warnings,
    );
    assert_eq!(survey.fixes.len(), 3, "all three #Fix rows parsed");
    assert!(
        survey.fixes.iter().all(|f| f.lat_long.is_some()),
        "WGS 1984 UTM fixes project to lat/long"
    );
    assert!(
        warnings.iter().all(|w| !w.contains("not recognized")),
        "WGS 1984 must not warn, got {warnings:?}"
    );
}

#[test]
fn unrecognized_datum_warns_once_per_survey() {
    // An unrecognized datum on many UTM fixes warns exactly ONCE, not once
    // per fixed point.
    let georef = Some(WallsRef {
        zone: 16,
        convergence: 0.0,
        latitude: 20.0,
        longitude: -87.0,
        datum: "Camp Area Astro".to_string(),
    });
    let srv = "#units rect order=ENU\n#units meters\n\
               #fix A 467427 2258942 0\n\
               #fix B 467500 2259000 0\n\
               #fix C 467600 2259100 0\n";
    let mut warnings = Vec::new();
    parse_srv_contents(
        srv,
        "t",
        "t",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        georef,
        true,
        false,
        &mut warnings,
    );
    let datum_warnings = warnings
        .iter()
        .filter(|w| w.contains("not recognized"))
        .count();
    assert_eq!(
        datum_warnings, 1,
        "exactly one aggregated datum warning, got {warnings:?}"
    );
}

// ----- depth-gauge dive surveys ---------------------------------------

#[test]
fn depth_gauge_basic() {
    let srv = "#DATE 2017-05-07\n#UNITS meters ORDER=DA Tape=SS\n\
               A\tB\t21.1\t339\t13\t13\n\
               B\tC\t8.1\t62\t13\t11.9\t<--,3,2,1>\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.vectors.len(), 2);
    let ab = &survey.vectors[0];
    assert_eq!(ab.from, "A");
    assert_eq!(ab.to, "B");
    assert!(ab.is_depth_gauge);
    assert!(approx(ab.length_m, 21.1));
    assert_eq!(ab.azimuth_in, Some(339.0));
    assert_eq!(ab.from_depth, Some(13.0));
    assert_eq!(ab.to_depth, Some(13.0));
    assert_eq!(ab.inc_deg, None);
    let bc = &survey.vectors[1];
    assert_eq!(bc.from_depth, Some(13.0));
    assert_eq!(bc.to_depth, Some(11.9));
    assert_eq!(bc.left, None); // "--" omitted
    assert_eq!(bc.right, Some(3.0));
    assert_eq!(bc.up, Some(2.0));
    assert_eq!(bc.down, Some(1.0));
    assert_eq!(
        survey.date,
        Some(NaiveDate::from_ymd_opt(2017, 5, 7).unwrap())
    );
}

#[test]
fn rect_vector_preserves_lrud() {
    // #241 (P0): RECT (E/N/U) vectors must keep their trailing <L R U D> passage
    // dimensions, exactly like CT vectors. Before the fix the RECT branch returned
    // left/right/up/down = None, silently dropping wall data on any RECT-format
    // Walls import.
    let srv = "#units rect order=ENU\n#units meters\n\
               A B 10 5 2 <1,2,3,4>\n";
    let v = &parse_srv(srv).vectors[0];
    assert!(v.is_rect, "sanity: this must be parsed as a RECT vector");
    assert_eq!(v.left, Some(1.0));
    assert_eq!(v.right, Some(2.0));
    assert_eq!(v.up, Some(3.0));
    assert_eq!(v.down, Some(4.0));
}

#[test]
fn rect_vector_preserves_partial_lrud() {
    // RECT LRUD must handle omitted entries (`--`) the same way the CT path does,
    // not just the all-present case. The LRUD span sits after the E/N/U measures,
    // a different column count than CT, so an extraction off-by-one would only
    // surface with a partial span.
    let srv = "#units rect order=ENU\n#units meters\n\
               A B 10 5 2 <1,--,3,-->\n";
    let v = &parse_srv(srv).vectors[0];
    assert!(v.is_rect, "sanity: this must be parsed as a RECT vector");
    assert_eq!(v.left, Some(1.0));
    assert_eq!(v.right, None, "omitted right must stay None");
    assert_eq!(v.up, Some(3.0));
    assert_eq!(v.down, None, "omitted down must stay None");
}

#[test]
fn depth_gauge_conversion_keeps_gauge_depths() {
    let srv = "#UNITS meters ORDER=DA Tape=SS\n\
               A B 21.1 339 13 13\n\
               B C 8.1 62 13 11.9\n";
    let (configs, closures, _w) = convert_one(parse_srv(srv));
    assert_eq!(configs.len(), 1);
    assert!(closures.is_empty());
    let sec = &configs[0];
    assert_eq!(sec.shots.len(), 2);
    assert!(approx(sec.shots[0].depth_in, 13.0));
    assert!(approx(sec.shots[0].depth_out, 13.0));
    assert!(approx(sec.shots[1].depth_in, 13.0));
    assert!(approx(sec.shots[1].depth_out, 11.9));
    assert!(approx(sec.shots[0].length, 21.1));
    assert!(approx(sec.shots[0].azimuth_in, 339.0));
    assert!(matches!(sec.placement, ImportPlacement::Origin));
}

#[test]
fn dive_depth_columns_scale_by_s_unit() {
    // The two trailing dive-depth columns scale by the S ("short") unit, NOT the
    // D unit - matching genuine Walls `parse_height`, which reads them via
    // `parse_length_arg(val, i, u->bFeetS)` (the S feet flag). Here D=Meters but
    // S=Feet, so depths `5.0`/`7.0` are feet and must come out ×0.3048, proving
    // `s_unit` (not `d_unit`) is applied. (If d_unit were used they'd stay 5/7 m.)
    let srv = "#UNITS Meters D=Meters S=Feet Order=DA Tape=SS\nA B 10 90 5.0 7.0\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.vectors.len(), 1);
    let v = &survey.vectors[0];
    assert!(v.is_depth_gauge);
    // Distance is the D unit (meters) and stays 10.
    assert!(approx(v.length_m, 10.0));
    // Depths are the S unit (feet) → meters.
    assert_eq!(v.from_depth, Some(5.0 * FEET_TO_METERS));
    assert_eq!(v.to_depth, Some(7.0 * FEET_TO_METERS));
    // Guard against a silent regression to meters.
    assert!(v.from_depth.unwrap() < 5.0);
    assert!(v.to_depth.unwrap() < 7.0);
}

/// `ParsedSurvey.used_feet` (surfaced on the import receipt, #391) must be set
/// when a shot was actually measured in feet, not merely defaulted. Only the
/// downstream note formatter (`any_feet_units`) had coverage before; this pins
/// the producer.
#[test]
fn used_feet_is_true_when_a_shot_was_measured_in_feet() {
    let srv = "#UNITS Feet Order=DA\nA B 10 90\n";
    let survey = parse_srv(srv);
    assert!(survey.used_feet);
}

#[test]
fn used_feet_is_false_when_every_shot_was_measured_in_meters() {
    let srv = "#UNITS Meters Order=DA\nA B 10 90\n";
    let survey = parse_srv(srv);
    assert!(!survey.used_feet);
}

// ----- foresight / backsight ------------------------------------------

#[test]
fn fsbs_azimuth_and_inclination_average() {
    let srv = "#UNITS meters order=DAV\nA B 10 30/210 5/-5\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.azimuth_in, Some(30.0)); // foresight
    assert_eq!(v.azimuth_out, Some(30.0)); // backsight reversed 180
    assert_eq!(v.inc_deg, Some(5.0)); // (+5, -(-5)) averaged
    assert_eq!(v.pitch_in, Some(-5.0)); // +down
    assert_eq!(v.pitch_out, Some(-5.0));
}

#[test]
fn foresight_omitted_uses_backsight() {
    let srv = "#UNITS meters order=DAV\nA B 10 /200 /-17\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.azimuth_in, Some(20.0)); // 200 reversed
    assert_eq!(v.azimuth_out, Some(20.0));
    assert_eq!(v.inc_deg, Some(17.0)); // -(-17)
}

#[test]
fn typeab_corrected_skips_reversal() {
    let srv = "#UNITS meters order=DAV TypeAB=C\nA B 10 30/30 0\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.azimuth_in, Some(30.0));
    assert_eq!(v.azimuth_out, Some(30.0)); // bs not reversed
}

// ----- units ----------------------------------------------------------

#[test]
fn grads_and_percent_units() {
    let srv = "#UNITS meters order=DAV A=Grads V=Percent\nA B 10 100 100\n";
    let v = &parse_srv(srv).vectors[0];
    assert!(approx(v.azimuth_in.unwrap(), 90.0)); // 100 grad
    assert!(approx(v.inc_deg.unwrap(), 45.0)); // 100% grade
}

#[test]
fn feet_units_convert_to_meters() {
    let srv = "#UNITS feet order=DAV\nA B 10 0 0\n";
    let v = &parse_srv(srv).vectors[0];
    assert!(approx(v.length_m, 3.048));
}

#[test]
fn save_restore_units_stack() {
    let srv = "#UNITS feet\n#UNITS Save Meters\nA B 10 0 0\n#UNITS Restore\nC D 10 0 0\n";
    let s = parse_srv(srv);
    assert!(approx(s.vectors[0].length_m, 10.0)); // meters
    assert!(approx(s.vectors[1].length_m, 3.048)); // back to feet
}

#[test]
fn prefix_and_case_folding() {
    let srv = "#UNITS Prefix=ALU Case=Upper order=DAV\na1 b2 10 0 0\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.from, "ALU:A1");
    assert_eq!(v.to, "ALU:B2");
}

#[test]
fn macro_expansion() {
    let srv = "#UNITS $sta=XY\nA$(sta) B 10 0 0\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.from, "AXY");
}

#[test]
fn block_comment_skipped() {
    let srv = "#[\nA B 10 0 0\n#]\nC D 5 0 0\n";
    let s = parse_srv(srv);
    assert_eq!(s.vectors.len(), 1);
    assert_eq!(s.vectors[0].from, "C");
}

#[test]
fn lrud_star_delimiters_and_order() {
    let srv = "#UNITS meters order=DAV LRUD=F:UDRL\nA B 10 0 0 *2 3 4 5*\n";
    let v = &parse_srv(srv).vectors[0];
    // order UDRL: up=2 down=3 right=4 left=5
    assert_eq!(v.up, Some(2.0));
    assert_eq!(v.down, Some(3.0));
    assert_eq!(v.right, Some(4.0));
    assert_eq!(v.left, Some(5.0));
}

#[test]
fn star_in_station_name_is_literal_not_lrud() {
    // Regression: stations may contain a literal `*` (real corpus:
    // `W-*/2/46` in the Tatra KATASTER cave). The `*` is only an LRUD
    // delimiter at a token boundary; glued inside a station name it must be
    // kept verbatim. Previously `W-*/2/46  W-*/2/50` was torn into
    // from="W-", a bogus `*…*` span, and to="/2/50", collapsing every such
    // station onto a phantom "W-" hub (turning a chain into a star of
    // single-shot sections all sharing one point).
    let srv = "#UNITS meters order=DAV\n\
               W-*/2/46 W-*/2/50 9.2 242 25\n\
               W-*/2/50 W-*/2/51 7.3 262 8\n\
               W-*/2/51 W-*/2/52 2.7 278 -6\n";
    let parsed = parse_srv(srv);
    assert_eq!(parsed.vectors.len(), 3);
    assert_eq!(parsed.vectors[0].from, "W-*/2/46");
    assert_eq!(parsed.vectors[0].to, "W-*/2/50");
    assert_eq!(parsed.vectors[1].from, "W-*/2/50");
    assert_eq!(parsed.vectors[2].to, "W-*/2/52");
    // No LRUD was leaked from the names.
    assert!(parsed
        .vectors
        .iter()
        .all(|v| v.left.is_none() && v.right.is_none() && v.up.is_none() && v.down.is_none()));
    // The three shots form ONE head-to-tail chain (one section), not three
    // fragments hanging off a phantom hub.
    let (configs, _closures, _w) = convert_one(parsed);
    assert_eq!(
        configs.len(),
        1,
        "star-named stations must chain into a single section, got {configs:?}"
    );
}

#[test]
fn multi_dash_station_is_anonymous_not_named() {
    // Regression: a from/to station of two-or-more dashes (`--`, `---`) is an
    // anonymous wall/splay endpoint per Walls/dewalls (`optionalStationRx =
    // "-+"`), not a real station. Corpus: survex/walls.srv uses `U3 -- 7.8
    // 098.7`. Previously only a single `-` was anonymous, so `--` became a
    // real station literally named `--`, and every `... -- ...` leg merged
    // onto one phantom node.
    // Genuine Walls `name_isnull` only treats `-` and `--` as null/anonymous;
    // `---`+ is a REAL station name and must NOT be flagged anonymous.
    assert!(is_anonymous("-"));
    assert!(is_anonymous("--"));
    assert!(!is_anonymous("---"), "3+ dashes is a real station in Walls");
    assert!(!is_anonymous("----"));
    assert!(!is_anonymous("U3"));
    assert!(!is_anonymous(""));

    // Two distinct legs each ending at `--` must NOT share a station: with
    // splays included they get globally-unique synthetic anon names.
    let srv = "#UNITS meters order=DAV\n\
               U3 -- 7.8 098.7 0\n\
               U5 -- 4.2 120.0 0\n";
    let survey = parse_srv(srv);
    // Both legs parse with the dashed endpoint flagged anonymous.
    assert_eq!(survey.vectors.len(), 2);
    assert!(survey.vectors.iter().all(|v| v.to_anon && v.is_reference));
    let parsed = ParsedWalls {
        project_name: "T".to_string(),
        book_count: 0,
        surveys: vec![survey],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let opts = WallsImportOptions {
        include_splays: true,
        ..Default::default()
    };
    let (configs, _closures, _w) = convert_walls_to_import_configs(&parsed, &opts).unwrap();
    // The two splays import as reference shots and stay independent - there
    // is no real shared station named after the dashes (the regression
    // produced a phantom `--` hub joining unrelated splays).
    let splay_shots: usize = configs.iter().map(|c| c.shots.len()).sum();
    assert_eq!(splay_shots, 2, "both splay legs import, got {configs:?}");
}

#[test]
fn wpj_no_grid_bit_does_not_rotate_azimuth() {
    // Regression (root grid_relative defaulted to `true`): a .wpj that
    // carries a .REF with nonzero grid convergence but sets NO grid STATUS
    // bit must NOT rotate azimuths - matching dewalls
    // `WpjEntry::gridRelative()` (false at the root) and our own
    // `derive_decl: false` default. Convergence here is 5°; a 90° reading
    // must import as 90°, not 85°.
    let dir = temp_dir("wpjgrid");
    std::fs::write(dir.join("S.SRV"), "#UNITS meters order=DAV\nA B 10 90 0\n").unwrap();
    let wpj = ";WALLS Project File\n\
               .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
               .REF 2258942.000 467427.000 16 5.0 30 6 20 25 42.950 87 18 44.073 19 \"NAD83\"\n\
               .SURVEY S\n.NAME S\n.STATUS 8\n\
               .ENDBOOK\n";
    let wpj_path = dir.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    let parsed = parse_walls(&wpj_path).unwrap();
    let (configs, _c, _w) =
        convert_walls_to_import_configs(&parsed, &WallsImportOptions::default()).unwrap();
    assert!(!configs.is_empty(), "survey must resolve");
    assert!(
        approx(configs[0].shots[0].azimuth_in, 90.0),
        "no grid bit => convergence must not rotate the azimuth, got {}",
        configs[0].shots[0].azimuth_in
    );
}

#[test]
fn star_named_station_still_allows_real_lrud() {
    // A literal `*` in the station name AND a genuine `*…*` LRUD on the same
    // line: the name keeps its `*`, the space-delimited `*L R U D*` is read.
    let srv = "#UNITS meters order=DAV LRUD=F:LRUD\nA*1 B*2 10 90 0 *1 2 3 4*\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.from, "A*1");
    assert_eq!(v.to, "B*2");
    assert_eq!(v.left, Some(1.0));
    assert_eq!(v.right, Some(2.0));
    assert_eq!(v.up, Some(3.0));
    assert_eq!(v.down, Some(4.0));
}

#[test]
fn unterminated_star_is_a_station_name_not_lrud() {
    // Genuine Walls only treats a token-initial `*` as an LRUD when a closing
    // `*` is followed by whitespace/EOL (wall-srv.cpp:1173). An unterminated
    // `*8` is a real to-station named `*8` (e.g. `U23 *8 5 15`), not an LRUD
    // that swallows the measurement columns.
    let srv = "#UNITS meters order=DAV\nU23 *8 5 15 0\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.from, "U23");
    assert_eq!(v.to, "*8", "unterminated `*8` is the to-station name");
    assert!((v.length_m - 5.0).abs() < 1e-9, "distance column read");
    // A REAL terminated LRUD still works (closer before whitespace).
    let v2 =
        &parse_srv("#UNITS meters order=DAV LRUD=F:LRUD\nA B 10 90 0 *1 2 3 4* x\n").vectors[0];
    assert_eq!(v2.left, Some(1.0));
    assert_eq!(v2.down, Some(4.0));
}

#[test]
fn triple_dash_is_a_real_station_not_anonymous() {
    // Genuine Walls `name_isnull` flags only `-`/`--`; `---` is a real station
    // name. Two legs ending at `---` must therefore tie to the SAME station
    // (a real shared point), not become independent splays.
    let srv = "#UNITS meters order=DAV\nA --- 10 90 0\nB --- 8 180 0\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.vectors.len(), 2);
    assert!(
        survey
            .vectors
            .iter()
            .all(|v| v.to == "---" && !v.to_anon && !v.is_reference),
        "`---` is a real, non-anonymous station"
    );
}

#[test]
fn comment_stripped_from_vector() {
    let srv = "#UNITS meters ORDER=DA Tape=SS\nA B 10 90 5 5 ; tie in DM RED\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.comment.as_deref(), Some("tie in DM RED"));
}

// ----- conversion: chains, closures -----------------------------------

#[test]
fn branch_splits_into_sections_with_closure() {
    // A-B-C linear, then a branch D-B (shares B) plus a loop back C-A.
    let srv = "#UNITS meters order=DAV\n\
               A B 10 90 0\n\
               B C 10 0 0\n\
               D B 10 45 0\n\
               C A 14 200 0\n";
    let (configs, closures, _w) = convert_one(parse_srv(srv));
    // Three chains: section 0 = [A,B,C], section 1 = [D,B], section 2 = [C,A].
    assert_eq!(configs.len(), 3);
    // B (section 1) and C (section 2) are the spanning-tree merge edges, so they
    // do NOT close. The only non-tree shared station is A: it is canonical in
    // section 0 (index 0) and revisited as the tail of section 2 (index 1), so
    // exactly ONE closure ties (2,1) back to (0,0).
    assert_eq!(
        closures.len(),
        1,
        "expected exactly one closure: {closures:?}"
    );
    let c = &closures[0];
    assert_eq!(
        (
            c.from_section_index,
            c.from_station_index,
            c.to_section_index,
            c.to_station_index
        ),
        (2, 1, 0, 0),
        "the C-A loop must tie station A at (2,1) back to (0,0): {closures:?}"
    );
    // FromImporting targets precede dependents after topo-sort.
    let first = &configs[0];
    assert!(matches!(
        first.placement,
        ImportPlacement::Origin | ImportPlacement::FromImporting { .. }
    ));
}

// ----- .WPJ tree ------------------------------------------------------

#[test]
fn wpj_tree_status_inheritance_and_resolution() {
    let dir = temp_dir("wpj");
    std::fs::write(
        dir.join("SURV1.SRV"),
        "#UNITS meters ORDER=DA Tape=SS\nA B 10 90 5 5\n",
    )
    .unwrap();
    let wpj = format!(
        ";WALLS Project File\n\
         .BOOK Test Project\n.NAME TESTPROJ\n.STATUS 1051139\n\
         .REF 2258942.000 467427.000 16 -0.109 30 6 20 25 42.950 87 18 44.073 19 \"NAD83\"\n\
         .BOOK Area One\n.NAME AREA1\n.STATUS 8\n\
         .SURVEY First Survey\n.NAME SURV1\n.STATUS 8\n\
         .SURVEY An Attachment.svgz\n.NAME An Attachment.svgz\n.STATUS 65544\n\
         .SURVEY Detached Survey\n.NAME SURV2\n.STATUS 10\n\
         .SURVEY Missing One\n.NAME MISSING\n.STATUS 8\n\
         .ENDBOOK\n.ENDBOOK\n"
    );
    let wpj_path = dir.join("Test.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();

    let parsed = parse_walls(&wpj_path).unwrap();
    assert_eq!(parsed.book_count, 2);
    // surveys vec: SURV1 (resolved) + MISSING (unresolved). Attachment +
    // detached are skipped.
    assert_eq!(parsed.surveys.len(), 2);
    assert_eq!(parsed.skipped_attachments, 1);
    assert_eq!(parsed.missing_files.len(), 1);
    let resolved: Vec<_> = parsed.surveys.iter().filter(|s| s.resolved).collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].vectors.len(), 1);
    // NAME_DEFINES_SEGMENT (bit 3) on the book AND the survey both
    // contribute a segment level (the root has bit 3 clear).
    assert_eq!(resolved[0].segment, "AREA1/SURV1");

    let preview = build_preview(&parsed);
    assert_eq!(preview.utm_zone, Some(16));
    assert_eq!(preview.datum.as_deref(), Some("NAD83"));
    assert!(preview.has_georeference);
}

/// #393 item 1: `execute_walls_import` now reads `(survey_count,
/// any_feet_units)` from `preview_scalars` instead of running the full
/// `build_preview` a second time. This pins that `preview_scalars` never
/// drifts from what `build_preview` itself reports for the same parse - an
/// edit to `build_preview`'s survey-count or feet-units logic that forgot to
/// update `preview_scalars` (or vice versa) fails here, not silently in the
/// receipt.
#[test]
fn preview_scalars_matches_build_preview_survey_count_and_feet_units() {
    let dir = temp_dir("wpj-preview-scalars");
    std::fs::write(
        dir.join("SURV1.SRV"),
        "#UNITS meters ORDER=DA Tape=SS\nA B 10 90 5 5\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("SURV2.SRV"),
        "#UNITS feet ORDER=DA Tape=SS\nC D 30 45 5 5\n",
    )
    .unwrap();
    let wpj = ";WALLS Project File\n\
         .BOOK Test Project\n.NAME TESTPROJ\n.STATUS 1051139\n\
         .SURVEY First Survey\n.NAME SURV1\n.STATUS 8\n\
         .SURVEY Second Survey\n.NAME SURV2\n.STATUS 8\n\
         .ENDBOOK\n";
    let wpj_path = dir.join("Test.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();

    let parsed = parse_walls(&wpj_path).unwrap();
    let (survey_count, any_feet_units) = preview_scalars(&parsed);
    let preview = build_preview(&parsed);
    assert_eq!(
        survey_count, preview.survey_count,
        "survey_count must match build_preview"
    );
    assert_eq!(
        any_feet_units, preview.any_feet_units,
        "any_feet_units must match build_preview"
    );
    // Precondition: the fixture actually mixes units, so this is not vacuous.
    assert!(any_feet_units);
}

#[test]
fn ref_line_decodes_lat_long() {
    let r =
        parse_ref("2258942.000 467427.000 16 -0.109 30 6 20 25 42.950 87 18 44.073 19 \"NAD83\"")
            .unwrap();
    assert_eq!(r.zone, 16);
    assert_eq!(r.datum, "NAD83");
    assert!(approx(r.convergence, -0.109));
    // lat 20 25 42.95 N (flags bit 0x08 not set -> +); lon West (0x04) -> negative
    assert!((r.latitude - 20.4286).abs() < 0.01);
    assert!((r.longitude - (-87.3122)).abs() < 0.01);
}

#[test]
fn latin1_decoding() {
    let dir = temp_dir("latin1");
    let p = dir.join("name.srv");
    // 0xE4 == 'ä' in Latin-1.
    std::fs::write(&p, [b';', b' ', 0xE4, b'\n']).unwrap();
    let s = read_cp1252(&p).unwrap().text;
    assert!(s.contains('\u{00E4}'));
}

/// Walls is a Windows program, so a `.SRV` is Windows-1252 text and `0x85` is
/// an ellipsis. Read as ISO-8859-1 it is U+0085 (NEXT LINE), which Rust counts
/// as whitespace, so a station named `A1<0x85>B2` was torn into two tokens and
/// every field after it shifted one column to the right.
#[test]
fn a_windows_1252_byte_in_a_station_name_does_not_split_the_name() {
    let dir = temp_dir("cp1252name");
    let p = dir.join("names.srv");
    let mut bytes = b"#UNITS meters order=DAV\nA1".to_vec();
    bytes.push(0x85);
    bytes.extend_from_slice(b"B2 C1 10 90 0\n");
    std::fs::write(&p, &bytes).unwrap();

    let parsed = parse_walls(&p).unwrap();
    let vectors: Vec<(String, String, f64)> = parsed.surveys[0]
        .vectors
        .iter()
        .map(|v| (v.from.clone(), v.to.clone(), v.length_m))
        .collect();
    println!("vectors (from, to, length_m): {vectors:?}");
    println!("warnings: {:?}", parsed.warnings);
    assert_eq!(
        vectors,
        [("A1\u{2026}B2".to_string(), "C1".to_string(), 10.0)],
        "0x85 is U+2026 in Windows-1252 and part of the station name"
    );
}

/// `0xA0` is a no-break space in Windows-1252 (U+00A0). Walls separates fields
/// with ASCII whitespace only, so the byte belongs to the name it sits in, but
/// `char::is_whitespace` counts U+00A0, which tore `A1<0xA0>B2` into two
/// stations and shifted every field after it one column to the right.
#[test]
fn a_no_break_space_in_a_walls_station_name_does_not_split_the_name() {
    let dir = temp_dir("cp1252nbspname");
    let p = dir.join("names.srv");
    std::fs::write(&p, b"#UNITS meters order=DAV\nA1\xa0B2 C1 10 90 0\n").unwrap();

    let parsed = parse_walls(&p).unwrap();
    let vectors: Vec<(String, String, f64)> = parsed.surveys[0]
        .vectors
        .iter()
        .map(|v| (v.from.clone(), v.to.clone(), v.length_m))
        .collect();
    println!("vectors (from, to, length_m): {vectors:?}");
    println!("warnings: {:?}", parsed.warnings);
    assert_eq!(
        vectors,
        [("A1\u{a0}B2".to_string(), "C1".to_string(), 10.0)],
        "0xA0 is U+00A0 in Windows-1252 and part of the station name"
    );
}

/// The same byte inside a `#Units Prefix=` value is part of the prefix that
/// every station name carries, not a break between two options.
#[test]
fn a_no_break_space_in_a_walls_units_prefix_does_not_split_the_prefix() {
    let dir = temp_dir("cp1252nbspprefix");
    let p = dir.join("prefix.srv");
    std::fs::write(
        &p,
        b"#UNITS meters order=DAV Prefix=X\xa0Y\nA1 A2 10 90 0\n",
    )
    .unwrap();

    let parsed = parse_walls(&p).unwrap();
    let vectors: Vec<(String, String)> = parsed.surveys[0]
        .vectors
        .iter()
        .map(|v| (v.from.clone(), v.to.clone()))
        .collect();
    println!("vectors (from, to): {vectors:?}");
    println!("warnings: {:?}", parsed.warnings);
    assert_eq!(
        vectors,
        [("X\u{a0}Y:A1".to_string(), "X\u{a0}Y:A2".to_string())],
        "the prefix keeps its U+00A0"
    );
    assert!(
        parsed.warnings.is_empty(),
        "no option was split off the prefix: {:?}",
        parsed.warnings
    );
}

/// A U+00A0 that opens a station name belongs to it wherever the name sits.
/// The tokenizer keeps it mid-line, but the whole-line trim and the `#Fix`
/// argument trim stripped it, so `<0xA0>B2` was one station as a TO name and
/// another (`B2`) at the start of a line or in a `#Fix`: the survey fell apart
/// in two and the fix landed on the wrong station, with no warning.
#[test]
fn a_leading_no_break_space_names_the_same_walls_station_in_every_position() {
    let dir = temp_dir("nbspleading");
    let p = dir.join("lead.srv");
    std::fs::write(
        &p,
        b"#UNITS meters order=DAV\nA1 \xa0B2 10 90 0\n\xa0B2 C3 10 90 0\n#Fix \xa0B2 100 200 5\n",
    )
    .unwrap();

    let parsed = parse_walls(&p).unwrap();
    let survey = &parsed.surveys[0];
    let vectors: Vec<(String, String)> = survey
        .vectors
        .iter()
        .map(|v| (v.from.clone(), v.to.clone()))
        .collect();
    let fixes: Vec<String> = survey.fixes.iter().map(|f| f.name.clone()).collect();
    println!("vectors (from, to): {vectors:?}");
    println!("fixes: {fixes:?}");
    println!("warnings: {:?}", parsed.warnings);
    assert_eq!(vectors.len(), 2, "both shots are read");
    assert_eq!(fixes.len(), 1, "the #Fix is read");
    println!(
        "station read as the TO name mid-line (line 2):  {:?}",
        vectors[0].1
    );
    println!(
        "station read as the FROM name at line start (line 3): {:?}",
        vectors[1].0
    );
    println!(
        "station read as the #Fix name (line 4):          {:?}",
        fixes[0]
    );
    assert_eq!(vectors[0].1, "\u{a0}B2", "the tokenizer keeps the U+00A0");
    assert_eq!(
        vectors[1].0, vectors[0].1,
        "line 2 TO and line 3 FROM are one station"
    );
    assert_eq!(fixes[0], vectors[0].1, "the #Fix names that station");
}

/// A prefix is the front of every station name under it, so a U+00A0 at the
/// start or end of a prefix value stays in it, the same as in a station name
/// typed out with that prefix. The prefix trims stripped it, so `#Prefix
/// <0xA0>X` then `A1` named `X:A1` while a shot to `<0xA0>X:A1` named another
/// station.
#[test]
fn a_no_break_space_at_the_edge_of_a_walls_prefix_stays_in_the_station_names() {
    let dir = temp_dir("nbspprefixedge");
    let p = dir.join("prefix.srv");
    std::fs::write(
        &p,
        b"#UNITS meters order=DAV\n\
          #Prefix \xa0X\nA1 A2 10 90 0\n\
          #Units Prefix=\xa0X\nA2 A3 10 90 0\n\
          #Prefix\nA3 \xa0X:A4 10 90 0\n\
          #Prefix Y\xa0\nB1 B2 10 90 0\n\
          #Units Prefix=Y\xa0\nB2 B3 10 90 0\n\
          #Prefix\nB3 Y\xa0:B4 10 90 0\n",
    )
    .unwrap();

    let parsed = parse_walls(&p).unwrap();
    let vectors: Vec<(String, String)> = parsed.surveys[0]
        .vectors
        .iter()
        .map(|v| (v.from.clone(), v.to.clone()))
        .collect();
    for (label, v) in [
        "#Prefix <0xA0>X",
        "#Units Prefix=<0xA0>X",
        "no prefix, <0xA0>X:A4 typed out",
        "#Prefix Y<0xA0> (line end)",
        "#Units Prefix=Y<0xA0> (line end)",
        "no prefix, Y<0xA0>:B4 typed out",
    ]
    .iter()
    .zip(&vectors)
    {
        println!("{label:34} -> {v:?}");
    }
    println!("warnings: {:?}", parsed.warnings);
    let s = |name: &str| name.to_string();
    assert_eq!(
        vectors,
        [
            (s("\u{a0}X:A1"), s("\u{a0}X:A2")),
            (s("\u{a0}X:A2"), s("\u{a0}X:A3")),
            (s("A3"), s("\u{a0}X:A4")),
            (s("Y\u{a0}:B1"), s("Y\u{a0}:B2")),
            (s("Y\u{a0}:B2"), s("Y\u{a0}:B3")),
            (s("B3"), s("Y\u{a0}:B4")),
        ],
        "every prefix position keeps the U+00A0 a typed-out name has"
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

/// The same holds for a prefix a `.wpj` hands down in `.OPTIONS`: a U+00A0
/// ending the line is the end of the prefix, not padding to trim.
#[test]
fn a_no_break_space_ending_a_wpj_options_prefix_stays_in_the_station_names() {
    let dir = temp_dir("nbspwpjoptions");
    std::fs::write(
        dir.join("S.SRV"),
        b"#UNITS meters order=DAV\nB1 B2 10 90 0\nB2 Y\xa0:B3 10 90 0\n",
    )
    .unwrap();
    let wpj_path = dir.join("T.wpj");
    std::fs::write(
        &wpj_path,
        b";WALLS Project File\n.BOOK Root\n.NAME ROOT\n.STATUS 8\n\
          .SURVEY S\n.NAME S\n.OPTIONS Prefix=Y\xa0\n.STATUS 8\n.ENDBOOK\n",
    )
    .unwrap();

    let parsed = parse_walls(&wpj_path).unwrap();
    let vectors: Vec<(String, String)> = parsed.surveys[0]
        .vectors
        .iter()
        .map(|v| (v.from.clone(), v.to.clone()))
        .collect();
    println!("vectors (from, to): {vectors:?}");
    println!("warnings: {:?}", parsed.warnings);
    assert_eq!(
        vectors,
        [
            ("Y\u{a0}:B1".to_string(), "Y\u{a0}:B2".to_string()),
            ("Y\u{a0}:B2".to_string(), "Y\u{a0}:B3".to_string()),
        ],
        "the .OPTIONS prefix keeps its trailing U+00A0"
    );
}

/// The preview's and the receipt's warnings that speak about the encoding.
fn encoding_warnings(parsed: &ParsedWalls) -> (Vec<String>, Vec<String>) {
    let preview = build_preview(parsed).warnings;
    let (_configs, _closures, receipt) =
        convert_walls_to_import_configs(parsed, &WallsImportOptions::default()).unwrap();
    println!("preview warnings: {preview:?}");
    println!("receipt warnings: {receipt:?}");
    let about_encoding = |ws: Vec<String>| -> Vec<String> {
        ws.into_iter()
            .filter(|w| w.contains("windows-1252"))
            .collect()
    };
    (about_encoding(preview), about_encoding(receipt))
}

/// The five bytes Windows-1252 leaves undefined decode to U+FFFD, which loses
/// whatever the surveyor typed. The TML reader counts that loss on the
/// receipt; a bare `.SRV` must too, on the preview and on the receipt.
///
/// Without the fix the decode is silent and neither list carries it.
#[test]
fn an_undefined_windows_1252_byte_in_an_srv_is_counted_in_a_warning() {
    let dir = temp_dir("cp1252undefined");
    let p = dir.join("undefined.srv");
    std::fs::write(&p, b"#UNITS meters order=DAV\nA1 A2 10 90 0 ;C\x81ve\n").unwrap();

    let parsed = parse_walls(&p).unwrap();
    assert_eq!(parsed.surveys[0].vectors.len(), 1, "premise: the shot read");
    let (preview, receipt) = encoding_warnings(&parsed);

    let expected =
        "The file was read as windows-1252, the encoding Walls writes; 1 character was replaced";
    assert_eq!(preview, [expected], "one warning, for the one file read");
    assert_eq!(
        receipt,
        [expected],
        "the warning must reach the receipt too"
    );
}

/// A `.WPJ` project reads many files, so each one that lost characters gets
/// its own warning naming it, with its own count - the project file itself
/// included - and a clean survey beside them gets none.
#[test]
fn each_walls_project_file_with_undefined_bytes_is_named_with_its_count() {
    let dir = temp_dir("cp1252project");
    std::fs::write(
        dir.join("DIRTY.SRV"),
        b"#UNITS meters order=DAV\nA1 A2 10 90 0 ;\x81 and \x90\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("CLEAN.SRV"),
        b"#UNITS meters order=DAV\nA2 A3 10 90 0 ;caf\xe9 don\x92t\n",
    )
    .unwrap();
    let wpj_path = dir.join("Test.wpj");
    std::fs::write(
        &wpj_path,
        b";WALLS Project File\n\
          .BOOK Cave\x8dA\n.NAME CAVEA\n.STATUS 8\n\
          .SURVEY Dirty Survey\n.NAME DIRTY\n.STATUS 8\n\
          .SURVEY Clean Survey\n.NAME CLEAN\n.STATUS 8\n\
          .ENDBOOK\n",
    )
    .unwrap();

    let parsed = parse_walls(&wpj_path).unwrap();
    assert_eq!(
        parsed.surveys.iter().filter(|s| s.resolved).count(),
        2,
        "premise: both surveys were read"
    );
    let (preview, receipt) = encoding_warnings(&parsed);

    let expected = [
        format!(
            "'{}' was read as windows-1252, the encoding Walls writes; 1 character was replaced",
            wpj_path.display()
        ),
        format!(
            "'{}' was read as windows-1252, the encoding Walls writes; 2 characters were replaced",
            dir.join("DIRTY.SRV").display()
        ),
    ];
    assert_eq!(
        preview, expected,
        "one named warning per file that lost characters"
    );
    assert_eq!(receipt, expected, "the warnings must reach the receipt too");
}

/// The warning must stay rare enough to mean something: a `.SRV` whose high
/// bytes are all defined in Windows-1252 lost nothing and gets no warning.
#[test]
fn an_srv_with_no_undefined_byte_gets_no_encoding_warning() {
    let dir = temp_dir("cp1252clean");
    let p = dir.join("clean.srv");
    std::fs::write(
        &p,
        b"#UNITS meters order=DAV\nA1 A2 10 90 0 ;\x80 caf\xe9 don\x92t \x85\n",
    )
    .unwrap();

    let parsed = parse_walls(&p).unwrap();
    assert_eq!(parsed.surveys[0].vectors.len(), 1, "premise: the shot read");
    assert!(
        parsed.warnings.is_empty(),
        "no parser warning at all, got {:?}",
        parsed.warnings
    );
    let (preview, receipt) = encoding_warnings(&parsed);
    assert!(preview.is_empty() && receipt.is_empty());
}

#[test]
fn fix_geographic_anchor_becomes_zero_datum() {
    let srv = "#UNITS meters order=DAV\n\
               #FIX A0 W87:18:44 N20:25:42 0\n\
               A0 B 10 90 0\n";
    let mut warnings = Vec::new();
    let survey = parse_srv_contents(
        srv,
        "Fixed",
        "FIX",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        None,
        true,
        false,
        &mut warnings,
    );
    assert_eq!(survey.fixes.len(), 1);
    assert!(survey.fixes[0].lat_long.is_some());
    let (configs, _c, _w) = convert_one(survey);
    assert!(configs
        .iter()
        .any(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. })));
}

// ----- regression tests for adversarial-review fixes -------------------

fn convert_survey_with(
    survey: ParsedSurvey,
    georef: Option<WallsRef>,
    opts: &WallsImportOptions,
) -> (Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>) {
    let parsed = ParsedWalls {
        project_name: "T".to_string(),
        book_count: 0,
        surveys: vec![survey],
        georef,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    convert_walls_to_import_configs(&parsed, opts).unwrap()
}

#[test]
fn selected_surveys_filters_conversion() {
    // Two surveys with disjoint station namespaces -> separate sections.
    let s0 = parse_srv("#UNITS meters order=DAV\nA0 A1 10 90 0\nA1 A2 10 90 0\n");
    let s1 = parse_srv("#UNITS meters order=DAV\nB0 B1 10 90 0\nB1 B2 10 90 0\n");
    let parsed = ParsedWalls {
        project_name: "T".to_string(),
        book_count: 0,
        surveys: vec![s0, s1],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let conv = |sel: Option<HashSet<u32>>| {
        convert_walls_to_import_configs(
            &parsed,
            &WallsImportOptions {
                selected_surveys: sel,
                ..Default::default()
            },
        )
        .unwrap()
        .0
    };

    let all = conv(None);
    let only0 = conv(Some([0].into_iter().collect()));
    let only1 = conv(Some([1].into_iter().collect()));
    let none = conv(Some(HashSet::new()));

    assert!(!all.is_empty(), "all surveys produce sections");
    assert!(!only0.is_empty() && !only1.is_empty());
    // Selecting each survey alone partitions the full output exactly.
    assert_eq!(
        only0.len() + only1.len(),
        all.len(),
        "per-survey selections must partition the unfiltered conversion"
    );
    assert!(none.is_empty(), "empty selection imports nothing");
}

// ----- directive comment / stray-space robustness ---------------------

#[test]
fn strip_inline_comment_respects_quotes() {
    assert_eq!(strip_inline_comment("2008-12-01\t;TODO x"), "2008-12-01\t");
    assert_eq!(
        strip_inline_comment("2000-06-06; oraz 2000-06-30"),
        "2000-06-06"
    );
    assert_eq!(strip_inline_comment("2003-08-01;TODO"), "2003-08-01");
    assert_eq!(strip_inline_comment("flag=\"a;b\" x"), "flag=\"a;b\" x");
    assert_eq!(strip_inline_comment("no comment here"), "no comment here");
}

#[test]
fn date_directive_ignores_trailing_comment() {
    // Real KATASTER lines: a valid date followed by a `;`-comment (tab- or
    // space-separated, or glued straight on) must still parse.
    for srv in [
        "#DATE 2008-12-01\t;TODO dzien nieznany\nA B 10 90 0\n",
        "#date 2000-06-06; oraz 2000-06-30\nA B 10 90 0\n",
        "#DATE 2003-08-01;TODO szacunkowa\nA B 10 90 0\n",
    ] {
        let mut warnings = Vec::new();
        let survey = parse_srv_contents(
            srv,
            "t",
            "t",
            "",
            &WallsUnits::default(),
            &HashMap::new(),
            None,
            true,
            false,
            &mut warnings,
        );
        assert!(
            survey.date.is_some(),
            "date must parse despite comment: {srv:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("unrecognized #Date")),
            "no #Date warning expected, got {warnings:?}"
        );
    }
}

#[test]
fn collapse_eq_spaces_glues_values_outside_quotes() {
    assert_eq!(collapse_eq_spaces("order =DAV"), "order=DAV");
    assert_eq!(collapse_eq_spaces("order= DAV"), "order=DAV");
    assert_eq!(collapse_eq_spaces("order = DAV"), "order=DAV");
    assert_eq!(collapse_eq_spaces("meters decl=2"), "meters decl=2");
    assert_eq!(collapse_eq_spaces("flag=\"a b\""), "flag=\"a b\"");
    // A space inside a quoted value is preserved.
    assert_eq!(collapse_eq_spaces("f=\"a = b\""), "f=\"a = b\"");
}

#[test]
fn units_tolerates_stray_spaces_around_equals() {
    // `order =ADV` (space before `=`) previously produced "Unknown #units
    // option ''"; `order= ADV` produced "Order=''" + "'adv'". Both must now
    // be read as `order=ADV` with no spurious warnings.
    for srv in [
        "#UNITS meters order =ADV\nA B 10 5 0\n",
        "#UNITS meters order= ADV\nA B 10 5 0\n",
    ] {
        let mut warnings = Vec::new();
        let survey = parse_srv_contents(
            srv,
            "t",
            "t",
            "",
            &WallsUnits::default(),
            &HashMap::new(),
            None,
            true,
            false,
            &mut warnings,
        );
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("Unknown #units option") || w.contains("Order=")),
            "stray spaces around = must not warn: {warnings:?}"
        );
        // order=ADV puts Distance in the 2nd column, so `A B 10 5 0` has
        // length 5 (not the default DAV's 10) - proving the order applied.
        assert_eq!(survey.vectors.len(), 1);
        assert!(
            (survey.vectors[0].length_m - 5.0).abs() < 1e-9,
            "order=ADV must apply (distance is column 2), got {}",
            survey.vectors[0].length_m
        );
    }
}

#[test]
fn zero_length_tie_clamped_not_rejected() {
    // `A B 0 0 0` is a coincident-station tie; the model forbids 0 length, so
    // the importer must emit a positive (negligible) length instead of a
    // zero-length shot that execute_import_sections would reject outright.
    let srv = "#UNITS meters order=DAV\nA B 0 0 0\nB C 10 90 0\n";
    let (configs, _closures, warnings) = convert_one(parse_srv(srv));
    assert!(
        configs
            .iter()
            .all(|c| c.shots.iter().all(|s| s.length > 0.0)),
        "no zero-length shot may be emitted: {configs:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("coincident-station tie")),
        "expected an aggregated coincident-tie warning, got {warnings:?}"
    );
}

fn test_ref(convergence: f64, lat: f64, lon: f64) -> WallsRef {
    WallsRef {
        zone: 16,
        convergence,
        latitude: lat,
        longitude: lon,
        datum: "WGS84".to_string(),
    }
}

fn parse_srv_geo(content: &str, georef: Option<WallsRef>, derive_decl: bool) -> ParsedSurvey {
    let mut w = Vec::new();
    parse_srv_contents(
        content,
        "t",
        "t",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        georef,
        true,
        derive_decl,
        &mut w,
    )
}

#[test]
fn splays_do_not_merge_unrelated_sections() {
    // Two geometrically unrelated sub-surveys, each with an anonymous wall
    // splay. Their "-" endpoints must NOT collapse to one shared station.
    let srv = "#UNITS meters order=DAV\n\
               A1 A2 10 0 0\n\
               A1 - 3 90 0\n\
               B1 B2 10 0 0\n\
               B1 - 3 90 0\n";
    let opts = WallsImportOptions {
        comments_on_stations: true,
        include_splays: true,
        ..Default::default()
    };
    let (configs, closures, _w) = convert_survey_with(parse_srv(srv), None, &opts);
    // The A-component and B-component are separate -> two Origin roots.
    let origins = configs
        .iter()
        .filter(|c| matches!(c.placement, ImportPlacement::Origin))
        .count();
    assert!(origins >= 2, "unrelated splay components must not merge");
    // No spurious closures from the shared anonymous marker.
    assert!(closures.is_empty(), "splays must not fabricate closures");
}

#[test]
fn dive_shot_with_omitted_distance_kept_via_depth() {
    // Distance omitted (--) but the depth gauges give a real 5 -> 9 drop.
    let srv = "#UNITS meters ORDER=DA Tape=SS\n\
               A B 10 90 5 5\n\
               B C -- 90 5 9\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    let sec = &configs[0];
    assert_eq!(
        sec.shots.len(),
        2,
        "the zero-distance dive leg must survive"
    );
    assert!(approx(sec.shots[1].depth_out, 9.0));
    // Slope length clamped up to the depth change so the model can form it.
    assert!(sec.shots[1].length >= 4.0 - 1e-9);
}

#[test]
fn auto_declination_gated_on_derive_flag() {
    // Alaska 2020 has a large (~16 deg E) declination.
    let georef = Some(test_ref(0.0, 60.0, -150.0));
    let srv = "#DATE 2020-01-01\n#UNITS meters order=DAV\nA B 10 90 0\n";

    let off = convert_survey_with(
        parse_srv_geo(srv, georef.clone(), false),
        georef.clone(),
        &WallsImportOptions::default(),
    )
    .0;
    assert!(
        approx(off[0].shots[0].azimuth_in, 90.0),
        "no flag -> no auto declination"
    );
    assert!(
        off[0]
            .azimuth_reference
            .expect("the importer records a reference")
            .is_identity(),
        "no flag -> no correction recorded either"
    );

    let on = convert_survey_with(
        parse_srv_geo(srv, georef.clone(), true),
        georef.clone(),
        &WallsImportOptions::default(),
    )
    .0;
    // #438: the READING is untouched - what the derive flag changes is the
    // correction recorded on the section, not the number in the shot.
    assert!(
        approx(on[0].shots[0].azimuth_in, 90.0),
        "the measured azimuth must survive the import verbatim, got {}",
        on[0].shots[0].azimuth_in
    );
    let reference = on[0]
        .azimuth_reference
        .expect("the importer records a reference");
    assert!(
        reference.applied_correction().abs() > 1.0,
        "derive flag -> IGRF declination lands on the section, got {}",
        reference.applied_correction()
    );
    assert!(
        matches!(
            reference.source,
            crate::models::CorrectionSource::IgrfAuto { epoch_year: 2020 }
        ),
        "the provenance must name the model and the epoch, got {:?}",
        reference.source
    );
    assert!(
        (reference.to_true(90.0) - (90.0 + reference.correction)).abs() < 1e-9,
        "the recorded correction is what geometry adds"
    );
}

#[test]
fn convergence_from_a_survey_georef_is_reported_not_applied() {
    // #438: the app's world frame is TRUE north - `gps_to_local` returns
    // true-north ENU and every datum path places against it - so a grid
    // convergence is not ours to apply. Walls subtracts it because Walls's own
    // output is grid-relative. The reading stays, the correction stays 0 (this
    // survey declares no declination), and the receipt says the convergence was
    // dropped rather than leaving the user to discover a 2 degree rotation.
    let georef = Some(test_ref(2.0, 0.0, 0.0));
    let (configs, _c, warnings) = convert_survey_with(
        parse_srv_geo(
            "#UNITS meters order=DAV\nA B 10 100 0\n",
            georef.clone(),
            false,
        ),
        georef.clone(),
        &WallsImportOptions::default(),
    );
    assert!(approx(configs[0].shots[0].azimuth_in, 100.0));
    let reference = configs[0]
        .azimuth_reference
        .expect("the importer records a reference");
    assert!(
        reference.is_identity(),
        "no declination declared means no correction, got {}",
        reference.applied_correction()
    );
    assert!(approx(reference.to_true(100.0), 100.0));
    assert!(
        warnings.iter().any(|w| w.contains("convergence")),
        "the dropped convergence must be in the receipt: {warnings:?}"
    );
}

/// #438: a chain whose vectors declare DIFFERENT declinations cannot be
/// expressed by one section-level constant, so each shot's own correction is
/// folded into its readings and the section is declared true north.
///
/// The label is only honest because the per-vector correction reaches TRUE
/// north (the grid convergence is never folded in): with a `Grid=` in play the
/// old code baked `declination - convergence` and still said "true north", so a
/// project exported the resulting GRID bearings under a true-north flag.
#[test]
fn a_mid_survey_declination_change_bakes_true_north_bearings_under_a_grid() {
    let srv = "#UNITS meters order=DAV Grid=1.5 Decl=2\n\
               A B 10 100 0\n\
               #UNITS Decl=6\n\
               B C 10 100 0\n";
    let (configs, _c, warnings) = convert_one(parse_srv(srv));
    let shots = &configs[0].shots;
    assert_eq!(shots.len(), 2, "premise: one chain of two shots");

    // Each shot carries its OWN declination, and only the declination: 100 + 2
    // and 100 + 6. A convergence folded in would put these at 100.5 and 104.5.
    assert!(
        approx(shots[0].azimuth_in, 102.0) && approx(shots[1].azimuth_in, 106.0),
        "expected true-north bearings 102 and 106, got {} and {}",
        shots[0].azimuth_in,
        shots[1].azimuth_in
    );
    let reference = configs[0]
        .azimuth_reference
        .expect("the importer records a reference");
    assert_eq!(reference.north, crate::models::NorthReference::True);
    assert!(
        reference.is_identity(),
        "already true north: nothing to add"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("more than one declination")),
        "rewriting the readings must be announced: {warnings:?}"
    );
}

/// #576 (B26): the fold-in fallback above re-wraps `reading + correction`, and a
/// bare `rem_euclid(360.0)` returns EXACTLY `360.0` for a tiny negative sum.
/// `360.0` trips `INVALID_AZIMUTH`, an error the user cannot dismiss. Ordinary
/// data reaches it: `0.7 + IncA 0.1` is `0.7999999999999999` in f64, so the
/// shot's own `Decl=-0.8` leaves about `-1.1e-16` - true north.
#[test]
fn a_folded_declination_that_cancels_the_reading_wraps_to_north_not_360() {
    let srv = "#UNITS meters order=DAV Decl=2\n\
               A B 10 100 0\n\
               #UNITS IncA=0.1 Decl=-0.8\n\
               B C 10 0.7 0\n";
    let (configs, _c, warnings) = convert_one(parse_srv(srv));
    let shots = &configs[0].shots;
    assert_eq!(shots.len(), 2, "premise: one chain of two shots");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("more than one declination")),
        "premise: the per-shot fold-in ran: {warnings:?}"
    );
    let shot = &shots[1];
    println!(
        "B->C imported azimuth_in = {:?}, azimuth_out = {:?}",
        shot.azimuth_in, shot.azimuth_out
    );
    for (name, az) in [
        ("azimuth_in", shot.azimuth_in),
        ("azimuth_out", shot.azimuth_out),
    ] {
        assert!(
            (0.0..360.0).contains(&az),
            "{name} = {az:?} is outside [0, 360) and trips INVALID_AZIMUTH"
        );
        assert!(approx(az, 0.0), "{name} = {az:?}, expected north");
    }
}

/// A chain whose vectors agree on the NUMBER but disagree on where it came from
/// must not attribute the whole section to whichever vector happened to be
/// first: the provenance is what the UI explains and what a policy change may
/// recompute, so an inconsistent one is `Unknown`.
#[test]
fn a_chain_whose_vectors_disagree_about_provenance_claims_none() {
    // Shot 1 pins an explicit `Decl=0`, which is a DECLARATION. Shot 2 clears
    // it, so the derive flag takes over - and with no georeference to derive
    // from it lands on 0 with nothing known about it. Same number, different
    // provenance.
    let srv = "#UNITS meters order=DAV Decl=0\n\
               A B 10 100 0\n\
               #UNITS Decl\n\
               B C 10 100 0\n";
    let (configs, _c, _w) = convert_survey_with(
        parse_srv_geo(srv, None, true),
        None,
        &WallsImportOptions::default(),
    );
    let reference = configs[0]
        .azimuth_reference
        .expect("the importer records a reference");
    assert!(
        reference.is_identity(),
        "the corrections agree at 0, so the section is still uniform"
    );
    assert_eq!(
        reference.source,
        crate::models::CorrectionSource::Unknown,
        "disagreeing provenance cannot be claimed as one section's"
    );
}

#[test]
fn explicit_decl_zero_suppresses_auto_derivation() {
    // Same large-declination georef as `auto_declination_gated_on_derive_flag`,
    // derive flag ON, but the survey pins `Decl=0` - auto-IGRF must NOT fire.
    let georef = Some(test_ref(0.0, 60.0, -150.0));
    let srv = "#DATE 2020-01-01\n#UNITS meters order=DAV Decl=0\nA B 10 90 0\n";
    let configs = convert_survey_with(
        parse_srv_geo(srv, georef.clone(), true),
        georef.clone(),
        &WallsImportOptions::default(),
    )
    .0;
    assert!(
        approx(configs[0].shots[0].azimuth_in, 90.0),
        "explicit Decl=0 must override the derive flag (no IGRF rotation)"
    );
}

#[test]
fn fix_assigns_lat_long_by_prefix_not_column() {
    // Coordinates written latitude-first (N before W) under the default
    // East,North order must still resolve to the right hemisphere.
    let srv = "#UNITS meters order=DAV\n\
               #FIX A0 N20:25:42 W87:18:44 0\n\
               A0 B 10 90 0\n";
    let survey = parse_srv(srv);
    let (lat, lon) = survey.fixes[0].lat_long.expect("geographic fix");
    assert!(
        (lat - 20.4283).abs() < 0.01,
        "N20 must be latitude, got {lat}"
    );
    assert!(
        (lon - (-87.3122)).abs() < 0.01,
        "W87 must be longitude, got {lon}"
    );
}

#[test]
fn unrecognized_datum_warns_on_utm_fix() {
    // A UTM #Fix under an unusual datum projects as WGS84 - warn about it.
    let georef = Some(WallsRef {
        zone: 16,
        convergence: 0.0,
        latitude: 20.0,
        longitude: -87.0,
        datum: "European 1950".to_string(),
    });
    let srv = "#UNITS meters order=DAV\n#FIX A0 467427 2258942 0\nA0 B 10 90 0\n";
    let mut warnings = Vec::new();
    parse_srv_contents(
        srv,
        "t",
        "t",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        georef,
        true,
        false,
        &mut warnings,
    );
    assert!(
        warnings.iter().any(|w| w.contains("not recognized")),
        "an unrecognized datum on a UTM #Fix should warn, got {warnings:?}"
    );
}

#[test]
fn macro_preserves_non_ascii() {
    // 'Ä' (U+00C4) preceding a macro reference must survive expansion.
    let srv = "#UNITS $s=X\n\u{00C4}$(s) B 10 0 0\n";
    let v = &parse_srv(srv).vectors[0];
    assert_eq!(v.from, "\u{00C4}X");
}

#[test]
fn non_finite_tokens_rejected() {
    assert_eq!(parse_length("nan", LengthUnit::Meters), None);
    assert_eq!(parse_length("inf", LengthUnit::Meters), None);
    assert_eq!(parse_length("1e999", LengthUnit::Meters), None);
    assert_eq!(parse_angle("nan", AngleUnit::Degrees), None);
    assert_eq!(parse_angle("1e999", AngleUnit::Degrees), None);
}

#[test]
fn feet_inches_token_edges() {
    assert_eq!(parse_length("i", LengthUnit::Meters), None); // bare 'i'
    assert!(approx(
        parse_length("i6", LengthUnit::Meters).unwrap(),
        0.5 * FEET_TO_METERS
    ));
    assert!(approx(
        parse_length("5i", LengthUnit::Meters).unwrap(),
        5.0 * FEET_TO_METERS
    ));
    assert!(approx(
        parse_length("10i6", LengthUnit::Meters).unwrap(),
        10.5 * FEET_TO_METERS
    ));
}

#[test]
fn resolve_survey_appends_srv_for_non_srv_extension() {
    let dir = temp_dir("ext");
    std::fs::write(dir.join("FOO.OLD.SRV"), "x").unwrap();
    let entry = WpjEntry {
        is_book: false,
        title: String::new(),
        name: Some("FOO.OLD".to_string()),
        status: 8,
        options: None,
        path: None,
        reference: None,
    };
    let resolved = resolve_survey(&entry, &dir).unwrap();
    assert_eq!(
        resolved.file_name().unwrap().to_string_lossy(),
        "FOO.OLD.SRV"
    );
}

#[test]
fn resolve_survey_rejects_path_like_name() {
    // A `.NAME` carrying a path separator / drive prefix is a traversal
    // attempt (`.NAME ../../../../etc/passwd`), not a node name - must be
    // refused (None ⇒ reported missing), never joined onto the dir.
    let dir = PathBuf::from("/proj");
    for bad in ["../../etc/passwd", "..\\..\\secret", "/etc/shadow", "C:\\x"] {
        let entry = WpjEntry {
            is_book: false,
            title: String::new(),
            name: Some(bad.to_string()),
            status: 8,
            options: None,
            path: None,
            reference: None,
        };
        assert!(
            resolve_survey(&entry, &dir).is_none(),
            "path-like .NAME {bad:?} must be rejected"
        );
    }
}

#[test]
fn resolve_path_clamps_traversal_and_honors_absolute() {
    let root = Path::new("/proj");
    // A relative `..` chain cannot climb above the project root.
    assert_eq!(
        resolve_path(root, root, "../../../../etc"),
        Ok(PathBuf::from("/proj/etc"))
    );
    // Relative descent works normally.
    assert_eq!(
        resolve_path(root, root, "Poligony"),
        Ok(PathBuf::from("/proj/Poligony"))
    );
    // A `..` from a subdir pops back toward root but stops AT root.
    assert_eq!(
        resolve_path(Path::new("/proj/a/b"), root, "../../.."),
        Ok(PathBuf::from("/proj"))
    );
    // An absolute local `.PATH` is honored verbatim (genuine Walls
    // behavior), not discarded.
    #[cfg(unix)]
    assert_eq!(
        resolve_path(root, root, "/data/caves"),
        Ok(PathBuf::from("/data/caves"))
    );
    // A drive letter is absolute only on Windows; elsewhere it is refused
    // rather than resolved against the working directory (#576 NF-W2C-2).
    #[cfg(windows)]
    assert_eq!(
        resolve_path(root, root, "D:\\Caves"),
        Ok(PathBuf::from("D:/Caves"))
    );
    #[cfg(not(windows))]
    assert_eq!(
        resolve_path(root, root, "D:\\Caves"),
        Err("its drive letter cannot be resolved on this system")
    );
    assert_eq!(
        resolve_path(root, root, "D:Caves"),
        Err("its drive letter cannot be resolved on this system")
    );
    // Network and device paths are refused from the text (#576 SEC-1).
    for unc in [
        "\\\\server\\share",
        "//server/share",
        "\\\\?\\C:\\x",
        "\\\\.\\COM1",
    ] {
        assert_eq!(
            resolve_path(root, root, unc),
            Err("network (UNC) and device paths are never read"),
            "{unc}"
        );
    }
    // A folder no file system could open is refused before it is stored.
    let long = "d/".repeat(20_000);
    assert_eq!(
        resolve_path(root, root, &long),
        Err("its folder path is longer than any file system allows")
    );
}

#[test]
fn absolute_path_escaping_root_warns_but_still_reads() {
    // An absolute `.PATH` that points outside the project tree is honored
    // (genuine Walls does), but the importer warns that it read outside the
    // project directory so the user is told.
    let proj = temp_dir("abspath_proj");
    let outside = temp_dir("abspath_outside");
    // The survey file lives in the OUTSIDE dir; the .PATH points there absolutely.
    std::fs::write(
        outside.join("S.SRV"),
        "#UNITS meters order=DAV\nA B 10 90 0\n",
    )
    .unwrap();
    let wpj = format!(
        ";WALLS Project File\n\
         .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
         .PATH {}\n\
         .SURVEY S\n.NAME S\n.STATUS 8\n\
         .ENDBOOK\n",
        outside.display()
    );
    let wpj_path = proj.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    let parsed = parse_walls(&wpj_path).unwrap();
    // The survey still resolved + parsed (read NOT blocked).
    assert!(
        parsed
            .surveys
            .iter()
            .any(|s| s.resolved && !s.vectors.is_empty()),
        "absolute .PATH must still be read"
    );
    // ...and a warning names the escape.
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("is read from outside the project directory")),
        "an absolute .PATH escaping the root must warn: {:?}",
        parsed.warnings
    );
}

/// One survey `S` under `.PATH {path}` in a project at `proj`, imported.
fn parse_one_survey_under_path(proj: &Path, path: &str) -> ParsedWalls {
    let wpj = format!(
        ";WALLS Project File\n\
         .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
         .PATH {path}\n\
         .SURVEY S\n.NAME S\n.STATUS 8\n\
         .ENDBOOK\n"
    );
    let wpj_path = proj.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    parse_walls(&wpj_path).unwrap()
}

const ONE_SHOT_SRV: &str = "#UNITS meters order=DAV\nA B 10 90 0\n";

/// #576 RV3-R-10: an absolute `.PATH` that climbs out of the project with
/// `..` is outside it. The check compared the text as written
/// (`<proj>/../elsewhere` "starts with" `<proj>`), so it read outside the
/// project with no warning.
#[test]
fn an_absolute_path_climbing_out_with_dotdot_warns() {
    let proj = temp_dir("dotdot_proj");
    let outside = temp_dir("dotdot_outside");
    std::fs::write(outside.join("S.SRV"), ONE_SHOT_SRV).unwrap();
    let path = format!(
        "{}/../{}",
        proj.display(),
        outside.file_name().unwrap().to_string_lossy()
    );
    let parsed = parse_one_survey_under_path(&proj, &path);
    println!(".PATH {path}: warnings {:?}", parsed.warnings);
    assert!(parsed
        .surveys
        .iter()
        .any(|s| s.resolved && !s.vectors.is_empty()));
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("survey 'S'") && w.contains("outside the project directory")),
        "{:?}",
        parsed.warnings
    );
}

/// #576 RV3-R-10: a relative `.PATH` through a folder inside the project that
/// is a symlink to one outside it reads from outside too, so it warns.
#[cfg(unix)]
#[test]
fn a_relative_path_through_a_symlinked_folder_warns() {
    let proj = temp_dir("symlink_proj");
    let outside = temp_dir("symlink_outside");
    std::fs::write(outside.join("S.SRV"), ONE_SHOT_SRV).unwrap();
    std::os::unix::fs::symlink(&*outside, proj.join("linked")).unwrap();
    let parsed = parse_one_survey_under_path(&proj, "linked");
    println!(".PATH linked (symlink): warnings {:?}", parsed.warnings);
    assert!(parsed
        .surveys
        .iter()
        .any(|s| s.resolved && !s.vectors.is_empty()));
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("survey 'S'") && w.contains("outside the project directory")),
        "{:?}",
        parsed.warnings
    );
}

/// #576 RV2-R-10 (the `.MAK` rule, shared): a `.PATH` naming a Windows DOS
/// device is refused from its text, and a `.NAME` that is one is never looked
/// up. Real files of those names exist here, so only the text can refuse them.
#[test]
fn dos_device_names_are_never_looked_up() {
    let proj = temp_dir("device_path");
    std::fs::create_dir_all(proj.join("CON")).unwrap();
    std::fs::write(proj.join("CON").join("S.SRV"), ONE_SHOT_SRV).unwrap();
    let parsed = parse_one_survey_under_path(&proj, "CON");
    println!(".PATH CON: warnings {:?}", parsed.warnings);
    assert!(parsed.surveys.iter().all(|s| !s.resolved));
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("survey 'S' was skipped") && w.contains("device")),
        "{:?}",
        parsed.warnings
    );

    let proj = temp_dir("device_name");
    std::fs::write(proj.join("nul.SRV"), ONE_SHOT_SRV).unwrap();
    let wpj = ";WALLS Project File\n\
               .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
               .SURVEY S\n.NAME nul\n.STATUS 8\n\
               .ENDBOOK\n";
    let wpj_path = proj.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    let parsed = parse_walls(&wpj_path).unwrap();
    println!(
        ".NAME nul: resolved {:?}",
        parsed
            .surveys
            .iter()
            .map(|s| s.resolved)
            .collect::<Vec<_>>()
    );
    assert!(parsed.surveys.iter().all(|s| !s.resolved));
}

// ----- multiple-#Fix anchoring (one zero datum per component) ----------

#[test]
fn multiple_fixes_anchor_separate_components() {
    // Two unconnected sub-surveys, each with its own fixed station.
    let srv = "#UNITS meters order=DAV\n\
               #FIX A0 W90:00:00 N30:00:00 0\n\
               A0 A1 10 90 0\n\
               #FIX B0 W91:00:00 N31:00:00 0\n\
               B0 B1 10 90 0\n";
    let survey = parse_srv(srv);
    assert_eq!(
        survey.fixes.iter().filter(|f| f.lat_long.is_some()).count(),
        2
    );
    let (configs, _c, _w) = convert_one(survey);
    let datums = configs
        .iter()
        .filter(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
        .count();
    assert_eq!(
        datums, 2,
        "each separate component with a #Fix gets its own datum"
    );
}

#[test]
fn multiple_fixes_in_one_component_yield_single_datum() {
    // A0 ... J ... B0 - both fixed, but connected through J (one component).
    // The model forbids 2 datums in one merge component, so only ONE datum
    // is emitted and the other fixed station is merged + closed in.
    let srv = "#UNITS meters order=DAV\n\
               #FIX A0 W90:00:00 N30:00:00 0\n\
               #FIX B0 W91:00:00 N31:00:00 0\n\
               A0 A1 10 90 0\n\
               A1 J 10 90 0\n\
               B0 J 10 270 0\n";
    let (configs, _closures, warnings) = convert_one(parse_srv(srv));
    let datums = configs
        .iter()
        .filter(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
        .count();
    assert_eq!(datums, 1, "one zero datum per connected merge component");
    // The second fixed station rides in via a merge, not a second datum.
    assert!(configs
        .iter()
        .any(|c| matches!(c.placement, ImportPlacement::FromImporting { .. })));
    assert!(
        warnings.iter().any(|w| w.contains("other fixed station")),
        "should explain the extra fixed station"
    );
}

#[test]
fn cross_section_loop_still_emits_closure() {
    // A-B-C, D-C, D-A: a loop spanning three legs must be closed.
    let srv = "#UNITS meters order=DAV\n\
               A B 10 0 0\n\
               B C 10 90 0\n\
               D C 10 180 0\n\
               D A 10 270 0\n";
    let (configs, closures, _w) = convert_one(parse_srv(srv));
    // Three chains: section 0 = [A,B,C], section 1 = [D,C], section 2 = [D,A].
    assert_eq!(configs.len(), 3);
    // C (section 1) and A (section 2) are the spanning-tree merge edges and do
    // NOT close; the one non-tree shared station is D, canonical in section 1
    // (index 0) and revisited as the head of section 2 (index 0), so exactly ONE
    // closure ties (2,0) back to (1,0).
    assert_eq!(
        closures.len(),
        1,
        "expected exactly one closure: {closures:?}"
    );
    let c = &closures[0];
    assert_eq!(
        (
            c.from_section_index,
            c.from_station_index,
            c.to_section_index,
            c.to_station_index
        ),
        (2, 0, 1, 0),
        "the loop must tie station D at (2,0) back to (1,0): {closures:?}"
    );
}

#[test]
fn unreferenced_fix_is_reported_not_anchored() {
    // GHOST is fixed but never appears in a shot -> no datum, with a warning.
    let srv = "#UNITS meters order=DAV\n\
               #FIX GHOST W90:00:00 N30:00:00 0\n\
               A B 10 90 0\n";
    let (configs, _c, warnings) = convert_one(parse_srv(srv));
    assert!(
        configs
            .iter()
            .all(|c| !matches!(c.placement, ImportPlacement::ZeroDatum { .. })),
        "an unreferenced #Fix anchors nothing"
    );
    assert!(warnings
        .iter()
        .any(|w| w.contains("isn't used by any shot")));
}

#[test]
fn preview_reports_fixed_station_count() {
    let survey = parse_srv(
        "#UNITS meters order=DAV\n\
         #FIX A0 W90:00:00 N30:00:00 0\n\
         A0 A1 10 90 0\n",
    );
    let parsed = ParsedWalls {
        project_name: "T".to_string(),
        book_count: 0,
        surveys: vec![survey],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let preview = build_preview(&parsed);
    assert_eq!(preview.fixed_station_count, 1);
}

#[test]
fn per_survey_fixed_station_count_dedups_and_validates() {
    // Two surveys share the fix `A0`, and survey 1 also carries `BAD` at an
    // out-of-range latitude (N95) that fails `GpsCoordinates::new`. The project
    // total (`resolve_anchors`) dedups `A0` across surveys and drops `BAD`, so it
    // is 1. The per-survey counts must use the SAME dedup + validity rule, so
    // neither survey can report more than the deduped, validated project total
    // (the old raw `lat_long.is_some()` count gave survey 1 a 2 and produced the
    // "2 / 1" mismatch this regresses).
    let survey1 = parse_srv(
        "#UNITS meters order=DAV\n\
         #FIX A0 W90:00:00 N30:00:00 0\n\
         #FIX BAD W30:00:00 N95:00:00 0\n\
         A0 A1 10 90 0\n",
    );
    let survey2 = parse_srv(
        "#UNITS meters order=DAV\n\
         #FIX A0 W90:00:00 N30:00:00 0\n\
         A0 A2 10 90 0\n",
    );
    let parsed = ParsedWalls {
        project_name: "T".to_string(),
        book_count: 0,
        surveys: vec![survey1, survey2],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let preview = build_preview(&parsed);
    // Project total: A0 (deduped across both surveys) - BAD is out of range.
    assert_eq!(preview.fixed_station_count, 1);
    // Each per-survey count must not exceed the deduped, validated total.
    for s in &preview.surveys {
        assert!(
            s.fixed_station_count <= preview.fixed_station_count,
            "per-survey count {} exceeds project total {}",
            s.fixed_station_count,
            preview.fixed_station_count
        );
    }
    // Specifically: survey 1's invalid `BAD` is dropped (so 1, not 2); survey 2
    // counts its lone valid `A0`.
    assert_eq!(preview.surveys[0].fixed_station_count, 1);
    assert_eq!(preview.surveys[1].fixed_station_count, 1);
}

// ----- ignored real-data validation (no project files committed) -------

/// Any Walls project tree (`WALLS_WPJ`, a maintainer-held private copy): what
/// holds for every one - it parses, the preview accounts for the parse, and the
/// import lands what the preview promised. [#576 HFI-11] It asserted ONE
/// project's book and survey counts and UTM zone, so it failed on every other
/// `.wpj`; no dataset's numbers belong here.
#[test]
#[ignore = "needs a private Walls project tree; set WALLS_WPJ to its .WPJ"]
fn real_project_tree() {
    let wpj = require_env_file("WALLS_WPJ");
    let parsed = parse_walls(&wpj).unwrap();
    let preview = build_preview(&parsed);
    println!(
        ".wpj: books={} surveys={} resolved={} missing={} skipped_attachments={} shots={} \
         splays={} fixed={} warnings={}",
        parsed.book_count,
        parsed.surveys.len(),
        preview.resolved_survey_count,
        parsed.missing_files.len(),
        parsed.skipped_attachments,
        preview.shot_count,
        preview.splay_count,
        preview.fixed_station_count,
        parsed.warnings.len(),
    );
    assert!(!parsed.surveys.is_empty(), "a project tree names surveys");

    // The preview is the parse, counted.
    assert_eq!(preview.survey_count as usize, parsed.surveys.len());
    assert_eq!(
        preview.resolved_survey_count as usize,
        parsed.surveys.iter().filter(|s| s.resolved).count()
    );
    assert_eq!(
        preview.missing_file_count as usize,
        parsed.missing_files.len()
    );
    let per_survey =
        |count: fn(&WallsSurveySummary) -> u32| -> u32 { preview.surveys.iter().map(count).sum() };
    assert_eq!(per_survey(|s| s.shot_count), preview.shot_count);
    assert_eq!(per_survey(|s| s.splay_count), preview.splay_count);
    assert_eq!(preview.utm_zone.is_some(), preview.has_georeference);

    // The import lands exactly the shots the preview counts, with and without
    // the splays, in non-empty sections, and anchors on at most the fixes it found.
    for include_splays in [false, true] {
        let opts = WallsImportOptions {
            include_splays,
            ..WallsImportOptions::default()
        };
        let (configs, _closures, _warnings) =
            convert_walls_to_import_configs(&parsed, &opts).unwrap();
        let shots: usize = configs.iter().map(|c| c.shots.len()).sum();
        let expected = preview.shot_count
            + if include_splays {
                preview.splay_count
            } else {
                0
            };
        assert_eq!(shots, expected as usize, "include_splays={include_splays}");
        assert!(configs.iter().all(|c| !c.shots.is_empty()));
        let zero_datums = configs
            .iter()
            .filter(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
            .count();
        assert!(zero_datums <= preview.fixed_station_count as usize);
    }
}

#[test]
#[ignore = "needs a folder of private .SRV files; set WALLS_SRV_DIR"]
fn real_srv_dir() {
    let dir = require_env_file("WALLS_SRV_DIR");
    let mut total_shots = 0usize;
    let mut files = 0usize;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let is_srv = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("srv"))
            .unwrap_or(false);
        if !is_srv {
            continue;
        }
        files += 1;
        let parsed = parse_walls(&p).unwrap();
        let shots: usize = parsed.surveys.iter().map(|s| s.vectors.len()).sum();
        let (configs, closures, warnings) =
            convert_walls_to_import_configs(&parsed, &WallsImportOptions::default()).unwrap();
        println!(
            "{}: {} shots -> {} sections, {} closures, {} warnings",
            p.file_name().unwrap().to_string_lossy(),
            shots,
            configs.len(),
            closures.len(),
            warnings.len(),
        );
        total_shots += shots;
    }
    println!("parsed {files} files, {total_shots} shots total");
    assert!(files > 0);
    assert!(total_shots > 0);
}

/// Recursively parse EVERY `.wpj` (full project) and `.srv` (standalone)
/// under `WALLS_CORPUS`, catching panics, and report a summary. Used to
/// sweep the public test corpus.
#[test]
#[ignore = "sweeps a Walls corpus kept outside the repo; set WALLS_CORPUS"]
fn corpus_sweep() {
    let root = require_env_file("WALLS_CORPUS");
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    let ext = ext.to_ascii_lowercase();
                    if ext == "wpj" || ext == "srv" {
                        out.push(p);
                    }
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();

    let (mut wpj_ok, mut srv_ok) = (0usize, 0usize);
    let (
        mut total_shots,
        mut total_sections,
        mut total_closures,
        mut total_datums,
        mut total_warnings,
    ) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut errored: Vec<String> = Vec::new();
    let mut panicked: Vec<String> = Vec::new();
    let mut by_shots: Vec<(usize, String)> = Vec::new();

    for f in &files {
        let is_wpj = f
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("wpj"))
            .unwrap_or(false);
        let outcome = std::panic::catch_unwind(|| {
            let parsed = parse_walls(f)?;
            let shots: usize = parsed.surveys.iter().map(|s| s.vectors.len()).sum();
            let (configs, closures, warnings) =
                convert_walls_to_import_configs(&parsed, &WallsImportOptions::default())?;
            let datums = configs
                .iter()
                .filter(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
                .count();
            Ok::<_, anyhow::Error>((shots, configs.len(), closures.len(), warnings.len(), datums))
        });
        match outcome {
            Ok(Ok((shots, sections, closures, warnings, datums))) => {
                if is_wpj {
                    wpj_ok += 1;
                } else {
                    srv_ok += 1;
                }
                total_shots += shots;
                total_sections += sections;
                total_closures += closures;
                total_datums += datums;
                total_warnings += warnings;
                by_shots.push((shots, f.display().to_string()));
            }
            Ok(Err(e)) => errored.push(format!("{}: {e}", f.display())),
            Err(_) => panicked.push(f.display().to_string()),
        }
    }

    println!(
        "CORPUS SWEEP: {} files parsed ({} wpj, {} srv); errored {}, panicked {}",
        files.len(),
        wpj_ok,
        srv_ok,
        errored.len(),
        panicked.len()
    );
    println!(
        "  totals: {total_shots} shots, {total_sections} sections, {total_closures} closures, \
         {total_datums} zero-datums, {total_warnings} warnings"
    );
    by_shots.sort_by(|a, b| b.0.cmp(&a.0));
    println!("  top files by shot count:");
    for (shots, name) in by_shots.iter().take(12) {
        println!("    {shots:>8}  {name}");
    }
    for e in &errored {
        println!("  ERROR  {e}");
    }
    for p in &panicked {
        println!("  PANIC  {p}");
    }
    assert!(
        panicked.is_empty(),
        "parser panicked on {} file(s)",
        panicked.len()
    );
    assert!(
        errored.is_empty(),
        "parser errored on {} file(s)",
        errored.len()
    );
}

// ----- regression tests for review fixes -------------------------------

#[test]
fn quadrant_azimuth_parsed() {
    // Walls quadrant bearings must resolve, not silently fall through to 0.
    assert!(approx(
        parse_angle("N79E", AngleUnit::Degrees).unwrap(),
        79.0
    ));
    assert!(approx(
        parse_angle("S79W", AngleUnit::Degrees).unwrap(),
        259.0
    ));
    assert!(approx(
        parse_angle("N30W", AngleUnit::Degrees).unwrap(),
        330.0
    ));
    assert!(approx(
        parse_angle("S45E", AngleUnit::Degrees).unwrap(),
        135.0
    ));
    // DMS magnitude inside a quadrant bearing.
    assert!(approx(
        parse_angle("N12:30E", AngleUnit::Degrees).unwrap(),
        12.5
    ));
    // Magnitude honors the azimuth unit.
    assert!(approx(
        parse_angle("N100W", AngleUnit::Grads).unwrap(),
        270.0
    ));
    // Plain values and non-quadrant junk still behave.
    assert!(approx(parse_angle("90", AngleUnit::Degrees).unwrap(), 90.0));
    assert_eq!(parse_angle("NE", AngleUnit::Degrees), None);
    assert_eq!(parse_angle("N", AngleUnit::Degrees), None);
}

#[test]
fn feet_inches_sign_preserved() {
    // The whole feet-and-inches token shares the leading sign.
    assert!(approx(
        parse_length("-0i7", LengthUnit::Meters).unwrap(),
        -(7.0 / 12.0) * FEET_TO_METERS
    ));
    assert!(approx(
        parse_length("-2i6", LengthUnit::Meters).unwrap(),
        -2.5 * FEET_TO_METERS
    ));
    assert!(approx(
        parse_length("2i6", LengthUnit::Meters).unwrap(),
        2.5 * FEET_TO_METERS
    ));
    assert!(approx(
        parse_length("i7", LengthUnit::Meters).unwrap(),
        (7.0 / 12.0) * FEET_TO_METERS
    ));
}

#[test]
fn floated_variance_leg_is_kept() {
    // A real measured leg with a floated `(?)` variance is NOT a splay and
    // must survive the default import (include_splays = false).
    let srv = "#UNITS meters order=DAV\n\
               A B 10 90 0 (?)\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    assert_eq!(configs.len(), 1);
    assert_eq!(
        configs[0].shots.len(),
        1,
        "floated-variance leg must not be dropped as a splay"
    );
    assert!(!configs[0].shots[0].kind.is_virtual());
}

#[test]
fn fix_elevation_read_under_dive_order() {
    // `Order=DA` (no Up element) must NOT truncate the #Fix elevation column.
    let srv = "#UNITS meters ORDER=DA Tape=SS\n\
               #FIX A0 5 10 100\n\
               A0 B 8 90 5 5\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.fixes.len(), 1);
    assert_eq!(
        survey.fixes[0].up,
        Some(100.0),
        "elevation column must be read regardless of vector Order="
    );
}

#[test]
fn rect_vector_skips_declination() {
    // RECT E/N/U deltas already carry an absolute grid azimuth; an explicit
    // Decl must NOT rotate them.
    let srv = "#UNITS RECT order=ENU Decl=5\n\
               A B 0 10 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    let shot = &configs[0].shots[0];
    assert!(
        approx(shot.azimuth_in, 0.0),
        "rect azimuth must stay grid-north, got {}",
        shot.azimuth_in
    );
}

#[test]
fn rect_angle_rotates_bearing() {
    // `#Units RECT=90` rotates the E/N frame 90° clockwise from north, so a
    // vector along +N (E=0, N=10) reads 0° in-frame -> 90° after rotation.
    let srv = "#UNITS RECT=90 order=ENU\n\
               A B 0 10 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    let shot = &configs[0].shots[0];
    assert!(
        approx(shot.azimuth_in, 90.0),
        "RECT=90 must rotate the bearing to 90°, got {}",
        shot.azimuth_in
    );
}

#[test]
fn rect_frame_angle_does_not_leak_into_later_ct() {
    // `RECT=90` sets the rectangular frame bearing; switching to `CT` must NOT
    // carry that 90° in as a (negative) grid convergence on later CT shots -
    // `rect` and `grid` are distinct fields consumed with opposite signs.
    let srv = "#UNITS RECT=90 order=ENU\n\
               A B 0 10 0\n\
               #UNITS CT order=DAV\n\
               B C 10 100 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    let azimuths: Vec<f64> = configs
        .iter()
        .flat_map(|c| c.shots.iter().map(|s| s.azimuth_in))
        .collect();
    assert!(
        azimuths.iter().any(|&a| approx(a, 100.0)),
        "CT azimuth must stay 100° (RECT frame angle must not leak as convergence), got {azimuths:?}"
    );
    assert!(
        !azimuths.iter().any(|&a| approx(a, 10.0)),
        "no shot should be rotated to 10° by a leaked RECT angle, got {azimuths:?}"
    );
}

#[test]
fn units_grid_convergence_subtracted_from_ct() {
    // `#Units Grid=2` is a manual grid convergence: azimuth = reading(100) +
    // decl(0) - convergence(2) = 98, exactly like a `.REF` convergence.
    let srv = "#UNITS meters order=DAV Grid=2\n\
               A B 10 100 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    // #438: stored measured, and the grid convergence is REPORTED rather than
    // applied - see `convergence_from_a_survey_georef_is_reported_not_applied`.
    assert!(
        approx(configs[0].shots[0].azimuth_in, 100.0),
        "the measured reading must survive the import, got {}",
        configs[0].shots[0].azimuth_in
    );
    let reference = configs[0]
        .azimuth_reference
        .expect("the importer records a reference");
    assert!(
        reference.is_identity(),
        "#Units Grid=2 is not a declination, got {}",
        reference.applied_correction()
    );
}

#[test]
fn out_of_range_date_warns_on_declination_derive() {
    // The bundled IGRF model stops at 2025; a later survey date with the
    // derive flag on must warn instead of silently using 0° declination.
    let georef = Some(test_ref(0.0, 60.0, -150.0));
    let srv = "#DATE 2030-01-01\n#UNITS meters order=DAV\nA B 10 90 0\n";
    let (configs, _c, warnings) = convert_survey_with(
        parse_srv_geo(srv, georef.clone(), true),
        georef,
        &WallsImportOptions::default(),
    );
    assert!(
        approx(configs[0].shots[0].azimuth_in, 90.0),
        "out-of-range derive must fall back to 0° declination"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("declination") && w.contains("IGRF")),
        "an out-of-range derive date must warn, got {warnings:?}"
    );
}

#[test]
fn utm_to_wgs84_reference_lat_long_not_swapped() {
    // Zone 16N easting/northing from the `.REF` fixture -> lat ~20.43°N,
    // lon ~-87.31°. Confirms this importer returns (lat, lon) - a swap would
    // put lat at ~-87 (out of the [-90, 90] band) and be obviously wrong.
    let (lat, lon) = utm_to_wgs84(467427.0, 2258942.0, 16, "NAD83").unwrap();
    assert!((lat - 20.43).abs() < 0.1, "latitude {lat}");
    assert!((lon + 87.31).abs() < 0.1, "longitude {lon}");
}

#[test]
fn unparseable_decl_warns_and_keeps_state() {
    // A garbage Decl must warn and leave the current declination state
    // untouched, NOT pin an explicit 0 (which would suppress auto-derive).
    let mut units = WallsUnits::default();
    let mut stack = Vec::new();
    let mut warns = Vec::new();
    apply_units_option(&mut units, "decl", Some("3"), &mut stack, &mut warns);
    assert_eq!(units.decl, Some(3.0));
    apply_units_option(&mut units, "decl", Some("oops"), &mut stack, &mut warns);
    assert_eq!(
        units.decl,
        Some(3.0),
        "unparseable Decl must not overwrite a real value"
    );
    assert!(warns.iter().any(|w| w.contains("Decl")));
    // A bare `Decl=` clears back to auto-derive (None).
    apply_units_option(&mut units, "decl", Some(""), &mut stack, &mut warns);
    assert_eq!(units.decl, None);
}

#[test]
fn malformed_units_order_tape_lrud_warn() {
    // A rejected Order/Tape/LRUD value must warn (not silently keep defaults).
    let mut units = WallsUnits::default();
    let mut stack = Vec::new();
    let mut w = Vec::new();
    let default_order = units.ct_order.clone();
    apply_units_option(&mut units, "order", Some("DAX"), &mut stack, &mut w);
    apply_units_option(&mut units, "tape", Some("ZZ"), &mut stack, &mut w);
    apply_units_option(&mut units, "lrud", Some("F:XYZW"), &mut stack, &mut w);
    assert!(
        w.iter().any(|m| m.contains("Order")),
        "bad Order must warn: {w:?}"
    );
    assert!(
        w.iter().any(|m| m.contains("Tape")),
        "bad Tape must warn: {w:?}"
    );
    assert!(
        w.iter().any(|m| m.contains("LRUD")),
        "bad LRUD must warn: {w:?}"
    );
    // Rejected values leave the prior state intact.
    assert_eq!(units.ct_order, default_order);
    assert_eq!(units.lrud_order, ['L', 'R', 'U', 'D']);
    assert_eq!(units.tape_from, TapeEnd::Instrument);
    assert_eq!(units.tape_to, TapeEnd::Target);
}

#[test]
fn zero_length_bearing_leg_kept() {
    // `A B 0 90 0`: zero tape but a real bearing - keep it as a tie so the
    // endpoints stay connected instead of splitting into components.
    let srv = "#UNITS meters order=DAV\n\
               A B 0 90 0\n\
               B C 10 90 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    assert_eq!(configs.len(), 1, "endpoints must stay in one component");
    let total: usize = configs.iter().map(|c| c.shots.len()).sum();
    assert_eq!(total, 2, "zero-length bearing leg must be kept");
}

#[test]
fn fix_coordinates_survive_inline_comment() {
    // A `;` comment with no leading space must not swallow the northing.
    let georef = test_ref(0.0, 20.0, -90.0);
    let survey = parse_srv_geo(
        "#UNITS meters order=DAV\n#FIX A0 467427 2258942;datum tie\nA0 B 10 90 0\n",
        Some(georef),
        false,
    );
    assert_eq!(survey.fixes.len(), 1);
    assert!(
        survey.fixes[0].lat_long.is_some(),
        "both UTM coords must survive a `;`-comment with no leading space"
    );
}

#[test]
fn preview_excludes_out_of_range_fix() {
    // An out-of-range geographic #Fix is rejected by resolve_anchors, so the
    // preview's fixed_station_count must not count it either.
    let srv = "#UNITS meters order=DAV\n\
               #FIX GOOD N30:00:00 W90:00:00 0\n\
               #FIX BAD N95:00:00 W30:00:00 0\n\
               GOOD BAD 10 90 0\n";
    let parsed = ParsedWalls {
        project_name: "t".to_string(),
        book_count: 0,
        surveys: vec![parse_srv(srv)],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let preview = build_preview(&parsed);
    assert_eq!(
        preview.fixed_station_count, 1,
        "only the in-range #Fix should be counted"
    );
}

#[test]
fn dive_station_keeps_own_gauge_depth() {
    // S1 reads 5 on the first leg but 6 on the second (gauge noise). Each
    // station's own authoritative gauge reading wins over an integrated one.
    let srv = "#UNITS meters ORDER=DA Tape=SS\n\
               S0 S1 10 90 0 5\n\
               S1 S2 10 90 6 9\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    let sec = &configs[0];
    assert_eq!(sec.shots.len(), 2);
    assert!(approx(sec.shots[1].depth_in, 5.0));
    assert!(
        approx(sec.shots[1].depth_out, 9.0),
        "S2 must keep its own gauge depth (9), not an integrated value, got {}",
        sec.shots[1].depth_out
    );
}

// ----- regression tests for the second review round -------------------

#[test]
fn two_digit_year_dates_parse_to_full_year() {
    // chrono's `%Y` must NOT greedily eat a 2-digit leading field: `05/07/17`
    // is year 17, not year 0005. Genuine Walls maps every 2-digit year into
    // 1900-1999 (`y += 1900`), so `17` => 1917 (NOT 2017).
    assert_eq!(
        parse_walls_date("05/07/17"),
        NaiveDate::from_ymd_opt(1917, 5, 7)
    );
    assert_eq!(
        parse_walls_date("5-7-99"),
        NaiveDate::from_ymd_opt(1999, 5, 7)
    );
    // 4-digit ISO and US forms still parse.
    assert_eq!(
        parse_walls_date("2017-05-07"),
        NaiveDate::from_ymd_opt(2017, 5, 7)
    );
    assert_eq!(
        parse_walls_date("05/07/2017"),
        NaiveDate::from_ymd_opt(2017, 5, 7)
    );
}

#[test]
fn rect_vector_honors_units_grid() {
    // `#Units RECT Grid=2`: the manual grid convergence applies to a RECT leg
    // exactly like CT - a bearing along +N (0°) becomes 0 - 2 = 358°.
    let srv = "#UNITS RECT order=ENU Grid=2\n\
               A B 0 10 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    // #438: a RECT vector's bearing comes from its E/N components, so it is
    // already a true-north bearing in the file's own Cartesian frame - nothing
    // to correct, and nothing for the grid convergence to do here either.
    assert!(
        approx(configs[0].shots[0].azimuth_in, 0.0),
        "the RECT-derived bearing must be stored as measured, got {}",
        configs[0].shots[0].azimuth_in
    );
    let reference = configs[0]
        .azimuth_reference
        .expect("the importer records a reference");
    assert_eq!(
        reference.north,
        crate::models::NorthReference::True,
        "a RECT bearing is not a compass reading"
    );
    assert!(reference.is_identity());
}

#[test]
fn lrud_to_shifts_cross_section_forward() {
    // `#Units LRUD=To`: the LRUD on leg A->B is measured at B, so it must ride
    // on the next shot (B->C), not on A->B.
    let srv = "#UNITS meters order=DAV LRUD=To\n\
               A B 10 90 0 <1,2,3,4>\n\
               B C 10 90 0 <5,6,7,8>\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    let shots = &configs[0].shots;
    assert_eq!(shots.len(), 2);
    assert_eq!(
        shots[0].left, None,
        "A->B must not carry B's LRUD as its own"
    );
    assert_eq!(
        shots[1].left,
        Some(1.0),
        "B's measured LRUD must shift forward onto B->C"
    );
}

#[test]
fn parse_ref_rejects_out_of_range_zone() {
    // A garbage UTM zone must be rejected, not silently saturated by `as i32`.
    let bad = "100 200 999 0 0 0 31 0 0 97 0 0 \"WGS84\"";
    assert!(parse_ref(bad).is_err(), "zone 999 must be rejected");
    let ok = "100 200 16 0 0 0 31 0 0 97 0 0 \"WGS84\"";
    assert!(parse_ref(ok).is_ok(), "a valid .REF must still parse");
}

#[test]
fn inch_not_applied_to_zero_length_bearing_tie() {
    // `#Units IncH=5` plus a zero-length bearing tie (A B 0 90) must NOT push
    // a 5 m vertical offset onto the tied station.
    let srv = "#UNITS meters order=DAV IncH=5\n\
               A B 0 90 0\n\
               B C 10 90 0\n";
    let (configs, _c, _w) = convert_one(parse_srv(srv));
    assert!(
        approx(configs[0].shots[0].depth_out, 0.0),
        "zero-length tie must not pick up an IncH offset, got {}",
        configs[0].shots[0].depth_out
    );
}

#[test]
fn rect_plumb_shot_has_no_spurious_bearing() {
    // A pure-vertical RECT shot (E=N=0, U=10) has no defined bearing; it must
    // be left unset, not recorded as 0° (due north).
    let srv = "#UNITS RECT order=ENU\n\
               A B 0 0 10\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.vectors.len(), 1);
    assert!(
        survey.vectors[0].azimuth_in.is_none(),
        "plumb RECT shot must have no azimuth"
    );
    assert!(
        survey.vectors[0]
            .inc_deg
            .map(|i| i.abs() > 89.0)
            .unwrap_or(false),
        "plumb RECT shot must be ~vertical"
    );
}

#[test]
fn preview_shot_count_excludes_splays() {
    // A survey of only anonymous-wall splays imports nothing by default, so
    // the preview shot count must be 0 - not the raw vector count.
    let srv = "#UNITS meters order=DAV\n\
               A - 3 90 0\n\
               A - 4 180 0\n";
    let parsed = ParsedWalls {
        project_name: "t".to_string(),
        book_count: 0,
        surveys: vec![parse_srv(srv)],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let preview = build_preview(&parsed);
    assert_eq!(
        preview.shot_count, 0,
        "a splay-only survey must preview 0 importable shots"
    );
    // ...but the splays are still importable with "Include splays" enabled,
    // so the dialog can offer import rather than greying the button out.
    assert_eq!(
        preview.splay_count, 2,
        "both anonymous-wall splays must be counted as opt-in importable"
    );
}

#[test]
fn gross_gauge_contradiction_warns() {
    // S1's first gauge reading (5) and a later one (50) on the same station
    // disagree grossly; the gauge still wins but the discontinuity must warn.
    let srv = "#UNITS meters ORDER=DA Tape=SS\n\
               S0 S1 10 90 0 5\n\
               S1 S2 10 90 50 9\n";
    let (_configs, _c, warnings) = convert_one(parse_srv(srv));
    assert!(
        warnings.iter().any(|w| w.contains("disagrees")),
        "a gross gauge/integrated contradiction must warn, got {warnings:?}"
    );
}

// ----- regression tests for the second code-review pass -----------------

#[test]
fn prefix_leading_colon_and_level_order_match_walls() {
    // dewalls `processStationName`: the explicit ':' count suppresses that
    // many of the LOWEST inherited prefix levels, and remaining levels are
    // emitted outermost-first (Prefix2:Prefix1:name). A leading colon cancels
    // the inherited Prefix1.
    let mut u = WallsUnits::default();
    set_prefix(&mut u, 0, "CAVE"); // Prefix1
    assert_eq!(normalize_station("A1", &u), "CAVE:A1");
    assert_eq!(normalize_station(":A1", &u), "A1");
    set_prefix(&mut u, 1, "AREA"); // Prefix2
    assert_eq!(normalize_station("A1", &u), "AREA:CAVE:A1");
    // One explicit segment suppresses only Prefix1, keeping Prefix2.
    assert_eq!(normalize_station("X:STN", &u), "AREA:X:STN");
}

#[test]
fn ct_after_rect_restores_order_and_keeps_shots() {
    // `#Units RECT` set order to E/N/U; a later bare `#Units CT` must restore
    // the CT order so the following compass-and-tape shot survives parsing.
    let srv = "#UNITS RECT\n#UNITS CT\nA B 10 90 0\n";
    let survey = parse_srv(srv);
    assert_eq!(
        survey.vectors.len(),
        1,
        "a CT shot after a RECT block must not be dropped"
    );
    assert!(approx(survey.vectors[0].length_m, 10.0));
}

#[test]
fn custom_ct_order_survives_rect_round_trip() {
    // ct_order and rect_order are independent (dewalls): a custom CT
    // `Order=AVD` set while in RECT must still govern the CT shot, and must
    // NOT be clobbered back to the default by the CT switch.
    let srv = "#UNITS RECT\n#UNITS Order=AVD CT\nA B 90 0 10\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.vectors.len(), 1);
    let v = &survey.vectors[0];
    // AVD => azimuth, inclination, distance columns.
    assert!(
        approx(v.length_m, 10.0),
        "distance must be the 3rd (D) column per AVD, got {}",
        v.length_m
    );
    assert_eq!(
        v.azimuth_in,
        Some(90.0),
        "azimuth must be the 1st (A) column per AVD"
    );
}

#[test]
fn project_macros_available_to_srv_body() {
    // A macro defined in the project/book `.OPTIONS` must expand in the child
    // `.SRV` body (regression: it was parsed then discarded).
    let mut initial_macros = HashMap::new();
    initial_macros.insert("cave".to_string(), "ABC".to_string());
    let mut warnings = Vec::new();
    let survey = parse_srv_contents(
        "#UNITS meters order=DAV\n$(cave)1 $(cave)2 10 0 0\n",
        "t",
        "t",
        "",
        &WallsUnits::default(),
        &initial_macros,
        None,
        true,
        false,
        &mut warnings,
    );
    assert_eq!(
        survey.vectors.len(),
        1,
        "a shot referencing a project macro must import, warnings: {warnings:?}"
    );
    assert_eq!(survey.vectors[0].from, "ABC1");
    assert_eq!(survey.vectors[0].to, "ABC2");
}

#[test]
fn malformed_date_keeps_previous_valid() {
    // A later unparseable `#Date` must not clobber an earlier valid one.
    let srv = "#DATE 2017-05-07\n#DATE 0000-99-99\n#UNITS meters order=DAV\nA B 10 0 0\n";
    let survey = parse_srv(srv);
    assert_eq!(survey.date, NaiveDate::from_ymd_opt(2017, 5, 7));
}

#[test]
fn backsight_only_zero_length_tie_kept() {
    // A zero-length leg carrying only a backsight bearing is a coincident-
    // station tie and must not be dropped (the guard checked only foresight).
    let srv = "#UNITS meters order=DAV\nA B 0 /200\n";
    let survey = parse_srv(srv);
    assert_eq!(
        survey.vectors.len(),
        1,
        "a backsight-only zero-length tie must be kept"
    );
}

#[test]
fn split_units_args_keeps_quoted_spans() {
    // Quoted `#Units` values with embedded spaces must stay single tokens.
    let toks = split_units_args("feet decl=0.0 $foo=\"meters decl\" Flag=\"loose rock\"");
    assert_eq!(
        toks,
        vec![
            "feet",
            "decl=0.0",
            "$foo=\"meters decl\"",
            "Flag=\"loose rock\""
        ]
    );
}

#[test]
fn quoted_units_value_emits_no_spurious_warning() {
    let mut warnings = Vec::new();
    let survey = parse_srv_contents(
        "#UNITS meters order=DAV Flag=\"loose rock\"\nA B 10 0 0\n",
        "t",
        "t",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        None,
        true,
        false,
        &mut warnings,
    );
    assert!(
        !warnings.iter().any(|w| w.contains("Unknown #units option")),
        "a quoted value with a space must not trip the unknown-option arm: {warnings:?}"
    );
    assert_eq!(survey.vectors.len(), 1);
}

#[test]
fn contiguous_self_loop_emits_closure() {
    // A->B->C->A in one survey is a single chain whose revisited station must
    // get an intra-section loop closure (regression: it was dropped).
    let srv = "#UNITS meters order=DAV\nA B 10 90 0\nB C 10 0 0\nC A 14 200 0\n";
    let (_configs, closures, _w) = convert_one(parse_srv(srv));
    assert!(
        closures
            .iter()
            .any(|c| c.from_section_index == c.to_section_index
                && c.from_station_index == 3
                && c.to_station_index == 0),
        "an intra-chain loop must emit a same-section closure, got {closures:?}"
    );
}

#[test]
fn second_fix_anchor_depth_disagreement_warns() {
    // Two #Fix in one connected component whose elevations disagree with the
    // integrated geometry must warn (regression: the 2nd anchor was ignored).
    let srv = "#UNITS meters order=DAV\n\
               #FIX AA W87:18:44 N20:25:42 100\n\
               #FIX ZZ W87:18:44 N20:25:42 50\n\
               AA ZZ 10 90 0\n";
    let (_c, _cl, warnings) = convert_one(parse_srv(srv));
    assert!(
        warnings.iter().any(|w| w.contains("fixed-station depth")),
        "a disagreeing second fixed-station depth must warn: {warnings:?}"
    );
    // ...but the measured shot geometry must be preserved (the 2nd anchor is
    // a consistency check, not a second datum) - a 10 m flat shot must not be
    // stretched by a fabricated 50 m vertical, so no length clamp fires.
    assert!(
        !warnings.iter().any(|w| w.contains("clamped")),
        "honoring the 2nd anchor must not corrupt shot length: {warnings:?}"
    );
}

#[test]
fn dive_leg_missing_one_depth_warns() {
    // A depth-gauge leg with only one depth column integrates as flat; that
    // estimate must be surfaced, not asserted silently.
    let srv = "#UNITS meters ORDER=DA Tape=SS\nA B 21.1 339 13 --\n";
    let (_c, _cl, warnings) = convert_one(parse_srv(srv));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("missing a depth reading")),
        "a single-depth dive leg must warn: {warnings:?}"
    );
}

#[test]
fn two_digit_year_maps_to_1900s_like_walls() {
    // Genuine Walls (`wall-srv.cpp` ParseDate) does `if (y < 100) y += 1900`,
    // so EVERY 2-digit year lands in 1900-1999 - never the 2000s, never the
    // future. `64` => 1964, `17` => 1917, `00` => 1900, `30` => 1930.
    for (s, y) in [
        ("03-15-64", 1964),
        ("05/07/17", 1917),
        ("1-1-00", 1900),
        ("6-6-30", 1930),
    ] {
        assert_eq!(
            parse_walls_date(s).map(|d| d.year()),
            Some(y),
            "{s} should map to {y}"
        );
    }
}

#[test]
fn two_digit_leap_day_stays_2000() {
    // The one arithmetic exception to "every 2-digit year → 1900s": 1900-02-29
    // does not exist (1900 was not a leap year), so `02-29-00` keeps its only
    // valid interpretation, 2000-02-29, rather than being forced to fail.
    assert_eq!(
        parse_walls_date("02-29-00"),
        NaiveDate::from_ymd_opt(2000, 2, 29)
    );
}

#[test]
fn parse_ref_rejects_infinite_field() {
    // An over-large numeric parses to +inf; it must be rejected, not turned
    // into NaN azimuths downstream.
    let line = "0 0 16 1e400 0 0 20 25 42 87 18 44";
    assert!(parse_ref(line).is_err());
}

// ----- byte-order marks -----------------------------------------------

/// The `EF BB BF` a Windows editor writes at the head of a "UTF-8 with BOM"
/// file - Notepad's default, PowerShell 5.1 `Out-File`, a common VS Code
/// setting.
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

fn write_prefixed(path: &Path, prefix: &[u8], text: &str) {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(path, bytes).unwrap();
}

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// `read_cp1252` maps each byte to one char, so a BOM arrives as three
/// Windows-1252 chars glued to line 1 and its directive stops being one. Without
/// the strip the `#DATE` here is dropped with no warning at all, and a file
/// whose line 1 is `#UNITS` or `#FIX` silently imports in the wrong units or
/// at the wrong place.
#[test]
fn a_utf8_bom_does_not_void_the_first_srv_directive() {
    let dir = temp_dir("srvbom");
    let srv = "#DATE 2026-08-05\r\n#UNITS meters ORDER=DA Tape=SS\r\nA\tB\t100.0\t90\t0\t0\r\n";

    let plain = dir.join("plain.srv");
    std::fs::write(&plain, srv).unwrap();
    let marked = dir.join("bom.srv");
    write_prefixed(&marked, &UTF8_BOM, srv);

    let expected = parse_walls(&plain).unwrap();
    assert_eq!(
        expected.surveys[0].date,
        NaiveDate::from_ymd_opt(2026, 8, 5),
        "premise: the same file without a BOM parses its date"
    );
    assert_eq!(expected.surveys[0].vectors.len(), 1);

    let parsed = parse_walls(&marked).unwrap();
    assert_eq!(
        parsed.surveys[0].date, expected.surveys[0].date,
        "the BOM must not swallow the first directive"
    );
    assert_eq!(parsed.surveys[0].vectors.len(), 1);
    assert!(
        parsed.warnings.is_empty(),
        "a stripped BOM is not worth a warning: {:?}",
        parsed.warnings
    );
}

/// The `.WPJ` reader splits the same Latin-1 text, so its line-1 `.BOOK` is
/// lost the same way, taking the project name and the book count with it.
#[test]
fn a_utf8_bom_does_not_void_the_first_wpj_directive() {
    let dir = temp_dir("wpjbom");
    std::fs::write(dir.join("S.SRV"), "#UNITS meters order=DAV\nA B 10 90 0\n").unwrap();
    let wpj = ".BOOK Test Project\n.NAME TESTPROJ\n.STATUS 8\n\
               .SURVEY First Survey\n.NAME S\n.STATUS 8\n\
               .ENDBOOK\n";

    let plain = dir.join("plain.wpj");
    std::fs::write(&plain, wpj).unwrap();
    let marked = dir.join("bom.wpj");
    write_prefixed(&marked, &UTF8_BOM, wpj);

    let expected = parse_walls(&plain).unwrap();
    assert_eq!(expected.project_name, "Test Project");
    assert_eq!(expected.book_count, 1);

    let parsed = parse_walls(&marked).unwrap();
    assert_eq!(parsed.project_name, expected.project_name);
    assert_eq!(parsed.book_count, expected.book_count);
    assert_eq!(parsed.surveys.len(), expected.surveys.len());
}

/// A UTF-16 file decodes to NUL-separated bytes that match no directive at
/// all, so every line is skipped and the import comes back empty and silent.
/// Refuse it by name instead.
#[test]
fn a_utf16_srv_is_refused_by_name() {
    let dir = temp_dir("srvutf16");
    let p = dir.join("u16.srv");
    std::fs::write(&p, utf16le("#DATE 2026-08-05\r\nA B 10 90 0\r\n")).unwrap();

    let err = parse_walls(&p)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("UTF-16"),
        "the error must name the encoding, got {err:?}"
    );
}

#[test]
fn lrud_to_end_of_chain_cross_section_is_reported() {
    // #576 B31: with `LRUD=To` the last leg's section is measured at a station
    // no shot leaves, so the model has nowhere to put it. It is dropped - and
    // the receipt now says so, in the same words as the Compass importer.
    let srv = "#UNITS meters order=DAV LRUD=To\n\
               A B 10 90 0 <1,2,3,4>\n\
               B C 10 90 0 <5,6,7,8>\n";
    let (configs, _c, warnings) = convert_one(parse_srv(srv));
    println!("warnings: {warnings:?}");
    assert_eq!(
        configs[0].shots[1].left,
        Some(1.0),
        "premise: B's section moved"
    );
    assert!(
        warnings.iter().any(|w| w
            == "Survey 'Test Survey': 1 end-of-chain cross-section had no shot to attach to \
                and was dropped (the survey records LRUDs at each shot's To station)."),
        "{warnings:?}"
    );

    // A From survey loses nothing and says nothing.
    let srv = "#UNITS meters order=DAV\n\
               A B 10 90 0 <1,2,3,4>\n\
               B C 10 90 0 <5,6,7,8>\n";
    let (_configs, _c, warnings) = convert_one(parse_srv(srv));
    assert!(
        warnings.iter().all(|w| !w.contains("cross-section")),
        "{warnings:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_named_like_a_srv_is_refused_at_once() {
    // #576 B27(g): Walls reads through the same capped, regular-files-only
    // reader as Compass, so a FIFO named like a `.SRV` no longer blocks the
    // import (and the app's main thread) forever.
    let dir = temp_dir("fifo-srv");
    let fifo = dir.join("pipe.srv");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success(), "premise: mkfifo");
    let (sender, receiver) = std::sync::mpsc::channel();
    let path = fifo.clone();
    let worker = std::thread::spawn(move || {
        let _ = sender.send(parse_walls(&path).map(|_| ()).map_err(|e| e.to_string()));
    });
    let outcome = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .ok();
    println!("parse_walls(pipe.srv) within 5 s: {outcome:?}");
    if outcome.is_none() {
        // Still blocked: open the FIFO for writing so the reader sees an
        // empty file and the thread can finish.
        drop(std::fs::OpenOptions::new().write(true).open(&fifo));
    }
    let _ = worker.join();
    let err = outcome
        .expect("the read must not block on a FIFO")
        .expect_err("a FIFO is not a survey file");
    assert!(err.contains("not a regular file"), "{err}");
}

// ----- LRUD closer followed by a no-break space (#576 WALLS-LRUD) ----------

/// Decision WALLS-LRUD-D1 ("only trailing NBSP closes"): an LRUD closer counts
/// when it is followed by end of line, an ASCII separator, or nothing but
/// no-break spaces (and separators) to the end of the line. Anywhere else a
/// U+00A0 joins the fields it sits between, as the tokenizer reads it, so a
/// station named `*A*<NBSP>B` is one name, not LRUD `A` plus station
/// `<NBSP>B`.
#[test]
fn an_lrud_closer_followed_by_a_no_break_space_closes_only_at_the_end_of_the_line() {
    let cases: [(&str, &[&str], Option<&str>); 6] = [
        // A name that starts with an LRUD delimiter and has a U+00A0 after
        // its "closer" is a name.
        (
            "*A*\u{a0}B C 10 0 0",
            &["*A*\u{a0}B", "C", "10", "0", "0"],
            None,
        ),
        (
            "<A>\u{a0}B C 10 0 0",
            &["<A>\u{a0}B", "C", "10", "0", "0"],
            None,
        ),
        // ... and so is one whose U+00A0 is followed by a separator.
        (
            "*A*\u{a0} B 10 0 0",
            &["*A*\u{a0}", "B", "10", "0", "0"],
            None,
        ),
        // An LRUD at the end of the line with a trailing U+00A0 (or a U+00A0
        // and a space) still closes, so it is not lost.
        (
            "A B 10 0 0 *1 2 3 4*\u{a0}",
            &["A", "B", "10", "0", "0"],
            Some("1 2 3 4"),
        ),
        (
            "A B 10 0 0 <1 2 3 4>\u{a0}\u{a0} ",
            &["A", "B", "10", "0", "0"],
            Some("1 2 3 4"),
        ),
        // The plain case is unchanged.
        (
            "A B 10 0 0 *1 2 3 4*",
            &["A", "B", "10", "0", "0"],
            Some("1 2 3 4"),
        ),
    ];
    let outcomes: Vec<_> = cases
        .iter()
        .map(|&(body, tokens, lrud)| {
            let got = extract_spans(body);
            println!("{body:?} -> tokens={:?} lrud={:?}", got.tokens, got.lrud);
            (body, tokens, lrud, got)
        })
        .collect();
    for (body, tokens, lrud, got) in outcomes {
        assert_eq!(got.tokens, tokens, "{body:?}");
        assert_eq!(got.lrud.as_deref(), lrud, "{body:?}");
    }
}

// ----- .PATH policy, the .MAK rules (#576 SEC-1, NF-AN4-6, NF-W2C-2) -------

/// A one-book `.wpj` in `proj` whose book's `.PATH` is `path_line` and whose
/// survey `S` reads `S.SRV` from there.
fn wpj_with_path(proj: &Path, path_line: &str) -> PathBuf {
    let wpj = format!(
        ";WALLS Project File\n\
         .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
         .PATH {path_line}\n\
         .SURVEY S\n.NAME S\n.STATUS 8\n\
         .ENDBOOK\n"
    );
    let wpj_path = proj.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    wpj_path
}

/// Decision SEC-1 ("same as .MAK"): a network (UNC) or device-namespace
/// `.PATH` is refused from its text alone, before anything asks the file
/// system about it - on Windows even checking whether `\\server\share\x`
/// exists hands the user's credentials to that server. On macOS and Linux a
/// path starting with two slashes names the local root, so a `.PATH` of
/// `//<local dir>` proves the point: it used to be READ (the survey
/// imported), and is refused now, with the reason named.
#[cfg(unix)]
#[test]
fn a_network_or_device_path_is_refused_before_it_is_touched() {
    let proj = temp_dir("unc_proj");
    let local = temp_dir("unc_local");
    std::fs::write(
        local.join("S.SRV"),
        "#UNITS meters order=DAV\nA B 10 90 0\n",
    )
    .unwrap();
    let local_str = local.display().to_string();
    for path_line in [
        format!("/{local_str}"),
        format!("\\{}", local_str.replace('/', "\\")),
        "\\\\server\\share\\caves".to_string(),
        "\\\\?\\C:\\caves".to_string(),
        "\\\\.\\COM1".to_string(),
    ] {
        let parsed = parse_walls(&wpj_with_path(&proj, &path_line)).unwrap();
        let read = parsed.surveys.iter().any(|s| s.resolved);
        println!(
            ".PATH {path_line}: survey read={read}, missing={:?}, warnings={:?}",
            parsed.missing_files, parsed.warnings
        );
        assert!(!read, ".PATH {path_line} must not be read");
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.contains("survey 'S' was skipped")
                    && w.contains("network (UNC) and device paths are never read")),
            ".PATH {path_line}: {:?}",
            parsed.warnings
        );
        assert_eq!(parsed.missing_files.len(), 1, "named as not imported");
    }
}

/// NF-W2C-2: a drive-letter `.PATH` is absolute only on Windows. Elsewhere
/// `D:\Caves` resolved as the RELATIVE path `D:/Caves`, i.e. against the
/// process's working directory - whatever folder the app was started from.
/// It is refused instead, as the `.MAK` reader refuses it.
#[cfg(not(windows))]
#[test]
fn a_drive_letter_path_does_not_resolve_against_the_working_directory() {
    let proj = temp_dir("drive_proj");
    for path_line in ["D:\\Caves", "D:Caves"] {
        let parsed = parse_walls(&wpj_with_path(&proj, path_line)).unwrap();
        println!(
            ".PATH {path_line}: missing={:?}, warnings={:?}",
            parsed.missing_files, parsed.warnings
        );
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.contains("survey 'S' was skipped")
                    && w.contains("its drive letter cannot be resolved on this system")),
            ".PATH {path_line}: {:?}",
            parsed.warnings
        );
        assert!(
            parsed.missing_files.iter().all(|m| !m.starts_with("D:")),
            "no working-directory-relative path may be looked up: {:?}",
            parsed.missing_files
        );
    }
}

/// A refused `.PATH` covers the books and surveys under it whose own `.PATH`
/// is relative; an absolute local `.PATH` below it replaces the base again
/// and is read (with the outside-the-project warning).
#[cfg(unix)]
#[test]
fn a_refused_path_covers_its_relative_children_only() {
    let proj = temp_dir("refused_proj");
    let local = temp_dir("refused_local");
    std::fs::write(
        local.join("S.SRV"),
        "#UNITS meters order=DAV\nA B 10 90 0\n",
    )
    .unwrap();
    std::fs::write(
        local.join("T.SRV"),
        "#UNITS meters order=DAV\nC D 10 90 0\n",
    )
    .unwrap();
    let wpj = format!(
        ";WALLS Project File\n\
         .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
         .PATH \\\\server\\share\n\
         .BOOK Inner\n.NAME INNER\n.STATUS 8\n.PATH sub\n\
         .SURVEY S\n.NAME S\n.STATUS 8\n\
         .ENDBOOK\n\
         .BOOK Local\n.NAME LOCAL\n.STATUS 8\n.PATH {}\n\
         .SURVEY T\n.NAME T\n.STATUS 8\n\
         .ENDBOOK\n\
         .ENDBOOK\n",
        local.display()
    );
    let wpj_path = proj.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    let parsed = parse_walls(&wpj_path).unwrap();
    let resolved: Vec<(&str, bool)> = parsed
        .surveys
        .iter()
        .map(|s| (s.name.as_str(), s.resolved))
        .collect();
    println!("surveys read: {resolved:?}, warnings={:?}", parsed.warnings);
    assert_eq!(
        resolved,
        [("S", false), ("T", true)],
        "{:?}",
        parsed.warnings
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("survey 'S' was skipped")
                && w.contains("network (UNC) and device paths are never read")),
        "the relative child inherits the refusal: {:?}",
        parsed.warnings
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("survey 'T' is read from outside the project directory")),
        "{:?}",
        parsed.warnings
    );
}

// ----- .WPJ book nesting: shared state, refused past 256 (#576 B12) --------

/// A `.wpj` of `depth` nested books, each with `.NAME N<k>` (a segment level)
/// and the `.OPTIONS` `options(k)`, around one survey `S`.
fn nested_wpj(dir: &Path, depth: usize, options: impl Fn(usize) -> String) -> PathBuf {
    use std::fmt::Write as _;
    let mut wpj = String::from(";WALLS Project File\n");
    for k in 1..=depth {
        write!(
            wpj,
            ".BOOK B{k}\n.NAME N{k}\n.STATUS 8\n.OPTIONS {}\n",
            options(k)
        )
        .unwrap();
    }
    wpj.push_str(".SURVEY S\n.NAME S\n.STATUS 8\n");
    for _ in 0..depth {
        wpj.push_str(".ENDBOOK\n");
    }
    std::fs::write(dir.join("S.SRV"), "A B 10 0 0\n").unwrap();
    let path = dir.join("Deep.wpj");
    std::fs::write(&path, wpj).unwrap();
    path
}

/// Decision B12-D1: a `.WPJ` nesting books deeper than 256 levels - deeper
/// than any real project - fails the import with a message naming the limit.
/// Each level used to copy every ancestor's options and segment, so a few
/// hundred kilobytes of nested books could exhaust memory.
#[test]
fn a_wpj_nested_deeper_than_256_books_is_refused() {
    let dir = temp_dir("deep_refused");
    let outcome = parse_walls(&nested_wpj(&dir, 300, |_| "decl=1".to_string()))
        .map(|p| (p.book_count, p.surveys.len()))
        .map_err(|e| e.to_string());
    println!("300 nested books: {outcome:?}");
    let err = outcome.expect_err("300 nested books must be refused");
    assert!(
        err.contains("more than 256 levels deep") && err.contains("'B257'"),
        "{err}"
    );

    let dir = temp_dir("deep_accepted");
    let parsed = parse_walls(&nested_wpj(&dir, 256, |_| "decl=1".to_string())).unwrap();
    assert_eq!(parsed.book_count, 256, "exactly 256 levels still import");
}

/// #576 RV3-R-NITS (B12): only `.ENDBOOK` closes a book, so a malformed `.WPJ`
/// that leaves them out nests every SIBLING book inside the one before it and
/// is refused past 256 books - where it used to import, nested wrongly. Walls
/// itself writes `.ENDBOOK`, so this stays refused, but the message has to name
/// the likelier cause instead of only a nesting depth nobody wrote.
#[test]
fn a_wpj_missing_endbook_lines_is_refused_naming_them() {
    use std::fmt::Write as _;
    let dir = temp_dir("no_endbook");
    let mut wpj = String::from(";WALLS Project File\n");
    for k in 1..=300 {
        write!(wpj, ".BOOK B{k}\n.NAME N{k}\n.STATUS 8\n").unwrap();
    }
    let path = dir.join("Flat.wpj");
    std::fs::write(&path, wpj).unwrap();
    let err = parse_walls(&path)
        .map(|p| p.book_count)
        .unwrap_err()
        .to_string();
    println!("300 books, no .ENDBOOK: {err}");
    assert!(
        err.contains("more than 256 levels deep") && err.contains(".ENDBOOK"),
        "{err}"
    );
}

/// Sharing the ancestors' state changes nothing about what a survey
/// inherits: every ancestor's options apply root first (the last `decl` and
/// the later `meters` win over the root's `feet`), and the segment is every
/// naming level joined.
#[test]
fn a_deep_survey_inherits_every_ancestor_in_order() {
    let dir = temp_dir("deep_inherit");
    let parsed = parse_walls(&nested_wpj(&dir, 200, |k| match k {
        1 => "feet decl=1".to_string(),
        150 => format!("meters decl={k}"),
        _ => format!("decl={k}"),
    }))
    .unwrap();
    let survey = &parsed.surveys[0];
    let expected_segment = (1..=200)
        .map(|k| format!("N{k}"))
        .chain(std::iter::once("S".to_string()))
        .collect::<Vec<_>>()
        .join("/");
    assert_eq!(survey.segment, expected_segment);
    let v = &survey.vectors[0];
    assert_eq!(v.units_decl, Some(200.0), "the deepest decl wins");
    assert!(
        approx(v.length_m, 10.0),
        "meters (level 150) beats feet (level 1)"
    );
}
