//! `%APPDATA%\EarthDesk\toolkit.json`: gestures, hotkeys, screenshots and the
//! clipboard history.
//!
//! Kept apart from config.json on purpose. config.json has a version number
//! that resets the desktop's layout and look when it is bumped; nothing here
//! should ever be caught up in that, and these rules are the kind of thing a
//! user builds up over months and may want to copy to another machine.
//!
//! Every field has a serde default, so a file written by an older build loads
//! with the new settings at their defaults. Built-in rules carry a stable
//! `id` ("default.*"); `defaults_level` records which of them this file has
//! already been offered, so a later version can add new built-ins without
//! bringing back ones the user deleted.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bump when `default_gestures` / `default_hotkeys` gain a rule that existing
/// files should receive. Each rule says which level introduced it.
pub const DEFAULTS_LEVEL: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Press a key combination, e.g. "Alt+Left", "Ctrl+Shift+T", "F5".
    /// Several separated by spaces are pressed one after another.
    Keys { keys: String },
    /// Start a program. `admin` starts it elevated; otherwise it starts with
    /// the signed-in user's normal rights even though EarthDesk itself runs
    /// as administrator.
    Run {
        path: String,
        #[serde(default)]
        args: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        admin: bool,
    },
    /// Open a web address, a folder or a document with its default program.
    Open { target: String },
    /// Type a piece of text into the window in front (through the clipboard,
    /// which is restored afterwards).
    Text { text: String },
    /// One of EarthDesk's own functions; see `actions::BUILTINS`.
    Builtin { name: String },
    /// Nothing -- a rule that only exists to switch a gesture off in a
    /// particular program.
    None,
}

/// Where a rule applies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Scope {
    /// "global" (everywhere), "only" (just in `apps`) or "except" (everywhere
    /// but `apps`).
    #[serde(default = "global")]
    pub mode: String,
    /// Executable names, lower case, e.g. "chrome.exe".
    #[serde(default)]
    pub apps: Vec<String>,
}

fn global() -> String {
    "global".into()
}

