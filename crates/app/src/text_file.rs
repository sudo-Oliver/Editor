//! Reads and writes documents in the encoding they came in. Older projects often
//! use Latin-1 or Windows-1252, with `\usepackage[latin1]{inputenc}`, and saving
//! such a file as UTF-8 would break its build.

use std::{borrow::Cow, fs, io, path::Path};

pub use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};

pub struct Loaded {
    pub text: String,
    pub encoding: &'static Encoding,
}

/// UTF-8 when the bytes allow it, else Windows-1252, which maps every byte to a
/// character and back to the same byte. A NUL byte means the file is no text.
pub fn decode(bytes: Vec<u8>) -> io::Result<Loaded> {
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Loaded { text, encoding: UTF_8 }),
        Err(error) => {
            let bytes = error.into_bytes();
            if bytes.contains(&0) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "this is not a text file"));
            }
            let (text, _) = WINDOWS_1252.decode_without_bom_handling(&bytes);
            Ok(Loaded { text: text.into_owned(), encoding: WINDOWS_1252 })
        }
    }
}

/// Larger files are data or logs, not writing, and would stall the editor.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn read(path: &Path) -> io::Result<Loaded> {
    let size = fs::metadata(path)?.len();
    if size > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("it is {} MB, more than the {} MB the editor opens", size / 1024 / 1024, MAX_BYTES / 1024 / 1024),
        ));
    }
    decode(fs::read(path)?)
}

/// The bytes for `text`, or the first character the encoding has no byte for.
pub fn encode<'a>(text: &'a str, encoding: &'static Encoding) -> Result<Cow<'a, [u8]>, char> {
    if encoding == UTF_8 {
        return Ok(Cow::Borrowed(text.as_bytes()));
    }
    let (bytes, _, had_errors) = encoding.encode(text);
    if had_errors {
        let missing = text.chars().find(|character| encoding.encode(character.encode_utf8(&mut [0; 4])).2).unwrap_or('?');
        return Err(missing);
    }
    Ok(bytes)
}

/// The error a save shows when the text holds a character the file's encoding lacks.
pub fn unencodable(character: char, encoding: &'static Encoding) -> io::Error {
    io::Error::other(format!(
        "this file is stored as {}, which has no “{character}”. Remove that character, or choose File > Convert to UTF-8",
        encoding.name()
    ))
}

/// Where `\usepackage[latin1]{inputenc}` names a single-byte encoding, for
/// switching it to `utf8`.
pub fn inputenc_option(text: &str) -> Option<std::ops::Range<usize>> {
    const SINGLE_BYTE: &[&str] = &["latin1", "latin9", "latin15", "ansinew", "cp1252"];
    for (start, _) in text.match_indices("\\usepackage[") {
        let options = start + "\\usepackage[".len();
        let rest = &text[options..];
        let Some(close) = rest.find(']') else { continue };
        if !rest[close..].starts_with("]{inputenc}") {
            continue;
        }
        let option = rest[..close].trim();
        if SINGLE_BYTE.contains(&option) {
            let offset = options + rest[..close].find(option).unwrap_or(0);
            return Some(offset..offset + option.len());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_stays_byte_for_byte() {
        for text in ["", "a\r\nb", "\u{feff}Bitte ä 🙂", "mixed\r\nendings\n"] {
            let loaded = decode(text.as_bytes().to_vec()).unwrap();
            assert_eq!(loaded.encoding, UTF_8);
            assert_eq!(loaded.text, text);
            assert_eq!(encode(&loaded.text, loaded.encoding).unwrap().as_ref(), text.as_bytes());
        }
    }

    #[test]
    fn every_single_byte_survives_the_round_trip() {
        // All bytes but NUL, which is not text, in one file that is not UTF-8.
        let bytes: Vec<u8> = (1..=255).collect();
        let loaded = decode(bytes.clone()).unwrap();
        assert_eq!(loaded.encoding, WINDOWS_1252);
        assert_eq!(encode(&loaded.text, loaded.encoding).unwrap().as_ref(), bytes.as_slice());
    }

    #[test]
    fn latin1_umlauts_read_and_write_back() {
        let bytes = b"\\usepackage[latin1]{inputenc}\nM\xfcller, Stra\xdfe, \xc4rger\n".to_vec();
        let loaded = decode(bytes.clone()).unwrap();
        assert!(loaded.text.contains("Müller, Straße, Ärger"));
        let edited = loaded.text.replace("Ärger", "Öl");
        assert_eq!(encode(&edited, loaded.encoding).unwrap().as_ref(), b"\\usepackage[latin1]{inputenc}\nM\xfcller, Stra\xdfe, \xd6l\n");
    }

    #[test]
    fn a_character_the_encoding_lacks_is_named() {
        assert_eq!(encode("ok 🙂 and ★", WINDOWS_1252).unwrap_err(), '🙂');
    }

    #[test]
    fn finds_the_inputenc_option_to_switch() {
        let text = "\\usepackage[T1]{fontenc}\n\\usepackage[latin1]{inputenc}\n";
        let range = inputenc_option(text).unwrap();
        assert_eq!(&text[range], "latin1");
        assert!(inputenc_option("\\usepackage[utf8]{inputenc}").is_none());
        assert!(inputenc_option("\\usepackage[latin1]{other}").is_none());
    }

    #[test]
    fn binary_files_are_refused() {
        assert!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec()).is_err());
    }
}
