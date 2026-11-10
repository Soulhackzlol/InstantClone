//! Files the app serves whole but keeps as pieces (the dashboard page, the
//! Integrations script). A skeleton holds the served file's own frame and an
//! include line where each piece goes:
//!
//! ```text
//! <style>
//! <!-- include hero/hero.css -->
//! </style>
//! ```
//!
//! build.rs builds the served files with this module (it includes it through
//! `#[path]`); the app compiles it only for its tests. Std only, so the
//! build script can use it. web/tests/web-files.mjs follows the same rules
//! for the page tests: a change here belongs there too.

/// The extensions a piece can have. Any other file in a skeleton's folder
/// (an editor's swap file, a `desktop.ini`) is not a piece and is ignored.
const PIECE_EXTENSIONS: &[&str] = &[".html", ".css", ".js"];

const INCLUDE_OPEN: &str = "<!-- include ";
const INCLUDE_CLOSE: &str = " -->";

/// Whether a file in a skeleton's folder (a path from that folder, `/`
/// between folders) is a piece: a page file whose name doesn't start with
/// a dot (hidden files, emacs lock files). Every piece must be included.
pub fn is_piece_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    !name.starts_with('.') && PIECE_EXTENSIONS.iter().any(|ext| name.ends_with(ext))
}

/// The piece an include line names, `Ok(None)` for any other line. A line
/// that starts like an include but isn't one exactly (a missing space, a
/// path that leaves the folder) is an error, so a typo can't ship as text.
fn include_target(line: &str) -> Result<Option<&str>, String> {
    let trimmed = line.trim();
    let Some(after_open) = trimmed.strip_prefix("<!--") else {
        return Ok(None);
    };
    let word = after_open.trim_start();
    let is_include = word
        .strip_prefix("include")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
    if !is_include {
        return Ok(None);
    }
    let path = trimmed
        .strip_prefix(INCLUDE_OPEN)
        .and_then(|rest| rest.strip_suffix(INCLUDE_CLOSE))
        .filter(|path| is_plain_piece_path(path))
        .ok_or_else(|| {
            format!(
                "\"{trimmed}\" is not an include line: write {INCLUDE_OPEN}folder/name.css{INCLUDE_CLOSE} \
                 with single spaces, a path inside the skeleton's folder (letters, digits, . _ -, \
                 / between folders) and a .html, .css or .js name"
            )
        })?;
    Ok(Some(path))
}

