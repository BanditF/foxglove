//! An app's icon from its program file (#981, `docs/knob-exclude-ux.md`: "apps by name and icon",
//! the Volume Mixer pattern). The knob's tray row and the Knob card's rows show it before the
//! app's name.
//!
//! Cheap by design: a program's icon is read once, on the first frame that shows it, and kept in
//! the egui context (a miss is kept too, so a file without an icon isn't read again). No timer and
//! no watch: the next read is the next new program. At most [`KEEP`] programs are kept; past that
//! the cache starts over.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{ColorImage, TextureHandle, TextureOptions};

/// The icon's size on screen, in points.
pub const SIZE: f32 = 24.0;
/// The size read from the file, in pixels (sharp at 200 % scaling).
const READ_PX: u32 = 48;
/// Programs kept at most.
const KEEP: usize = 64;

#[derive(Clone, Default)]
struct Cache(Arc<HashMap<String, Option<TextureHandle>>>);

fn cache_id() -> egui::Id {
    egui::Id::new("foxglove-app-icons")
}

fn key(path: &str) -> String {
    path.to_lowercase()
}

/// Keeps `image` as the icon of `path` (or no icon), replacing a kept one.
pub fn insert(ctx: &egui::Context, path: &str, image: Option<ColorImage>) -> Option<TextureHandle> {
    let tex =
        image.map(|i| ctx.load_texture(format!("app-icon:{path}"), i, TextureOptions::LINEAR));
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(cache_id());
        let map = Arc::make_mut(&mut cache.0);
        if map.len() >= KEEP {
            map.clear();
        }
        map.insert(key(path), tex.clone());
    });
    tex
}

/// The icon of the program at `path`: read on first use, then kept. `None`: it has none (or the
/// path isn't a full one, e.g. a shipped list's bare file name).
pub fn get(ctx: &egui::Context, path: &str) -> Option<TextureHandle> {
    if let Some(kept) = ctx.data(|d| {
        d.get_temp::<Cache>(cache_id())
            .and_then(|c| c.0.get(&key(path)).cloned())
    }) {
        return kept;
    }
    let image = if is_full_path(path) { read(path) } else { None };
    insert(ctx, path, image)
}

/// A full path (a drive or a share), so a bare file name is never looked up in the current folder.
fn is_full_path(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() > 2 && b[1] == b':' && matches!(b[2], b'\\' | b'/')) || path.starts_with(r"\\")
}

/// Draws the icon of `path`, or a plain tile of the same size (so names line up).
pub fn show(ui: &mut egui::Ui, path: Option<&str>) {
    let tex = path.and_then(|p| get(ui.ctx(), p));
    let size = egui::vec2(SIZE, SIZE);
    match tex {
        Some(t) => {
            ui.add(egui::Image::new((t.id(), size)));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let p = super::theme::palette(ui);
            // A filled tile, no outline (an outline reads as a checkbox): the generic app icon.
            ui.painter().rect_filled(rect.shrink(3.0), 4.0, p.line);
        }
    }
}

