//! 联想: after Chinese goes in, the words likely to come next, picked with
//! the digit keys.
//!
//! Two sources, the user's own habits first:
//! - what the user typed after what (learned as words go in, nextword.log;
//!   and, if tools/ime-bench/mkpersonal.py made it from the user's writing,
//!   nextword_personal.tsv in the Rime folder);
//! - the phrase dictionaries (rime-ice's base and ext, and the personal
//!   dictionary): a phrase that starts with the word just typed continues
//!   it ("中秋" -> 中秋节 / 中秋快乐 / 中秋月饼: 节, 快乐, 月饼), weighted by
//!   how common the phrase is. The word just typed is the longest end of the
//!   sentence that is a word itself ("今天天气" -> 天气 -> 预报, 很好), so
//!   the context is a whole word, never half of one.
//!
//! The dictionaries (a million phrases) load in the background at start;
//! until then only the learned habits are used.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Phrases sorted by text, for prefix lookups.
struct Phrases {
    text: String,
    /// (offset, length in bytes, weight) into `text`, sorted by the text.
    idx: Vec<(u32, u32, f32)>,
}

impl Phrases {
    fn at(&self, i: usize) -> &str {
        let (o, l, _) = self.idx[i];
        &self.text[o as usize..(o + l) as usize]
    }

    /// Entries whose text starts with `prefix` (the prefix itself included).
    fn range(&self, prefix: &str) -> std::ops::Range<usize> {
        let lo = self.idx.partition_point(|&(o, l, _)| &self.text[o as usize..(o + l) as usize] < prefix);
        let mut hi = lo;
        while hi < self.idx.len() && self.at(hi).starts_with(prefix) {
            hi += 1;
        }
        lo..hi
    }

    fn contains(&self, word: &str) -> bool {
        let r = self.range(word);
        r.start < r.end && self.at(r.start) == word
    }

    /// Rime dictionary files (`text<Tab>code<Tab>weight` after the "..."
    /// line); `boost` multiplies their weights. Only Chinese phrases of two
    /// characters or more are kept.
    fn load(files: &[(PathBuf, f32)]) -> Phrases {
        let mut rows: Vec<(String, f32)> = Vec::new();
        for (path, boost) in files {
            let Ok(data) = std::fs::read_to_string(path) else { continue };
            let body = match data.find("\n...") {
                Some(i) => &data[i + 4..],
                None => &data[..],
            };
            for line in body.lines() {
                if line.starts_with('#') {
                    continue;
                }
                let mut cols = line.split('\t');
                let Some(text) = cols.next() else { continue };
                if text.chars().count() < 2 || text.chars().count() > 8 || !text.chars().all(is_han) {
                    continue;
                }
                let weight = cols.filter_map(|c| c.trim().parse::<f32>().ok()).next_back().unwrap_or(1.0);
                rows.push((text.to_string(), (weight.max(0.0) + 1.0) * boost));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut text = String::new();
        let mut idx: Vec<(u32, u32, f32)> = Vec::with_capacity(rows.len());
        for (t, w) in rows {
            if let Some(last) = idx.last_mut() {
                if &text[last.0 as usize..(last.0 + last.1) as usize] == t.as_str() {
                    last.2 = last.2.max(w);
                    continue;
                }
            }
            idx.push((text.len() as u32, t.len() as u32, w));
            text.push_str(&t);
        }
        Phrases { text, idx }
    }
}

pub fn is_han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{3134F}')
}

/// What the user typed after what: word (its last four characters) -> next
/// -> times.
#[derive(Default)]
struct Learned {
    map: HashMap<String, HashMap<String, u32>>,
    /// Appended to as the user types (one "word<Tab>next" line each).
    log: Option<PathBuf>,
    lines: usize,
}

impl Learned {
    fn add(&mut self, word: &str, next: &str, n: u32) {
        *self.map.entry(word.to_string()).or_default().entry(next.to_string()).or_insert(0) += n;
    }

