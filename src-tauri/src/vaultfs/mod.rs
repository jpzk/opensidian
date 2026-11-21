// SPDX-License-Identifier: GPL-3.0-or-later
/* vaultfs — platform split: unix (openat/O_NOFOLLOW) and windows (reparse-safe
   handle walk). Public API is identical; see each backend's file header. */
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// A vault-relative path as the UI, the index and the bookmarks spell it: /-separated. Windows
/// writes a relative Path with \; on unix \ is a legal file name character and stays as is.
pub fn rel_slash(rel: &std::path::Path) -> String {
    let s = rel.to_string_lossy().into_owned();
    #[cfg(windows)]
    let s = s.replace('\\', "/");
    s
}

/// Fill uf from the OS CSPRNG: /dev/urandom on unix, RtlGenRandom on Windows (no new crate).
pub fn os_random(buf: &mut [u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(buf)
    }
    #[cfg(windows)]
    {
        windows::getrandom_fill(buf)
    }
}
