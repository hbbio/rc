use std::ffi::{OsStr, OsString};

/// A reversible, single-line spelling for native paths in editable text fields.
/// Backslashes are doubled; invalid UTF-8 bytes and control bytes use `\xHH`.
pub fn encode_path_text(value: &OsStr) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut result = String::new();
        let mut bytes = value.as_bytes();
        while !bytes.is_empty() {
            let (text, invalid) = match std::str::from_utf8(bytes) {
                Ok(text) => (text, 0),
                Err(error) => (
                    std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid prefix"),
                    error
                        .error_len()
                        .unwrap_or(bytes.len() - error.valid_up_to()),
                ),
            };
            for ch in text.chars() {
                if ch == '\\' {
                    result.push_str("\\\\");
                } else if ch.is_control() {
                    let mut buf = [0; 4];
                    for byte in ch.encode_utf8(&mut buf).as_bytes() {
                        append_byte(&mut result, *byte);
                    }
                } else {
                    result.push(ch);
                }
            }
            bytes = &bytes[text.len()..];
            for byte in &bytes[..invalid] {
                append_byte(&mut result, *byte);
            }
            bytes = &bytes[invalid..];
        }
        if result.trim() != result {
            let leading = result.len() - result.trim_start().len();
            let end = result.trim_end().len().max(leading);
            let mut escaped = String::new();
            for byte in &result.as_bytes()[..leading] {
                append_byte(&mut escaped, *byte);
            }
            escaped.push_str(&result[leading..end]);
            for byte in &result.as_bytes()[end..] {
                append_byte(&mut escaped, *byte);
            }
            escaped
        } else {
            result
        }
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().into_owned()
    }
}

#[cfg(unix)]
fn append_byte(text: &mut String, byte: u8) {
    use std::fmt::Write;
    write!(text, "\\x{byte:02X}").expect("writing to a String");
}

/// Decode editable path text; unrecognized escapes remain literal.
pub fn decode_path_text(text: &str) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut result = Vec::with_capacity(text.len());
        let mut bytes = text.as_bytes();
        while let Some((&first, rest)) = bytes.split_first() {
            if first == b'\\' {
                if rest.first() == Some(&b'\\') {
                    result.push(b'\\');
                    bytes = &rest[1..];
                    continue;
                }
                if rest.len() >= 3
                    && rest[0] == b'x'
                    && let (Some(high), Some(low)) = (hex(rest[1]), hex(rest[2]))
                {
                    result.push(high * 16 + low);
                    bytes = &rest[3..];
                    continue;
                }
            }
            result.push(first);
            bytes = rest;
        }
        OsString::from_vec(result)
    }
    #[cfg(not(unix))]
    {
        text.into()
    }
}

#[cfg(unix)]
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn path_text_round_trips_every_byte_and_distinguishes_literal_escapes() {
        let value = OsString::from_vec((1..=255).collect());
        let text = encode_path_text(&value);
        assert!(text.chars().all(|ch| !ch.is_control()));
        assert_eq!(decode_path_text(&text), value);
        for value in [
            OsString::from("notes/日本語\\xFF.txt"),
            OsString::from_vec(b"bad\xff\n".to_vec()),
        ] {
            assert_eq!(decode_path_text(&encode_path_text(&value)), value);
        }
        for value in [
            "",
            " ",
            "  ",
            " a ",
            " a",
            "a ",
            "\u{a0}a\u{2003}",
            "a\u{a0}",
            "\u{2003}\u{a0}",
        ] {
            assert_eq!(
                encode_path_text(OsStr::new(value)).trim(),
                encode_path_text(OsStr::new(value))
            );
            assert_eq!(
                decode_path_text(&encode_path_text(OsStr::new(value))),
                OsStr::new(value)
            );
        }
        assert_eq!(
            decode_path_text(r"literal\q\xQQ"),
            OsStr::new(r"literal\q\xQQ")
        );
    }
}
