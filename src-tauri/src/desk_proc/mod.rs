// SPDX-License-Identifier: GPL-3.0-or-later
/* desk_proc — platform split:
   - linux: O_PATH pin, process groups, xdg-open shell opener (linux.rs)
   - windows/macOS: CreateProcess/posix_spawn-style spawn + shell stubs (non_linux.rs)
   Public API used by main.rs / consent.rs is identical across backends. */
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
mod non_linux;
#[cfg(not(target_os = "linux"))]
pub use non_linux::*;
