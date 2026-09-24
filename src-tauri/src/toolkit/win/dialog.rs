//! "Choose a program" for the rule editor: the standard Open dialog.

use windows::core::{w, PCWSTR};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;

pub fn pick_program() -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC { pszName: w!("程序和快捷方式"), pszSpec: w!("*.exe;*.lnk;*.bat;*.cmd;*.ps1;*.url") },
            COMDLG_FILTERSPEC { pszName: w!("所有文件"), pszSpec: w!("*.*") },
        ];
        let _ = dlg.SetFileTypes(&filters);
        let _ = dlg.SetTitle(w!("选择要启动的程序"));
        // Keep .lnk as the shortcut itself rather than its target, so
        // arguments stored in the shortcut still apply.
        let opts = dlg.GetOptions().unwrap_or_default();
        let _ = dlg.SetOptions(opts | FOS_NODEREFERENCELINKS | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM);
        dlg.Show(None).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        let _ = PCWSTR::null();
        s
    }
}
