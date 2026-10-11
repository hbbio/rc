use std::io::{self, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

const OSC52_PREFIX: &[u8] = b"\x1b]52;c;";
const OSC_STRING_TERMINATOR: &[u8] = b"\x1b\\";

pub(crate) fn write_text(writer: &mut impl Write, text: &str) -> io::Result<()> {
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
}