/// Reads the first icon of a program file.
#[cfg(windows)]
pub fn read(path: &str) -> Option<ColorImage> {
    use windows::core::HSTRING;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    /// A bitmap's pixels as top-down BGRA.
    unsafe fn pixels(bm: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = unsafe { GetDC(None) };
        let lines = unsafe {
            GetDIBits(
                dc,
                bm,
                0,
                h as u32,
                Some(buf.as_mut_ptr().cast()),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        unsafe { ReleaseDC(None, dc) };
        (lines == h).then_some(buf)
    }

    let mut icon = HICON::default();
    let hr =
        unsafe { SHDefExtractIconW(&HSTRING::from(path), 0, 0, Some(&mut icon), None, READ_PX) };
    if hr.is_err() || icon.is_invalid() {
        return None;
    }
    let mut info = ICONINFO::default();
    let got = unsafe { GetIconInfo(icon, &mut info) };
    let _ = unsafe { DestroyIcon(icon) };
    got.ok()?;
    let out = (|| {
        if info.hbmColor.is_invalid() {
            return None;
        }
        let mut bm = BITMAP::default();
        let n = unsafe {
            GetObjectW(
                HGDIOBJ(info.hbmColor.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some((&mut bm as *mut BITMAP).cast()),
            )
        };
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if n == 0 || w <= 0 || h <= 0 || w > 256 || h > 256 {
            return None;
        }
        let mut px = unsafe { pixels(info.hbmColor, w, h) }?;
        // An icon without alpha: its mask says what's see-through (white is).
        if px.as_chunks::<4>().0.iter().all(|c| c[3] == 0) {
            let mask = unsafe { pixels(info.hbmMask, w, h) }?;
            for (c, m) in px
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(mask.as_chunks::<4>().0)
            {
                c[3] = if m[0] == 0 { 255 } else { 0 };
            }
        }
        let rgba: Vec<u8> = px
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| [c[2], c[1], c[0], c[3]])
            .collect();
        Some(ColorImage::from_rgba_unmultiplied(
            [w as usize, h as usize],
            &rgba,
        ))
    })();
    unsafe {
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
    }
    out
}

#[cfg(not(windows))]
pub fn read(_path: &str) -> Option<ColorImage> {
    None
}

/// The full paths of the running processes whose file name is one of `names` (any case): one
/// process snapshot, taken when a caller asks (the Knob card's first frame, #981), never on a
/// timer. One path per program; a process whose path can't be read is left out.
#[cfg(windows)]
pub fn running_paths(names: &[&str]) -> Vec<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let mut out: Vec<String> = Vec::new();
    if names.is_empty() {
        return out;
    }
    // SAFETY: a process snapshot handle, closed below on every path.
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return out;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry` is a valid PROCESSENTRY32W with dwSize set; `snap` is open.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while more {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        let wanted = names.iter().any(|n| n.eq_ignore_ascii_case(&name));
        if wanted && !out.iter().any(|p| super::knob_hide::same_program(p, &name)) {
            // SAFETY: a query-only handle, closed right after its one read.
            unsafe {
                if let Ok(h) = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    entry.th32ProcessID,
                ) {
                    let mut buf = [0u16; 1024];
                    let mut len = buf.len() as u32;
                    let got = QueryFullProcessImageNameW(
                        h,
                        PROCESS_NAME_WIN32,
                        PWSTR(buf.as_mut_ptr()),
                        &mut len,
                    );
                    let _ = CloseHandle(h);
                    if got.is_ok() {
                        out.push(String::from_utf16_lossy(&buf[..len as usize]));
                    }
                }
            }
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: closes the snapshot opened above, once.
    let _ = unsafe { CloseHandle(snap) };
    out
}

#[cfg(not(windows))]
pub fn running_paths(_names: &[&str]) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_full_paths_are_read() {
        assert!(is_full_path(r"C:\Apps\a.exe"));
        assert!(is_full_path(r"\\host\share\a.exe"));
        assert!(!is_full_path("a.exe"));
        assert!(!is_full_path(r"Apps\a.exe"));
    }

    #[test]
    fn an_icon_is_read_once_and_kept() {
        let ctx = egui::Context::default();
        // A bare file name is never looked up, and the miss is kept.
        assert!(get(&ctx, "nothing-here.exe").is_none());
        let kept = insert(
            &ctx,
            r"C:\Apps\a.exe",
            Some(ColorImage::filled([4, 4], egui::Color32::RED)),
        );
        assert!(kept.is_some());
        // Matched in any case, from the cache (the file doesn't exist).
        assert_eq!(
            get(&ctx, r"c:\apps\A.EXE").map(|t| t.id()),
            kept.map(|t| t.id())
        );
    }

    #[test]
    fn the_cache_has_a_bound() {
        let ctx = egui::Context::default();
        for i in 0..KEEP + 5 {
            insert(&ctx, &format!(r"C:\Apps\{i}.exe"), None);
        }
        let n = ctx.data(|d| d.get_temp::<Cache>(cache_id()).map(|c| c.0.len()));
        assert!(n.unwrap() <= KEEP);
    }

    /// A system file with icons (no app): the pipeline gives a square, mostly opaque image.
    #[cfg(windows)]
    #[test]
    fn a_program_files_icon_reads() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let img = read(&format!(r"{root}\System32\shell32.dll")).expect("an icon");
        assert_eq!(img.size[0], img.size[1]);
        assert!(img.pixels.iter().any(|p| p.a() > 0));
        assert!(read(&format!(r"{root}\no-such-file.exe")).is_none());
    }
}
