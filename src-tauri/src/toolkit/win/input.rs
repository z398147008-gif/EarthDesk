//! Injecting keyboard and mouse input.
//!
//! Everything we inject carries `MAGIC` in dwExtraInfo so our own hooks let
//! it straight through instead of treating it as the user's input.

use crate::toolkit::keys::{self, Combo, ALT, CTRL, SHIFT, WIN};
use std::sync::atomic::{AtomicU16, Ordering};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub const MAGIC: usize = ime_proto::INJECTED; // "EDK1"; the input method lets these keys through

/// An unassigned virtual-key code. Pressing it between a modifier going down
/// and up stops Windows treating that as "Alt alone" (menu bar) or "Win
/// alone" (Start menu) -- the same trick AutoHotkey uses.
const MASK_VK: u16 = 0xE8;

/// Which modifier keys the user is physically holding, one bit per key in
/// `SLOTS` order. Maintained by the keyboard hook from real (not injected)
/// events.
static PHYSICAL: AtomicU16 = AtomicU16::new(0);
const SLOTS: [u16; 8] = [0xA2, 0xA3, 0xA4, 0xA5, 0xA0, 0xA1, 0x5B, 0x5C];

fn slot(vk: u16) -> Option<usize> {
    SLOTS.iter().position(|&s| s == vk)
}

/// The hook reports generic codes for some keyboards; map them to the left
/// key so the bookkeeping still balances.
fn specific(vk: u16) -> u16 {
    match vk {
        0x11 => 0xA2,
        0x12 => 0xA4,
        0x10 => 0xA0,
        v => v,
    }
}

pub fn note_physical(vk: u16, down: bool) {
    if let Some(i) = slot(specific(vk)) {
        if down {
            PHYSICAL.fetch_or(1 << i, Ordering::SeqCst);
        } else {
            PHYSICAL.fetch_and(!(1 << i), Ordering::SeqCst);
        }
    }
}

/// Forget modifiers Windows says are up. A release we never saw (Win+L, a
/// UAC prompt: both happen on another desktop) would otherwise leave a
/// modifier "held" forever and no hotkey would match again.
pub fn reconcile() {
    let p = PHYSICAL.load(Ordering::SeqCst);
    if p == 0 {
        return;
    }
    for (i, vk) in SLOTS.iter().enumerate() {
        if p & (1 << i) != 0 && unsafe { GetAsyncKeyState(*vk as i32) } as u16 & 0x8000 == 0 {
            PHYSICAL.fetch_and(!(1 << i), Ordering::SeqCst);
        }
    }
}

/// Modifier bits (keys::CTRL ...) the user is holding right now.
pub fn physical_mods() -> u8 {
    let p = PHYSICAL.load(Ordering::SeqCst);
    let mut m = 0;
    for (i, vk) in SLOTS.iter().enumerate() {
        if p & (1 << i) != 0 {
            m |= keys::modifier_bit(*vk);
        }
    }
    m
}

fn held_keys() -> Vec<u16> {
    let p = PHYSICAL.load(Ordering::SeqCst);
    SLOTS.iter().enumerate().filter(|(i, _)| p & (1 << i) != 0).map(|(_, v)| *v).collect()
}

fn key(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if keys::is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC },
        },
    }
}

fn unicode(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: MAGIC },
        },
    }
}

fn send(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Tap the mask key: see MASK_VK.
pub fn mask() {
    send(&[key(MASK_VK, false), key(MASK_VK, true)]);
}

const MOD_KEYS: [(u8, u16); 4] = [(CTRL, 0xA2), (ALT, 0xA4), (SHIFT, 0xA0), (WIN, 0x5B)];

/// Press one combination as if typed, whatever modifiers the user happens
/// to be holding (a hotkey fires while its own keys are still down: sending
/// "Ctrl+C" from "Ctrl+Alt+K" must lift Alt for the moment).
pub fn press(combo: Combo) {
    let held = held_keys();
    let mut v = Vec::new();
    let stray: Vec<u16> = held.iter().copied().filter(|k| combo.mods & keys::modifier_bit(*k) == 0).collect();
    if stray.iter().any(|k| keys::modifier_bit(*k) & (ALT | WIN) != 0) {
        v.push(key(MASK_VK, false));
        v.push(key(MASK_VK, true));
    }
    for k in &stray {
        v.push(key(*k, true));
    }
    let held_mods: u8 = held.iter().map(|k| keys::modifier_bit(*k)).fold(0, |a, b| a | b);
    let mut pressed = Vec::new();
    for (bit, vk) in MOD_KEYS {
        if combo.mods & bit != 0 && held_mods & bit == 0 {
            v.push(key(vk, false));
            pressed.push(vk);
        }
    }
    // The modifiers stay down a moment around the key: an input method (or
    // a busy program) may look at whether Ctrl is down only when it gets to
    // the key, and with everything sent at once Ctrl is already up by then
    // (Ctrl+W then arrives as a plain "w").
    let with_mods = !pressed.is_empty() || combo.mods != 0;
    if with_mods {
        send(&v);
        v.clear();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    v.push(key(combo.vk, false));
    v.push(key(combo.vk, true));
    if with_mods {
        send(&v);
        v.clear();
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    if pressed.iter().any(|k| keys::modifier_bit(*k) & (ALT | WIN) != 0) {
        v.push(key(MASK_VK, false));
        v.push(key(MASK_VK, true));
    }
    for vk in pressed.iter().rev() {
        v.push(key(*vk, true));
    }
    send(&v);
    // Put back what the user is still holding, so the next press of their
    // hotkey is seen with its modifiers.
    let now = held_keys();
    let back: Vec<INPUT> = stray.iter().filter(|k| now.contains(k)).map(|k| key(*k, false)).collect();
    send(&back);
}

pub fn press_sequence(combos: &[Combo]) {
    for (i, c) in combos.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        press(*c);
    }
}

/// Type text directly as Unicode key events (no clipboard involved).
pub fn type_text(text: &str) {
    // Release anything held so "Ctrl" + typed letters do not become
    // shortcuts.
    let held = held_keys();
    let mut v: Vec<INPUT> = Vec::new();
    if held.iter().any(|k| keys::modifier_bit(*k) & (ALT | WIN) != 0) {
        v.push(key(MASK_VK, false));
        v.push(key(MASK_VK, true));
    }
    for k in &held {
        v.push(key(*k, true));
    }
    let text = text.replace("\r\n", "\n");
    for ch in text.chars() {
        if ch == '\n' {
            v.push(key(0x0D, false));
            v.push(key(0x0D, true));
            continue;
        }
        let mut buf = [0u16; 2];
        for unit in ch.encode_utf16(&mut buf) {
            v.push(unicode(*unit, false));
            v.push(unicode(*unit, true));
        }
    }
    for chunk in v.chunks(200) {
        send(chunk);
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: MAGIC },
        },
    }
}

/// Give a swallowed right-button press back to the program under the pointer.
pub fn right_down() {
    send(&[mouse(MOUSEEVENTF_RIGHTDOWN, 0)]);
}

pub fn right_click() {
    send(&[mouse(MOUSEEVENTF_RIGHTDOWN, 0), mouse(MOUSEEVENTF_RIGHTUP, 0)]);
}
