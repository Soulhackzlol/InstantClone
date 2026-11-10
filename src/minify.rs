//! The minifier the build runs on the dashboard and the dock (see build.rs,
//! which includes this file; the app includes it only for its tests).
//! Line by line: comments and indentation go, everything else stays.

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
pub fn minify(input: &str) -> Vec<u8> {
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
/// Aware of `'`, `"`, and `` ` `` strings and of regex literals, so it won't
/// eat a URL's `//` or the one in `/^https:\/\//`.
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

        // A regex literal is copied as it is. `//` is never one (a regex
        // can't be empty), so that stays a comment.
        let next = bytes.get(i + 1).copied();
        if b == b'/' && next != Some(b'/') && regex_may_start(&out) {
            let end = regex_end(bytes, i);
            out.extend_from_slice(&bytes[i..end]);
            i = end;
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

/// Whether a `/` after `before` opens a regex rather than divides: it does
/// where a value is expected. A wrong guess only copies the rest of the line
/// unstripped, which is always safe.
fn regex_may_start(before: &[u8]) -> bool {
    let code = trim_ws(before);
    match code.last() {
        None => true,
        Some(b) if b"(,=:[!&|?{};+-*%~^".contains(b) => true,
        Some(_) => code.ends_with(b"return") || code.ends_with(b"typeof"),
    }
}

/// Where the regex starting at `start` ends (after its closing `/`), or the
/// end of the line when it doesn't close there.
fn regex_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    let mut in_class = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' => in_class = true,
            b']' => in_class = false,
            b'/' if !in_class => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::minify;

    fn min(js: &str) -> String {
        String::from_utf8(minify(js)).unwrap()
    }

    #[test]
    fn comments_go_and_code_stays() {
        assert_eq!(min("// a note\nx();"), "\nx();\n");
        assert_eq!(min("/* a note */ x();"), "x();\n");
        assert_eq!(min("const half = a / 2; // half"), "const half = a / 2;\n");
    }

    #[test]
    fn a_regex_with_slashes_is_not_a_comment() {
        let line = r"const ok = /^https:\/\//.test(uri); // https only";
        assert_eq!(min(line), "const ok = /^https:\\/\\//.test(uri);\n");
        assert_eq!(min("s.replace(/[/]/g, '')"), "s.replace(/[/]/g, '')\n");
        assert_eq!(
            min(r"return /a\/\/b/.test(s);"),
            "return /a\\/\\/b/.test(s);\n"
        );
    }

    #[test]
    fn urls_and_strings_keep_their_slashes() {
        assert_eq!(
            min(r#"<a href="https://www.twitch.tv/activate">x</a>"#),
            "<a href=\"https://www.twitch.tv/activate\">x</a>\n"
        );
        assert_eq!(min("go('https://x.y/z'); // go"), "go('https://x.y/z');\n");
        assert_eq!(
            min("Read more at https://x.y"),
            "Read more at https://x.y\n"
        );
    }
}
