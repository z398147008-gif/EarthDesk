//! Where things live.
//!
//! Installed:  <install dir>\ime\EarthDeskIME.exe, rime.dll, data\ (the
//!             shipped schemas and dictionaries: read-only, shared)
//! Per user:   %APPDATA%\EarthDesk\ime\rime\ (user dictionary, our
//!             generated customisations, build output) and ime.log
//!
//! Environment overrides (tests, development): EARTHDESK_IME_SHARED,
//! EARTHDESK_IME_USER, EARTHDESK_RIME_LIB.

use std::path::{Path, PathBuf};

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

pub fn shared_dir() -> PathBuf {
    if let Ok(p) = std::env::var("EARTHDESK_IME_SHARED") {
        return PathBuf::from(p);
    }
    exe_dir().join("data")
}

fn base() -> PathBuf {
    #[cfg(windows)]
    let root = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(exe_dir);
    #[cfg(not(windows))]
    let root = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")).unwrap_or_else(exe_dir);
    root.join("EarthDesk").join("ime")
}

pub fn user_dir() -> PathBuf {
    if let Ok(p) = std::env::var("EARTHDESK_IME_USER") {
        return PathBuf::from(p);
    }
    base().join("rime")
}

pub fn log_dir() -> PathBuf {
    let d = if std::env::var("EARTHDESK_IME_USER").is_ok() { user_dir() } else { base() };
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A file of ours next to the log (mode per program, language habits).
pub fn data_file(name: &str) -> PathBuf {
    log_dir().join(name)
}

/// Mozc's conversion engine (earthdesk_mozc.dll, see ime/mozc/). Optional:
/// without it the input method is Chinese only.
pub fn mozc_library() -> PathBuf {
    if let Ok(p) = std::env::var("EARTHDESK_MOZC_LIB") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    return exe_dir().join("earthdesk_mozc.dll");
    #[cfg(not(windows))]
    exe_dir().join("libearthdesk_mozc.so")
}

/// Mozc's learning data and user dictionary.
pub fn mozc_profile() -> PathBuf {
    log_dir().join("mozc")
}

/// Written by the EarthDesk settings page.
pub fn settings_path() -> PathBuf {
    base().join("settings.json")
}

pub fn load_settings() -> ime_proto::Settings {
    std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn rime_library() -> PathBuf {
    if let Ok(p) = std::env::var("EARTHDESK_RIME_LIB") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    return exe_dir().join("rime.dll");
    #[cfg(not(windows))]
    PathBuf::from("librime.so.1")
}

/// Our settings on top of the shipped rime-ice configuration, as Rime
/// "custom" patch files in the user folder. Rewritten on every start (and
/// on Deploy) so an update of EarthDesk or a change on the settings page
/// reaches them; files the user writes by hand under other names are left
/// alone.
pub fn write_customisations(user: &Path, st: &ime_proto::Settings) {
    let page = st.page_size.clamp(3, 9);
    let shift = if st.shift_toggle { "commit_code" } else { "noop" };
    let default = format!(
        r#"# 由地球桌面生成，每次启动会覆盖；请在 地球桌面 设置 → 输入法 里修改。
patch:
  schema_list:
    - schema: double_pinyin_mspy
  menu/page_size: {page}
  # Caps Lock 不管中英文，只管大小写。
  ascii_composer/good_old_caps_lock: true
  ascii_composer/switch_key:
    Shift_L: {shift}
    Shift_R: {shift}
    Control_L: noop
    Control_R: noop
    Caps_Lock: clear
    Eisu_toggle: clear
"#
    );
    let emoji = if st.emoji { 1 } else { 0 };
    let schema = format!(
        r#"# 由地球桌面生成，每次启动会覆盖；请在 地球桌面 设置 → 输入法 里修改。
patch:
  # 用户词库：按使用频率和最近使用自动调整顺序，自动造词。
  translator/enable_user_dict: true
  translator/enable_sentence: true
  translator/enable_encoder: true
  translator/encode_commit_history: true
  # librime 自带的相邻键纠错会把纠错结果排到正确结果前面（ni → 不），
  # 所以关掉；纠错由地球桌面自己按上下文做（第三阶段）。
  translator/enable_correction: false
  switches/@3/reset: {emoji}
  menu/page_size: {page}
"#
    );
    // The schema's quick phrases (text<Tab>code<Tab>weight, always first).
    // The user's file: created once, never overwritten.
    let phrases = user.join("custom_phrase_double.txt");
    if !phrases.exists() {
        let _ = std::fs::write(
            &phrases,
            "# Rime table\n# coding: utf-8\n#@/db_name\tcustom_phrase_double.txt\n#@/db_type\ttabledb\n#\n\
             # 地球桌面输入法 快捷短语：每行 文字<Tab>编码<Tab>权重，打出编码时排在第一位。\n\
             # 例：\n# 我的邮箱是 someone@example.com\tyx\t1\n#\n",
        );
    }
    write_if_changed(&user.join("default.custom.yaml"), &default);
    write_if_changed(&user.join("double_pinyin_mspy.custom.yaml"), &schema);
}

/// Rime redeploys when a file's time changes; do not touch it for nothing.
fn write_if_changed(p: &Path, text: &str) {
    if std::fs::read_to_string(p).ok().as_deref() != Some(text) {
        let _ = std::fs::write(p, text);
    }
}