/// A path from the skeleton's folder that stays inside it and reads the
/// same on every system: `/` between folders, no `.` or `..` steps, no
/// spaces or backslashes, and a piece's extension.
fn is_plain_piece_path(path: &str) -> bool {
    let plain_step = |step: &str| {
        !step.is_empty()
            && step != "."
            && step != ".."
            && step
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    path.split('/').all(plain_step) && is_piece_name(path)
}

/// A piece as it goes into the served file: without a leading byte order
/// mark (it would land mid-file), ending in a line break (otherwise its last
/// line runs into the next one, and a closing `// comment` would swallow
/// it), and with no include line of its own (includes don't nest).
fn prepare_piece(name: &str, text: String) -> Result<String, String> {
    let mut text = match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    };
    for (index, line) in text.lines().enumerate() {
        if include_target(line).ok().flatten().is_some() {
            return Err(format!(
                "{name}:{}: an include line only works in the skeleton",
                index + 1
            ));
        }
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// The pieces in a skeleton's folder `dir` (and folders inside it), as
/// paths from it with `/` between folders, sorted. The skeleton itself and
/// files that are not pieces are left out. A piece whose name an include
/// line couldn't write (a space in it) is an error, not a silent skip.
pub fn pieces_in(dir: &std::path::Path, skeleton_name: &str) -> Result<Vec<String>, String> {
    let mut pieces = Vec::new();
    collect_files(dir, "", &mut pieces)?;
    pieces.retain(|piece| piece != skeleton_name && is_piece_name(piece));
    if let Some(odd) = pieces.iter().find(|piece| !is_plain_piece_path(piece)) {
        return Err(format!(
            "{odd} (next to {skeleton_name}): rename it to letters, digits and . _ - only, \
             so an include line can name it"
        ));
    }
    pieces.sort_unstable();
    Ok(pieces)
}

/// Every file under `dir` into `files`, as a path from it starting with `prefix`.
fn collect_files(
    dir: &std::path::Path,
    prefix: &str,
    files: &mut Vec<String>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let is_dir = entry
            .file_type()
            .map_err(|e| format!("read {name}: {e}"))?
            .is_dir();
        if is_dir {
            collect_files(&entry.path(), &format!("{name}/"), files)?;
        } else {
            files.push(name);
        }
    }
    Ok(())
}

/// The served file built from its skeleton: each include line becomes the
/// piece it names, every other line stays as it is. `skeleton_label` names
/// the skeleton in errors; `pieces` are the piece files in its folder
/// (see `is_piece_name`, the skeleton itself left out), and each must be
/// included exactly once. `read_piece` reads one by its path.
pub fn join(
    skeleton_label: &str,
    skeleton: &str,
    pieces: &[String],
    mut read_piece: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    // A byte order mark would sit before the file's first line, where the
    // page tests (JS trims it as whitespace) and this would read it apart.
    let skeleton = skeleton.strip_prefix('\u{feff}').unwrap_or(skeleton);
    let mut joined = String::with_capacity(skeleton.len());
    let mut included: Vec<&str> = Vec::new();
    for (index, line) in skeleton.split_inclusive('\n').enumerate() {
        let at = format!("{skeleton_label}:{}", index + 1);
        let Some(piece) = include_target(line).map_err(|e| format!("{at}: {e}"))? else {
            joined.push_str(line);
            continue;
        };
        if !pieces.iter().any(|known| known == piece) {
            return Err(format!(
                "{at}: includes {piece}, but there is no such piece (names are case-sensitive)"
            ));
        }
        if included.contains(&piece) {
            return Err(format!("{at}: includes {piece} a second time"));
        }
        joined.push_str(&prepare_piece(piece, read_piece(piece)?)?);
        included.push(piece);
    }

    let mut forgotten: Vec<&str> = pieces
        .iter()
        .map(String::as_str)
        .filter(|piece| !included.contains(piece))
        .collect();
    forgotten.sort_unstable();
    if !forgotten.is_empty() {
        return Err(format!(
            "{skeleton_label} has no include line for {} (every piece in its folder must be included)",
            forgotten.join(", ")
        ));
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Joins `skeleton` over in-memory pieces.
    fn join_with(skeleton: &str, pieces: &[(&str, &str)]) -> Result<String, String> {
        let files: HashMap<String, String> = pieces
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect();
        let names: Vec<String> = files.keys().cloned().collect();
        join(
            "skel.html",
            skeleton,
            &names,
            |name| Ok(files[name].clone()),
        )
    }

    #[test]
    fn include_lines_become_their_pieces_and_other_lines_stay_byte_for_byte() {
        let skeleton = "<style>\r\n  <!-- include a/one.css -->\r\n</style>\r\n<!-- a note -->\r\n";
        let joined = join_with(skeleton, &[("a/one.css", ".x{}\r\n")]).unwrap();
        assert_eq!(joined, "<style>\r\n.x{}\r\n</style>\r\n<!-- a note -->\r\n");
    }

    #[test]
    fn a_piece_left_out_of_the_skeleton_is_named() {
        let error = join_with("<!-- include a.css -->\n", &[("a.css", ""), ("b.js", "")]);
        assert!(error.unwrap_err().contains("no include line for b.js"));
    }

    #[test]
    fn a_piece_included_twice_is_refused() {
        let skeleton = "<!-- include a.css -->\n<!-- include a.css -->\n";
        let error = join_with(skeleton, &[("a.css", "x\n")]).unwrap_err();
        assert!(error.starts_with("skel.html:2:") && error.contains("second time"));
    }

    #[test]
    fn a_piece_that_is_not_there_is_refused_even_by_case() {
        let error = join_with("<!-- include A.css -->\n", &[("a.css", "x\n")]).unwrap_err();
        assert!(error.contains("no such piece"));
    }

    #[test]
    fn a_line_that_starts_like_an_include_must_be_one() {
        for bad in [
            "<!--include a.css-->",
            "<!-- include  a.css -->",
            "<!-- include a.css-->",
            "<!-- include ../a.css -->",
            "<!-- include ./a.css -->",
            "<!-- include a\\b.css -->",
            "<!-- include /a.css -->",
            "<!-- include a b.css -->",
            "<!-- include a.txt -->",
            "<!-- include -->",
        ] {
            let error = join_with(&format!("{bad}\n"), &[("a.css", "x\n")]).unwrap_err();
            assert!(error.contains("is not an include line"), "{bad}: {error}");
        }
    }

    #[test]
    fn a_comment_that_only_mentions_include_is_text() {
        let skeleton = "<!-- included below -->\n<!-- the include lines -->\n";
        assert_eq!(join_with(skeleton, &[]).unwrap(), skeleton);
    }

    #[test]
    fn a_piece_without_a_final_line_break_gets_one() {
        let skeleton = "<!-- include a.js -->\n<!-- include b.js -->\n";
        let joined = join_with(skeleton, &[("a.js", "f() // done"), ("b.js", "g()\n")]).unwrap();
        assert_eq!(joined, "f() // done\ng()\n");
    }

    #[test]
    fn a_byte_order_mark_is_dropped_from_a_piece_and_the_skeleton() {
        let joined = join_with("<!-- include a.css -->\n", &[("a.css", "\u{feff}.x{}\n")]).unwrap();
        assert_eq!(joined, ".x{}\n");
        let joined = join_with("\u{feff}<!-- include a.css -->\n", &[("a.css", ".x{}\n")]).unwrap();
        assert_eq!(joined, ".x{}\n");
    }

    #[test]
    fn an_include_line_inside_a_piece_is_refused() {
        let pieces = [("a.html", "<p>\n<!-- include b.html -->\n"), ("b.html", "")];
        let error = join_with(
            "<!-- include a.html -->\n<!-- include b.html -->\n",
            &pieces,
        );
        assert!(error
            .unwrap_err()
            .starts_with("a.html:2: an include line only works"));
    }

    #[test]
    fn only_page_files_count_as_pieces() {
        for piece in ["hero/hero.css", "a.html", "page/dom.js"] {
            assert!(is_piece_name(piece), "{piece}");
        }
        for junk in [
            ".DS_Store",
            "Thumbs.db",
            "desktop.ini",
            "hero/.hero.js.swp",
            "hero/.#hero.js",
            "hero/hero.js~",
            "notes.md",
        ] {
            assert!(!is_piece_name(junk), "{junk}");
        }
    }

    /// The real skeletons: every piece on disk included once, and the
    /// result framed the way the served files start and end.
    #[test]
    fn the_shipped_skeletons_build() {
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
        for (skeleton, starts, ends) in [
            ("dashboard/index.html", "<!doctype html>", "</html>"),
            (
                "integrations/integrations.js",
                "// Integrations tab",
                "})();",
            ),
        ] {
            let path = web.join(skeleton);
            let dir = path.parent().unwrap();
            let skeleton_name = path.file_name().unwrap().to_string_lossy().into_owned();
            let pieces = pieces_in(dir, &skeleton_name).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            let joined = join(skeleton, &text, &pieces, |piece| {
                std::fs::read_to_string(dir.join(piece)).map_err(|e| e.to_string())
            })
            .unwrap();
            assert!(joined.starts_with(starts), "{skeleton}");
            assert!(joined.trim_end().ends_with(ends), "{skeleton}");
        }
    }

    #[test]
    fn a_folder_lists_its_pieces_without_junk_and_refuses_unwritable_names() {
        let dir = crate::test_support::TestDir::new("pieced");
        std::fs::create_dir_all(dir.join("hero")).unwrap();
        for file in [
            "skel.html",
            "hero/hero.css",
            "hero/.hero.css.swp",
            "Thumbs.db",
            "b.js",
        ] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        assert_eq!(
            pieces_in(dir.path(), "skel.html").unwrap(),
            ["b.js", "hero/hero.css"]
        );

        std::fs::write(dir.join("my piece.css"), "").unwrap();
        let error = pieces_in(dir.path(), "skel.html").unwrap_err();
        assert!(error.starts_with("my piece.css"), "{error}");
    }
}
