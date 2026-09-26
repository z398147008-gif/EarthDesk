//! 中日混合输入：同一串字母同时交给双拼（Rime）和罗马字（Mozc），再决定两边的
//! 候选怎么排在一起。三层判断，前一层分不出来才看下一层：
//!
//!   1. 拼不拼得通：双拼每个字两个键，罗马字辅音元音交替，大多数串只有一边
//!      读得通（wojntmqu 只能是中文，watashi 只能是日语）。
//!   2. 词库有没有：两边都读得通时，看哪边是完整的词。中文：整串长度的候选
//!      不止一个（词库里有这个编码的词，而不只是拼出来的句子）；日语：Mozc
//!      有一个一段、读音正好是整串的候选（不算单纯的假名转写）。
//!   3. 上下文和习惯：刚上屏的是哪种语言；这串编码以前多半选哪边。

use crate::mozc::{JaCand, JaView};
use crate::rime::Candidate;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    Zh,
    Ja,
    En,
}

/// One entry of the merged candidate list.
#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub lang: Lang,
    /// Zh: index in Rime's whole list. Ja: Mozc's candidate id.
    pub key: i64,
    pub text: String,
    pub comment: String,
}

const PINYIN: &[&str] = &[
    "a", "ai", "an", "ang", "ao", "ba", "bai", "ban", "bang", "bao", "bei", "ben", "beng", "bi",
    "bian", "biang", "biao", "bie", "bin", "bing", "bo", "bu", "ca", "cai", "can", "cang", "cao",
    "ce", "cei", "cen", "ceng", "cha", "chai", "chan", "chang", "chao", "che", "chen", "cheng",
    "chi", "chong", "chou", "chu", "chua", "chuai", "chuan", "chuang", "chui", "chun", "chuo",
    "ci", "cong", "cou", "cu", "cuan", "cui", "cun", "cuo", "da", "dai", "dan", "dang", "dao",
    "de", "dei", "den", "deng", "di", "dia", "dian", "diao", "die", "ding", "diu", "dong", "dou",
    "du", "duan", "dui", "dun", "duo", "e", "ei", "en", "eng", "er", "fa", "fan", "fang", "fei",
    "fen", "feng", "fiao", "fo", "fou", "fu", "ga", "gai", "gan", "gang", "gao", "ge", "gei",
    "gen", "geng", "gong", "gou", "gu", "gua", "guai", "guan", "guang", "gui", "gun", "guo", "ha",
    "hai", "han", "hang", "hao", "he", "hei", "hen", "heng", "hong", "hou", "hu", "hua", "huai",
    "huan", "huang", "hui", "hun", "huo", "ji", "jia", "jian", "jiang", "jiao", "jie", "jin",
    "jing", "jiong", "jiu", "ju", "juan", "jue", "jun", "ka", "kai", "kan", "kang", "kao", "ke",
    "kei", "ken", "keng", "kong", "kou", "ku", "kua", "kuai", "kuan", "kuang", "kui", "kun", "kuo",
    "la", "lai", "lan", "lang", "lao", "le", "lei", "leng", "li", "lia", "lian", "liang", "liao",
    "lie", "lin", "ling", "liu", "lo", "long", "lou", "lu", "luan", "lue", "lun", "luo", "lv",
    "lve", "lü", "lüe", "ma", "mai", "man", "mang", "mao", "me", "mei", "men", "meng", "mi",
    "mian", "miao", "mie", "min", "ming", "miu", "mo", "mou", "mu", "na", "nai", "nan", "nang",
    "nao", "ne", "nei", "nen", "neng", "ni", "nian", "niang", "niao", "nie", "nin", "ning", "niu",
    "nong", "nou", "nu", "nuan", "nue", "nuo", "nv", "nve", "nü", "nüe", "o", "ou", "pa", "pai",
    "pan", "pang", "pao", "pei", "pen", "peng", "pi", "pian", "piao", "pie", "pin", "ping", "po",
    "pou", "pu", "qi", "qia", "qian", "qiang", "qiao", "qie", "qin", "qing", "qiong", "qiu", "qu",
    "quan", "que", "qun", "ran", "rang", "rao", "re", "ren", "reng", "ri", "rong", "rou", "ru",
    "rua", "ruan", "rui", "run", "ruo", "sa", "sai", "san", "sang", "sao", "se", "sen", "seng",
    "sha", "shai", "shan", "shang", "shao", "she", "shei", "shen", "sheng", "shi", "shou", "shu",
    "shua", "shuai", "shuan", "shuang", "shui", "shun", "shuo", "si", "song", "sou", "su", "suan",
    "sui", "sun", "suo", "ta", "tai", "tan", "tang", "tao", "te", "tei", "teng", "ti", "tian",
    "tiao", "tie", "ting", "tong", "tou", "tu", "tuan", "tui", "tun", "tuo", "wa", "wai", "wan",
    "wang", "wei", "wen", "weng", "wo", "wu", "xi", "xia", "xian", "xiang", "xiao", "xie", "xin",
    "xing", "xiong", "xiu", "xu", "xuan", "xue", "xun", "ya", "yan", "yang", "yao", "ye", "yi",
    "yin", "ying", "yo", "yong", "you", "yu", "yuan", "yue", "yun", "za", "zai", "zan", "zang",
    "zao", "ze", "zei", "zen", "zeng", "zha", "zhai", "zhan", "zhang", "zhao", "zhe", "zhei",
    "zhen", "zheng", "zhi", "zhong", "zhou", "zhu", "zhua", "zhuai", "zhuan", "zhuang", "zhui",
    "zhun", "zhuo", "zi", "zong", "zou", "zu", "zuan", "zui", "zun", "zuo",
];

