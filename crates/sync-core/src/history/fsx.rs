// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Private-permission file helpers: dirs 0700, files 0600 on unix (the
//! store holds plaintext copies of notes, including deleted secrets).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(crate) fn mkdir_private(p: &Path) -> io::Result<()> {
    fs::create_dir_all(p)?;
    #[cfg(unix)]
    fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn private(o: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    o.mode(0o600);
    o
}

/// Create/truncate a private file for writing.
pub(crate) fn create_private(p: &Path) -> io::Result<File> {
    private(OpenOptions::new().write(true).create(true).truncate(true)).open(p)
}

/// Open (creating) a private file for appending.
pub(crate) fn append_private(p: &Path) -> io::Result<File> {
    private(OpenOptions::new().append(true).create(true)).open(p)
}

/// Directory fsync so a rename or create inside it survives power loss.
/// Errors are returned: callers must not act on a change (e.g. collect blobs
/// a replaced journal no longer names) that might not be durable.
pub(crate) fn sync_dir(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(p)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        Ok(())
    }
}