impl Scope {
    pub fn global() -> Self {
        Scope { mode: global(), apps: vec![] }
    }
    pub fn only(apps: &[&str]) -> Self {
        Scope { mode: "only".into(), apps: apps.iter().map(|s| s.to_string()).collect() }
    }
    fn has(&self, exe: &str) -> bool {
        self.apps.iter().any(|a| a.eq_ignore_ascii_case(exe))
    }
    /// How specifically this scope names `exe`: 2 = listed by name, 1 =
    /// applies without naming it, None = does not apply.
    pub fn rank(&self, exe: &str) -> Option<u8> {
        match self.mode.as_str() {
            "only" => self.has(exe).then_some(2),
            "except" => (!self.has(exe)).then_some(1),
            _ => Some(1),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GestureRule {
    pub id: String,
    /// Directions as arrows: "←", "↓→", "↑↓" ... (diagonals ↖↗↙↘).
    pub gesture: String,
    pub name: String,
    pub action: Action,
    #[serde(default = "Scope::global")]
    pub scope: Scope,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotkeyRule {
    pub id: String,
    /// "Ctrl+Alt+T", "F1", "Win+Shift+S" ...
    pub keys: String,
    pub name: String,
    pub action: Action,
    #[serde(default = "Scope::global")]
    pub scope: Scope,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gestures {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Pixels (at 100% scaling) the pointer must travel with the button held
    /// before it counts as a gesture rather than a right click.
    #[serde(default = "default_min_distance")]
    pub min_distance: f64,
    /// Holding the button still this long hands it back to the program as an
    /// ordinary right-button press (for right-drag in apps that use it).
    #[serde(default = "default_hold_ms")]
    pub hold_ms: u64,
    /// Recognise the four diagonals as well as up/down/left/right.
    #[serde(default = "yes")]
    pub diagonals: bool,
    #[serde(default = "yes")]
    pub trail: bool,
    #[serde(default = "default_trail_color")]
    pub trail_color: String,
    #[serde(default = "default_trail_width")]
    pub trail_width: f64,
    /// Show the recognised gesture and what it will do next to the pointer.
    #[serde(default = "yes")]
    pub hint: bool,
    /// Stand aside in full-screen programs (games, video).
    #[serde(default = "yes")]
    pub skip_fullscreen: bool,
    /// Programs in which gestures never start at all.
    #[serde(default = "default_gesture_excludes")]
    pub exclude_apps: Vec<String>,
    #[serde(default)]
    pub rules: Vec<GestureRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hotkeys {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Hotkeys stand aside while one of these is in front.
    #[serde(default = "default_hotkey_excludes")]
    pub exclude_apps: Vec<String>,
    #[serde(default)]
    pub rules: Vec<HotkeyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Where "save" puts files. Empty = Pictures\EarthDesk.
    #[serde(default)]
    pub save_dir: String,
    /// {yyyy} {MM} {dd} {HH} {mm} {ss} {ms} are replaced.
    #[serde(default = "default_file_name")]
    pub file_name: String,
    /// "png" or "jpg".
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_jpeg_quality")]
    pub jpeg_quality: u8,
    /// Highlight windows and controls under the pointer to select them with
    /// one click.
    #[serde(default = "yes")]
    pub detect_elements: bool,
    #[serde(default = "yes")]
    pub magnifier: bool,
    #[serde(default)]
    pub include_cursor: bool,
    /// Past selections kept for "," / "." in the capture screen.
    #[serde(default = "default_history")]
    pub history: usize,
    /// Copy to the clipboard whenever a screenshot is saved as a file too.
    #[serde(default)]
    pub copy_on_save: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clipboard {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_clip_items")]
    pub max_items: usize,
    #[serde(default = "default_clip_days")]
    pub max_days: u32,
    #[serde(default = "yes")]
    pub images: bool,
    /// Larger images are not kept (megabytes of pixels, not file size).
    #[serde(default = "default_clip_image_mb")]
    pub max_image_mb: u32,
    /// Copies made in these programs are never recorded.
    #[serde(default = "default_clip_excludes")]
    pub exclude_apps: Vec<String>,
    /// Picking an item in the panel also pastes it into the window that was
    /// in front.
    #[serde(default = "yes")]
    pub paste_on_pick: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolkitConfig {
    #[serde(default)]
    pub defaults_level: u32,
    /// Start with administrator rights (through a scheduled task, no UAC
    /// prompt) so gestures, hotkeys and screenshots also work over
    /// elevated windows such as Task Manager.
    #[serde(default = "yes")]
    pub elevate: bool,
    #[serde(default = "Gestures::default")]
    pub gestures: Gestures,
    #[serde(default = "Hotkeys::default")]
    pub hotkeys: Hotkeys,
    #[serde(default = "Capture::default")]
    pub capture: Capture,
    #[serde(default = "Clipboard::default")]
    pub clipboard: Clipboard,
}

fn yes() -> bool {
    true
}
fn default_min_distance() -> f64 {
    10.0
}
fn default_hold_ms() -> u64 {
    450
}
fn default_trail_color() -> String {
    "#3d8bff".into()
}
fn default_trail_width() -> f64 {
    4.0
}
fn default_gesture_excludes() -> Vec<String> {
    // Remote desktops and virtual machines: the gesture belongs to the
    // machine on the other side.
    ["mstsc.exe", "vmconnect.exe", "vmware.exe", "vmware-vmx.exe", "virtualboxvm.exe", "anydesk.exe", "todesk.exe", "sunloginclient.exe"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
fn default_hotkey_excludes() -> Vec<String> {
    ["mstsc.exe", "vmconnect.exe", "vmware.exe", "virtualboxvm.exe"].iter().map(|s| s.to_string()).collect()
}
fn default_file_name() -> String {
    "截图_{yyyy}{MM}{dd}_{HH}{mm}{ss}".into()
}
fn default_format() -> String {
    "png".into()
}
fn default_jpeg_quality() -> u8 {
    92
}
fn default_history() -> usize {
    50
}
fn default_clip_items() -> usize {
    1000
}
fn default_clip_days() -> u32 {
    30
}
fn default_clip_image_mb() -> u32 {
    40
}
fn default_clip_excludes() -> Vec<String> {
    ["keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe", "enpass.exe", "lastpass.exe"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "vivaldi.exe",
    "opera.exe",
    "360chrome.exe",
    "360se.exe",
    "qqbrowser.exe",
    "sogouexplorer.exe",
    "chromium.exe",
];

fn keys(k: &str) -> Action {
    Action::Keys { keys: k.into() }
}
fn builtin(n: &str) -> Action {
    Action::Builtin { name: n.into() }
}

/// Built-in gesture rules, with the defaults level that introduced each.
fn default_gestures() -> Vec<(u32, GestureRule)> {
    let g = |id: &str, gesture: &str, name: &str, action: Action, scope: Scope| GestureRule {
        id: format!("default.{id}"),
        gesture: gesture.into(),
        name: name.into(),
        action,
        scope,
        enabled: true,
    };
    vec![
        (1, g("back", "←", "返回", keys("Alt+Left"), Scope::global())),
        (1, g("forward", "→", "前进", keys("Alt+Right"), Scope::global())),
        (1, g("refresh", "↑↓", "刷新", keys("F5"), Scope::global())),
        (1, g("close", "↓→", "关闭窗口", builtin("window_close"), Scope::global())),
        (1, g("close_tab", "↓→", "关闭标签页", keys("Ctrl+W"), Scope::only(BROWSERS))),
        (1, g("reopen_tab", "↓←", "恢复关闭的标签页", keys("Ctrl+Shift+T"), Scope::only(BROWSERS))),
        (1, g("prev_tab", "↑←", "上一个标签页", keys("Ctrl+Shift+Tab"), Scope::only(BROWSERS))),
        (1, g("next_tab", "↑→", "下一个标签页", keys("Ctrl+Tab"), Scope::only(BROWSERS))),
        (1, g("minimize", "↓", "最小化", builtin("window_minimize"), Scope::global())),
        (1, g("maximize", "↑", "最大化 / 还原", builtin("window_maximize"), Scope::global())),
        (1, g("top", "→↑", "到顶部", keys("Ctrl+Home"), Scope::global())),
        (1, g("bottom", "→↓", "到底部", keys("Ctrl+End"), Scope::global())),
    ]
}

fn default_hotkeys() -> Vec<(u32, HotkeyRule)> {
    let h = |id: &str, k: &str, name: &str, action: Action, enabled: bool| HotkeyRule {
        id: format!("default.{id}"),
        keys: k.into(),
        name: name.into(),
        action,
        scope: Scope::global(),
        enabled,
    };
    vec![
        // Each arrives with the part of the toolkit that implements it.
        (2, h("capture", "F1", "截图", builtin("capture"), true)),
        (3, h("pin", "F3", "把剪贴板贴到屏幕", builtin("pin_clipboard"), true)),
        (4, h("clip_panel", "Alt+V", "剪贴板历史", builtin("clipboard_panel"), true)),
        (
            1,
            h(
                "terminal",
                "Ctrl+Alt+T",
                "打开终端（示例，默认关闭）",
                Action::Run { path: "wt.exe".into(), args: String::new(), cwd: String::new(), admin: false },
                false,
            ),
        ),
    ]
}

impl Default for Gestures {
    fn default() -> Self {
        Gestures {
            enabled: true,
            min_distance: default_min_distance(),
            hold_ms: default_hold_ms(),
            diagonals: true,
            trail: true,
            trail_color: default_trail_color(),
            trail_width: default_trail_width(),
            hint: true,
            skip_fullscreen: true,
            exclude_apps: default_gesture_excludes(),
            rules: vec![],
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys { enabled: true, exclude_apps: default_hotkey_excludes(), rules: vec![] }
    }
}

impl Default for Capture {
    fn default() -> Self {
        Capture {
            enabled: true,
            save_dir: String::new(),
            file_name: default_file_name(),
            format: default_format(),
            jpeg_quality: default_jpeg_quality(),
            detect_elements: true,
            magnifier: true,
            include_cursor: false,
            history: default_history(),
            copy_on_save: false,
        }
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Clipboard {
            enabled: true,
            max_items: default_clip_items(),
            max_days: default_clip_days(),
            images: true,
            max_image_mb: default_clip_image_mb(),
            exclude_apps: default_clip_excludes(),
            paste_on_pick: true,
        }
    }
}

impl Default for ToolkitConfig {
    fn default() -> Self {
        ToolkitConfig {
            defaults_level: 0,
            elevate: true,
            gestures: Gestures::default(),
            hotkeys: Hotkeys::default(),
            capture: Capture::default(),
            clipboard: Clipboard::default(),
        }
    }
}

pub fn path() -> PathBuf {
    crate::config::config_path().with_file_name("toolkit.json")
}

impl ToolkitConfig {
    /// Offer the built-in rules this file has not seen yet.
    fn add_new_defaults(&mut self) -> bool {
        if self.defaults_level >= DEFAULTS_LEVEL {
            return false;
        }
        let seen = self.defaults_level;
        for (level, rule) in default_gestures() {
            if level > seen && level <= DEFAULTS_LEVEL && !self.gestures.rules.iter().any(|r| r.id == rule.id) {
                self.gestures.rules.push(rule);
            }
        }
        for (level, rule) in default_hotkeys() {
            if level > seen && level <= DEFAULTS_LEVEL && !self.hotkeys.rules.iter().any(|r| r.id == rule.id) {
                self.hotkeys.rules.push(rule);
            }
        }
        self.defaults_level = DEFAULTS_LEVEL;
        true
    }

    pub fn load() -> Self {
        let p = path();
        let mut cfg = match std::fs::read_to_string(&p) {
            Ok(text) => match serde_json::from_str::<ToolkitConfig>(text.trim_start_matches('\u{feff}')) {
                Ok(c) => c,
                Err(e) => {
                    // Never silently throw away someone's rules: keep the file
                    // that failed to parse next to the fresh one.
                    eprintln!("toolkit.json parse failed ({e}); keeping a copy and starting fresh");
                    let _ = std::fs::copy(&p, p.with_extension("json.broken"));
                    ToolkitConfig::default()
                }
            },
            Err(_) => ToolkitConfig::default(),
        };
        cfg.normalise();
        if cfg.add_new_defaults() || !p.exists() {
            cfg.save();
        }
        cfg
    }

    /// Lower-case app names so every comparison is a plain string compare.
    pub fn normalise(&mut self) {
        let low = |v: &mut Vec<String>| {
            for a in v.iter_mut() {
                *a = a.trim().to_lowercase();
            }
            v.retain(|a| !a.is_empty());
        };
        low(&mut self.gestures.exclude_apps);
        low(&mut self.hotkeys.exclude_apps);
        low(&mut self.clipboard.exclude_apps);
        for r in &mut self.gestures.rules {
            low(&mut r.scope.apps);
        }
        for r in &mut self.hotkeys.rules {
            low(&mut r.scope.apps);
        }
        self.gestures.min_distance = self.gestures.min_distance.clamp(3.0, 80.0);
        self.gestures.trail_width = self.gestures.trail_width.clamp(1.0, 16.0);
        self.gestures.hold_ms = self.gestures.hold_ms.clamp(150, 3000);
        self.capture.jpeg_quality = self.capture.jpeg_quality.clamp(30, 100);
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            // Write-then-rename so a crash mid-write cannot leave half a file.
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    /// The gesture rule to run for `gesture` over `exe`: rules that name the
    /// program beat global ones, and among equals the first in the list wins.
    /// Like `gesture_rule`, but when the direction string names no rule,
    /// the rule whose shape is closest to the stroke (see gesture.rs).
    pub fn gesture_rule_for(&self, gesture: &str, points: &[(f64, f64)], exe: &str) -> Option<&GestureRule> {
        if let Some(r) = self.gesture_rule(gesture, exe) {
            return Some(r);
        }
        let usable: Vec<&str> = self
            .gestures
            .rules
            .iter()
            .filter(|r| r.enabled && !r.gesture.is_empty() && r.scope.rank(exe).is_some())
            .map(|r| r.gesture.as_str())
            .collect();
        let g = crate::toolkit::gesture::best_shape(points, usable.into_iter())?;
        self.gesture_rule(g, exe)
    }

    pub fn gesture_rule(&self, gesture: &str, exe: &str) -> Option<&GestureRule> {
        self.gestures
            .rules
            .iter()
            .filter(|r| r.enabled && r.gesture == gesture)
            .filter_map(|r| r.scope.rank(exe).map(|k| (k, r)))
            .fold(None::<(u8, &GestureRule)>, |best, (k, r)| match best {
                Some((bk, _)) if bk >= k => best,
                _ => Some((k, r)),
            })
            .map(|(_, r)| r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_rules_beat_global_ones() {
        let mut c = ToolkitConfig::default();
        c.add_new_defaults();
        assert_eq!(c.gesture_rule("↓→", "chrome.exe").unwrap().id, "default.close_tab");
        assert_eq!(c.gesture_rule("↓→", "notepad.exe").unwrap().id, "default.close");
        assert!(c.gesture_rule("↖", "notepad.exe").is_none());
    }

    #[test]
    fn deleted_defaults_stay_deleted() {
        let mut c = ToolkitConfig::default();
        c.add_new_defaults();
        c.gestures.rules.retain(|r| r.id != "default.back");
        assert!(!c.add_new_defaults());
        assert!(c.gestures.rules.iter().all(|r| r.id != "default.back"));
    }

    #[test]
    fn except_scope() {
        let s = Scope { mode: "except".into(), apps: vec!["code.exe".into()] };
        assert_eq!(s.rank("code.exe"), None);
        assert_eq!(s.rank("chrome.exe"), Some(1));
    }
}