fn is_syllable(s: &str) -> bool {
    PINYIN.binary_search(&s).is_ok()
}

/// A syllable still being typed: a lone initial ("zh", "q"...).
fn is_partial(s: &str) -> bool {
    matches!(s, "b" | "p" | "m" | "f" | "d" | "t" | "n" | "l" | "g" | "k" | "h" | "j" | "q" | "x" | "zh" | "ch" | "sh" | "r" | "z" | "c" | "s" | "y" | "w")
        || PINYIN.iter().any(|p| p.starts_with(s))
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZhInfo {
    pub valid: bool,
    /// The last syllable is only half typed (双拼: an odd number of keys).
    pub partial: bool,
    pub syllables: usize,
    /// Candidates covering the whole input among the first few.
    pub whole: usize,
}

/// Rime's preedit spells the input out ("wo jin tian q"); letters it cannot
/// read stay glued together ("ninichiha").
pub fn zh_info(preedit: &str, cands: &[Candidate]) -> ZhInfo {
    let tokens: Vec<&str> = preedit.split([' ', '\'']).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return ZhInfo::default();
    }
    let last = tokens.len() - 1;
    let valid = tokens.iter().enumerate().all(|(i, t)| is_syllable(t) || (i == last && is_partial(t)));
    let syllables = tokens.iter().filter(|t| is_syllable(t)).count();
    let whole = cands.iter().take(12).filter(|c| c.text.chars().count() == syllables).count();
    let partial = !is_syllable(tokens[last]);
    ZhInfo { valid, partial, syllables, whole }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct JaInfo {
    pub valid: bool,
    /// A dictionary word for the whole reading.
    pub hit: bool,
    /// No letters left waiting for a vowel.
    pub complete: bool,
}

fn transliteration(c: &JaCand) -> bool {
    if c.value.chars().all(|ch| ch.is_ascii() || ('\u{FF01}'..='\u{FF5E}').contains(&ch)) {
        return true;
    }
    let d = c.description.as_str();
    d.contains("ひらがな") || d.contains("カタカナ") || d.contains("英数") || d.contains("[全]") || d.contains("[半]")
}

pub fn ja_info(v: &JaView) -> JaInfo {
    let valid = crate::mozc::romaji_valid(&v.preedit);
    // Mozc leaves a candidate's key empty when it is the reading itself.
    let complete = valid && !v.preedit.chars().last().map(|c| c.is_ascii_alphabetic() || ('\u{FF21}'..='\u{FF5A}').contains(&c)).unwrap_or(true);
    // A single kana (に = 二) says nothing: every letter pair is one.
    let hit = complete && v.reading.chars().count() >= 2 && v.candidates.iter().take(8).any(|c| c.segments <= 1 && (c.key.is_empty() || c.key == v.reading) && !transliteration(c));
    JaInfo { valid, hit, complete }
}

/// Context for the third layer.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    pub last: Option<Lang>,
    /// How often this exact input was picked as Chinese / Japanese before.
    pub zh_picks: u32,
    pub ja_picks: u32,
}

