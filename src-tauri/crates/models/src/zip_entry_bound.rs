//! Bound a ZIP archive's entry count from its raw bytes, BEFORE the `zip`
//! crate parses its central directory (#576 RV19a-4, RV19a-4b, W52-4).
//!
//! `zip::ZipArchive::new` builds a record for every entry the directory
//! declares before it returns, and the crate exposes no hook to bound that: a
//! count checked on `archive.len()` afterwards caps what follows the parse,
//! not the parse (575 MB and 4 s for a million entries). `archive.len()`
//! counts unique names, too, so a million records all named alike were parsed
//! and then let through. The `.tml` reader and the SpeleoDB dump reader both
//! bound the count here first, on the bytes they then hand to that parse.
//!
//! Three checks. The count the last end record declares gives a refusal its
//! plain words, but bounds nothing on its own: the `zip` crate searches the
//! whole file backwards for end records and, whenever one fails, falls back on
//! an earlier one (zip 2.4.2 spec.rs:605-820, read.rs:655-680), so a broken
//! last record can hide any count behind it. The bound is the second check:
//! every entry the reader parses starts with a central-directory signature at
//! its own offset (read.rs:708-711 and 1242, spec.rs:180-189), whichever end
//! record it went by, so the number of signatures in the file caps the entries
//! any attempt can parse. A signature also turns up by chance in compressed
//! data, with probability 2^-32 at each byte offset; a real archive of a
//! handful of entries never comes near 10 000 of them.
//!
//! The third covers what the second cannot (#576 RV19a-4b): before it reads
//! one directory record, the reader reserves room for every entry a ZIP64 end
//! record declares (read.rs:692-706, about 200 bytes an entry), so a record
//! declaring 11 million reserves 2 GB with no directory record in the file.
//! And it does not stop at the record the locator points at: it searches
//! forward from there (spec.rs:762-790), so that record can sit anywhere
//! before the locator. What it takes is fixed by structure instead: a locator
//! right before an end record (spec.rs:621, 657), and a ZIP64 end record whose
//! size field ends it exactly where that locator starts (spec.rs:752, 787).
//! Every ZIP64 end record of that shape that declares more than the limit is
//! refused. The exact end is what keeps a signature that turns up by chance
//! in compressed data from refusing a real archive: its random counts are
//! over the limit, but its size field would have to reach a locator to the
//! byte.

use std::io::{self, Read, Seek, SeekFrom};

/// Why an archive is over the entry limit, for the caller's own refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryExcess {
    /// An end record the ZIP reader could go by declares this many entries.
    Declared(u64),
    /// The file holds this many central-directory record signatures.
    DirectoryRecords(u64),
    /// The file holds more than [`MAX_END_CANDIDATES`] ZIP64 end records over
    /// the limit, or more locators.
    EndRecords,
}

/// How many ZIP64 end records over the limit, and how many locators, the scan
/// keeps. A real archive holds one of each at most, and a signature turns up by
/// chance about once in 4 GiB; more is a crafted archive, refused outright so
/// the scan's own memory stays bounded.
pub const MAX_END_CANDIDATES: usize = 1024;

/// Whether the archive `reader` holds is over `limit` entries by any of the
/// three checks (see the module docs), reading it once from its start plus its
/// last 64 KiB. `None` when it is within the limit; a missing or garbled end
/// record is the ZIP reader's to refuse. Leaves `reader` at its start.
///
/// # Errors
///
/// A read or seek on `reader` failed.
pub fn entry_count_excess<R: Read + Seek>(
    reader: &mut R,
    limit: u64,
) -> io::Result<Option<EntryExcess>> {
    let len = reader.seek(SeekFrom::End(0))?;
    let declared = declared_entry_count(reader, len)?;
    let scan = scan_directory_signatures(reader, limit)?;
    let excess = if let Some(declared) = declared
        .filter(|&count| count > limit)
        .or(scan.zip64_declared_over)
    {
        Some(EntryExcess::Declared(declared))
    } else if scan.records > limit {
        Some(EntryExcess::DirectoryRecords(scan.records))
    } else if scan.end_records_overflow {
        Some(EntryExcess::EndRecords)
    } else {
        None
    };
    reader.rewind()?;
    Ok(excess)
}

/// `PK\x01\x02`, the signature every central-directory record starts with.
const DIRECTORY_RECORD_SIGNATURE: [u8; 4] = *b"PK\x01\x02";

