// SPDX-License-Identifier: GPL-3.0-or-later
/* pick_win — the folder dialog Windows users know (the Explorer one: address
   bar, quick access, OneDrive, network places, "New folder", their own pinned
   folders), for choosing a vault.

   The in-app directory list stays the picker on Linux, where it exists because
   a sandboxed window may not list arbitrary folders (spawner.rs). Windows has
   no such constraint and a hand-made list there is simply a worse Explorer.

   The dialog only PROPOSES a path. Nothing is opened or created here: the path
   goes back to the page and then through the very same commands as a typed one
   (pick_vault / create_vault -> vault_rules -> picker_allows), so every rule
   about what may be a vault still runs, in one place.

   IFileOpenDialog is COM: it needs a single-threaded apartment and blocks its
   thread while it is open. So it runs on a thread of its own, never on the UI
   thread (which must keep pumping the window it is modal to) and never on the
   async runtime. FOS_FORCEFILESYSTEM keeps the choice to real directories: no
   library, no search folder, nothing without a path. */

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};

/// HRESULT_FROM_WIN32(ERROR_CANCELLED): the user closed the dialog
const CANCELLED: i32 = 0x800704C7u32 as i32;

/// Show the dialog, modal to the window `owner` (an HWND as an integer: a raw
/// pointer does not cross threads, its value does). Ok(None) = cancelled.
pub fn pick_folder(owner: isize, title: String, start: Option<String>) -> Result<Option<String>, String> {
    std::thread::Builder::new()
        .name("folder-dialog".into())
        .spawn(move || {
            // SAFETY: COM is initialised and released on this thread only, and
            // every interface pointer below lives and dies between the two calls
            unsafe {
                let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
                if hr.is_err() {
                    return Err(format!("folder dialog: COM: {hr:?}"));
                }
                let r = show(owner, &title, start.as_deref());
                CoUninitialize();
                r
            }
        })
        .map_err(|e| format!("folder dialog: {e}"))?
        .join()
        .map_err(|_| "folder dialog: the dialog thread panicked".to_string())?
}

/// SAFETY: call with COM initialised (STA) on the current thread
unsafe fn show(owner: isize, title: &str, start: Option<&str>) -> Result<Option<String>, String> {
    let e = |what: &str, x: windows::core::Error| format!("folder dialog: {what}: {x}");
    let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).map_err(|x| e("create", x))?;
    let opts = dlg.GetOptions().map_err(|x| e("options", x))?;
    dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).map_err(|x| e("options", x))?;
    let _ = dlg.SetTitle(&HSTRING::from(title)); // cosmetic: a default title is fine
    // where it opens: best effort. A start folder that is gone must not stop
    // the dialog; it then opens where the user last was, as Explorer does.
    if let Some(s) = start.filter(|s| !s.is_empty()) {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(s), None) {
            let _ = dlg.SetFolder(&item);
        }
    }
    let owner = (owner != 0).then_some(HWND(owner as *mut core::ffi::c_void));
    match dlg.Show(owner) {
        Ok(()) => {}
        Err(x) if x.code().0 == CANCELLED => return Ok(None),
        Err(x) => return Err(e("show", x)),
    }
    let item = dlg.GetResult().map_err(|x| e("result", x))?;
    let name = item.GetDisplayName(SIGDN_FILESYSPATH).map_err(|x| e("path", x))?;
    let path = PCWSTR(name.0).to_string();
    CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
    path.map(Some).map_err(|_| "folder dialog: the chosen path is not valid UTF-16".to_string())
}
