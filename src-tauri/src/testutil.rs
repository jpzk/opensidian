// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! test-only helpers shared by the unit tests of several modules (main.rs: #[cfg(test)] mod testutil)

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/* ETXTBSY (test flake, sc12 red run 1): a test that wrote a script with std::fs::write and then
   exec'd it failed now and then with "Text file busy". The write fd is O_CLOEXEC, but between
   its open and close another test thread may fork (every spawn does); the forked child holds
   a copy of the fd until IT execs, and while any process holds a file open for writing,
   execve of that file fails with ETXTBSY. So tests write executables through a short-lived
   child (sh: cat > file; chmod 755): the write fd only ever exists in that child, never in
   this process's fd table, so no fork here can inherit it; the child has exited (fd closed)
   before write_exe returns. */
/// test helper: create `p` (mode 755) holding `body`, never holding a write fd to it here
pub fn write_exe(p: impl AsRef<Path>, body: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut c = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(p.as_ref())
        .stdin(Stdio::piped())
        .spawn()?;
    let w = c.stdin.take().map(|mut i| i.write_all(body.as_ref()));
    let st = c.wait()?;
    match w {
        Some(Ok(())) if st.success() => Ok(()),
        Some(Err(e)) => Err(e),
        _ => Err(std::io::Error::new(std::io::ErrorKind::Other, format!("write_exe {}: {st}", p.as_ref().display()))),
    }
}

