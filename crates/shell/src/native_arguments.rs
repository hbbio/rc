use std::collections::HashSet;
use std::ffi::{OsStr, OsString};

use crate::{ShellDialect, quote_literal_token};

const PREFIX: &str = "RC_NATIVE_HEX_";

/// A normal shell literal where possible, otherwise a self-contained reference
/// to a native environment value. The hex identity survives command history.
pub fn quote_native_argument(value: &OsStr, dialect: ShellDialect) -> String {
    if let Some(text) = value.to_str()
        && !text.chars().any(char::is_control)
    {
        return quote_literal_token(text, dialect);
    }
    use std::fmt::Write;
    let mut token = format!("\"${PREFIX}");
    for byte in value.as_encoded_bytes() {
        write!(token, "{byte:02X}").expect("writing a String");
    }
    token.push('"');
    token
}

/// Restore the native values referred to by generated argument tokens. Quoting
/// remains in the shell script, and values are never injected into shell syntax.
pub fn native_argument_environment(script: &str) -> Vec<(String, OsString)> {
    let mut environment = Vec::new();
    let mut seen = HashSet::new();
    for (offset, _) in script.match_indices(&format!("${PREFIX}")) {
        let rest = &script[offset + 1..];
        let end = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        let name = &rest[..end];
        if seen.insert(name)
            && let Some(value) = decode_native_variable(name)
        {
            environment.push((name.to_owned(), value));
        }
    }
    environment
}

pub(crate) fn decode_native_variable(name: &str) -> Option<OsString> {
    let hex = name.strip_prefix(PREFIX)?;
    if hex.is_empty() || hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().as_chunks::<2>().0 {
        let high = char::from(pair[0]).to_digit(16)? as u8;
        let low = char::from(pair[1]).to_digit(16)? as u8;
        let byte = high * 16 + low;
        if byte == 0 {
            return None;
        }
        bytes.push(byte);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(OsString::from)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn native_shell_arguments_preserve_bytes_quotes_and_trailing_newlines() {
        let value = OsString::from_vec(b"raw\xff'\"$(false)\n\n".to_vec());
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh"] {
            if !std::path::Path::new(shell).exists() {
                continue;
            }
            let token = quote_native_argument(&value, ShellDialect::Posix);
            let script = format!("printf '%s' {token}");
            let output = std::process::Command::new(shell)
                .arg("-c")
                .arg(&script)
                .envs(native_argument_environment(&script))
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, value.as_encoded_bytes());
        }
    }

    #[test]
    fn native_references_are_bounded_by_input_and_deduplicated() {
        let value = OsString::from_vec(vec![0xff]);
        let token = quote_native_argument(&value, ShellDialect::Fish);
        assert_eq!(
            native_argument_environment(&format!("{token} {token}")),
            vec![(format!("{PREFIX}FF"), value)]
        );
        for invalid in [
            "RC_NATIVE_HEX_00",
            "RC_NATIVE_HEX_0",
            "RC_NATIVE_HEX_FFG",
            "RC_NATIVE_HEX_",
        ] {
            assert!(decode_native_variable(invalid).is_none());
        }
        assert_eq!(
            quote_native_argument(OsStr::new("a b"), ShellDialect::Posix),
            "'a b'"
        );
    }

    #[test]
    fn fish_preserves_native_environment_arguments_when_available() {
        let Some(fish) = crate::find_program_on_path(OsStr::new("fish"), std::path::Path::new("."))
        else {
            return;
        };
        let value = OsString::from_vec(b"raw\xff'\"$(false)\n\n".to_vec());
        let token = quote_native_argument(&value, ShellDialect::Fish);
        let script = format!("printf '%s' {token}");
        let output = std::process::Command::new(fish)
            .arg("--no-config")
            .arg("-c")
            .arg(&script)
            .envs(native_argument_environment(&script))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, value.as_encoded_bytes());
    }
}
