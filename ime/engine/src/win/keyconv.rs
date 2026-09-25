//! Windows key events -> Rime key codes (X11 keysyms) and modifier masks.
//!
//! This used to live in the TSF DLL; it is here so a fix reaches programs
//! that are already open as soon as the engine restarts. The DLL sends the
//! key as Windows reported it (virtual key, scan code, modifier state and the
//! program's keyboard layout) and we do the rest: special keys by table,
//! everything else through ToUnicodeEx with Ctrl/Alt masked out so Ctrl+A
//! still yields 'a' (the way Weasel does it).

use ime_proto::{keysym::*, mask, rawkey};
use std::collections::HashMap;
use std::sync::Mutex;
use windows::core::HSTRING;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

fn special(vk: u16, extended: bool, scan: u32) -> Option<u32> {
    Some(match VIRTUAL_KEY(vk) {
        VK_BACK => BACKSPACE,
        VK_TAB => TAB,
        VK_RETURN => {
            if extended {
                KP_ENTER
            } else {
                RETURN
            }
        }
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => {
            if scan == 0x36 || VIRTUAL_KEY(vk) == VK_RSHIFT {
                SHIFT_R
            } else {
                SHIFT_L
            }
        }
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => {
            if extended || VIRTUAL_KEY(vk) == VK_RCONTROL {
                CONTROL_R
            } else {
                CONTROL_L
            }
        }
        VK_MENU | VK_LMENU | VK_RMENU => {
            if extended {
                ALT_R
            } else {
                ALT_L
            }
        }
        VK_PAUSE => PAUSE,
        VK_CAPITAL => CAPS_LOCK,
        VK_ESCAPE => ESCAPE,
        VK_SPACE => 0x20,
        VK_PRIOR => PAGE_UP,
        VK_NEXT => PAGE_DOWN,
        VK_END => END,
        VK_HOME => HOME,
        VK_LEFT => LEFT,
        VK_UP => UP,
        VK_RIGHT => RIGHT,
        VK_DOWN => DOWN,
        VK_INSERT => INSERT,
        VK_DELETE => DELETE,
        v if (VK_F1.0..=VK_F12.0).contains(&v.0) => F1 + (v.0 - VK_F1.0) as u32,
        v if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&v.0) => KP_0 + (v.0 - VK_NUMPAD0.0) as u32,
        // The keypad keeps its own identity: Rime passes KP_Decimal through
        // as ".", where a plain "." would become "。".
        VK_DECIMAL => KP_DECIMAL,
        VK_ADD => KP_ADD,
        VK_SUBTRACT => KP_SUBTRACT,
        VK_MULTIPLY => KP_MULTIPLY,
        VK_DIVIDE => KP_DIVIDE,
        VK_SEPARATOR => KP_SEPARATOR,
        _ => return None,
    })
}

/// Our own handle for the program's keyboard layout (layout handles belong
/// to a process), loaded once per layout.
fn load_layout(program_hkl: u64) -> HKL {
    static CACHE: Mutex<Option<HashMap<u64, isize>>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(h) = map.get(&program_hkl) {
        return HKL(*h as *mut _);
    }
    let lang = (program_hkl & 0xffff) as u32;
    let device = ((program_hkl >> 16) & 0xffff) as u32;
    // HKL -> KLID: "0000xxxx" for a language's default layout; variants
    // (0xFxxx devices) are looked up by their "Layout Id" in the registry.
    let klid = if device & 0xf000 == 0xf000 {
        variant_klid(device & 0x0fff).unwrap_or(format!("{lang:08X}"))
    } else if device == lang || device == 0 {
        format!("{lang:08X}")
    } else {
        format!("{device:08X}")
    };
    let h = unsafe { LoadKeyboardLayoutW(&HSTRING::from(klid.as_str()), KLF_NOTELLSHELL) }.map(|h| h.0 as isize).unwrap_or(0);
    let h = if h == 0 { unsafe { GetKeyboardLayout(0) }.0 as isize } else { h };
    map.insert(program_hkl, h);
    HKL(h as *mut _)
}