/// What [`scan_directory_signatures`] found.
#[derive(Debug, Default)]
struct DirectoryScan {
    /// Central-directory signatures in the file.
    records: u64,
    /// The largest count over the limit that a ZIP64 end record the reader
    /// could take declares, if any.
    zip64_declared_over: Option<u64>,
    /// More ZIP64 end records over the limit, or more locators, than
    /// [`MAX_END_CANDIDATES`].
    end_records_overflow: bool,
}

/// Read `reader` once from its start in 64 KiB chunks: count its
/// central-directory signatures, and find every ZIP64 end record declaring
/// more than `limit` entries that ends exactly at a locator standing right
/// before an end record - the shape the ZIP reader takes (see the module
/// docs).
fn scan_directory_signatures<R: Read + Seek>(
    reader: &mut R,
    limit: u64,
) -> io::Result<DirectoryScan> {
    const CHUNK: usize = 64 * 1024;
    // The longest field a signature is read with: a ZIP64 end record's counts
    // end 40 bytes in. The bytes past the last offset whose whole window is in
    // the buffer are carried into the next chunk, so every offset is looked at
    // once, with its window whole wherever the file holds it.
    const WINDOW: usize = 40;
    reader.rewind()?;
    let mut scan = DirectoryScan::default();
    // (where the record ends, the larger count it declares)
    let mut zip64_over: Vec<(u64, u64)> = Vec::new();
    let mut locators: Vec<u64> = Vec::new();
    let mut buffer = vec![0u8; WINDOW + CHUNK];
    let mut carried = 0;
    let mut base = 0u64;
    loop {
        let read = match reader.read(&mut buffer[carried..]) {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let filled = carried + read;
        let at_end = read == 0;
        let looked_at = if at_end {
            filled
        } else {
            filled.saturating_sub(WINDOW - 1)
        };
        for at in 0..looked_at {
            let window = &buffer[at..filled.min(at + WINDOW)];
            if !window.starts_with(b"PK") {
                continue;
            }
            let offset = base + at as u64;
            if window.starts_with(&DIRECTORY_RECORD_SIGNATURE) {
                scan.records += 1;
            } else if window.starts_with(&ZIP64_END_SIGNATURE) && window.len() >= WINDOW {
                let declared = u64_at(window, 24).max(u64_at(window, 32));
                let ends_at = offset
                    .checked_add(12)
                    .and_then(|end| end.checked_add(u64_at(window, 4)));
                if let Some(ends_at) = ends_at.filter(|_| declared > limit) {
                    zip64_over.push((ends_at, declared));
                }
            } else if window.starts_with(&ZIP64_LOCATOR_SIGNATURE)
                && window.get(20..24) == Some(&END_SIGNATURE[..])
            {
                locators.push(offset);
            }
            if zip64_over.len() > MAX_END_CANDIDATES || locators.len() > MAX_END_CANDIDATES {
                scan.end_records_overflow = true;
                return Ok(scan);
            }
        }
        if at_end {
            break;
        }
        buffer.copy_within(looked_at..filled, 0);
        carried = filled - looked_at;
        base += looked_at as u64;
    }
    // `locators` is in file order, so sorted.
    scan.zip64_declared_over = zip64_over
        .into_iter()
        .filter(|(ends_at, _)| locators.binary_search(ends_at).is_ok())
        .map(|(_, declared)| declared)
        .max();
    Ok(scan)
}

/// `PK\x05\x06`, the signature of the 22-byte end-of-central-directory record.
const END_SIGNATURE: [u8; 4] = *b"PK\x05\x06";
/// The end record's fixed part; a comment of up to 65 535 bytes follows it.
const END_LEN: usize = 22;
/// `PK\x06\x07`, the signature of the 20-byte ZIP64 locator that sits right
/// before the end record and points at the ZIP64 end record.
const ZIP64_LOCATOR_SIGNATURE: [u8; 4] = *b"PK\x06\x07";
const ZIP64_LOCATOR_LEN: u64 = 20;
/// `PK\x06\x06`, the signature of the ZIP64 end record (56 bytes before its
/// extensible data), which carries the 64-bit entry counts.
const ZIP64_END_SIGNATURE: [u8; 4] = *b"PK\x06\x06";
const ZIP64_END_LEN: usize = 56;

/// The little-endian `u64` at `offset` in `bytes`.
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut field = [0; 8];
    field.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(field)
}

