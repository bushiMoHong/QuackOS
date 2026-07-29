//! `linux_dirent64` parser.
//!
//! The FsServer returns raw `linux_dirent64` structures (the same format
//! Linux's `getdents64(2)` syscall produces).  This module provides a
//! safe iterator over those entries.

/// A parsed directory entry.
#[derive(Debug, Clone)]
pub struct Dirent {
    /// Inode number.
    pub d_ino: u64,
    /// Offset to the next entry.
    pub d_off: i64,
    /// File type (`DT_REG`, `DT_DIR`, etc.).
    pub d_type: u8,
    /// Filename (null-terminated in the raw buffer).
    pub name: [u8; 256],
    /// Actual length of the name (without null terminator).
    pub name_len: usize,
}

impl Dirent {
    /// Return the filename as a `&str` if it is valid UTF-8.
    pub fn name_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.name[..self.name_len]).ok()
    }

    /// Return the filename as a byte slice.
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len]
    }
}

// File type constants
pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO:    u8 = 1;
pub const DT_CHR:     u8 = 2;
pub const DT_DIR:     u8 = 4;
pub const DT_BLK:     u8 = 6;
pub const DT_REG:     u8 = 8;
pub const DT_LNK:     u8 = 10;
pub const DT_SOCK:    u8 = 12;
pub const DT_WHT:     u8 = 14;

// linux_dirent64 layout:
//   d_ino:    u64  (offset 0,  8 bytes)
//   d_off:    i64  (offset 8,  8 bytes)
//   d_reclen: u16  (offset 16, 2 bytes)
//   d_type:   u8   (offset 18, 1 byte)
//   d_name:   []   (offset 19, null-terminated)

/// Parse a single `linux_dirent64` at `offset` in `buf`.
///
/// Returns `(dirent, next_offset)` on success, or `None` if the offset is
/// past the end or the entry is malformed.
pub fn parse_one(buf: &[u8], offset: usize) -> Option<(Dirent, usize)> {
    if offset + 19 > buf.len() {
        return None;
    }
    let reclen = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
    if reclen == 0 || offset + reclen > buf.len() {
        return None;
    }

    let d_ino = u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap());
    let d_off = i64::from_le_bytes(buf[offset + 8..offset + 16].try_into().unwrap());
    let d_type = buf[offset + 18];

    let name_start = offset + 19;
    let name_end = (offset + reclen).min(buf.len());
    let name_slice = &buf[name_start..name_end];

    let mut name = [0u8; 256];
    let mut name_len = 0;
    while name_len < name_slice.len() && name_slice[name_len] != 0 {
        if name_len < 256 {
            name[name_len] = name_slice[name_len];
        }
        name_len += 1;
    }
    let name_len = name_len.min(256);

    Some((Dirent { d_ino, d_off, d_type, name, name_len }, offset + reclen))
}

/// Iterate over all directory entries in a getdents64 buffer.
///
/// Entries for `"."` and `".."` are included; filter them out if desired.
pub fn parse_dirents(buf: &[u8]) -> DirentIterator<'_> {
    DirentIterator { buf, offset: 0 }
}

/// Iterator over [`Dirent`] entries parsed from a getdents64 buffer.
pub struct DirentIterator<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for DirentIterator<'a> {
    type Item = Dirent;

    fn next(&mut self) -> Option<Self::Item> {
        let (entry, next) = parse_one(self.buf, self.offset)?;
        self.offset = next;
        Some(entry)
    }
}
