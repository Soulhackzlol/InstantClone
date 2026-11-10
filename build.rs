//! Build-time HTML pipeline: minify + gzip web/*.html, emit gz blobs to
//! OUT_DIR so the binary embeds only the compressed bytes. Skips the
//! minifier on anything that looks risky (template literals, <pre>).
//! The dashboard page and the Integrations script are first built from
//! their skeletons, web/dashboard/index.html and
//! web/integrations/integrations.js (see `read_pieced`).
//!
//! Net effect: the dashboard's ~870 KB of HTML/CSS/JS in source goes out
//! as ~165 KB gzipped. The release binary serves the gz blob directly with
//! `Content-Encoding: gzip` - every real browser supports it.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use flate2::write::GzEncoder;
use flate2::Compression;

// The skeleton rules, shared with the app's tests (see the module's header).
#[path = "src/pieced.rs"]
mod pieced;
// The page minifier, shared with the app's tests the same way.
#[path = "src/minify.rs"]
mod minify;

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let web_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("web");

    // (file, run_minifier). The overlay runtime is gzip-only: it is
    // regex- and template-literal-heavy (the live overlay JS lives in
    // String.raw templates), which is exactly where the conservative
    // line minifier has edge cases - gzip alone compresses it fine.
    for (name, do_min) in &[
        ("index.html", true),
        ("dock.html", true),
        // Dock logic is template-literal / regex heavy (same reasons as the
        // overlay runtime), so skip the line minifier and let gzip do it.
        ("dock.js", false),
        ("overlay-runtime.js", false),
        // Integrations UI: template-literal heavy like the dock.
        ("integrations.js", false),
        // The Overlay Studio editor, the same way.
        ("studio.js", false),
    ] {
        let raw = match PIECED.iter().find(|(served, _)| served == name) {
            Some((_, skeleton)) => read_pieced(&web_dir, skeleton),
            None => read_web(&web_dir.join(name)),
        };
        let processed: Vec<u8> = if *do_min {
            minify::minify(&raw)
        } else {
            raw.clone().into_bytes()
        };

        let gz_size = write_gz(&out_dir.join(format!("{name}.gz")), &processed);
        println!(
            "cargo:warning={}: {} B raw -> {} B out -> {} B gz",
            name,
            raw.len(),
            processed.len(),
            gz_size
        );
    }

    // The pages' translator: alone for English, and behind each other
    // language's strings (web/i18n/<lang>.json, a JSON object, so it drops
    // into the script as it is).
    let translator = read_web(&web_dir.join("i18n.js"));
    write_gz(&out_dir.join("i18n-en.js.gz"), translator.as_bytes());
    for lang in TRANSLATED {
        let strings = read_web(&web_dir.join("i18n").join(format!("{lang}.json")));
        let script = format!("window.IC_I18N={{lang:\"{lang}\",strings:{strings}}};\n{translator}");
        let gz_size = write_gz(
            &out_dir.join(format!("i18n-{lang}.js.gz")),
            script.as_bytes(),
        );
        println!(
            "cargo:warning=i18n {lang}: {} B raw -> {} B gz",
            script.len(),
            gz_size
        );
    }

    // Tray icon (Windows only). We hand-build a multi-resolution ICO so
    // we don't need a runtime image decoder. Three sizes (16/32/48) so
    // the tray + alt-tab + start-menu all get a clean version. Design:
    // cyan rounded square with two white pause bars (pause = "delay").
    #[cfg(windows)]
    {
        let ico_path = out_dir.join("icon.ico");
        let bytes = make_ico(&[16, 32, 48]);
        fs::write(&ico_path, &bytes).unwrap();
        println!(
            "cargo:warning=icon.ico: {} bytes ({} sizes)",
            bytes.len(),
            3
        );
    }

    println!("cargo:rerun-if-changed=build.rs");
}

/// The languages with strings in web/i18n/, besides English (the code's
/// own). `config::UI_LANGUAGES` and `web::page_language` list them too.
const TRANSLATED: &[&str] = &["es"];