    fn load(log: &Path, personal: &Path) -> Learned {
        let mut l = Learned { log: Some(log.to_path_buf()), ..Default::default() };
        // From the user's writing: word<Tab>next<Tab>count.
        if let Ok(data) = std::fs::read_to_string(personal) {
            for line in data.lines() {
                let mut c = line.split('\t');
                if let (Some(w), Some(n), Some(k)) = (c.next(), c.next(), c.next()) {
                    if let Ok(k) = k.trim().parse::<u32>() {
                        l.add(w, n, k.min(20));
                    }
                }
            }
        }
        if let Ok(data) = std::fs::read_to_string(log) {
            for line in data.lines() {
                if let Some((w, n)) = line.split_once('\t') {
                    l.add(w, n, 3);
                    l.lines += 1;
                }
            }
        }
        l
    }

    fn record(&mut self, word: &str, next: &str) {
        self.add(word, next, 3);
        let Some(log) = self.log.clone() else { return };
        self.lines += 1;
        if self.lines > 50_000 {
            // Too long: keep what is typed most, once each.
            let mut all: Vec<(String, String, u32)> = self.map.iter().flat_map(|(w, m)| m.iter().map(move |(n, k)| (w.clone(), n.clone(), *k))).collect();
            all.sort_by(|a, b| b.2.cmp(&a.2));
            all.truncate(20_000);
            let body: String = all.iter().map(|(w, n, _)| format!("{w}\t{n}\n")).collect();
            let _ = std::fs::write(&log, body);
            self.lines = all.len();
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{word}\t{next}");
        }
    }
}

pub struct Predictor {
    phrases: Arc<OnceLock<Phrases>>,
    learned: Mutex<Learned>,
}

/// The last word of a commit, as learned contexts are keyed: its last four
/// Chinese characters.
fn key_of(word: &str) -> Option<String> {
    let han: Vec<char> = word.chars().rev().take_while(|c| is_han(*c)).collect();
    if han.is_empty() {
        return None;
    }
    Some(han.iter().take(4).rev().collect())
}

impl Predictor {
    /// Start loading the dictionaries in the background.
    pub fn start(dicts: Vec<(PathBuf, f32)>, log: &Path, personal: &Path) -> Predictor {
        let phrases = Arc::new(OnceLock::new());
        let slot = phrases.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let p = Phrases::load(&dicts);
            crate::log(&format!("prediction: {} phrases in {} ms", p.idx.len(), t.elapsed().as_millis()));
            let _ = slot.set(p);
        });
        Predictor { phrases, learned: Mutex::new(Learned::load(log, personal)) }
    }

    /// Wait (at most `limit`) for the dictionaries: the command-line test
    /// modes type at once.
    pub fn wait(&self, limit: std::time::Duration) {
        let t = std::time::Instant::now();
        while self.phrases.get().is_none() && t.elapsed() < limit {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// For tests: only these phrases, loaded now.
    #[cfg(test)]
    fn with(dict: &[(&str, f32)]) -> Predictor {
        let dir = std::env::temp_dir().join(format!("edime-predict-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("t.dict.yaml");
        let body: String = dict.iter().map(|(t, w)| format!("{t}\tx\t{w}\n")).collect();
        std::fs::write(&f, format!("---\nname: t\n...\n{body}")).unwrap();
        let log = dir.join("nextword.log");
        let _ = std::fs::remove_file(&log);
        let p = Predictor { phrases: Arc::new(OnceLock::new()), learned: Mutex::new(Learned::load(&log, &dir.join("none"))) };
        let _ = p.phrases.set(Phrases::load(&[(f, 1.0)]));
        p
    }

    /// `word` was typed right after `before` (both commits, Chinese).
    pub fn learn(&self, before: &str, word: &str) {
        let (Some(k), true) = (key_of(before), word.chars().count() <= 8 && word.chars().all(is_han)) else { return };
        if let Ok(mut l) = self.learned.lock() {
            l.record(&k, word);
        }
    }

    /// Up to `n` words likely to follow `recent` (the text just typed, its
    /// last commit being `last`).
    pub fn predict(&self, recent: &str, last: &str, n: usize) -> Vec<String> {
        let tail: Vec<char> = recent.chars().rev().take_while(|c| is_han(*c)).take(8).collect::<Vec<_>>().into_iter().rev().collect();
        if tail.is_empty() || n == 0 {
            return Vec::new();
        }
        let mut score: HashMap<String, f32> = HashMap::new();
        let mut bump = |w: String, s: f32| {
            let e = score.entry(w).or_insert(0.0);
            if s > *e {
                *e = s;
            }
        };
        // The user's own: what followed this word before.
        if let (Some(k), Ok(l)) = (key_of(last), self.learned.lock()) {
            if let Some(m) = l.map.get(&k) {
                for (next, times) in m {
                    bump(next.clone(), 30.0 + 10.0 * (*times).min(10) as f32);
                }
            }
        }
        if let Some(p) = self.phrases.get() {
            // The words the sentence ends in: its longest end that is a word,
            // and the next longest.
            let mut words: Vec<String> = Vec::new();
            for len in (2..=tail.len().min(6)).rev() {
                let w: String = tail[tail.len() - len..].iter().collect();
                if p.contains(&w) {
                    words.push(w);
                    if words.len() == 2 {
                        break;
                    }
                }
            }
            if words.is_empty() {
                words.push(tail[tail.len() - 1].to_string());
            }
            for w in &words {
                let wl = w.chars().count();
                for i in p.range(w) {
                    let phrase = p.at(i);
                    let rest = &phrase[w.len()..];
                    let rl = rest.chars().count();
                    // A single character goes on to a whole word, never to
                    // the second half of one (我 -> 们, 我的天 -> 的天).
                    if rl == 0 || rl > 4 || (wl == 1 && !p.contains(rest)) {
                        continue;
                    }
                    // A word of its own (快乐, 月饼) over a fragment of a
                    // longer phrase (中秋夜梦记: 夜梦记).
                    let mut s = p.idx[i].2.ln() + 3.0 * wl as f32;
                    if p.contains(rest) {
                        s += 3.0;
                    } else if rl > 1 {
                        s -= 2.0;
                    }
                    bump(rest.to_string(), s);
                }
            }
        }
        let mut list: Vec<(String, f32)> = score.into_iter().collect();
        list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        list.into_iter().take(n).map(|(w, _)| w).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_the_last_word() {
        let p = Predictor::with(&[
            ("中秋", 90000.0),
            ("中秋节", 50000.0),
            ("中秋快乐", 3000.0),
            ("中秋月饼", 800.0),
            ("快乐", 90000.0),
            ("天气", 80000.0),
            ("天气预报", 20000.0),
            ("天气很好", 900.0),
            ("今天", 90000.0),
            ("我们", 90000.0),
            ("我的", 90000.0),
            ("我的天", 5000.0),
        ]);
        // Words first (快乐 is one), then single characters and fragments.
        let got = p.predict("中秋", "中秋", 5);
        assert_eq!(got[0], "快乐", "{got:?}");
        assert!(got.contains(&"节".to_string()) && got.contains(&"月饼".to_string()), "{got:?}");
        // The sentence's last word, not its last character.
        let got = p.predict("今天天气", "今天天气", 3);
        assert_eq!(got[0], "预报", "{got:?}");
        // After a single character, no second halves of words (我 -> 们).
        assert!(!p.predict("我", "我", 5).contains(&"们".to_string()));
        // Learned habits lead.
        p.learn("中秋", "假期");
        assert_eq!(p.predict("中秋", "中秋", 3)[0], "假期");
        // Nothing Chinese at the end: nothing.
        assert!(p.predict("hello", "hello", 3).is_empty());
    }
}
