//! Generates `data.rs` (the `MODELS` table `lib.rs` includes) from IAGA's
//! published IGRF coefficient table, `data/igrf14coeffs.txt`.
//!
//! Stalagmight fork: upstream parsed NOAA's geomag70 `IGRF13.COF`. NOAA's
//! geomag70 package was not updated for IGRF-14, so this reads the IAGA table
//! itself - the file NOAA publishes at
//! <https://www.ngdc.noaa.gov/IAGA/vmod/coeffs/igrf14coeffs.txt>, committed
//! verbatim. It emits the same `MODELS` table the COF parser did: fed
//! `igrf13coeffs.txt`, it reproduces upstream's generated `data.rs` exactly
//! (see MODIFICATIONS.md). The next generation is a file swap plus the
//! `COEFFS` path below.
//!
//! Table layout: `#` comment lines; a `c/s deg ord` row naming each column's
//! model kind (IGRF or DGRF); a `g/h n m` row giving each column's epoch, with
//! a final secular-variation column (`2025-30`); then one row per coefficient,
//! `g n m v1 v2 ...` or `h n m v1 v2 ...`, ordered by degree n, then order m,
//! with g before h.

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::str::FromStr;

const COEFFS: &str = include_str!("./data/igrf14coeffs.txt");

struct FieldModel {
    name: String,
    /// Epoch of model.
    epoch: f64,
    /// Min year of model.
    yrmin: f64,
    /// Max year of model.
    yrmax: f64,
    /// Minimum height of each model.
    altmin: f64,
    /// Maximum height of each model.
    altmax: f64,
    /// Main field coefficient.
    max1: usize,
    /// Secular variation coefficient.
    max2: usize,
    /// Acceleration coefficient.
    max3: usize,
    /// Schmidt quasi-normal internal spherical harmonic coeff.
    gh1: Vec<f64>,
    /// Schmidt quasi-normal internal spherical harmonic coeff.
    gh2: Vec<f64>,
}

/// One coefficient row: its degree and the value in every column (each model
/// epoch, then the secular variation).
struct Row {
    n: usize,
    values: Vec<f64>,
}

/// The highest degree with a non-zero value in `column`. A model published to a
/// lower degree than the table's width (pre-2000 models stop at n=10, the
/// secular variation at n=8) carries zeros above it.
fn degree_of(rows: &[Row], column: usize) -> usize {
    rows.iter()
        .filter(|row| row.values[column] != 0.0)
        .map(|row| row.n)
        .max()
        .expect("every column has a non-zero coefficient")
}

/// The geomag70 COF names upstream's models carried: two-digit years before
/// 2000 (`IGRF00`, `DGRF45`), four after (`DGRF2000`).
fn model_name(kind: &str, epoch: f64) -> String {
    let year = epoch as u32;
    if year < 2000 {
        format!("{}{:02}", kind, year % 100)
    } else {
        format!("{}{}", kind, year)
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/igrf14coeffs.txt");

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("data.rs");
    let mut out = BufWriter::new(File::create(&dest_path).unwrap());

    let mut kinds: Vec<String> = Vec::new();
    let mut epochs: Vec<f64> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();

    for line in COEFFS.lines() {
        let parts: Vec<&str> = line.split_ascii_whitespace().collect();
        match parts.first().copied() {
            None | Some("#") => {}
            Some(first) if first.starts_with('#') => {}
            Some("c/s") => {
                // The kinds are written into generated Rust as string
                // literals, so only the two the table defines are accepted.
                kinds = parts[3..].iter().map(|s| s.to_string()).collect();
                let (models, sv) = kinds.split_at(kinds.len() - 1);
                assert!(
                    models.iter().all(|k| k == "IGRF" || k == "DGRF") && sv == ["SV"],
                    "unexpected model kinds in the coefficient table: {:?}",
                    kinds
                );
            }
            Some("g/h") => {
                // Every column but the last is a model epoch; the last is the
                // secular-variation span ("2025-30"), which is not a number.
                let columns = &parts[3..];
                epochs = columns[..columns.len() - 1]
                    .iter()
                    .map(|s| f64::from_str(s).expect("epoch"))
                    .collect();
            }
            Some("g") | Some("h") => rows.push(Row {
                n: usize::from_str(parts[1]).expect("n part"),
                values: parts[3..]
                    .iter()
                    .map(|s| f64::from_str(s).expect("coefficient"))
                    .collect(),
            }),
            Some(other) => panic!("unexpected row in the coefficient table: {}", other),
        }
    }

    assert!(!epochs.is_empty(), "no g/h header row");
    assert_eq!(kinds.len(), epochs.len() + 1, "c/s and g/h rows disagree");
    assert!(
        rows.iter().all(|row| row.values.len() == kinds.len()),
        "a coefficient row does not have one value per model plus the secular variation"
    );
    let sv_column = epochs.len();

    let mut models: Vec<FieldModel> = Vec::new();
    for (i, epoch) in epochs.iter().copied().enumerate() {
        let is_last = i + 1 == epochs.len();
        let max1 = degree_of(&rows, i);
        let max2 = if is_last { degree_of(&rows, sv_column) } else { 0 };

        let mut gh1 = vec![0.0];
        let mut gh2 = vec![0.0];
        for row in &rows {
            if row.n <= max1 {
                gh1.push(row.values[i]);
            }
            if row.n <= max2 {
                gh2.push(row.values[sv_column]);
            }
        }

        models.push(FieldModel {
            name: model_name(&kinds[i], epoch),
            epoch,
            yrmin: epoch,
            yrmax: epoch + 5.0,
            altmin: -1.0,
            altmax: 600.0,
            max1,
            max2,
            max3: 0,
            gh1,
            gh2,
        });
    }

    // A model without its own secular variation runs linearly toward the next
    // model's main field (upstream's rule, unchanged).
    for i in 0..models.len() {
        if models[i].max2 == 0 {
            models[i].gh2 = models[i + 1].gh1.clone();
        }
    }

    write!(out, "const MODELS: &[FieldModel] = &[").unwrap();

    for model in models {
        write!(
            out,
            r#"
                FieldModel {{
                    name: "{}",
                    epoch: {}f64,
                    yrmin: {}f64,
                    yrmax: {}f64,
                    altmin: {}f64,
                    altmax: {}f64,
                    max1: {},
                    max2: {},
                    max3: {},
                    gh1: &[
            "#,
            model.name,
            model.epoch,
            model.yrmin,
            model.yrmax,
            model.altmin,
            model.altmax,
            model.max1,
            model.max2,
            model.max3,
        )
        .unwrap();

        for v in model.gh1 {
            write!(out, "{}f64,", v).unwrap();
        }

        write!(
            out,
            r#"
                    ],
                    gh2: &[
            "#,
        )
        .unwrap();

        for v in model.gh2 {
            write!(out, "{}f64,", v).unwrap();
        }

        write!(
            out,
            r#"
                    ],
                }},
            "#,
        )
        .unwrap();
    }

    write!(out, "];").unwrap();
}
