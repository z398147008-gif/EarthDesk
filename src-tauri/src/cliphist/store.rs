//! The clipboard history on disk: `%APPDATA%\EarthDesk\clipboard\`
//! - `history.db`   SQLite, one row per entry
//! - `<hash>.png`   full image of an image entry
//! - `<hash>.t.png` its thumbnail (at most 320 px), for the lists
//!
//! Plain Rust (no Win32), so it is unit-tested on any platform.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct Store {
    db: Connection,
    dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: i64,
    /// "text" | "image" | "files"
    pub kind: String,
    /// Text entries: the start of the text; files: the paths, one per line.
    pub preview: String,
    pub chars: i64,
    pub width: i64,
    pub height: i64,
    pub app: String,
    pub created: i64,
    pub used: i64,
    pub pinned: bool,
    pub has_html: bool,
}

pub enum New {
    Text { text: String, html: Option<Vec<u8>> },
    Image { png: Vec<u8>, thumb: Vec<u8>, width: u32, height: u32 },
    Files(Vec<String>),
}

/// FNV-1a, 64 bit: enough to spot the same copy twice.
pub fn hash(parts: &[&[u8]]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.iter() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

const PREVIEW_CHARS: usize = 400;

impl Store {
    pub fn open(dir: &Path) -> rusqlite::Result<Store> {
        let _ = std::fs::create_dir_all(dir);
        let db = Connection::open(dir.join("history.db"))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS items(
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                text TEXT,
                html BLOB,
                files TEXT,
                width INTEGER DEFAULT 0,
                height INTEGER DEFAULT 0,
                hash TEXT NOT NULL UNIQUE,
                app TEXT DEFAULT '',
                created INTEGER NOT NULL,
                used INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS items_used ON items(used);",
        )?;
        Ok(Store { db, dir: dir.to_path_buf() })
    }

    pub fn image_path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.png"))
    }

    pub fn thumb_path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.t.png"))
    }

    /// Record a copy. The same content copied again just moves to the top.
    /// Returns (id, was_new).
    pub fn add(&self, item: New, app: &str, now: i64) -> rusqlite::Result<(i64, bool)> {
        let h = match &item {
            New::Text { text, .. } => hash(&[b"t", text.as_bytes()]),
            New::Image { png, .. } => hash(&[b"i", png]),
            New::Files(f) => hash(&[b"f", f.join("\n").as_bytes()]),
        };
        if let Some(id) = self
            .db
            .query_row("SELECT id FROM items WHERE hash=?1", params![h], |r| r.get::<_, i64>(0))
            .optional()?
        {
            self.db.execute("UPDATE items SET used=?1, app=CASE WHEN ?2<>'' THEN ?2 ELSE app END WHERE id=?3", params![now, app, id])?;
            return Ok((id, false));
        }
        match item {
            New::Text { text, html } => {
                self.db.execute(
                    "INSERT INTO items(kind,text,html,hash,app,created,used) VALUES('text',?1,?2,?3,?4,?5,?5)",
                    params![text, html, h, app, now],
                )?;
            }
            New::Image { png, thumb, width, height } => {
                let _ = std::fs::write(self.image_path(&h), &png);
                let _ = std::fs::write(self.thumb_path(&h), &thumb);
                self.db.execute(
                    "INSERT INTO items(kind,width,height,hash,app,created,used) VALUES('image',?1,?2,?3,?4,?5,?5)",
                    params![width, height, h, app, now],
                )?;
            }
            New::Files(f) => {
                self.db.execute(
                    "INSERT INTO items(kind,files,hash,app,created,used) VALUES('files',?1,?2,?3,?4,?4)",
                    params![f.join("\n"), h, app, now],
                )?;
            }
        }
        Ok((self.db.last_insert_rowid(), true))
    }

    fn row(r: &rusqlite::Row) -> rusqlite::Result<Item> {
        let kind: String = r.get(1)?;
        let text: Option<String> = r.get(2)?;
        let files: Option<String> = r.get(3)?;
        let (preview, chars) = match kind.as_str() {
            "text" => {
                let t = text.unwrap_or_default();
                let n = t.chars().count() as i64;
                (t.chars().take(PREVIEW_CHARS).collect(), n)
            }
            "files" => {
                let f = files.unwrap_or_default();
                let n = f.lines().count() as i64;
                (f.lines().take(20).collect::<Vec<_>>().join("\n"), n)
            }
            _ => (String::new(), 0),
        };
        Ok(Item {
            id: r.get(0)?,
            kind,
            preview,
            chars,
            width: r.get(4)?,
            height: r.get(5)?,
            app: r.get(6)?,
            created: r.get(7)?,
            used: r.get(8)?,
            pinned: r.get::<_, i64>(9)? != 0,
            has_html: r.get::<_, i64>(10)? != 0,
        })
    }

    /// Newest (most recently used) first; pinned entries first when
    /// `pinned_first`. `kind`: "" | text | image | files | pinned.
    pub fn list(&self, query: &str, kind: &str, offset: i64, limit: i64, pinned_first: bool) -> rusqlite::Result<Vec<Item>> {
        let mut sql = String::from(
            "SELECT id,kind,text,files,width,height,app,created,used,pinned,html IS NOT NULL FROM items WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        match kind {
            "text" | "image" | "files" => {
                sql.push_str(" AND kind=?");
                args.push(kind.to_string().into());
            }
            "pinned" => sql.push_str(" AND pinned=1"),
            _ => {}
        }
        for word in query.split_whitespace().take(6) {
            sql.push_str(" AND (IFNULL(text,'') LIKE ? ESCAPE '\\' OR IFNULL(files,'') LIKE ? ESCAPE '\\' OR app LIKE ? ESCAPE '\\')");
            let w = format!("%{}%", word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            args.push(w.clone().into());
            args.push(w.clone().into());
            args.push(w.into());
        }
        sql.push_str(if pinned_first { " ORDER BY pinned DESC, used DESC" } else { " ORDER BY used DESC" });
        sql.push_str(" LIMIT ? OFFSET ?");
        args.push(limit.into());
        args.push(offset.into());
        let mut st = self.db.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args), Self::row)?;
        rows.collect()
    }

    pub fn get(&self, id: i64) -> rusqlite::Result<Option<Full>> {
        self.db
            .query_row(
                "SELECT kind,text,html,files,hash,width,height FROM items WHERE id=?1",
                params![id],
                |r| {
                    Ok(Full {
                        kind: r.get(0)?,
                        text: r.get(1)?,
                        html: r.get(2)?,
                        files: r.get::<_, Option<String>>(3)?.map(|f| f.lines().map(String::from).collect()),
                        hash: r.get(4)?,
                        width: r.get(5)?,
                        height: r.get(6)?,
                    })
                },
            )
            .optional()
    }

    pub fn touch(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.db.execute("UPDATE items SET used=?1 WHERE id=?2", params![now, id]).map(|_| ())
    }

    pub fn set_pinned(&self, id: i64, on: bool) -> rusqlite::Result<()> {
        self.db.execute("UPDATE items SET pinned=?1 WHERE id=?2", params![on as i64, id]).map(|_| ())
    }

    fn remove_files(&self, hashes: &[String]) {
        for h in hashes {
            let _ = std::fs::remove_file(self.image_path(h));
            let _ = std::fs::remove_file(self.thumb_path(h));
        }
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        let h: Option<(String, String)> = self
            .db
            .query_row("SELECT hash,kind FROM items WHERE id=?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        self.db.execute("DELETE FROM items WHERE id=?1", params![id])?;
        if let Some((h, k)) = h {
            if k == "image" {
                self.remove_files(&[h]);
            }
        }
        Ok(())
    }

    /// Everything that is not pinned.
    pub fn clear(&self) -> rusqlite::Result<usize> {
        let hashes = self.hashes("SELECT hash FROM items WHERE pinned=0 AND kind='image'", params![])?;
        let n = self.db.execute("DELETE FROM items WHERE pinned=0", [])?;
        self.remove_files(&hashes);
        Ok(n)
    }

    fn hashes(&self, sql: &str, p: impl rusqlite::Params) -> rusqlite::Result<Vec<String>> {
        let mut st = self.db.prepare(sql)?;
        let rows = st.query_map(p, |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// Keep at most `max_items` unpinned entries, none older than
    /// `max_days` (0 = no age limit). Pinned entries are never pruned.
    pub fn prune(&self, max_items: usize, max_days: u32, now: i64) -> rusqlite::Result<usize> {
        let cutoff = if max_days == 0 { i64::MIN } else { now - max_days as i64 * 86_400_000 };
        let where_ = "pinned=0 AND (used < ?1 OR id NOT IN (SELECT id FROM items WHERE pinned=0 ORDER BY used DESC LIMIT ?2))";
        let hashes = self.hashes(&format!("SELECT hash FROM items WHERE kind='image' AND {where_}"), params![cutoff, max_items as i64])?;
        let n = self.db.execute(&format!("DELETE FROM items WHERE {where_}"), params![cutoff, max_items as i64])?;
        self.remove_files(&hashes);
        Ok(n)
    }

    pub fn count(&self) -> i64 {
        self.db.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0)).unwrap_or(0)
    }
}

