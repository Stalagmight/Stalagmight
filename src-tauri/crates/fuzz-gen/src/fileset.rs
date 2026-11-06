//! [`FileSet`]: one import's files (the opened file plus anything it links),
//! the flat byte container the raw multi-file fuzz targets mutate, and the
//! scratch directory every importer reads them from.
//!
//! Importers take a path, not bytes (they check the extension, the file type
//! and its size before reading, and resolve linked files next to it), so every
//! run writes its files to a private scratch directory first: the fuzzer
//! exercises the exact read path the app uses.

use std::cell::RefCell;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

/// Marker that starts a further file in a [`FileSet`] container.
pub const CONTAINER_MARKER: &[u8] = b"\n\x00@@ ";

/// The files of one import. `files[0]` is the file the user opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileSet {
    pub files: Vec<(String, Vec<u8>)>,
}

impl FileSet {
    /// A single-file import.
    pub fn single(name: &str, bytes: Vec<u8>) -> Self {
        Self {
            files: vec![(name.to_string(), bytes)],
        }
    }

    /// Add a linked file.
    pub fn push(&mut self, name: &str, bytes: Vec<u8>) {
        self.files.push((name.to_string(), bytes));
    }

    /// Name of the opened file.
    pub fn entry_name(&self) -> &str {
        self.files.first().map_or("", |(n, _)| n.as_str())
    }

    /// Flatten into the raw container: the opened file's bytes, then each
    /// linked file as `MARKER name '\n' bytes`. The opened file's name is not
    /// stored; [`Self::from_container`] gets it from the target.
    pub fn to_container(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, (name, bytes)) in self.files.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(CONTAINER_MARKER);
                out.extend_from_slice(name.as_bytes());
                out.push(b'\n');
            }
            out.extend_from_slice(bytes);
        }
        out
    }

    /// Split a raw container back into files; `entry` names the first one.
    /// Linked names are sanitised (see [`sanitize_name`]); a later file with
    /// the name of an earlier one replaces it.
    pub fn from_container(entry: &str, data: &[u8]) -> Self {
        let mut chunks = split_on(data, CONTAINER_MARKER).into_iter();
        let mut set = Self::single(entry, chunks.next().unwrap_or_default().to_vec());
        for chunk in chunks {
            let (name, body) = match chunk.iter().position(|&b| b == b'\n') {
                Some(nl) => (
                    chunk.get(..nl).unwrap_or(&[]),
                    chunk.get(nl + 1..).unwrap_or(&[]),
                ),
                None => (chunk, &[][..]),
            };
            let name = sanitize_name(&String::from_utf8_lossy(name));
            set.files.retain(|(n, _)| *n != name);
            set.files.push((name, body.to_vec()));
        }
        set
    }

    /// Write every file under `dir` (which must be empty) and return the path
    /// of the opened file. A linked file whose name collides with another
    /// one's directory (`a.dat` and `a.dat/b.dat`) is left out, as if the
    /// project linked a file that is not there; only the opened file must be
    /// written.
    pub fn materialize(&self, dir: &Path) -> Result<PathBuf> {
        let mut entry = None;
        for (name, bytes) in &self.files {
            let path = dir.join(sanitize_name(name));
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, bytes));
            match (written, entry.is_none()) {
                (Ok(()), true) => entry = Some(path),
                (_, false) => {}
                (Err(e), true) => {
                    return Err(e).with_context(|| format!("write {}", path.display()));
                }
            }
        }
        entry.context("empty file set")
    }
}

fn split_on<'a>(data: &'a [u8], marker: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + marker.len() <= data.len() {
        if data.get(i..i + marker.len()) == Some(marker) {
            out.push(data.get(start..i).unwrap_or(&[]));
            i += marker.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(data.get(start..).unwrap_or(&[]));
    out
}

/// A relative path that stays inside the scratch directory: at most three
/// components of `[A-Za-z0-9._ -]`, no `..`, no root, never empty.
pub fn sanitize_name(raw: &str) -> String {
    let parts: Vec<String> = Path::new(&raw.replace('\\', "/"))
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => {
                let clean: String = s
                    .to_string_lossy()
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ' '))
                    .take(48)
                    .collect();
                let clean = clean.trim().to_string();
                (!clean.is_empty() && clean.chars().any(|ch| ch != '.')).then_some(clean)
            }
            _ => None,
        })
        .take(3)
        .collect();
    if parts.is_empty() {
        "f".to_string()
    } else {
        parts.join("/")
    }
}

thread_local! {
    static SCRATCH: RefCell<Option<tempfile::TempDir>> = const { RefCell::new(None) };
}

/// Run `f` with an empty private directory. One directory per thread is reused
/// (emptied before each call), under `/dev/shm` when it exists so a fuzzer's
/// millions of runs do not touch a disk.
pub fn with_scratch_dir<T>(f: impl FnOnce(&Path) -> T) -> Result<T> {
    SCRATCH.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let shm = Path::new("/dev/shm");
            let dir = if shm.is_dir() {
                tempfile::Builder::new().prefix("stalfuzz").tempdir_in(shm)
            } else {
                tempfile::Builder::new().prefix("stalfuzz").tempdir()
            }
            .context("create scratch dir")?;
            *slot = Some(dir);
        }
        let dir = slot.as_ref().context("scratch dir")?.path().to_path_buf();
        empty_dir(&dir)?;
        Ok(f(&dir))
    })
}

fn empty_dir(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).context("read scratch dir")? {
        let path = entry.context("scratch entry")?.path();
        if path.is_dir() && !path.is_symlink() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }
        .with_context(|| format!("clear {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::{sanitize_name, FileSet};

    #[test]
    fn container_round_trips_and_names_stay_inside() {
        let mut set = FileSet::single("p.wpj", b"book".to_vec());
        set.push("sub/a.srv", b"#units feet\n".to_vec());
        set.push("b.srv", Vec::new());
        let back = FileSet::from_container("p.wpj", &set.to_container());
        assert_eq!(back, set);
        assert_eq!(sanitize_name("../../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_name("/abs/x.dat"), "abs/x.dat");
        assert_eq!(sanitize_name(".."), "f");
        assert_eq!(sanitize_name("C:\\dir\\x.dat"), "C/dir/x.dat");
    }

    #[test]
    fn a_linked_name_colliding_with_a_directory_is_left_out() {
        let mut set = FileSet::single("p.mak", b"#a.dat;".to_vec());
        set.push("a.dat", b"x".to_vec());
        set.push("a.dat/b.dat", b"y".to_vec());
        let dir = tempfile::tempdir().unwrap();
        let entry = set.materialize(dir.path()).unwrap();
        assert!(entry.ends_with("p.mak"));
        assert!(dir.path().join("a.dat").is_file());
    }
}