/// A file under web/, read for the build and watched for changes.
fn read_web(path: &std::path::Path) -> String {
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Files served whole but kept as pieces: (served name, skeleton under
/// web/). A skeleton is the served file's own frame (the page's tags, the
/// script's wrapping function) with an include line where each piece goes.
const PIECED: &[(&str, &str)] = &[
    ("index.html", "dashboard/index.html"),
    ("integrations.js", "integrations/integrations.js"),
];

/// A served file built from its skeleton (see src/pieced.rs for the
/// rules). Any broken rule stops the build with the file and line to fix.
fn read_pieced(web_dir: &std::path::Path, skeleton: &str) -> String {
    let path = web_dir.join(skeleton);
    let dir = path.parent().unwrap();
    let skeleton_name = path.file_name().unwrap().to_string_lossy().into_owned();
    // Watching the folder itself also catches a piece added or removed.
    println!("cargo:rerun-if-changed={}", dir.display());
    let text = read_web(&path);
    pieced::pieces_in(dir, &skeleton_name)
        .and_then(|pieces| {
            pieced::join(&format!("web/{skeleton}"), &text, &pieces, |piece| {
                Ok(read_web(&dir.join(piece)))
            })
        })
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Gzip `bytes` into `path`; returns the compressed size.
fn write_gz(path: &std::path::Path, bytes: &[u8]) -> u64 {
    let f = fs::File::create(path).unwrap();
    let mut enc = GzEncoder::new(f, Compression::best());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap();
    fs::metadata(path).unwrap().len()
}

// ---------- ICO generation ----------
//
// ICO is a tiny container: a 6-byte directory header, N × 16-byte entries
// pointing to images, then the images themselves. Each image is a
// BITMAPINFOHEADER with `height` set to 2× the real height (the
// historical "AND mask is glued underneath" convention) followed by the
// XOR pixel data (bottom-up BGRA) and then the AND mask. Modern Windows
// keys off the alpha channel of 32bpp images, but we still ship a
// correct (all-zero) AND mask for safety.

#[cfg(windows)]
fn make_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| make_image(s)).collect();
    let header_size = 6 + 16 * sizes.len();
    let mut buf = Vec::with_capacity(header_size + images.iter().map(|i| i.len()).sum::<usize>());

    // ICONDIR
    buf.extend_from_slice(&0u16.to_le_bytes()); // reserved
    buf.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    buf.extend_from_slice(&(sizes.len() as u16).to_le_bytes());

    let mut offset = header_size as u32;
    for (i, &size) in sizes.iter().enumerate() {
        let img_size = images[i].len() as u32;
        // 0 means 256 in ICONDIRENTRY; otherwise the actual dimension.
        let dim = if size >= 256 { 0u8 } else { size as u8 };
        buf.push(dim); // width
        buf.push(dim); // height
        buf.push(0); // color count (>=8bpp)
        buf.push(0); // reserved
        buf.extend_from_slice(&1u16.to_le_bytes()); // planes
        buf.extend_from_slice(&32u16.to_le_bytes()); // bpp
        buf.extend_from_slice(&img_size.to_le_bytes()); // bytes_in_res
        buf.extend_from_slice(&offset.to_le_bytes()); // image_offset
        offset += img_size;
    }
    for img in images {
        buf.extend_from_slice(&img);
    }
    buf
}

#[cfg(windows)]
fn make_image(size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut img = Vec::new();

    // BITMAPINFOHEADER (40 bytes). `height` is doubled to account for
    // the trailing AND mask.
    img.extend_from_slice(&40u32.to_le_bytes());
    img.extend_from_slice(&(size as i32).to_le_bytes());
    img.extend_from_slice(&((size as i32) * 2).to_le_bytes());
    img.extend_from_slice(&1u16.to_le_bytes()); // planes
    img.extend_from_slice(&32u16.to_le_bytes()); // bpp
    img.extend_from_slice(&0u32.to_le_bytes()); // compression = BI_RGB
    img.extend_from_slice(&0u32.to_le_bytes()); // size_image
    img.extend_from_slice(&0i32.to_le_bytes()); // x_ppm
    img.extend_from_slice(&0i32.to_le_bytes()); // y_ppm
    img.extend_from_slice(&0u32.to_le_bytes()); // clr_used
    img.extend_from_slice(&0u32.to_le_bytes()); // clr_important

    // Render top-down into a BGRA buffer, then flip rows to bottom-up
    // (ICO convention) when appending.
    let mut pixels = vec![0u8; n * n * 4];
    draw_icon(&mut pixels, n);
    let row_bytes = n * 4;
    for y in (0..n).rev() {
        let row = &pixels[y * row_bytes..(y + 1) * row_bytes];
        img.extend_from_slice(row);
    }

    // AND mask: 1 bit per pixel, rows padded to 4-byte boundary, bottom-up.
    // We use the alpha channel for transparency on 32bpp icons, so the
    // mask is all zeros (meaning "use XOR color for every pixel").
    let stride = n.div_ceil(32) * 4;
    img.extend(std::iter::repeat_n(0u8, stride * n));

    img
}

/// Draw a cyan rounded square with two white pause bars centred inside.
/// Proportional to icon size so 16/32/48 all stay legible.
#[cfg(windows)]
fn draw_icon(pixels: &mut [u8], n: usize) {
    // Brand cyan #5ac8fa  (BGRA)
    const CYAN: [u8; 4] = [250, 200, 90, 255];
    // Brand white #f4f6f9 (BGRA)
    const WHITE: [u8; 4] = [249, 246, 244, 255];

    let pad = ((n as f32) * 0.06).round().max(1.0) as usize;
    let radius = ((n as f32) * 0.22).round().max(2.0) as usize;
    let bar_w = ((n as f32) * 0.09).round().max(1.0) as usize;
    let bar_h = ((n as f32) * 0.42).round().max(4.0) as usize;
    let bar_gap = ((n as f32) * 0.13).round().max(1.0) as usize;

    let x0 = pad;
    let x1 = n.saturating_sub(pad);
    let y0 = pad;
    let y1 = n.saturating_sub(pad);

    // Filled rounded square (cyan)
    for y in 0..n {
        for x in 0..n {
            if !inside_rrect(x, y, x0, y0, x1, y1, radius) {
                continue;
            }
            let i = (y * n + x) * 4;
            pixels[i..i + 4].copy_from_slice(&CYAN);
        }
    }

    // Two pause bars (white). Centre them on the icon.
    let cx = n / 2;
    let cy = n / 2;
    let inner_w = bar_gap + 2 * bar_w;
    let left = cx.saturating_sub(inner_w / 2);
    let bar_y0 = cy.saturating_sub(bar_h / 2);
    let bar_y1 = (bar_y0 + bar_h).min(n);

    for y in bar_y0..bar_y1 {
        for x in left..(left + bar_w).min(n) {
            let i = (y * n + x) * 4;
            pixels[i..i + 4].copy_from_slice(&WHITE);
        }
        let r0 = left + bar_w + bar_gap;
        for x in r0..(r0 + bar_w).min(n) {
            let i = (y * n + x) * 4;
            pixels[i..i + 4].copy_from_slice(&WHITE);
        }
    }
}

/// True iff `(x, y)` falls inside the rounded rectangle described by
/// `[x0, x1) × [y0, y1)` with corner radius `r`. Pixel-perfect, no AA -
/// the small ICO sizes hide the staircase well enough.
#[cfg(windows)]
fn inside_rrect(x: usize, y: usize, x0: usize, y0: usize, x1: usize, y1: usize, r: usize) -> bool {
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return false;
    }
    // Snap onto a corner centre if we're in any of the four corner squares.
    let in_left = x < x0 + r;
    let in_right = x + r >= x1;
    let in_top = y < y0 + r;
    let in_bottom = y + r >= y1;
    let (cx, cy) = match (in_left, in_right, in_top, in_bottom) {
        (true, false, true, false) => (x0 + r, y0 + r),
        (false, true, true, false) => (x1 - r - 1, y0 + r),
        (true, false, false, true) => (x0 + r, y1 - r - 1),
        (false, true, false, true) => (x1 - r - 1, y1 - r - 1),
        _ => return true, // straight edge or pure interior
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let r2 = (r as i32) * (r as i32);
    dx * dx + dy * dy <= r2
}