fn variant_klid(layout_id: u32) -> Option<String> {
    use windows::Win32::System::Registry::*;
    let base = HSTRING::from(r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts");
    unsafe {
        let mut root = HKEY::default();
        RegOpenKeyExW(HKEY_LOCAL_MACHINE, &base, None, KEY_READ, &mut root).ok().ok()?;
        let mut i = 0u32;
        let mut found = None;
        loop {
            let mut name = [0u16; 16];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(root, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            let klid = String::from_utf16_lossy(&name[..len as usize]);
            let mut id = [0u16; 8];
            let mut sz = (id.len() * 2) as u32;
            let sub = HSTRING::from(klid.as_str());
            if RegGetValueW(root, &sub, &HSTRING::from("Layout Id"), RRF_RT_REG_SZ, None, Some(id.as_mut_ptr() as *mut _), Some(&mut sz)).is_ok() {
                let n = (sz as usize / 2).saturating_sub(1).min(id.len());
                if u32::from_str_radix(&String::from_utf16_lossy(&id[..n]), 16).ok() == Some(layout_id) {
                    found = Some(klid);
                    break;
                }
            }
            i += 1;
        }
        let _ = RegCloseKey(root);
        found
    }
}

/// Japanese (JIS) keyboard with a non-Japanese layout: read the keys as JIS,
/// which is what is printed on them.
fn jis() -> Option<HKL> {
    static JIS: Mutex<Option<isize>> = Mutex::new(None);
    if !crate::paths::jis_keyboard() {
        return None;
    }
    let mut g = JIS.lock().unwrap_or_else(|p| p.into_inner());
    let h = *g.get_or_insert_with(|| unsafe { LoadKeyboardLayoutW(windows::core::w!("00000411"), KLF_NOTELLSHELL).map(|h| h.0 as isize).unwrap_or(0) });
    (h != 0).then(|| HKL(h as *mut _))
}

/// The JIS keys a US layout has no character for.
fn jis_fallback(scan: u16, shift: bool) -> Option<u32> {
    Some(match (scan, shift) {
        (0x73, false) | (0x7D, false) => '\\' as u32,
        (0x73, true) => '_' as u32,
        (0x7D, true) => '|' as u32,
        _ => return None,
    })
}

/// (keysym, mask), or None for keys we have nothing to do with.
pub fn convert(vk: u16, scan: u16, flags: u32, program_hkl: u64) -> Option<(u32, u32)> {
    let up = flags & rawkey::UP != 0;
    // Some programs hand the input method a keyboard state that lags behind
    // (Ctrl+V once arrived as a plain v, Shift+/ as /): also ask whether the
    // modifier key is physically down right now.
    let held = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(k.0 as i32) } as u16 & 0x8000 != 0;
    let is_modifier_key = matches!(VIRTUAL_KEY(vk), VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_MENU | VK_LMENU | VK_RMENU);
    let mut flags = flags;
    if !up && !is_modifier_key {
        if held(VK_SHIFT) {
            flags |= rawkey::SHIFT;
        }
        if held(VK_CONTROL) {
            flags |= rawkey::CONTROL;
        }
        if held(VK_MENU) {
            flags |= rawkey::ALT;
        }
    }
    let extended = flags & rawkey::EXTENDED != 0;
    let mut m = 0;
    if flags & rawkey::SHIFT != 0 {
        m |= mask::SHIFT;
    }
    if flags & rawkey::CAPS != 0 {
        m |= mask::LOCK;
    }
    if flags & rawkey::CONTROL != 0 {
        m |= mask::CONTROL;
    }
    if flags & rawkey::ALT != 0 {
        m |= mask::ALT;
    }
    if up {
        m |= mask::RELEASE;
    }
    if VIRTUAL_KEY(vk) == VK_CAPITAL && !up {
        // Rime expects Caps_Lock before the lock state changes; Windows has
        // already flipped it.
        m ^= mask::LOCK;
    }
    if let Some(code) = special(vk, extended, scan as u32) {
        return Some((code, m));
    }
    let own = load_layout(program_hkl);
    let lang_ja = (program_hkl & 0xffff) == 0x0411;
    let (hkl, vk) = match jis().filter(|_| !lang_ja) {
        // Another layout than the program's: the virtual key must come from
        // that layout too (VK_OEM_* keys differ between US and JIS).
        Some(j) => match unsafe { MapVirtualKeyExW(scan as u32, MAPVK_VSC_TO_VK_EX, Some(j)) } {
            0 => (j, vk as u32),
            v => (j, v),
        },
        None => (own, vk as u32),
    };
    let mut table = [0u8; 256];
    if flags & rawkey::SHIFT != 0 {
        table[VK_SHIFT.0 as usize] = 0x80;
    }
    if flags & rawkey::CAPS != 0 {
        table[VK_CAPITAL.0 as usize] = 0x01;
    }
    let mut buf = [0u16; 8];
    // Flag 4: do not change the keyboard state (dead keys stay pending in
    // the app, not in us). Windows 10 1607+.
    let n = unsafe { ToUnicodeEx(vk, scan as u32, &table, &mut buf, 4, Some(hkl)) };
    if n == 1 {
        return Some((buf[0] as u32, m));
    }
    jis_fallback(scan, flags & rawkey::SHIFT != 0).map(|c| (c, m))
}
