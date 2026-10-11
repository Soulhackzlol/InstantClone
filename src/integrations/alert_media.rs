//! Sounds and pictures uploaded for alerts, kept in `alerts-media` next to
//! the config and served to the alerts page at `/alerts/media/<name>`.
//!
//! A name is checked against a strict character set and a short list of
//! extensions, so it can never leave the folder, and each file is served
//! with the type its extension says (never sniffed): an upload can only
//! ever be played as a sound or shown as a picture.

use std::path::{Path, PathBuf};

pub const DIR: &str = "alerts-media";
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
const MAX_FILES: usize = 200;
const MAX_NAME: usize = 64;
/// How a step or the page's look names an upload: `media:ding.mp3`.
pub const REF_PREFIX: &str = "media:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sound,
    Picture,
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::Sound => "sound",
            Kind::Picture => "picture",
        }
    }
}

/// Extension, kind, and the content type it is served with.
const TYPES: &[(&str, Kind, &str)] = &[
    ("mp3", Kind::Sound, "audio/mpeg"),
    ("ogg", Kind::Sound, "audio/ogg"),
    ("wav", Kind::Sound, "audio/wav"),
    ("m4a", Kind::Sound, "audio/mp4"),
    ("webm", Kind::Sound, "audio/webm"),
    ("png", Kind::Picture, "image/png"),
    ("jpg", Kind::Picture, "image/jpeg"),
    ("jpeg", Kind::Picture, "image/jpeg"),
    ("gif", Kind::Picture, "image/gif"),
    ("webp", Kind::Picture, "image/webp"),
];

pub struct Media {
    pub name: String,
    pub kind: Kind,
    pub bytes: u64,
}

/// Where uploads live: beside the config, like the buffer.
pub fn folder(cfg_path: &Path) -> PathBuf {
    crate::config::beside_config(cfg_path, Path::new(DIR))
}

/// The kind and content type of a stored file's name, or `None` when the
/// name isn't one this folder could hold.
pub fn type_of(name: &str) -> Option<(Kind, &'static str)> {
    let safe = !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        && !is_device_name(name.split('.').next().unwrap_or(""));
    if !safe {
        return None;
    }
    let (_, ext) = name.rsplit_once('.')?;
    TYPES
        .iter()
        .find(|(e, ..)| *e == ext)
        .map(|(_, kind, ctype)| (*kind, *ctype))
}

/// A name Windows keeps for a device (`con`, `nul`, `com1`...), whatever
/// follows its first dot: a file saved as `con.mp3` would go to the device.
fn is_device_name(stem: &str) -> bool {
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
    };
    matches!(stem, "con" | "prn" | "aux" | "nul") || numbered("com") || numbered("lpt")
}

/// `media:<name>` naming an upload of this kind.
pub fn is_ref(text: &str, kind: Kind) -> bool {
    text.strip_prefix(REF_PREFIX)
        .and_then(type_of)
        .is_some_and(|(k, _)| k == kind)
}

/// The name an upload is stored under: lower case, spaces as dashes,
/// anything else dropped. `None` when it isn't a type we serve.
pub fn clean_name(original: &str) -> Option<String> {
    let base = original
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let (stem, ext) = base.rsplit_once('.')?;
    let stem: String = stem
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(MAX_NAME - 10)
        .collect();
    let stem = stem.trim_matches(['-', '_']);
    let name = match stem {
        "" => format!("file.{ext}"),
        device if is_device_name(device) => format!("{device}-file.{ext}"),
        _ => format!("{stem}.{ext}"),
    };
    type_of(&name).map(|_| name)
}