/// Scores (None = cannot be that language).
pub fn score(zh: ZhInfo, ja: JaInfo, ctx: Ctx) -> (Option<f32>, Option<f32>) {
    // Chinese is the default language: it leads unless something says
    // otherwise (a Japanese word, Japanese just typed, habit).
    let mut sz = zh.valid.then_some(1.4f32);
    let mut sj = ja.valid.then_some(1.0f32);
    if let Some(s) = sz.as_mut() {
        // The whole input is a dictionary word (or two readings of it).
        if zh.whole >= 2 {
            *s += 1.0;
        } else if zh.whole == 1 && !zh.partial {
            *s += 0.8;
        }
        // A single syllable is always "a word"; leave it to the context.
        if zh.syllables <= 1 {
            *s -= 0.3;
        }
    }
    if let Some(s) = sj.as_mut() {
        if ja.hit {
            *s += 1.0;
        }
    }
    // Words are typed whole: a half-typed last syllable on one side against
    // a finished reading on the other says which one is meant.
    if let (Some(z), Some(j)) = (sz.as_mut(), sj.as_mut()) {
        if zh.partial && ja.complete {
            *z -= 0.5;
        } else if !zh.partial && !ja.complete {
            *j -= 0.5;
        }
    }
    match ctx.last {
        Some(Lang::Zh) => sz = sz.map(|s| s + 0.6),
        Some(Lang::Ja) => sj = sj.map(|s| s + 0.6),
        Some(Lang::En) | None => {}
    }
    let total = ctx.zh_picks + ctx.ja_picks;
    if total > 0 {
        let z = ctx.zh_picks as f32 / total as f32;
        sz = sz.map(|s| s + 1.5 * z);
        sj = sj.map(|s| s + 1.5 * (1.0 - z));
    }
    (sz, sj)
}

/// Put the two lists together. The stronger side leads; when it clearly
/// dominates it keeps the first four places, otherwise the other side's best
/// comes second.
#[allow(dead_code)]
pub fn merge(zh: &[Candidate], ja: &[JaCand], sz: Option<f32>, sj: Option<f32>) -> Vec<Merged> {
    merge_with(zh, ja, sz, sj, true)
}

/// `loser_real`: the losing side has a real word for the input (not only
/// guesses); if not, it waits until the winner's first five.
pub fn merge_with(zh: &[Candidate], ja: &[JaCand], sz: Option<f32>, sj: Option<f32>, loser_real: bool) -> Vec<Merged> {
    let zs: Vec<Merged> = zh
        .iter()
        .enumerate()
        .map(|(i, c)| Merged { lang: Lang::Zh, key: i as i64, text: c.text.clone(), comment: c.comment.clone() })
        .collect();
    let js: Vec<Merged> = ja
        .iter()
        .filter(|c| !c.value.is_empty())
        .map(|c| Merged { lang: Lang::Ja, key: c.id as i64, text: c.value.clone(), comment: "日".into() })
        .collect();
    let (w, l, gap) = match (sz, sj) {
        (Some(_), None) => return zs,
        (None, Some(_)) => return js,
        (None, None) => return Vec::new(),
        (Some(a), Some(b)) => {
            if a >= b {
                (zs, js, a - b)
            } else {
                (js, zs, b - a)
            }
        }
    };
    let lead = if !loser_real { 5 } else if gap >= 1.0 { 4 } else { 1 };
    let mut out = Vec::with_capacity(w.len() + l.len());
    let (mut wi, mut li) = (w.into_iter(), l.into_iter());
    out.extend(wi.by_ref().take(lead));
    // Then one of theirs, two of ours, repeat.
    loop {
        let a = li.next();
        let b: Vec<Merged> = wi.by_ref().take(2).collect();
        if a.is_none() && b.is_empty() {
            break;
        }
        out.extend(a);
        out.extend(b);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|m| seen.insert(m.text.clone()));
    out
}

