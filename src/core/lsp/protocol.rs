//! What the JSON-RPC messages carry: locations and positions (UTF-16 columns on the wire,
//! byte columns here) and file URIs.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::OpenDocument;
use crate::core::location::{Location, Position, Range};

pub(super) fn parse_lsp_locations(
    result: &Value,
    open_documents: &HashMap<String, OpenDocument>,
) -> Vec<Location> {
    match result {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| parse_lsp_location(item, open_documents))
            .collect(),
        Value::Object(_) => parse_lsp_location(result, open_documents)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_lsp_location(
    value: &Value,
    open_documents: &HashMap<String, OpenDocument>,
) -> Option<Location> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let path = file_uri_to_path(uri)?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))
        .or_else(|| value.get("targetRange"))?;
    let start = parse_lsp_position(uri, &path, range.get("start")?, open_documents)?;
    let end = parse_lsp_position(uri, &path, range.get("end")?, open_documents)?;

    Some(Location::new(path, Range::new(start, end)))
}

fn parse_lsp_position(
    uri: &str,
    path: &Path,
    value: &Value,
    open_documents: &HashMap<String, OpenDocument>,
) -> Option<Position> {
    let line = value.get("line")?.as_u64()? as u32;
    let character = value.get("character")?.as_u64()? as u32;
    let text = open_documents
        .get(uri)
        .map(|document| document.text.as_str())
        .map(str::to_owned)
        .or_else(|| fs::read_to_string(path).ok());
    let character = text
        .as_deref()
        .and_then(|text| utf16_character_to_byte_character(text, line, character))
        .unwrap_or(character);
    Some(Position::new(line, character))
}

pub(super) fn byte_position_to_lsp_position(text: &str, position: Position) -> Value {
    let character = byte_character_to_utf16_character(text, position.line, position.character)
        .unwrap_or(position.character);
    json!({
        "line": position.line,
        "character": character,
    })
}

fn byte_character_to_utf16_character(text: &str, line: u32, character: u32) -> Option<u32> {
    let line = line_text(text, line)?;
    let mut byte_index = character.min(line.len() as u32) as usize;
    while byte_index > 0 && !line.is_char_boundary(byte_index) {
        byte_index -= 1;
    }
    Some(line[..byte_index].encode_utf16().count() as u32)
}

fn utf16_character_to_byte_character(text: &str, line: u32, character: u32) -> Option<u32> {
    let line = line_text(text, line)?;
    let mut utf16_count = 0_u32;
    for (byte_index, ch) in line.char_indices() {
        let next_count = utf16_count + ch.len_utf16() as u32;
        if next_count > character {
            return Some(byte_index as u32);
        }
        utf16_count = next_count;
    }
    Some(line.len() as u32)
}

fn line_text(text: &str, line: u32) -> Option<&str> {
    text.split('\n')
        .nth(line as usize)
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

pub(super) fn path_to_file_uri(path: &Path) -> String {
    let mut path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    if cfg!(windows) {
        path = normalize_windows_file_uri_path(path);
        if !path.starts_with('/') {
            path = format!("/{path}");
        }
    }
    if cfg!(windows) && path.starts_with("//") {
        format!("file:{}", percent_encode_uri_path(&path))
    } else {
        format!("file://{}", percent_encode_uri_path(&path))
    }
}

fn normalize_windows_file_uri_path(path: String) -> String {
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        return format!("//{rest}");
    }
    if let Some(rest) = path.strip_prefix("//?/") {
        return rest.to_owned();
    }
    if let Some(rest) = path.strip_prefix("//./") {
        return rest.to_owned();
    }
    path
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let path = percent_decode_uri_path(path);
    if !cfg!(windows) {
        return Some(PathBuf::from(path));
    }
    let path = match path.strip_prefix("localhost") {
        Some(rest) if rest.starts_with('/') => rest.to_owned(),
        _ => path,
    };
    // UNC paths come as `file://server/share/...` (what `path_to_file_uri` writes) or
    // `file:////server/share/...`.
    let unc = if let Some(rest) = path.strip_prefix("//") {
        Some(rest)
    } else if !path.starts_with('/') {
        Some(path.as_str())
    } else {
        None
    };
    if let Some(unc) = unc {
        return Some(PathBuf::from(format!(r"\\{}", unc.replace('/', "\\"))));
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[2] == b':' {
        Some(PathBuf::from(&path[1..]))
    } else {
        Some(PathBuf::from(path))
    }
}

fn percent_encode_uri_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode_uri_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&path[index + 1..index + 3], 16)
        {
            decoded.push(value);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_byte_position_to_lsp_utf16_position() {
        let text = "abc\nあb😀c\n";
        assert_eq!(
            byte_character_to_utf16_character(text, 1, "あb".len() as u32),
            Some(2)
        );
        assert_eq!(
            byte_character_to_utf16_character(text, 1, "あb😀".len() as u32),
            Some(4)
        );
    }

    #[test]
    fn converts_lsp_utf16_position_to_byte_position() {
        let text = "abc\nあb😀c\n";
        assert_eq!(
            utf16_character_to_byte_character(text, 1, 2),
            Some("あb".len() as u32)
        );
        assert_eq!(
            utf16_character_to_byte_character(text, 1, 4),
            Some("あb😀".len() as u32)
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_verbatim_path_uri_strips_device_prefix() {
        let path = PathBuf::from(r"\\?\C:\git\code-reader\src\app.rs");
        assert_eq!(
            path_to_file_uri(&path),
            "file:///C:/git/code-reader/src/app.rs"
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_unc_verbatim_path_uri_uses_unc_authority() {
        let path = PathBuf::from(r"\\?\UNC\server\share\src\app.rs");
        assert_eq!(path_to_file_uri(&path), "file://server/share/src/app.rs");
    }

    #[test]
    #[cfg(windows)]
    fn windows_file_uris_round_trip_drive_and_unc_paths() {
        assert_eq!(
            file_uri_to_path("file://server/share/src/my%20app.rs"),
            Some(PathBuf::from(r"\\server\share\src\my app.rs"))
        );
        assert_eq!(
            file_uri_to_path("file:////server/share/src/app.rs"),
            Some(PathBuf::from(r"\\server\share\src\app.rs"))
        );
        assert_eq!(
            file_uri_to_path("file:///C:/git/app.rs"),
            Some(PathBuf::from("C:/git/app.rs"))
        );
        assert_eq!(
            file_uri_to_path("file://localhost/C:/git/app.rs"),
            Some(PathBuf::from("C:/git/app.rs"))
        );
        let unc = PathBuf::from(r"\\server\share\src\app.rs");
        assert_eq!(file_uri_to_path(&path_to_file_uri(&unc)), Some(unc));
    }
}
