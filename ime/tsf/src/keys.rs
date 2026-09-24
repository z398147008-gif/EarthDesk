//! Windows key events -> Rime key codes (X11 keysyms) and modifier masks,
//! the way Weasel does it: special keys by table, everything else through
//! ToUnicodeEx with Ctrl/Alt masked out so Ctrl+A still yields 'a'.

use ime_proto::{keysym::*, mask};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub struct Key {
    pub code: u32,
    pub mask: u32,
}

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
        _ => return None,
    })
}

/// `lparam` as in WM_KEYDOWN: bits 16-23 scan code, bit 24 extended, bit 31
/// transition (1 = key up).
pub fn convert(vk: u16, lparam: isize, up: bool) -> Option<Key> {
    let mut state = [0u8; 256];
    unsafe {
        if GetKeyboardState(&mut state).is_err() {
            return None;
        }
    }
    let down = |k: VIRTUAL_KEY| state[k.0 as usize] & 0x80 != 0;
    let mut m = 0;
    if down(VK_SHIFT) {
        m |= mask::SHIFT;
    }
    if state[VK_CAPITAL.0 as usize] & 1 != 0 {
        m |= mask::LOCK;
    }
    if down(VK_CONTROL) {
        m |= mask::CONTROL;
    }
    if down(VK_MENU) {
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
    let scan = ((lparam >> 16) & 0xff) as u32;
    let extended = (lparam >> 24) & 1 == 1;
    if let Some(code) = special(vk, extended, scan) {
        return Some(Key { code, mask: m });
    }
    let mut table = state;
    table[VK_CONTROL.0 as usize] = 0;
    table[VK_LCONTROL.0 as usize] = 0;
    table[VK_RCONTROL.0 as usize] = 0;
    table[VK_MENU.0 as usize] = 0;
    table[VK_LMENU.0 as usize] = 0;
    table[VK_RMENU.0 as usize] = 0;
    let mut buf = [0u16; 8];
    // Flag 4: do not change the keyboard state (dead keys stay pending in
    // the app, not in us). Windows 10 1607+.
    let n = unsafe { ToUnicodeEx(vk as u32, scan, &table, &mut buf, 4, Some(GetKeyboardLayout(0))) };
    if n == 1 {
        return Some(Key { code: buf[0] as u32, mask: m });
    }
    None
}