/// The entry count the archive's last end record declares - the record the
/// ZIP reader tries first, found by searching back from the end over the
/// longest comment it may carry - or, when that record defers to ZIP64 (a
/// count, the directory size or its offset saturated), the count of the
/// ZIP64 end record its locator points at. Of the two counts a record holds
/// (on this disk, in all), the larger. `None` when no count can be read: a
/// missing or garbled record is the ZIP reader's to refuse.
fn declared_entry_count<R: Read + Seek>(reader: &mut R, len: u64) -> io::Result<Option<u64>> {
    let tail_len = len.min((END_LEN + usize::from(u16::MAX)) as u64);
    let tail_at = len - tail_len;
    let tail = read_at(reader, tail_at, tail_len as usize)?;
    let Some(at) = tail
        .windows(END_LEN)
        .rposition(|record| record.starts_with(&END_SIGNATURE))
    else {
        return Ok(None);
    };
    let record = &tail[at..at + END_LEN];
    let u16_at = |offset: usize| u16::from_le_bytes([record[offset], record[offset + 1]]);
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            record[offset],
            record[offset + 1],
            record[offset + 2],
            record[offset + 3],
        ])
    };
    let counts = [u16_at(8), u16_at(10)];
    let saturated = counts.contains(&u16::MAX) || u32_at(12) == u32::MAX || u32_at(16) == u32::MAX;
    if saturated {
        if let Some(count) = zip64_entry_count(reader, tail_at + at as u64)? {
            return Ok(Some(count));
        }
    }
    Ok(counts
        .into_iter()
        .filter(|&count| count != u16::MAX)
        .map(u64::from)
        .max())
}

/// The larger entry count of the ZIP64 end record that the locator right
/// before the end record at `end_at` points at, or `None` when there is no
/// locator there or no ZIP64 end record where it points.
fn zip64_entry_count<R: Read + Seek>(reader: &mut R, end_at: u64) -> io::Result<Option<u64>> {
    let Some(locator_at) = end_at.checked_sub(ZIP64_LOCATOR_LEN) else {
        return Ok(None);
    };
    let locator = read_at(reader, locator_at, ZIP64_LOCATOR_LEN as usize)?;
    if !locator.starts_with(&ZIP64_LOCATOR_SIGNATURE) {
        return Ok(None);
    }
    let record_at = u64_at(&locator, 8);
    // The record must end before the locator starts.
    if record_at
        .checked_add(ZIP64_END_LEN as u64)
        .is_none_or(|record_end| record_end > locator_at)
    {
        return Ok(None);
    }
    let record = read_at(reader, record_at, ZIP64_END_LEN)?;
    if !record.starts_with(&ZIP64_END_SIGNATURE) {
        return Ok(None);
    }
    Ok(Some(u64_at(&record, 24).max(u64_at(&record, 32))))
}

/// `len` bytes of `reader` from `offset`.
fn read_at<R: Read + Seek>(reader: &mut R, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: a failed setup should panic"
)]
mod tests {
    use std::io::{Cursor, Seek as _};

    use super::{entry_count_excess, EntryExcess};

    /// A 22-byte end record without a comment, declaring `entries`.
    fn end_record(entries: u16) -> Vec<u8> {
        let mut record = b"PK\x05\x06".to_vec();
        record.extend_from_slice(&[0; 4]);
        record.extend_from_slice(&entries.to_le_bytes());
        record.extend_from_slice(&entries.to_le_bytes());
        record.extend_from_slice(&[0; 10]);
        record
    }

    /// The check works on any seekable reader, an in-memory dump included, and
    /// leaves it at its start for the ZIP reader whatever it finds.
    #[test]
    fn the_check_reads_a_cursor_and_rewinds_it() {
        for (entries, expected) in [(0, None), (6, Some(EntryExcess::Declared(6)))] {
            let mut reader = Cursor::new(end_record(entries));
            reader.seek(std::io::SeekFrom::End(0)).unwrap();
            let excess = entry_count_excess(&mut reader, 5).unwrap();
            assert_eq!(excess, expected, "an end record declaring {entries}");
            assert_eq!(reader.stream_position().unwrap(), 0, "rewound");
        }
    }
}
