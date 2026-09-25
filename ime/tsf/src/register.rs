//! regsvr32 EarthDeskTSF64.dll / EarthDeskTSF32.dll (the installer does it,
//! elevated): the COM class, then the TSF language profile and the
//! capability categories that tell Windows where the text service may run
//! (Store apps, the lock screen's secure mode, and so on).

use crate::{module, CLSID, NAME, PROFILE};
use windows::core::{Result, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::MAX_PATH;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;

const LANG_ZH_CN: u16 = 0x0804;

const CATEGORIES: &[GUID] = &[
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_SECUREMODE,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_COMLESS,
    GUID_TFCAT_TIPCAP_WOW16,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
];

fn dll_path() -> String {
    let mut buf = [0u16; MAX_PATH as usize * 2];
    let n = unsafe { GetModuleFileNameW(Some(module().into()), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn guid_string(g: &GUID) -> String {
    format!("{{{g:?}}}")
}

fn set_value(key: HKEY, name: Option<&str>, value: &str) -> Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
    let name = name.map(HSTRING::from);
    unsafe {
        RegSetValueExW(
            key,
            match &name {
                Some(n) => PCWSTR(n.as_ptr()),
                None => PCWSTR::null(),
            },
            None,
            REG_SZ,
            Some(bytes),
        )
        .ok()
    }
}

fn create_key(parent: HKEY, path: &str) -> Result<HKEY> {
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(parent, &HSTRING::from(path), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
            .ok()?;
    }
    Ok(key)
}

fn register_class() -> Result<()> {
    let base = format!(r"SOFTWARE\Classes\CLSID\{}", guid_string(&CLSID));
    let k = create_key(HKEY_LOCAL_MACHINE, &base)?;
    set_value(k, None, NAME)?;
    let inproc = create_key(k, "InprocServer32")?;
    set_value(inproc, None, &dll_path())?;
    set_value(inproc, Some("ThreadingModel"), "Apartment")?;
    unsafe {
        let _ = RegCloseKey(inproc);
        let _ = RegCloseKey(k);
    }
    Ok(())
}

fn unregister_class() {
    let base = format!(r"SOFTWARE\Classes\CLSID\{}", guid_string(&CLSID));
    unsafe {
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(base));
    }
}

/// The icon shown in the language switcher: ime.ico next to the DLL.
fn icon_path() -> String {
    let p = dll_path();
    match p.rfind('\\') {
        Some(i) => format!("{}\\ime.ico", &p[..i]),
        None => "ime.ico".into(),
    }
}

/// The keyboard layout Windows should use while we are active. Chinese
/// profiles default to the US layout, which on a Japanese (JIS) keyboard puts
/// half the symbols on the wrong keys (、 ¥ @ : ...); on such keyboards we
/// ask for the Japanese layout instead.
fn keyboard_layout() -> HKL {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardType, LoadKeyboardLayoutW, KLF_NOTELLSHELL};
    unsafe {
        if GetKeyboardType(0) == 7 {
            if let Ok(h) = LoadKeyboardLayoutW(windows::core::w!("00000411"), KLF_NOTELLSHELL) {
                return h;
            }
        }
    }
    HKL::default()
}

pub fn register() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    register_class()?;
    unsafe {
        let desc: Vec<u16> = NAME.encode_utf16().collect();
        let icon: Vec<u16> = icon_path().encode_utf16().collect();
        // Windows 8+: one call. The older interface is kept as a fallback
        // (some Windows builds and compatibility layers lack the new one).
        let mgr = CoCreateInstance::<_, ITfInputProcessorProfileMgr>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER);
        let done = match &mgr {
            Ok(m) => m.RegisterProfile(&CLSID, LANG_ZH_CN, &PROFILE, &desc, &icon, 0, keyboard_layout(), 0, true, 0).is_ok(),
            Err(_) => false,
        };
        if !done {
            let old: ITfInputProcessorProfiles = CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            old.Register(&CLSID)?;
            old.AddLanguageProfile(&CLSID, LANG_ZH_CN, &PROFILE, &desc, &icon, 0)?;
        }
        let cats: ITfCategoryMgr = CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for c in CATEGORIES {
            cats.RegisterCategory(&CLSID, c, &CLSID)?;
        }
    }
    Ok(())
}

pub fn unregister() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Ok(cats) = CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) {
            for c in CATEGORIES {
                let _ = cats.UnregisterCategory(&CLSID, c, &CLSID);
            }
        }
        let gone = match CoCreateInstance::<_, ITfInputProcessorProfileMgr>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
            Ok(mgr) => mgr.UnregisterProfile(&CLSID, LANG_ZH_CN, &PROFILE, 0).is_ok(),
            Err(_) => false,
        };
        if !gone {
            if let Ok(old) = CoCreateInstance::<_, ITfInputProcessorProfiles>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
                let _ = old.Unregister(&CLSID);
            }
        }
    }
    unregister_class();
    Ok(())
}
