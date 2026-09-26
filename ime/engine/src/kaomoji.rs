//! 颜文字 (kaomoji) among the candidates, the way emoji are: the ones whose
//! keywords appear in the word or sentence being typed ("中秋节快乐" brings
//! the moon-viewing ones first, then the happy ones). The table is
//! data/kaomoji.tsv, built by tools/ime-kaomoji/mkkaomoji.py from the ones
//! popular in Japan and Mozc's list, most popular first.

use std::collections::HashMap;

const TABLE: &str = include_str!("../data/kaomoji.tsv");

pub struct Kaomoji {
    faces: Vec<&'static str>,
    /// Keyword -> faces (indexes into `faces`, most popular first).
    index: HashMap<&'static str, Vec<u32>>,
    /// The longest keyword, in characters.
    longest: usize,
}

/// A face for what is being typed.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub face: String,
    /// A keyword is the whole text ("开心"): the face takes its place, as
    /// an emoji does. Otherwise it goes after the text ("中秋节快乐(っ˘ω˘ｃ)🌕").
    pub whole: bool,
}

impl Kaomoji {
    pub fn load() -> Kaomoji {
        let mut faces = Vec::new();
        let mut index: HashMap<&'static str, Vec<u32>> = HashMap::new();
        let mut longest = 0;
        for line in TABLE.lines() {
            if line.starts_with('#') {
                continue;
            }
            let Some((face, words)) = line.split_once('\t') else { continue };
            let id = faces.len() as u32;
            faces.push(face);
            for w in words.split(' ').filter(|w| !w.is_empty()) {
                longest = longest.max(w.chars().count());
                let list = index.entry(w).or_default();
                if list.last() != Some(&id) {
                    list.push(id);
                }
            }
        }
        Kaomoji { faces, index, longest }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// Up to `n` faces for `text`: the ones matching more (and longer)
    /// keywords in it first, then the more popular. A one-character keyword
    /// (笑, 猫, 困) counts only for a short text or at its end ("好困"), not
    /// inside a longer word (热情, 猫腻).
    pub fn find(&self, text: &str, n: usize) -> Vec<Found> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() || chars.len() > 40 || n == 0 {
            return Vec::new();
        }
        let mut score: HashMap<u32, (u32, bool)> = HashMap::new();
        let mut seen: Vec<&str> = Vec::new();
        for start in 0..chars.len() {
            for len in 1..=self.longest.min(chars.len() - start) {
                let word: String = chars[start..start + len].iter().collect();
                let Some((key, ids)) = self.index.get_key_value(word.as_str()) else { continue };
                if len == 1 && chars.len() > 2 && start + 1 != chars.len() {
                    continue;
                }
                if seen.contains(key) {
                    continue;
                }
                seen.push(key);
                let whole = len == chars.len();
                for &id in ids {
                    let e = score.entry(id).or_insert((0, false));
                    e.0 += len.min(4) as u32;
                    e.1 |= whole;
                }
            }
        }
        let mut found: Vec<(u32, (u32, bool))> = score.into_iter().collect();
        found.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
        found.into_iter().take(n).map(|(id, (_, whole))| Found { face: self.faces[id as usize].to_string(), whole }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_meaning() {
        let k = Kaomoji::load();
        assert!(k.len() > 300);
        // The moon-viewing ones lead for 中秋节快乐, and go after the text.
        let f = k.find("中秋节快乐", 3);
        assert_eq!(f.len(), 3);
        assert!(f[0].face.contains('🌕') || f[0].face.contains('🎑') || f[0].face.contains('🥮') || f[0].face.contains('🍡'), "{f:?}");
        assert!(f.iter().all(|x| !x.whole));
        // A word that is a keyword itself: the face takes its place.
        let f = k.find("开心", 2);
        assert_eq!(f[0].face, "( ˶ˆᗜˆ˵ )");
        assert!(f[0].whole);
        // Japanese.
        assert!(!k.find("ありがとう", 1).is_empty());
        assert!(!k.find("おやすみ", 1).is_empty());
        // Nothing to do with feelings: none.
        assert!(k.find("数据库", 3).is_empty());
        // One character inside a longer word does not count; at the end it does.
        assert!(k.find("热情的人", 3).is_empty());
        assert!(!k.find("今天好困", 1).is_empty());
    }
}