// --- English ----------------------------------------------------------------------

/// English words from the shipped dictionaries (rime-ice en_dicts), keyed by
/// their lower-case spelling, with the spellings people use ("iPhone",
/// "GitHub", "Inbox").
#[derive(Default)]
pub struct EnDict {
    map: HashMap<String, Vec<String>>,
    /// How the user writes a word (from their own documents), first.
    prefer: HashMap<String, String>,
}

impl EnDict {
    pub fn load(dir: &Path) -> EnDict {
        let mut d = EnDict::default();
        for f in ["en.dict.yaml", "en_ext.dict.yaml"] {
            let Ok(text) = std::fs::read_to_string(dir.join(f)) else { continue };
            let body = text.split_once("\n...").map(|(_, b)| b).unwrap_or("");
            for line in body.lines() {
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                let word = line.split('\t').next().unwrap_or("").trim();
                d.add(word);
            }
        }
        d
    }

    /// en_personal.txt: one word per line, in the user's casing.
    pub fn load_personal(&mut self, file: &Path) {
        let Ok(text) = std::fs::read_to_string(file) else { return };
        for w in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            self.add(w);
            self.prefer.insert(w.to_ascii_lowercase(), w.to_string());
        }
    }

    pub fn add(&mut self, word: &str) {
        if word.len() < 2 || !word.chars().all(|c| c.is_ascii_alphabetic()) {
            return;
        }
        let forms = self.map.entry(word.to_ascii_lowercase()).or_default();
        if !forms.iter().any(|f| f == word) {
            forms.push(word.to_string());
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Spellings for what was typed, best first. At the start of a
    /// sentence a lower-case word is also offered capitalised (first).
    pub fn lookup(&self, typed: &str, sentence_start: bool) -> Vec<String> {
        let Some(forms) = self.map.get(&typed.to_ascii_lowercase()) else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        // A proper spelling (iPhone, GitHub) beats the plain one.
        let mut sorted = forms.clone();
        sorted.sort_by_key(|f| (f.chars().all(|c| c.is_ascii_lowercase()), f.clone()));
        if let Some(p) = self.prefer.get(&typed.to_ascii_lowercase()) {
            sorted.retain(|f| f != p);
            sorted.insert(0, p.clone());
        }
        for f in sorted {
            if sentence_start && f.chars().all(|c| c.is_ascii_lowercase()) {
                let mut c = f.chars();
                let cap: String = c.next().map(|h| h.to_ascii_uppercase()).into_iter().chain(c).collect();
                if !out.contains(&cap) {
                    out.push(cap);
                }
            }
            if !out.contains(&f) {
                out.push(f);
            }
        }
        out
    }
}

/// Is the next word the start of a sentence (after our own output)?
pub fn sentence_start(recent: &str) -> bool {
    match recent.trim_end_matches(' ').chars().last() {
        None => true,
        Some(c) => ".!?。！？\n".contains(c),
    }
}

/// Score of the English reading of `typed` (None = not an English word).
/// Short words are weak: "de", "he", "the" are far more often pinyin.
pub fn en_score(typed: &str, hit: bool, last: Option<Lang>) -> Option<f32> {
    if !hit {
        return None;
    }
    // Just under a Chinese reading that is a real word (1.4 + 1.0), above
    // one that is only a made-up sentence (1.4).
    let mut s = 1.9f32;
    let n = typed.chars().count();
    if n <= 3 {
        s -= 0.6;
    } else if n >= 5 {
        s += 0.3;
    }
    if last == Some(Lang::En) {
        s += 0.6;
    }
    Some(s)
}

/// Put the English candidates into the merged list: first if English wins,
/// second if it is close, otherwise after the third.
pub fn insert_english(merged: &mut Vec<Merged>, words: &[String], se: f32, best_other: Option<f32>) {
    let pos = match best_other {
        None => 0,
        Some(b) if se > b => 0,
        Some(b) if se >= b - 0.6 => 1,
        Some(_) => 3,
    }
    .min(merged.len());
    let items: Vec<Merged> = words
        .iter()
        .enumerate()
        .map(|(i, w)| Merged { lang: Lang::En, key: i as i64, text: w.clone(), comment: "英".into() })
        .collect();
    merged.retain(|m| !words.contains(&m.text));
    let pos = pos.min(merged.len());
    for (k, it) in items.into_iter().enumerate() {
        merged.insert(pos + k, it);
    }
}

// --- English words the user picked --------------------------------------------------

/// Per input (the letters as typed): how often an English word was picked
/// for it, and the spelling picked last ("Inbox", "iPhone"). That spelling
/// comes first from then on, capitals and all, wherever in the sentence;
/// and English moves up the list with the habit. Kept in
/// %APPDATA%\EarthDesk\ime\enpref.json.
pub struct EnPref {
    path: PathBuf,
    map: HashMap<String, (u32, String)>,
}

impl EnPref {
    pub fn load(path: &Path) -> EnPref {
        let map = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        EnPref { path: path.to_path_buf(), map }
    }

    pub fn get(&self, code: &str) -> Option<(u32, String)> {
        self.map.get(&code.to_ascii_lowercase()).cloned()
    }

    pub fn record(&mut self, code: &str, spelling: &str) {
        if code.len() < 2 || !spelling.eq_ignore_ascii_case(code) {
            return;
        }
        let e = self.map.entry(code.to_ascii_lowercase()).or_insert((0, String::new()));
        e.0 = e.0.saturating_add(1);
        e.1 = spelling.to_string();
        if let Ok(b) = serde_json::to_vec(&self.map) {
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// English with the user's habit: the learned spelling first, and a score
/// raised by the share of picks that were English for this input (all of
/// them: above anything else).
pub fn en_learned(words: &mut Vec<String>, se: Option<f32>, learned: Option<(u32, String)>, zh_picks: u32, ja_picks: u32) -> Option<f32> {
    let Some((n, spelling)) = learned.filter(|l| l.0 > 0) else { return se };
    words.retain(|w| *w != spelling);
    words.insert(0, spelling);
    let share = n as f32 / (n + zh_picks + ja_picks) as f32;
    Some(se.unwrap_or(1.9) + 3.0 * share)
}

// --- Candidates the user deleted ---------------------------------------------------

/// Words the user removed with a right click, per input (the letters as
/// typed): never offered for that input again. Kept in
/// %APPDATA%\EarthDesk\ime\hidden.json. The engines forget what they learned
/// themselves (Rime's user dictionary, Mozc's history); this also covers
/// their shipped dictionaries, which cannot be changed.
pub struct Hidden {
    path: PathBuf,
    map: HashMap<String, Vec<String>>,
}

impl Hidden {
    pub fn load(path: &Path) -> Hidden {
        let map = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Hidden { path: path.to_path_buf(), map }
    }

    pub fn is_hidden(&self, code: &str, text: &str) -> bool {
        self.map.get(code).map(|v| v.iter().any(|t| t == text)).unwrap_or(false)
    }

    pub fn hide(&mut self, code: &str, text: &str) {
        if code.is_empty() || self.is_hidden(code, text) {
            return;
        }
        self.map.entry(code.to_string()).or_default().push(text.to_string());
        if let Ok(b) = serde_json::to_vec_pretty(&self.map) {
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

// --- Which language an input usually is ------------------------------------------

/// Per input code: how often it ended up Chinese / Japanese. This is the
/// same kind of learning as the Rime and Mozc user dictionaries (which
/// already remember the words themselves), kept in
/// %APPDATA%\EarthDesk\ime\langpref.json.
pub struct LangPref {
    path: PathBuf,
    map: HashMap<String, (u32, u32, u64)>,
    tick: u64,
    dirty: u32,
}

const PREF_CAP: usize = 20_000;

impl LangPref {
    pub fn load(path: &Path) -> LangPref {
        let map: HashMap<String, (u32, u32, u64)> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let tick = map.values().map(|v| v.2).max().unwrap_or(0);
        LangPref { path: path.to_path_buf(), map, tick, dirty: 0 }
    }

    pub fn get(&self, code: &str) -> (u32, u32) {
        self.map.get(code).map(|v| (v.0, v.1)).unwrap_or((0, 0))
    }

    pub fn record(&mut self, code: &str, lang: Lang) {
        if code.len() < 2 {
            return;
        }
        self.tick += 1;
        let e = self.map.entry(code.to_string()).or_insert((0, 0, 0));
        match lang {
            Lang::Zh => e.0 = e.0.saturating_add(1),
            Lang::Ja => e.1 = e.1.saturating_add(1),
            Lang::En => {}
        }
        e.2 = self.tick;
        if self.map.len() > PREF_CAP {
            // Forget the longest-unused tenth.
            let mut ticks: Vec<u64> = self.map.values().map(|v| v.2).collect();
            ticks.sort_unstable();
            let cut = ticks[PREF_CAP / 10];
            self.map.retain(|_, v| v.2 > cut);
        }
        self.dirty += 1;
        if self.dirty >= 10 {
            self.save();
        }
    }

    pub fn save(&mut self) {
        if self.dirty == 0 {
            return;
        }
        if let Ok(b) = serde_json::to_vec(&self.map) {
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
                self.dirty = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(t: &str) -> Candidate {
        Candidate { text: t.into(), comment: String::new() }
    }

    fn j(id: i32, v: &str, k: &str, desc: &str) -> JaCand {
        JaCand { id, value: v.into(), key: k.into(), segments: 1, description: desc.into() }
    }

    #[test]
    fn syllables_sorted() {
        let mut v = PINYIN.to_vec();
        v.sort();
        assert_eq!(v, PINYIN);
    }

    #[test]
    fn chinese_validity() {
        assert!(zh_info("wo jin tian qu", &[c("我今天去")]).valid);
        assert!(zh_info("wo jin tian q", &[c("我今天")]).valid);
        assert!(!zh_info("kuo ninichiha", &[]).valid);
        assert!(!zh_info("taberu", &[]).valid);
        let k = zh_info("ka na", &[c("卡那"), c("卡纳"), c("卡")]);
        assert!(k.valid && k.whole == 2 && k.syllables == 2);
    }

    #[test]
    fn japanese_validity() {
        let v = JaView { preedit: "わたし".into(), reading: "わたし".into(), candidates: vec![j(1, "私", "わたし", ""), j(2, "わたし", "わたし", "ひらがな")], ..Default::default() };
        assert_eq!(ja_info(&v), JaInfo { valid: true, hit: true, complete: true });
        let v = JaView { preedit: "をjn".into(), reading: "をjn".into(), ..Default::default() };
        assert!(!ja_info(&v).valid);
        let v = JaView { preedit: "わたさん".into(), reading: "わたさん".into(), candidates: vec![j(1, "わたさん", "わたさん", "ひらがな")], ..Default::default() };
        assert_eq!(ja_info(&v), JaInfo { valid: true, hit: false, complete: true });
    }

    #[test]
    fn merging() {
        let zh = [c("卡那"), c("卡纳"), c("卡")];
        let ja = [j(10, "かな", "かな", ""), j(11, "仮名", "かな", "")];
        // Close scores, Chinese ahead: 卡那, かな, 卡纳, 卡, 仮名
        let m = merge(&zh, &ja, Some(2.0), Some(1.8));
        let t: Vec<&str> = m.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(t, ["卡那", "かな", "卡纳", "卡", "仮名"]);
        // Japanese clearly ahead (just typed Japanese, picked it before).
        let m = merge(&zh, &ja, Some(1.0), Some(3.5));
        assert_eq!(m[0].text, "かな");
        assert_eq!(m[1].text, "仮名");
        assert_eq!(m[2].text, "卡那");
        // Only one side valid.
        assert_eq!(merge(&zh, &ja, None, Some(1.0)).len(), 2);
    }

    #[test]
    fn scoring_layers() {
        // watashi: Chinese reads "wa ta sang ch" (valid but only a sentence),
        // Japanese is a dictionary word.
        let zh = ZhInfo { valid: true, partial: true, syllables: 3, whole: 1 };
        let ja = JaInfo { valid: true, hit: true, complete: true };
        let (a, b) = score(zh, ja, Ctx::default());
        assert!(b.unwrap() > a.unwrap());
        // kana after Japanese text: Japanese first; after Chinese: Chinese.
        let zh = ZhInfo { valid: true, partial: false, syllables: 2, whole: 2 };
        let (a, b) = score(zh, ja, Ctx { last: Some(Lang::Ja), ..Default::default() });
        assert!(b.unwrap() > a.unwrap());
        let (a, b) = score(zh, ja, Ctx { last: Some(Lang::Zh), ..Default::default() });
        assert!(a.unwrap() > b.unwrap());
        // Habit wins over a weak context.
        let (a, b) = score(zh, ja, Ctx { last: Some(Lang::Zh), zh_picks: 0, ja_picks: 5 });
        assert!(b.unwrap() > a.unwrap());
    }

    #[test]
    fn english() {
        let mut d = EnDict::default();
        for w in ["inbox", "Inbox", "iPhone", "the", "GitHub"] {
            d.add(w);
        }
        assert_eq!(d.lookup("inbox", false), ["Inbox", "inbox"]);
        assert_eq!(d.lookup("iphone", true), ["iPhone"]);
        assert_eq!(d.lookup("github", false), ["GitHub"]);
        assert!(d.lookup("sss", false).is_empty());
        let mut d2 = EnDict::default();
        d2.add("hello");
        assert_eq!(d2.lookup("hello", true), ["Hello", "hello"]);
        assert!(sentence_start("你好。"));
        assert!(!sentence_start("我在用"));
        // inbox: Chinese reads only a sentence (1.4), Japanese is half-typed.
        let se = en_score("inbox", true, None).unwrap();
        assert!(se > 1.4 + 0.5);
        let mut m = vec![Merged { lang: Lang::Ja, key: 1, text: "韻母x".into(), comment: "日".into() }];
        insert_english(&mut m, &["Inbox".into(), "inbox".into()], se, Some(1.4));
        assert_eq!(m[0].text, "Inbox");
        // "the" is weak against Chinese.
        assert!(en_score("the", true, None).unwrap() <= 1.4);
    }

    #[test]
    fn english_habit() {
        let p = std::env::temp_dir().join(format!("enpref-{}.json", std::process::id()));
        let mut e = EnPref::load(&p);
        e.record("inbox", "Inbox");
        e.record("inbox", "韻母x"); // not a spelling of the letters: ignored
        assert_eq!(e.get("inbox"), Some((1, "Inbox".to_string())));
        assert_eq!(EnPref::load(&p).get("inbox"), Some((1, "Inbox".to_string())));
        let _ = std::fs::remove_file(p);
        // Picked once as English, never otherwise: English beats a strong
        // Japanese reading (its score here 2.2 + 3.0), "Inbox" first.
        let mut words = vec!["inbox".to_string()];
        let se = en_learned(&mut words, en_score("inbox", true, None), Some((1, "Inbox".into())), 0, 0).unwrap();
        assert_eq!(words, ["Inbox", "inbox"]);
        assert!(se > 1.0 + 1.0 + 1.5 + 0.6);
        // No habit: unchanged.
        let mut words = vec!["inbox".to_string()];
        assert_eq!(en_learned(&mut words, Some(2.2), None, 0, 0), Some(2.2));
    }

    #[test]
    fn hidden_store() {
        let p = std::env::temp_dir().join(format!("hidden-{}.json", std::process::id()));
        let mut h = Hidden::load(&p);
        h.hide("inbox", "韻母x");
        assert!(h.is_hidden("inbox", "韻母x"));
        assert!(!h.is_hidden("inbo", "韻母x") && !h.is_hidden("inbox", "inbox"));
        assert!(Hidden::load(&p).is_hidden("inbox", "韻母x"));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn pref_store() {
        let p = std::env::temp_dir().join(format!("langpref-{}.json", std::process::id()));
        let mut s = LangPref::load(&p);
        s.record("kana", Lang::Ja);
        s.record("kana", Lang::Ja);
        s.record("kana", Lang::Zh);
        assert_eq!(s.get("kana"), (1, 2));
        s.save();
        assert_eq!(LangPref::load(&p).get("kana"), (1, 2));
        let _ = std::fs::remove_file(p);
    }
}
