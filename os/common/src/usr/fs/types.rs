//! Core types for the VFS layer — file statistics, open flags, error codes,
//! seek direction, and file type classification.

use core::fmt;

// ---------------------------------------------------------------------------
// File type (matches S_IFMT in ext4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FileType {
    Fifo = 0x1000,
    ChrDev = 0x2000,
    Dir = 0x4000,
    BlkDev = 0x6000,
    RegFile = 0x8000,
    Symlink = 0xA000,
    Socket = 0xC000,
}

impl FileType {
    pub fn from_mode(mode: u16) -> Self {
        match mode & 0xF000 {
            0x1000 => FileType::Fifo,
            0x2000 => FileType::ChrDev,
            0x4000 => FileType::Dir,
            0x6000 => FileType::BlkDev,
            0x8000 => FileType::RegFile,
            0xA000 => FileType::Symlink,
            0xC000 => FileType::Socket,
            _ => FileType::RegFile,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, FileType::Dir)
    }

    pub fn is_reg(&self) -> bool {
        matches!(self, FileType::RegFile)
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self, FileType::Symlink)
    }

    pub fn is_chrdev(&self) -> bool {
        matches!(self, FileType::ChrDev)
    }

    pub fn is_blkdev(&self) -> bool {
        matches!(self, FileType::BlkDev)
    }
}

// ---------------------------------------------------------------------------
// Kstat — file metadata (like Linux `struct kstat`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Kstat {
    pub ino: u64,
    pub mode: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blksize: u32,
    pub blocks: u64,
    pub atime_sec: i64,
    pub atime_nsec: i64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
    pub ctime_sec: i64,
    pub ctime_nsec: i64,
    pub rdev: u64,
    pub file_type: FileType,
}

impl Default for Kstat {
    fn default() -> Self {
        Self {
            ino: 0,
            mode: 0,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            blksize: 4096,
            blocks: 0,
            atime_sec: 0,
            atime_nsec: 0,
            mtime_sec: 0,
            mtime_nsec: 0,
            ctime_sec: 0,
            ctime_nsec: 0,
            rdev: 0,
            file_type: FileType::RegFile,
        }
    }
}

// ---------------------------------------------------------------------------
// OpenFlags — how a file was opened
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(u32);

impl OpenFlags {
    pub const O_RDONLY: Self = Self(0x0000);
    pub const O_WRONLY: Self = Self(0x0001);
    pub const O_RDWR: Self = Self(0x0002);
    pub const O_CREAT: Self = Self(0x0040);
    pub const O_EXCL: Self = Self(0x0080);
    pub const O_TRUNC: Self = Self(0x0200);
    pub const O_APPEND: Self = Self(0x0400);
    pub const O_DIRECTORY: Self = Self(0x10000);

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn bits(&self) -> u32 {
        self.0
    }

    pub fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_writable(&self) -> bool {
        self.0 & (Self::O_WRONLY.0 | Self::O_RDWR.0) != 0
    }

    pub fn is_readable(&self) -> bool {
        self.0 & (Self::O_RDONLY.0 | Self::O_RDWR.0) != 0 || self.0 == 0
    }
}

impl core::ops::BitOr for OpenFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ---------------------------------------------------------------------------
// SeekWhence — seek direction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekWhence {
    Set = 0,
    Cur = 1,
    End = 2,
}

// ---------------------------------------------------------------------------
// Errno — POSIX error codes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Errno {
    EPERM = 1,
    ENOENT = 2,
    EIO = 5,
    EBADF = 9,
    ENOMEM = 12,
    EACCES = 13,
    EFAULT = 14,
    EEXIST = 17,
    ENODEV = 19,
    ENOTDIR = 20,
    EISDIR = 21,
    EINVAL = 22,
    ENOSPC = 28,
    EROFS = 30,
    ENOSYS = 38,
    ENOTEMPTY = 39,
}

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::EPERM => "Operation not permitted",
            Self::ENOENT => "No such file or directory",
            Self::EIO => "I/O error",
            Self::EBADF => "Bad file descriptor",
            Self::ENOMEM => "Cannot allocate memory",
            Self::EACCES => "Permission denied",
            Self::EFAULT => "Bad address",
            Self::EEXIST => "File exists",
            Self::ENODEV => "No such device",
            Self::ENOTDIR => "Not a directory",
            Self::EISDIR => "Is a directory",
            Self::EINVAL => "Invalid argument",
            Self::ENOSPC => "No space left on device",
            Self::EROFS => "Read-only file system",
            Self::ENOSYS => "Function not implemented",
            Self::ENOTEMPTY => "Directory not empty",
        };
        write!(f, "{}", s)
    }
}

pub type SyscallRet = Result<usize, Errno>;

// ---------------------------------------------------------------------------
// TimeSpec — wall-clock timestamp (stub until RTC driver is integrated)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct TimeSpec {
    pub sec: i64,
    pub nsec: i64,
}

impl TimeSpec {
    /// Return the current wall-clock time.
    /// TODO: integrate with RTC driver for real time values.
    pub fn new_wall_time() -> Self {
        Self { sec: 0, nsec: 0 }
    }
}

// ---------------------------------------------------------------------------
// Process context stub — returns default uid/gid (root).
// TODO: replace with real proc::task integration.
// ---------------------------------------------------------------------------

pub fn current_task_uid_gid() -> (u32, u32) {
    (0, 0)
}
