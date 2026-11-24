//! The exe's Windows resources (version info, icon, manifest), written as a
//! compiled `.res` file that the MSVC linker takes as a plain input.
//!
//! Why: an exe with no version info, no icon and no manifest is one of the
//! signals antivirus models score as malware (Defender quarantined
//! InstantClone mid-stream, issue #101). Hand-built like the tray icon, so
//! the build needs no resource compiler and no extra crates.
//!
//! Format reference: "Resource File Formats" (RESOURCEHEADER) and
//! "VS_VERSIONINFO structure" in the Win32 docs.

use std::path::Path;

/// Resource type ids (winuser.h).
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
/// en-US: the language the version strings are written in.
const LANGUAGE_EN_US: u16 = 0x0409;
/// The code page the version strings declare (UTF-16).
const CODE_PAGE_UNICODE: u16 = 1200;
/// The memory flags rc.exe writes: MOVEABLE | PURE, plus DISCARDABLE for icons.
const FLAGS_DEFAULT: u16 = 0x0030;
const FLAGS_ICON: u16 = 0x1010;
const FLAGS_ICON_GROUP: u16 = 0x1030;

/// Requests normal user rights (no elevation prompt) and declares Windows
/// 10/11. Deliberately leaves out DPI awareness and Common Controls v6 so
/// the tray menu and message boxes look exactly as before.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

/// What the exe says about itself in Explorer's Properties > Details.
pub struct ProductInfo<'a> {
    pub version: &'a str,
    pub company: &'a str,
    pub product: &'a str,
}

/// Write the `.res` holding the version info, the manifest, and `icons`
/// (one `(size, ICO image bytes)` pair per size) as the exe's icon.
pub fn write_res(path: &Path, info: &ProductInfo, icons: &[(u32, Vec<u8>)]) {
    // A .res starts with an empty entry that marks it as 32-bit.
    let mut res = resource(0, 0, 0, &[]);
    for (index, (_, image)) in icons.iter().enumerate() {
        res.extend(resource(RT_ICON, index as u16 + 1, FLAGS_ICON, image));
    }
    res.extend(resource(
        RT_GROUP_ICON,
        1,
        FLAGS_ICON_GROUP,
        &icon_group(icons),
    ));
    res.extend(resource(RT_VERSION, 1, FLAGS_DEFAULT, &version_info(info)));
    res.extend(resource(RT_MANIFEST, 1, FLAGS_DEFAULT, MANIFEST.as_bytes()));
    std::fs::write(path, res).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// One RESOURCEHEADER (type and name as ordinals) plus its data, padded
/// to a 4-byte boundary as the format requires.
fn resource(kind: u16, id: u16, flags: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + data.len() + 3);
    let language = if kind == 0 { 0 } else { LANGUAGE_EN_US };
    out.extend((data.len() as u32).to_le_bytes()); // DataSize
    out.extend(32u32.to_le_bytes()); // HeaderSize
    out.extend(
        [0xFFFF, kind, 0xFFFF, id]
            .iter()
            .flat_map(|w| w.to_le_bytes()),
    );
    out.extend(0u32.to_le_bytes()); // DataVersion
    out.extend(flags.to_le_bytes()); // MemoryFlags
    out.extend(language.to_le_bytes()); // LanguageId
    out.extend(0u32.to_le_bytes()); // Version
    out.extend(0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    pad_to_4(&mut out);
    out
}

/// GRPICONDIR: the ICO directory, with each image's file offset replaced by
/// the RT_ICON id it was stored under.
fn icon_group(icons: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes()); // reserved
    out.extend(1u16.to_le_bytes()); // type: icon
    out.extend((icons.len() as u16).to_le_bytes());
    for (index, (size, image)) in icons.iter().enumerate() {
        // 0 means 256 in an icon directory entry.
        let dimension = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([dimension, dimension, 0, 0]); // width, height, colors, reserved
        out.extend(1u16.to_le_bytes()); // planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((image.len() as u32).to_le_bytes());
        out.extend((index as u16 + 1).to_le_bytes());
    }
    out
}

