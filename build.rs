//! Build-time HTML pipeline: minify + gzip web/*.html, emit gz blobs to
//! OUT_DIR so the binary embeds only the compressed bytes. Skips the
//! minifier on anything that looks risky (template literals, <pre>).
//!
//! Net effect: ~125 KB of HTML/CSS/JS in source compresses to ~25 KB in
//! the binary. The release binary serves the gz blob directly with
//! `Content-Encoding: gzip` - every real browser supports it.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use flate2::write::GzEncoder;
use flate2::Compression;

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
        ("overlay-runtime.js", false),
    ] {
        let src_path = web_dir.join(name);
        println!("cargo:rerun-if-changed={}", src_path.display());

        let raw = fs::read_to_string(&src_path)
            .unwrap_or_else(|e| panic!("read {}: {}", src_path.display(), e));
        let processed: Vec<u8> = if *do_min {
            minify(&raw)
        } else {
            raw.clone().into_bytes()
        };

        let out_path = out_dir.join(format!("{name}.gz"));
        let f = fs::File::create(&out_path).unwrap();
        let mut enc = GzEncoder::new(f, Compression::best());
        enc.write_all(&processed).unwrap();
        enc.finish().unwrap();

        let gz_size = fs::metadata(&out_path).unwrap().len();
        println!(
            "cargo:warning={}: {} B raw -> {} B out -> {} B gz",
            name,
            raw.len(),
            processed.len(),
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

/// Conservative line-oriented minifier. Strips JS `//` line comments and
/// `/* */` block comments outside of strings, drops leading/trailing
/// whitespace on each line, and collapses runs of blank lines. Does NOT
/// touch content inside <pre>, <textarea>, or backtick template literals
/// (it just leaves the whole line alone if we detect we're inside one).
///
/// Operates on raw bytes so multi-byte UTF-8 sequences (em-dashes, ·,
/// ▶, ✕, etc.) pass through unchanged. The first cut of this function
/// did `push(b as char)` which decoded each byte as Latin-1 and corrupted
/// every non-ASCII glyph in the dashboard. Don't do that.
fn minify(input: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut in_pre = false;
    let mut in_textarea = false;
    let mut in_template = false; // backtick template literal that spans lines
    let mut in_block_comment = false;
    let mut prev_blank = false;

    for line in input.lines() {
        // Detect pre/textarea open/close on the line (very rough).
        let lower = line.to_ascii_lowercase();
        if lower.contains("<pre") {
            in_pre = true;
        }
        if lower.contains("</pre>") {
            in_pre = false;
        }
        if lower.contains("<textarea") {
            in_textarea = true;
        }
        if lower.contains("</textarea>") {
            in_textarea = false;
        }

        if in_pre || in_textarea {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
            prev_blank = false;
            continue;
        }

        // Track unbalanced backticks across lines - if we're inside a
        // multi-line template literal, leave the line as-is so we don't
        // corrupt the string content.
        let backticks = line.matches('`').count();
        if backticks % 2 == 1 {
            in_template = !in_template;
        }

        if in_template {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
            prev_blank = false;
            continue;
        }

        // Strip line + block JS comments outside of strings.
        let stripped = strip_line(line, &mut in_block_comment);
        let trimmed = trim_ws(&stripped);

        if trimmed.is_empty() {
            if !prev_blank {
                out.push(b'\n');
                prev_blank = true;
            }
            continue;
        }

        out.extend_from_slice(trimmed);
        out.push(b'\n');
        prev_blank = false;
    }

    out
}

/// Trim ASCII whitespace from both ends of a byte slice. We only need to
/// match the four whitespace chars JS / HTML use (space, tab, CR, LF) -
/// no need to handle Unicode whitespace.
fn trim_ws(b: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = b.len();
    while s < e && matches!(b[s], b' ' | b'\t' | b'\r' | b'\n') {
        s += 1;
    }
    while e > s && matches!(b[e - 1], b' ' | b'\t' | b'\r' | b'\n') {
        e -= 1;
    }
    &b[s..e]
}

/// Walks one line and removes `//` line comments + any block comments.
/// Aware of `'`, `"`, and `` ` `` strings so it won't eat URL `//` etc.
/// Operates on raw bytes to preserve multi-byte UTF-8 sequences.
fn strip_line(line: &str, in_block: &mut bool) -> Vec<u8> {
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_str: Option<u8> = None;

    while i < bytes.len() {
        let b = bytes[i];

        // Inside a block comment - look for terminator
        if *in_block {
            if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                *in_block = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Inside a string - copy literally, watching for escapes + close
        if let Some(q) = in_str {
            out.push(b);
            if b == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
            i += 1;
            continue;
        }

        // Start of a string
        if b == b'"' || b == b'\'' || b == b'`' {
            in_str = Some(b);
            out.push(b);
            i += 1;
            continue;
        }

        // Block comment start
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            *in_block = true;
            i += 2;
            continue;
        }

        // Line comment - stop here, but ONLY if this looks like JS (not
        // a URL like https://). Treat `//` after `:` as URL noise.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            let prev = if i == 0 { b' ' } else { bytes[i - 1] };
            if prev != b':' {
                break;
            }
        }

        out.push(b);
        i += 1;
    }

    out
}