pub struct Full {
    pub kind: String,
    pub text: Option<String>,
    pub html: Option<Vec<u8>>,
    pub files: Option<Vec<String>>,
    pub hash: String,
    pub width: i64,
    pub height: i64,
}

// --- images ---------------------------------------------------------------------------

/// Decode any PNG to RGBA8.
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let out = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some((w, h, out))
}

/// Box-filter downscale so the longer side is at most `max`.
pub fn thumbnail(w: u32, h: u32, rgba: &[u8], max: u32) -> (u32, u32, Vec<u8>) {
    let scale = (max as f64 / w.max(h) as f64).min(1.0);
    let (tw, th) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    if tw == w && th == h {
        return (w, h, rgba.to_vec());
    }
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = (ty as u64 * h as u64 / th as u64) as u32;
        let y1 = (((ty + 1) as u64 * h as u64 / th as u64) as u32).max(y0 + 1);
        for tx in 0..tw {
            let x0 = (tx as u64 * w as u64 / tw as u64) as u32;
            let x1 = (((tx + 1) as u64 * w as u64 / tw as u64) as u32).max(x0 + 1);
            let mut acc = [0u64; 4];
            // Sample at most 4x4 source pixels per output pixel: plenty for
            // a list thumbnail and bounded for huge screenshots.
            let sy = ((y1 - y0) / 4).max(1);
            let sx = ((x1 - x0) / 4).max(1);
            let mut n = 0u64;
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let o = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += rgba[o + c] as u64;
                    }
                    n += 1;
                    x += sx;
                }
                y += sy;
            }
            let o = ((ty * tw + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (tw, th, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("clip-test-{}-{}", std::process::id(), rand()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn rand() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    #[test]
    fn dedupe_moves_to_top() {
        let s = Store::open(&tmp()).unwrap();
        let (a, new_a) = s.add(New::Text { text: "hello".into(), html: None }, "x.exe", 1).unwrap();
        s.add(New::Text { text: "world".into(), html: None }, "x.exe", 2).unwrap();
        let (a2, new_a2) = s.add(New::Text { text: "hello".into(), html: None }, "", 3).unwrap();
        assert!(new_a && !new_a2 && a == a2);
        let l = s.list("", "", 0, 10, false).unwrap();
        assert_eq!(l[0].preview, "hello");
        assert_eq!(l[0].app, "x.exe");
    }

    #[test]
    fn search_and_kinds() {
        let s = Store::open(&tmp()).unwrap();
        s.add(New::Text { text: "张三的地址 50%".into(), html: None }, "", 1).unwrap();
        s.add(New::Files(vec![r"C:\a\report.docx".into()]), "explorer.exe", 2).unwrap();
        assert_eq!(s.list("地址", "", 0, 10, false).unwrap().len(), 1);
        assert_eq!(s.list("50%", "", 0, 10, false).unwrap().len(), 1);
        assert_eq!(s.list("report", "files", 0, 10, false).unwrap().len(), 1);
        assert_eq!(s.list("", "image", 0, 10, false).unwrap().len(), 0);
    }

    #[test]
    fn prune_keeps_pinned() {
        let s = Store::open(&tmp()).unwrap();
        for i in 0..10 {
            s.add(New::Text { text: format!("t{i}"), html: None }, "", i).unwrap();
        }
        let first = s.list("t0", "", 0, 1, false).unwrap()[0].id;
        s.set_pinned(first, true).unwrap();
        s.prune(3, 0, 100).unwrap();
        let l = s.list("", "", 0, 100, true).unwrap();
        assert_eq!(l.len(), 4);
        assert!(l[0].pinned && l[0].preview == "t0");
        // Age limit: everything unpinned is older than a day at t = 2 days.
        s.prune(100, 1, 2 * 86_400_000).unwrap();
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn thumbnails() {
        let px = vec![200u8; 1000 * 500 * 4];
        let (w, h, t) = thumbnail(1000, 500, &px, 320);
        assert_eq!((w, h), (320, 160));
        assert_eq!(t[0], 200);
    }
}
