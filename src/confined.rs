//! Directory access confined to a pinned root, without a process sandbox.

use std::{
    ffi::OsString,
    fs::{File, Metadata},
    io,
    os::unix::ffi::OsStrExt,
    path::Path,
};

use rustix::{
    fd::OwnedFd,
    fs::{Dir, Mode, OFlags, ResolveFlags, open, openat2},
};

pub struct Root(OwnedFd);

impl Root {
    pub fn new(path: &Path) -> io::Result<Self> {
        let root = Self(open(
            path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        // Probe the exact policy at startup, including directory read access.
        // Never silently fall back if the kernel or syscall policy rejects it.
        root.open(Path::new("."), OFlags::RDONLY | OFlags::DIRECTORY)?;
        Ok(root)
    }

    fn open(&self, path: &Path, flags: OFlags) -> io::Result<OwnedFd> {
        // TODO: should we retry openat2, if it returns EAGAIN?
        // Currently we choose not to retry, as this is acceptable for now.
        Ok(openat2(
            &self.0,
            path,
            flags | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
        )?)
    }

    pub fn entries(&self, path: &Path, limit: usize) -> io::Result<Vec<(OsString, Metadata)>> {
        let dir = Dir::new(self.open(path, OFlags::RDONLY | OFlags::DIRECTORY)?)?;
        let mut entries = Vec::new();
        // getdents includes dot entries, unlike std::fs::read_dir. Exclude them
        // before applying the existing limit semantics.
        for entry in dir
            .filter(|entry| {
                entry.as_ref().map_or(true, |entry| {
                    !matches!(entry.file_name().to_bytes(), b"." | b"..")
                })
            })
            .take(limit)
        {
            let Ok(entry) = entry else { continue };
            let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes());
            // Resolve from the root, not the listed directory: ../sibling links
            // are valid, but a directory or link swapped outside is not.
            let Ok(fd) = self.open(&path.join(name), OFlags::PATH) else {
                continue;
            };
            if let Ok(metadata) = File::from(fd).metadata() {
                entries.push((name.to_owned(), metadata));
            }
        }
        Ok(entries)
    }
}
