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
