//! Just enough of the zip format to read a Mudlet package (`.mpackage`): the central directory,
//! stored and deflated entries, nothing else (no zip64, encryption or other methods). Every
//! size is checked before anything is inflated, and inflating stops at the size the archive
//! declared, so a small file cannot expand past the limits.

use std::io::Read;

/// One file in the archive, as its central directory describes it.
#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    method: u16,
    compressed: u64,
    pub size: u64,
    offset: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ZipError {
    /// Not a zip archive this reader understands.
    Invalid,
    /// An entry is larger than the reader was allowed.
    TooLarge,
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, ZipError> {
    data.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(ZipError::Invalid)
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, ZipError> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(ZipError::Invalid)
}

pub struct ZipArchive<'a> {
    data: &'a [u8],
    pub entries: Vec<ZipEntry>,
}

impl<'a> ZipArchive<'a> {
    /// Read the central directory, at most `max_entries` entries.
    pub fn open(data: &'a [u8], max_entries: usize) -> Result<Self, ZipError> {
        // The end of central directory record: 22 bytes plus a comment of up to 65,535.
        let earliest = data.len().saturating_sub(22 + 65_535);
        let end = (earliest..=data.len().saturating_sub(22))
            .rev()
            .find(|&i| data[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))
            .ok_or(ZipError::Invalid)?;
        let count = usize::from(u16_at(data, end + 10)?);
        let directory_size = u32_at(data, end + 12)? as usize;
        let directory_offset = u32_at(data, end + 16)? as usize;
        if count == 0xFFFF || directory_offset == 0xFFFF_FFFF {
            return Err(ZipError::Invalid);
        }
        if count > max_entries {
            return Err(ZipError::Invalid);
        }
        if directory_offset
            .checked_add(directory_size)
            .is_none_or(|e| e > data.len())
        {
            return Err(ZipError::Invalid);
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = directory_offset;
        for _ in 0..count {
            if u32_at(data, at)? != 0x0201_4b50 {
                return Err(ZipError::Invalid);
            }
            let flags = u16_at(data, at + 8)?;
            let method = u16_at(data, at + 10)?;
            let compressed = u64::from(u32_at(data, at + 20)?);
            let size = u64::from(u32_at(data, at + 24)?);
            let name_length = usize::from(u16_at(data, at + 28)?);
            let extra_length = usize::from(u16_at(data, at + 30)?);
            let comment_length = usize::from(u16_at(data, at + 32)?);
            let offset = u64::from(u32_at(data, at + 42)?);
            let name_bytes = data.get(at + 46..at + 46 + name_length).ok_or(ZipError::Invalid)?;
            if flags & 1 != 0 {
                // Encrypted.
                return Err(ZipError::Invalid);
            }
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            entries.push(ZipEntry {
                name,
                method,
                compressed,
                size,
                offset,
            });
            at += 46 + name_length + extra_length + comment_length;
        }
        Ok(Self { data, entries })
    }

    /// An entry's contents, refused when it is (or turns out to be) larger than `limit` or than
    /// the size the directory declared.
    pub fn read(&self, entry: &ZipEntry, limit: u64) -> Result<Vec<u8>, ZipError> {
        if entry.size > limit {
            return Err(ZipError::TooLarge);
        }
        let at = usize::try_from(entry.offset).map_err(|_| ZipError::Invalid)?;
        if u32_at(self.data, at)? != 0x0403_4b50 {
            return Err(ZipError::Invalid);
        }
        let name_length = usize::from(u16_at(self.data, at + 26)?);
        let extra_length = usize::from(u16_at(self.data, at + 28)?);
        let start = at + 30 + name_length + extra_length;
        let length = usize::try_from(entry.compressed).map_err(|_| ZipError::Invalid)?;
        let raw = self.data.get(start..start + length).ok_or(ZipError::Invalid)?;
        let declared = entry.size;
        let out = match entry.method {
            0 => raw.to_vec(),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .take(declared + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| ZipError::Invalid)?;
                out
            }
            _ => return Err(ZipError::Invalid),
        };
        if out.len() as u64 != declared {
            // The data does not match what the directory says: a damaged or lying archive.
            return Err(ZipError::Invalid);
        }
        Ok(out)
    }
}

/// Builds small archives for tests: stored or deflated entries, sizes as given.
#[cfg(test)]
pub(crate) mod writer {
    use std::io::Write;

    pub struct Entry<'a> {
        pub name: &'a str,
        pub data: Vec<u8>,
        pub deflate: bool,
        /// Declare this uncompressed size instead of the real one.
        pub declared: Option<u32>,
    }

    pub fn build(entries: &[Entry<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for entry in entries {
            let payload = if entry.deflate {
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
                encoder.write_all(&entry.data).unwrap();
                encoder.finish().unwrap()
            } else {
                entry.data.clone()
            };
            let method: u16 = if entry.deflate { 8 } else { 0 };
            let size = entry.declared.unwrap_or(entry.data.len() as u32);
            let offset = out.len() as u32;
            let name = entry.name.as_bytes();
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(&payload);
            directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            directory.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            directory.extend_from_slice(&method.to_le_bytes());
            directory.extend_from_slice(&[0; 8]);
            directory.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            directory.extend_from_slice(&size.to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&[0; 12]);
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name);
        }
        let directory_offset = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}
