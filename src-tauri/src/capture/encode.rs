//! Turning the finished screenshot into a clipboard entry or a file.

use std::path::{Path, PathBuf};

/// Pictures\EarthDesk unless the settings name another folder.
pub fn default_dir() -> PathBuf {
    dirs::picture_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))).join("EarthDesk")
}

fn save_dir() -> PathBuf {
    let d = crate::toolkit::current().capture.save_dir.trim().to_string();
    if d.is_empty() {
        default_dir()
    } else {
        PathBuf::from(d)
    }
}

#[cfg(windows)]
fn now() -> (u16, u16, u16, u16, u16, u16, u16) {
    super::win::local_time()
}

#[cfg(not(windows))]
fn now() -> (u16, u16, u16, u16, u16, u16, u16) {
    (2026, 1, 1, 0, 0, 0, 0)
}

/// Expand the file-name pattern. Characters Windows forbids in names are
/// replaced so a pattern can never produce an unusable path.
pub fn file_stem(pattern: &str, t: (u16, u16, u16, u16, u16, u16, u16)) -> String {
    let (y, mo, d, h, mi, s, ms) = t;
    let out = pattern
        .replace("{yyyy}", &format!("{y:04}"))
        .replace("{MM}", &format!("{mo:02}"))
        .replace("{dd}", &format!("{d:02}"))
        .replace("{HH}", &format!("{h:02}"))
        .replace("{mm}", &format!("{mi:02}"))
        .replace("{ss}", &format!("{s:02}"))
        .replace("{ms}", &format!("{ms:03}"));
    let clean: String = out
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_end_matches('.').to_string();
    if clean.is_empty() {
        "截图".into()
    } else {
        clean
    }
}

/// A path in `dir` that does not exist yet: "name.png", "name (2).png" ...
fn unique(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..10_000).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

pub fn png(w: u32, h: u32, rgba: &[u8], fast: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(if fast { png::Compression::Fast } else { png::Compression::Balanced });
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba[..(w * h * 4) as usize]).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn jpeg(w: u32, h: u32, rgba: &[u8], quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut out, quality);
    enc.encode(&rgba[..(w * h * 4) as usize], w as u16, h as u16, jpeg_encoder::ColorType::Rgba)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn bmp(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut v = Vec::with_capacity(size as usize);
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&size.to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&(w as i32).to_le_bytes());
    v.extend_from_slice(&(h as i32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        let start = (y * w * 4) as usize;
        for px in rgba[start..start + (w * 4) as usize].chunks_exact(4) {
            v.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        v.extend(std::iter::repeat(0).take((row - w * 3) as usize));
    }
    v
}

fn write_file(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
    let data = match ext.as_str() {
        "jpg" | "jpeg" => jpeg(w, h, rgba, crate::toolkit::current().capture.jpeg_quality)?,
        "bmp" => bmp(w, h, rgba),
        _ => png(w, h, rgba, false)?,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建文件夹：{e}"))?;
    }
    std::fs::write(path, data).map_err(|e| format!("保存失败：{e}"))
}

pub fn copy(w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    #[cfg(windows)]
    {
        let png = png(w, h, rgba, true).ok();
        crate::toolkit::win::clipboard::set_image(w, h, rgba, png.as_deref(), false)
    }
    #[cfg(not(windows))]
    {
        let _ = (w, h, rgba);
        Ok(())
    }
}

/// Carry out `op`. Returns the saved path, if any.
pub fn finish(op: &str, w: u32, h: u32, rgba: &[u8]) -> Result<Option<String>, String> {
    let cfg = crate::toolkit::current();
    let ext = if cfg.capture.format == "jpg" { "jpg" } else { "png" };
    match op {
        "copy" => copy(w, h, rgba).map(|_| None),
        "save" | "save-as" => {
            let dir = save_dir();
            let stem = file_stem(&cfg.capture.file_name, now());
            let path = if op == "save" {
                unique(&dir, &stem, ext)
            } else {
                #[cfg(windows)]
                {
                    let _ = std::fs::create_dir_all(&dir);
                    match super::win::save_dialog(&dir.to_string_lossy(), &format!("{stem}.{ext}"), ext == "jpg") {
                        Some(p) => PathBuf::from(p),
                        None => return Ok(None),
                    }
                }
                #[cfg(not(windows))]
                unique(&dir, &stem, ext)
            };
            write_file(&path, w, h, rgba)?;
            if cfg.capture.copy_on_save {
                copy(w, h, rgba)?;
            }
            Ok(Some(path.to_string_lossy().into_owned()))
        }
        other => Err(format!("unknown op {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let t = (2026, 9, 4, 7, 5, 3, 12);
        assert_eq!(file_stem("截图_{yyyy}{MM}{dd}_{HH}{mm}{ss}", t), "截图_20260904_070503");
        assert_eq!(file_stem("a:b/{ms}", t), "a_b_012");
        assert_eq!(file_stem("  ", t), "截图");
    }

    #[test]
    fn bmp_layout() {
        let v = bmp(3, 2, &[255u8; 24]);
        assert_eq!(v.len(), 54 + 12 * 2);
    }
}
