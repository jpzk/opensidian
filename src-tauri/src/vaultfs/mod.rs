// SPDX-License-Identifier: GPL-3.0-or-later
/* vaultfs — platform split:
   - linux: openat(O_NOFOLLOW) fd walk (unix.rs)
   - windows: OPEN_REPARSE_POINT handle walk (windows.rs)
   - macos: experimental path-walk with symlink_metadata (macos.rs)
   Public API is identical across backends. */
#[cfg(target_os = "linux")]
mod unix;
#[cfg(target_os = "linux")]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
