use unicode_segmentation::UnicodeSegmentation;

pub const DISPLAY_FIELD_LIMIT_BYTES: usize = 4 * 1024;
pub const DISPLAY_LINE_LIMIT_BYTES: usize = 8 * 1024;

pub fn sanitize_display_field(value: &str) -> String {
    sanitize_and_bound(value, DISPLAY_FIELD_LIMIT_BYTES)
}

pub fn sanitize_display_line(value: &str) -> String {
    sanitize_and_bound(value, DISPLAY_LINE_LIMIT_BYTES)
}

pub fn sanitize_display_lossy(value: &[u8]) -> String {
    sanitize_display_field(&String::from_utf8_lossy(value))
}

fn sanitize_and_bound(value: &str, limit: usize) -> String {
    let mut sanitized = String::with_capacity(value.len().min(limit));
    for character in value.chars() {
        match character {
            '\r' | '\n' | '\t' => sanitized.push(' '),
            character if is_terminal_control(character) => {
                let code = character as u32;
                if code <= 0xff {
                    sanitized.push_str(&format!("\\x{code:02x}"));
                } else {
                    sanitized.push_str(&format!("\\u{{{code:x}}}"));
                }
            }
            character => sanitized.push(character),
        }
        // Keep retained memory bounded. Once the sanitized prefix crosses the final-output
        // budget, truncate_graphemes has enough data to omit any partially observed grapheme.
        if sanitized.len() > limit {
            break;
        }
    }
    truncate_graphemes(&sanitized, limit)
}

fn is_terminal_control(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

fn truncate_graphemes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let ellipsis = "…";
    let budget = limit.saturating_sub(ellipsis.len());
    let mut end = 0;
    for (start, grapheme) in value.grapheme_indices(true) {
        let next = start.saturating_add(grapheme.len());
        if next > budget {
            break;
        }
        end = next;
    }
    let mut output = value[..end].to_string();
    if limit >= ellipsis.len() {
        output.push_str(ellipsis);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_sanitizer_never_replays_terminal_controls() {
        assert_eq!(
            sanitize_display_field("a\n\t\u{1b}\u{85}b"),
            "a  \\x1b\\x85b"
        );
    }

    #[test]
    fn display_sanitizer_bounds_on_grapheme_boundary() {
        let value = "x".repeat(DISPLAY_FIELD_LIMIT_BYTES) + "e\u{301}";
        let sanitized = sanitize_display_field(&value);
        assert!(sanitized.len() <= DISPLAY_FIELD_LIMIT_BYTES);
        assert!(!sanitized.ends_with('\u{301}'));

        let pathological = format!(
            "{}e{}",
            "x".repeat(DISPLAY_FIELD_LIMIT_BYTES.saturating_sub(4)),
            "\u{301}".repeat(100_000)
        );
        let sanitized = sanitize_display_field(&pathological);
        assert!(sanitized.len() <= DISPLAY_FIELD_LIMIT_BYTES);
        assert!(sanitized.ends_with('…'));
    }
}