/// Every upload, by name.
pub fn list(folder: &Path) -> Vec<Media> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<Media> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (kind, _) = type_of(&name)?;
            let bytes = e.metadata().ok().filter(|m| m.is_file())?.len();
            Some(Media { name, kind, bytes })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Keep an upload; returns the name it was saved under (with a number
/// added when that name is taken).
pub fn save(folder: &Path, original: &str, data: &[u8]) -> Result<String, String> {
    let name = clean_name(original)
        .ok_or("use an MP3, OGG, WAV, M4A or WEBM sound, or a PNG, JPG, GIF or WEBP picture")?;
    if data.is_empty() {
        return Err("that file is empty".to_string());
    }
    if data.len() > MAX_BYTES {
        return Err("that file is over 10 MB".to_string());
    }
    if list(folder).len() >= MAX_FILES {
        return Err(format!(
            "there are already {MAX_FILES} uploads: delete some first"
        ));
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("couldn't make the folder: {e}"))?;
    let name = free_name(folder, &name);
    // Written aside, then moved in: the page never plays half a file.
    let part = folder.join(format!(".{name}.part"));
    std::fs::write(&part, data).map_err(|e| format!("couldn't save it: {e}"))?;
    std::fs::rename(&part, folder.join(&name)).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("couldn't save it: {e}")
    })?;
    Ok(name)
}

fn free_name(folder: &Path, name: &str) -> String {
    if !folder.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| format!("{stem}-{n}.{ext}"))
        .find(|candidate| !folder.join(candidate).exists())
        .unwrap_or_else(|| name.to_string())
}

pub fn delete(folder: &Path, name: &str) -> Result<(), String> {
    type_of(name).ok_or("there's no such upload")?;
    std::fs::remove_file(folder.join(name)).map_err(|e| format!("couldn't delete it: {e}"))
}

/// Where an upload is on disk and its content type, for serving.
pub fn locate(folder: &Path, name: &str) -> Option<(PathBuf, &'static str)> {
    let (_, ctype) = type_of(name)?;
    Some((folder.join(name), ctype))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_can_never_leave_the_folder() {
        for bad in [
            "../config.mp3",
            "..\\x.mp3",
            "a/b.mp3",
            ".hidden.mp3",
            "page.html",
            "pic.svg",
            "Ding.mp3",
            "",
            "con.mp3",
            "nul.tar.png",
            "com1.wav",
        ] {
            assert!(type_of(bad).is_none(), "{bad}");
        }
        assert_eq!(type_of("ding-2.mp3").map(|t| t.1), Some("audio/mpeg"));
        assert_eq!(type_of("party.gif").map(|t| t.0), Some(Kind::Picture));
        assert!(is_ref("media:party.gif", Kind::Picture));
        assert!(!is_ref("media:party.gif", Kind::Sound));
        assert!(!is_ref("media:../x.mp3", Kind::Sound));
    }

    #[test]
    fn uploaded_names_are_cleaned() {
        assert_eq!(
            clean_name("C:\\Users\\me\\My Follow Sound!.MP3").as_deref(),
            Some("my-follow-sound.mp3")
        );
        assert_eq!(clean_name("???.png").as_deref(), Some("file.png"));
        // Windows device names can't be files: they get a suffix.
        assert_eq!(clean_name("CON.mp3").as_deref(), Some("con-file.mp3"));
        assert_eq!(clean_name("lpt9.gif").as_deref(), Some("lpt9-file.gif"));
        assert_eq!(clean_name("console.mp3").as_deref(), Some("console.mp3"));
        assert_eq!(clean_name("evil.html"), None);
        assert_eq!(clean_name("noext"), None);
    }

    #[test]
    fn uploads_are_kept_listed_and_deleted() {
        let dir = Scratch(
            std::env::temp_dir().join(format!("ic-media-{}", &crate::crypto::random_token()[..8])),
        );
        let first = save(&dir.0, "Ding.mp3", b"ID3data").unwrap();
        let second = save(&dir.0, "ding.mp3", b"ID3more").unwrap();
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("ding.mp3", "ding-2.mp3")
        );
        assert!(save(&dir.0, "empty.png", b"").is_err());
        let names: Vec<String> = list(&dir.0).into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["ding-2.mp3", "ding.mp3"]);
        delete(&dir.0, "ding.mp3").unwrap();
        assert_eq!(list(&dir.0).len(), 1);
        assert!(delete(&dir.0, "../ding-2.mp3").is_err());
    }
}