/// VS_VERSIONINFO: the fixed numeric block, then the strings Explorer shows.
fn version_info(info: &ProductInfo) -> Vec<u8> {
    let [major, minor, patch] = version_numbers(info.version);
    let packed_high = (u32::from(major) << 16) | u32::from(minor);
    let packed_low = u32::from(patch) << 16;
    let fixed: Vec<u8> = [
        0xFEEF_04BD, // signature
        0x0001_0000, // structure version 1.0
        packed_high, // file version
        packed_low,
        packed_high, // product version
        packed_low,
        0x3F,        // flags mask: every defined flag
        0,           // flags: a plain release build
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,           // subtype
        0,           // date (unused)
        0,
    ]
    .iter()
    .flat_map(|dword: &u32| dword.to_le_bytes())
    .collect();

    let file_name = "instantclone.exe";
    let strings = [
        ("CompanyName", info.company),
        ("FileDescription", info.product),
        ("FileVersion", info.version),
        ("InternalName", "instantclone"),
        ("LegalCopyright", "Copyright (C) s1moscs. GPL-3.0-or-later"),
        ("OriginalFilename", file_name),
        ("ProductName", info.product),
        ("ProductVersion", info.version),
    ];
    let string_nodes: Vec<Vec<u8>> = strings
        .iter()
        .map(|(key, value)| text_node(key, value))
        .collect();
    let table_key = format!("{LANGUAGE_EN_US:04x}{CODE_PAGE_UNICODE:04x}");
    let table = node(&table_key, NodeValue::None, &string_nodes);
    let string_file_info = node("StringFileInfo", NodeValue::None, &[table]);

    let translation: Vec<u8> = [LANGUAGE_EN_US, CODE_PAGE_UNICODE]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let var = node("Translation", NodeValue::Binary(&translation), &[]);
    let var_file_info = node("VarFileInfo", NodeValue::None, &[var]);

    node(
        "VS_VERSION_INFO",
        NodeValue::Binary(&fixed),
        &[string_file_info, var_file_info],
    )
}

/// `"1.2.3"` (or `"1.2.3-rc1"`) as its three numbers; missing parts are 0.
fn version_numbers(version: &str) -> [u16; 3] {
    let mut numbers = [0u16; 3];
    let release = version.split(['-', '+']).next().unwrap_or("");
    for (slot, part) in numbers.iter_mut().zip(release.split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    numbers
}

enum NodeValue<'a> {
    None,
    Binary(&'a [u8]),
    Text(&'a str),
}

fn text_node(key: &str, value: &str) -> Vec<u8> {
    node(key, NodeValue::Text(value), &[])
}

/// One version-info block: length, value length, type, key, value, children.
/// Each part starts on a 4-byte boundary; the length counts everything up
/// to the end of the last child.
fn node(key: &str, value: NodeValue, children: &[Vec<u8>]) -> Vec<u8> {
    let (value_bytes, value_length, is_text) = match value {
        NodeValue::None => (Vec::new(), 0, true),
        NodeValue::Binary(bytes) => (bytes.to_vec(), bytes.len(), false),
        // Text values count UTF-16 units, including the terminating NUL.
        NodeValue::Text(text) => {
            let units = utf16_with_nul(text);
            (
                units.iter().flat_map(|u| u.to_le_bytes()).collect(),
                units.len(),
                true,
            )
        }
    };
    let mut out = vec![0u8; 6]; // length, value length and type, filled below
    out.extend(utf16_with_nul(key).iter().flat_map(|u| u.to_le_bytes()));
    pad_to_4(&mut out);
    out.extend(&value_bytes);
    for child in children {
        pad_to_4(&mut out);
        out.extend(child);
    }
    let length = out.len() as u16;
    out[0..2].copy_from_slice(&length.to_le_bytes());
    out[2..4].copy_from_slice(&(value_length as u16).to_le_bytes());
    out[4..6].copy_from_slice(&u16::from(is_text).to_le_bytes());
    out
}

fn utf16_with_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pad_to_4(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}
