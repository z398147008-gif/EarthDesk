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

/// The layout to read characters with: the thread's, except that on a
/// Japanese (JIS) keyboard running a non-Japanese layout we read the keys as
/// JIS, which is what is printed on them.
fn layout() -> HKL {
    use std::sync::OnceLock;
    static JIS: OnceLock<isize> = OnceLock::new();
    let cur = unsafe { GetKeyboardLayout(0) };
    if (cur.0 as usize & 0xffff) == 0x0411 {
        return cur;
    }
    let jis = *JIS.get_or_init(|| unsafe {
        if GetKeyboardType(0) == 7 {
            LoadKeyboardLayoutW(windows::core::w!("00000411"), KLF_NOTELLSHELL).map(|h| h.0 as isize).unwrap_or(0)
        } else {
            0
        }
    });
    if jis != 0 {
        HKL(jis as *mut _)
    } else {
        cur
    }
}

/// The key for the engine as Windows reported it (see ime_proto::rawkey):
/// virtual key, scan code, flags, and the thread's keyboard layout. All the
/// interpretation happens in the engine.
pub fn raw(vk: u16, lparam: isize, up: bool) -> (u16, u16, u32, u64) {
    use ime_proto::rawkey as rk;
    let mut state = [0u8; 256];
    unsafe {
        let _ = GetKeyboardState(&mut state);
    }
    let down = |k: VIRTUAL_KEY| state[k.0 as usize] & 0x80 != 0;
    let physical = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(k.0 as i32) } as u16 & 0x8000 != 0;
    let mut f = 0;
    if up {
        f |= rk::UP;
    }
    if (lparam >> 24) & 1 == 1 {
        f |= rk::EXTENDED;
    }
    if down(VK_SHIFT) {
        f |= rk::SHIFT;
    }
    if down(VK_CONTROL) || physical(VK_CONTROL) {
        f |= rk::CONTROL;
    }
    if down(VK_MENU) || physical(VK_MENU) {
        f |= rk::ALT;
    }
    if state[VK_CAPITAL.0 as usize] & 1 != 0 {
        f |= rk::CAPS;
    }
    let scan = ((lparam >> 16) & 0xff) as u16;
    let hkl = unsafe { GetKeyboardLayout(0) }.0 as usize as u64;
    (vk, scan, f, hkl)
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
    // Ctrl / Alt: the thread's key state, or the keyboard itself. Some
    // programs hand us the V of Ctrl+V before their key state shows Ctrl
    // (Chromium-based apps), and a missed Ctrl turns a shortcut into typing.
    let physical = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(k.0 as i32) } as u16 & 0x8000 != 0;
    if down(VK_CONTROL) || physical(VK_CONTROL) {
        m |= mask::CONTROL;
    }
    if down(VK_MENU) || physical(VK_MENU) {
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
    // Reading with another layout than the thread's: the virtual key must
    // come from that layout too (VK_OEM_* keys differ between US and JIS).
    let hkl = layout();
    let vk = if hkl != unsafe { GetKeyboardLayout(0) } {
        match unsafe { MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK_EX, Some(hkl)) } {
            0 => vk as u32,
            v => v,
        }
    } else {
        vk as u32
    };
    let n = unsafe { ToUnicodeEx(vk, scan, &table, &mut buf, 4, Some(hkl)) };
    if n == 1 {
        return Some(Key { code: buf[0] as u32, mask: m });
    }
    None
}

/// Move the caret `n` characters left the way the user would, with ←
/// (between a pair of brackets just put in: （|）). Programs differ in
/// whether they honour a caret the input method sets while committing
/// (Chromium puts it after the text), but every one moves on ←. Marked as
/// ours (INJECTED), so the text service lets it through untouched. Shift is
/// let go first: （ is typed with it held, and Shift+← would select.
pub fn caret_left(n: u32) {
    let key = |vk: VIRTUAL_KEY, up: bool, extended: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: (if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }) | (if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) }),
                time: 0,
                dwExtraInfo: ime_proto::INJECTED,
            },
        },
    };
    let mut inputs = Vec::new();
    for vk in [VK_LSHIFT, VK_RSHIFT] {
        if unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0 {
            inputs.push(key(vk, true, vk == VK_RSHIFT));
        }
    }
    for _ in 0..n.min(8) {
        inputs.push(key(VK_LEFT, false, true));
        inputs.push(key(VK_LEFT, true, true));
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Shift, Ctrl or Alt held on the keyboard right now.
pub fn modifiers_held() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU].iter().any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } as u16 & 0x8000 != 0)
}

/// Toggle Caps Lock for the program (the JIS keyboard's 英数 key, see
/// service.rs), marked as ours (INJECTED) so the text service lets it
/// through. Sent as the virtual key itself: the Japanese layout turns only
/// the key's scan code, not VK_CAPITAL, into 英数.
pub fn toggle_caps() {
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_CAPITAL,
                wScan: 0x3A,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: ime_proto::INJECTED,
            },
        },
    };
    unsafe {
        SendInput(&[key(false), key(true)], std::mem::size_of::<INPUT>() as i32);
    }
}
