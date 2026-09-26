//! EarthDeskIME.exe — the engine process of 地球桌面输入法.
//!
//! Runs once per logon session (a named mutex keeps it single), without
//! elevation. The TSF DLL in each program connects over a named pipe; this
//! process owns librime, the user's dictionaries and the candidate window.
//!
//!   EarthDeskIME.exe            serve (started at logon, or by the DLL)
//!   EarthDeskIME.exe --deploy   build the dictionaries and exit (installer)
//!   EarthDeskIME.exe --register / --unregister / --enable / --disable
//!                               see win/setup.rs (installer)
//!   EarthDeskIME --cli KEYS     (any platform) type KEYS and print what
//!                               Rime answers; used by the tests

#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod compose;
mod kaomoji;
mod mixed;
mod mozc;
mod paths;
mod predict;
mod rime;
mod rime_sys;
mod session;
#[cfg(windows)]
mod win;

use std::io::Write;

/// Append a line to ime.log (next to the user data). Best effort.
pub fn log(line: &str) {
    let p = paths::log_dir().join("ime.log");
    if let Ok(meta) = std::fs::metadata(&p) {
        if meta.len() > 2 << 20 {
            let _ = std::fs::rename(&p, p.with_extension("log.old"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} {line}");
    }
}

pub const SCHEMA: &str = "double_pinyin_mspy";

fn start_rime() -> Result<rime::Rime, String> {
    let r = rime::Rime::load(&paths::rime_library())?;
    let user = paths::user_dir();
    let _ = std::fs::create_dir_all(&user);
    paths::write_customisations(&user, &paths::load_settings());
    r.start(&paths::shared_dir(), &user, &paths::log_dir());
    log(&format!("librime {} shared={} user={}", r.version(), paths::shared_dir().display(), user.display()));
    Ok(r)
}

/// Mozc, if its library is there (the input method works without it:
/// Chinese only).
pub fn start_mozc() -> Option<mozc::Mozc> {
    let lib = paths::mozc_library();
    if !lib.exists() {
        log(&format!("no Japanese engine ({} missing)", lib.display()));
        return None;
    }
    match mozc::Mozc::load(&lib, &paths::mozc_profile()) {
        Ok(m) => {
            log(&format!("mozc {}", m.data_version()));
            Some(m)
        }
        Err(e) => {
            log(&format!("mozc: {e}"));
            None
        }
    }
}

/// The candidate window of --cli: remembers what it was asked to show.
#[derive(Default)]
struct CliUi(std::sync::Mutex<Option<session::View>>);

impl session::Ui for std::sync::Arc<CliUi> {
    fn show(&self, view: session::View, _caret: Option<session::Caret>) {
        *self.0.lock().unwrap() = Some(view);
    }
    fn move_to(&self, _caret: session::Caret) {}
    fn hide(&self) {
        *self.0.lock().unwrap() = None;
    }
    fn notify(&self, _notify: u64) {}
}

/// `--cli KEYS`: type KEYS through the whole engine (letters, digits,
/// punctuation, and {space} {BackSpace} {Return} {Escape} {Page_Down}
/// {Down} {Shift_L} {Shift_L_up} {C-S-j}, {U-32} for a release) and print
/// the result after each.
fn cli(keys: &str) -> Result<(), String> {
    use ime_proto::{keysym as k, mask};
    let r = start_rime()?;
    r.maintain(false, true);
    let ui = std::sync::Arc::new(CliUi::default());
    let e = session::Engine::new(r, start_mozc(), Box::new(ui.clone()), SCHEMA);
    e.predictor.wait(std::time::Duration::from_secs(60));
    let sid = e.test_session(&std::env::var("EARTHDESK_IME_EXE").unwrap_or_else(|_| "cli.exe".into()));
    let mut chars = keys.chars().peekable();
    while let Some(c) = chars.next() {
        let (code, m, label) = if c == '{' {
            let name: String = chars.by_ref().take_while(|c| *c != '}').collect();
            let (code, m) = match name.as_str() {
                "space" => (0x20, 0),
                "BackSpace" => (k::BACKSPACE, 0),
                "Return" => (k::RETURN, 0),
                "Escape" => (k::ESCAPE, 0),
                "Page_Down" => (k::PAGE_DOWN, 0),
                "Down" => (k::DOWN, 0),
                "Up" => (k::UP, 0),
                "Left" => (k::LEFT, 0),
                "Right" => (k::RIGHT, 0),
                "F7" => (k::F1 + 6, 0),
                "Shift_L" => (k::SHIFT_L, 0),
                "Shift_L_up" => (k::SHIFT_L, mask::RELEASE | mask::SHIFT),
                "C-S-j" => ('J' as u32, mask::CONTROL | mask::SHIFT),
                "C-v" => ('v' as u32, mask::CONTROL),
                "C-v_up" => ('v' as u32, mask::CONTROL | mask::RELEASE),
                "Control_L" => (k::CONTROL_L, mask::CONTROL),
                "Control_L0" => (k::CONTROL_L, 0),
                "Control_L_up" => (k::CONTROL_L, mask::CONTROL | mask::RELEASE),
                "KP_Decimal" => (0xffae, 0),
                // {S-63}: Shift + key code 63 ('?'); {L-97}: with Caps Lock on.
                other if other.starts_with("S-") => (other[2..].parse::<u32>().unwrap_or(0), mask::SHIFT),
                other if other.starts_with("L-") => (other[2..].parse::<u32>().unwrap_or(0), mask::LOCK),
                // {U-32}: the release of key code 32.
                other if other.starts_with("U-") => (other[2..].parse::<u32>().unwrap_or(0), mask::RELEASE),
                "Caps_Lock" => (k::CAPS_LOCK, 0),
                "Caps_Lock_up" => (k::CAPS_LOCK, mask::LOCK | mask::RELEASE),
                "Caps_off" => (k::CAPS_LOCK, mask::LOCK),
                "Caps_off_up" => (k::CAPS_LOCK, mask::RELEASE),
                other => (other.parse::<u32>().unwrap_or(0), 0),
            };
            (code, m, name)
        } else {
            (c as u32, if c.is_ascii_uppercase() { mask::SHIFT } else { 0 }, c.to_string())
        };
        let reply = e.handle(ime_proto::Request::Key { session: sid, keycode: code, mask: m });
        let ime_proto::Reply::State(st) = reply else {
            println!("{label:>9} {reply:?}");
            continue;
        };
        let view = ui.0.lock().unwrap().clone();
        let cands: Vec<String> = view
            .map(|v| {
                v.candidates
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}{}{}", if i as i32 == v.highlighted { "*" } else { "" }, c.0, if c.1.is_empty() { String::new() } else { format!("({})", c.1) }))
                    .collect()
            })
            .unwrap_or_default();
        println!(
            "{label:>9} eaten={} commit={:?}{} preedit={:?} cands={}",
            st.eaten as u8,
            st.commit.unwrap_or_default(),
            if st.delete_before > 0 { format!(" del={}", st.delete_before) } else { String::new() },
            st.preedit.map(|p| p.text).unwrap_or_default(),
            cands.join(" ")
        );
    }
    Ok(())
}

/// `--bench ITEMS [zh|mixed]`: type every word of ITEMS (made by
/// tools/ime-bench/mkitems.py from the user's own writing) the way the user
/// would, pick it from the candidates if it is there, and report how often it
/// was the 1st, 2nd, 3rd ... candidate. Only the totals are printed (and the
/// words that most often missed, to tune against).
fn bench(file: &str, mode: &str) -> Result<(), String> {
    use ime_proto::{keysym as k, Reply, Request};
    use mixed::Lang;
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let r = start_rime()?;
    r.maintain(false, true);
    let ui = std::sync::Arc::new(CliUi::default());
    let mz = if mode == "zh" { None } else { start_mozc() };
    let e = session::Engine::new(r, mz, Box::new(ui.clone()), SCHEMA);
    let mut sid = e.test_session("bench.exe");
    if mode == "zh" {
        e.bench_set_mode(sid, compose::Mode::Zh);
    }
    let key = |sid: u64, code: u32| e.handle(Request::Key { session: sid, keycode: code, mask: 0 });
    // rank histogram per kind: [1st, 2nd, 3rd, 4th..page, not on the first page]
    let mut hist: std::collections::BTreeMap<&str, [u32; 5]> = Default::default();
    let mut misses: std::collections::HashMap<String, u32> = Default::default();
    let mut keys_typed = 0u64;
    let mut picks = 0u64;
    for line in text.lines() {
        let mut f = line.splitn(3, '\t');
        let (kind, code, word) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
        match kind {
            "#" => {
                e.handle(Request::Bye { session: sid });
                sid = e.test_session("bench.exe");
                if mode == "zh" {
                    e.bench_set_mode(sid, compose::Mode::Zh);
                }
                continue;
            }
            "C" => {
                e.bench_commit(sid, word, Lang::Zh);
                continue;
            }
            "Z" | "E" => {}
            _ => continue,
        }
        let label = if kind == "Z" { "中文" } else { "英文" };
        for c in code.chars() {
            key(sid, c as u32);
            keys_typed += 1;
        }
        let view = ui.0.lock().unwrap().clone();
        let rank = view.as_ref().and_then(|v| v.candidates.iter().position(|c| c.0 == word));
        let h = hist.entry(label).or_default();
        match rank {
            Some(i) => {
                h[i.min(3)] += 1;
                picks += 1;
                // Space for the first, the digit otherwise.
                let code = if i == 0 { 0x20 } else { '1' as u32 + i as u32 };
                match key(sid, code) {
                    Reply::State(st) if st.commit.as_deref() == Some(word) => {}
                    _ => e.bench_commit(sid, word, if kind == "Z" { Lang::Zh } else { Lang::En }),
                }
            }
            None => {
                h[4] += 1;
                *misses.entry(format!("{word}({code})")).or_default() += 1;
                key(sid, k::ESCAPE);
                e.bench_commit(sid, word, if kind == "Z" { Lang::Zh } else { Lang::En });
            }
        }
    }
    println!("mode={mode}  keys={keys_typed}  picks={picks}");
    for (label, h) in &hist {
        let n: u32 = h.iter().sum();
        let pct = |x: u32| 100.0 * x as f64 / n.max(1) as f64;
        println!(
            "{label}: {n} words  首选 {:.1}%  二选 {:.1}%  三选 {:.1}%  4-{} 选 {:.1}%  首页没有 {:.1}%  (前三合计 {:.1}%)",
            pct(h[0]), pct(h[1]), pct(h[2]), e.page_size(), pct(h[3]), pct(h[4]), pct(h[0] + h[1] + h[2])
        );
    }
    let mut m: Vec<_> = misses.into_iter().collect();
    m.sort_by(|a, b| b.1.cmp(&a.1));
    println!("most missed: {}", m.iter().take(25).map(|(w, n)| format!("{w}x{n}")).collect::<Vec<_>>().join(" "));
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--bench") {
        let file = args.get(i + 1).cloned().unwrap_or_default();
        let mode = args.get(i + 2).cloned().unwrap_or_else(|| "mixed".into());
        if let Err(e) = bench(&file, &mode) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--cli") {
        if let Err(e) = cli(args.get(i + 1).map(String::as_str).unwrap_or("")) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    {
        let steps: [(&str, fn() -> bool); 4] =
            [("--register", win::setup::register), ("--unregister", win::setup::unregister), ("--enable", win::setup::enable), ("--disable", win::setup::disable)];
        let mut any = false;
        let mut ok = true;
        for (flag, f) in steps {
            if args.iter().any(|a| a == flag) {
                any = true;
                ok &= f();
            }
        }
        if any && !args.iter().any(|a| a == "--deploy") {
            std::process::exit(if ok { 0 } else { 1 });
        }
    }
    if args.iter().any(|a| a == "--deploy") {
        match start_rime() {
            Ok(r) => {
                r.maintain(true, true);
                log("deploy (installer) finished");
            }
            Err(e) => {
                log(&e);
                std::process::exit(1);
            }
        }
        return;
    }
    #[cfg(windows)]
    win::serve();
    #[cfg(not(windows))]
    eprintln!("the engine only serves on Windows; use --cli to test");
}
