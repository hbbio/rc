use std::io::{self, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rc_core::MAX_CLIPBOARD_TEXT_BYTES;

const OSC52_PREFIX: &[u8] = b"\x1b]52;c;";
const OSC_STRING_TERMINATOR: &[u8] = b"\x1b\\";

pub(crate) fn write_text(writer: &mut impl Write, text: &str) -> io::Result<()> {
    if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("OSC 52 copy limit is {MAX_CLIPBOARD_TEXT_BYTES} bytes"),
        ));
    }
    writer.write_all(OSC52_PREFIX)?;
    writer.write_all(STANDARD.encode(text).as_bytes())?;
    writer.write_all(OSC_STRING_TERMINATOR)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_is_base64_encoded_in_an_osc52_sequence() {
        let mut output = Vec::new();

        write_text(&mut output, "/tmp/a file.txt").expect("clipboard sequence should be written");

        assert_eq!(output, b"\x1b]52;c;L3RtcC9hIGZpbGUudHh0\x1b\\");
    }

    #[test]
    fn clipboard_text_cannot_inject_terminal_controls() {
        let mut output = Vec::new();

        write_text(&mut output, "path\x1b]52;c;injected\x07")
            .expect("clipboard sequence should be written");

        assert_eq!(output.iter().filter(|byte| **byte == 0x1b).count(), 2);
        assert!(!output.contains(&0x07));
    }

    #[test]
    fn osc52_limit_includes_base64_padding_and_framing() {
        let mut input = "a".repeat(MAX_CLIPBOARD_TEXT_BYTES);
        let mut output = Vec::new();
        write_text(&mut output, &input).expect("maximum-length selection should be written");
        assert!(output.len() <= 600_000);

        output.clear();
        input.push('a');
        let error = write_text(&mut output, &input).expect_err("oversized copy should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(output.is_empty());
    }
}
