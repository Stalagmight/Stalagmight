//! The fuzz targets, by name: what each one does with an input, and the
//! synthetic seed inputs each one starts from.
//!
//! The cargo-fuzz crate (`src-tauri/fuzz`) is a set of one-line wrappers over
//! [`run`]; the fixed-seed tests call [`run`] on [`seed_input`]s. Both see the
//! exact same behaviour for the same bytes, so a farm finding replays in a
//! stable `cargo test`.

use crate::fileset::FileSet;
use crate::gen;
use crate::oracle::{self, Depth, Format, Knobs, Outcome, Violation};
use crate::source::{seed_bytes, ByteSource};
use crate::stalx;

/// One fuzz target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Raw bytes as the opened file (plus linked files for the multi-file
    /// formats, in the [`FileSet`] container layout).
    Raw(Format),
    /// Bytes as decisions for the format's structure-aware generator.
    Gen(Format),
    /// Bytes as SQL-level mutations of a synthetic `.stalx`.
    StalxSql,
    /// Bytes as a whole `.stalx` file.
    StalxRaw,
}

impl Target {
    /// Every target.
    pub fn all() -> Vec<Self> {
        let mut out: Vec<Self> = Format::ALL.iter().map(|&f| Self::Raw(f)).collect();
        out.extend(Format::ALL.iter().map(|&f| Self::Gen(f)));
        out.push(Self::StalxSql);
        out.push(Self::StalxRaw);
        out
    }

    /// The cargo-fuzz target name (`raw_tml`, `gen_wpj`, `stalx_sql`, ...).
    pub fn name(self) -> String {
        match self {
            Self::Raw(f) => format!("raw_{}", f.slug()),
            Self::Gen(f) => format!("gen_{}", f.slug()),
            Self::StalxSql => "stalx_sql".to_string(),
            Self::StalxRaw => "stalx_raw".to_string(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|t| t.name() == name)
    }
}

/// Run `target` on `data`.
pub fn run(target: Target, data: &[u8]) -> Result<Outcome, Violation> {
    match target {
        Target::Raw(format) => {
            let files = FileSet::from_container(format.entry_name(), data);
            oracle::run(format, &files, Knobs::of(data), Depth::Apply)
        }
        Target::Gen(format) => {
            let mut src = ByteSource::new(data);
            let knobs = Knobs(src.u8());
            let files = gen::generate(format, &mut src);
            oracle::run(format, &files, knobs, Depth::Apply)
        }
        Target::StalxSql => stalx::run_sql(data),
        Target::StalxRaw => stalx::run_raw(data),
    }
}

/// The files `target` imports for `data` (the rendered generator output, or
/// the unpacked container), for triage: write them out and open them by hand.
/// `None` for the `.stalx` targets, whose input is not a file set.
pub fn files_of(target: Target, data: &[u8]) -> Option<FileSet> {
    match target {
        Target::Raw(format) => Some(FileSet::from_container(format.entry_name(), data)),
        Target::Gen(format) => {
            let mut src = ByteSource::new(data);
            let _knobs = src.u8();
            Some(gen::generate(format, &mut src))
        }
        Target::StalxSql | Target::StalxRaw => None,
    }
}

/// Seed input number `seed` for `target`: generator decisions for the
/// generator-driven targets, a rendered file (set) for the raw ones, a base
/// project for the raw `.stalx` target. Synthetic and deterministic.
pub fn seed_input(target: Target, seed: u64) -> Result<Vec<u8>, Violation> {
    // Vary the decision-stream length so seeds cover small and large files.
    let len = 64 + ((seed.wrapping_mul(2_654_435_761) >> 7) % 6000) as usize;
    let decisions = seed_bytes(seed, len);
    Ok(match target {
        Target::Gen(_) | Target::StalxSql => decisions,
        Target::Raw(format) => {
            gen::generate(format, &mut ByteSource::new(&decisions)).to_container()
        }
        Target::StalxRaw => stalx::base_bytes(seed)?,
    })
}
