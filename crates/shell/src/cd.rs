use std::path::PathBuf;

use crate::ShellDialect;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiteralCd {
    Home,
    Previous,
    Path { path: PathBuf, expand_home: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiteralToken {
    value: String,
    decorated: bool,
    unquoted_tilde_start: bool,
    expand_home: bool,
}

/// Parse only the deliberately small literal `cd` subset owned by rc.
///
/// `None` always means "delegate the original command to the selected shell".
pub fn parse_literal_cd(command: &str, dialect: ShellDialect) -> Option<LiteralCd> {
    let tokens = scan_literal_tokens(command, dialect)?;
    let [first, rest @ ..] = tokens.as_slice() else {
        return None;
    };
    if first.decorated || first.value != "cd" {
        return None;
    }

    match rest {
        [] => Some(LiteralCd::Home),
        [only] if !only.decorated && only.value == "-" => Some(LiteralCd::Previous),
        [separator] if !separator.decorated && separator.value == "--" => Some(LiteralCd::Home),
        [separator, path] if !separator.decorated && separator.value == "--" => literal_path(path),
        [path] => literal_path(path),
        _ => None,
    }
}

fn literal_path(token: &LiteralToken) -> Option<LiteralCd> {
    if token.value.is_empty() {
        return None;
    }
    let expand_home = token.expand_home;
    if token.unquoted_tilde_start && token.value.starts_with('~') && !expand_home {
        // Named-user expansion is shell-owned until rc has a tested account lookup adapter.
        return None;
    }
    Some(LiteralCd::Path {
        path: PathBuf::from(&token.value),
        expand_home,
    })
}

fn scan_literal_tokens(command: &str, dialect: ShellDialect) -> Option<Vec<LiteralToken>> {
    if command.contains('\0') {
        return None;
    }
    let mut characters = command.chars().peekable();
    let mut tokens = Vec::new();

    loop {
        while characters
            .peek()
            .is_some_and(|character| matches!(character, ' ' | '\t'))
        {
            characters.next();
        }
        if characters.peek().is_none() {
            break;
        }

        let mut value = String::new();
        let mut decorated = false;
        let mut unquoted_tilde_start = false;
        let mut expand_home = false;
        let mut token_started = false;
        while let Some(character) = characters.peek().copied() {
            if matches!(character, ' ' | '\t') {
                break;
            }
            token_started = true;
            characters.next();
            match character {
                '\'' => {
                    decorated = true;
                    scan_single_quoted(&mut characters, &mut value, dialect)?;
                }
                '"' => {
                    decorated = true;
                    scan_double_quoted(&mut characters, &mut value, dialect)?;
                }
                '\\' if dialect == ShellDialect::Fish => return None,
                '\\' => {
                    decorated = true;
                    value.push(characters.next()?);
                }
                character if is_ambiguous_unquoted(character, dialect) => return None,
                character if character.is_control() => return None,
                character => {
                    if value.is_empty() && character == '~' {
                        unquoted_tilde_start = true;
                    } else if unquoted_tilde_start && value == "~" && character == '/' {
                        expand_home = true;
                    }
                    value.push(character);
                }
            }
        }
        if !token_started {
            return None;
        }
        if unquoted_tilde_start && value == "~" {
            expand_home = true;
        }
        tokens.push(LiteralToken {
            value,
            decorated,
            unquoted_tilde_start,
            expand_home,
        });
    }

    Some(tokens)
}

fn scan_single_quoted(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    value: &mut String,
    dialect: ShellDialect,
) -> Option<()> {
    while let Some(character) = characters.next() {
        match character {
            '\'' => return Some(()),
            '\\' if dialect == ShellDialect::Fish => {
                let escaped = characters.next()?;
                if matches!(escaped, '\\' | '\'') {
                    value.push(escaped);
                } else {
                    return None;
                }
            }
            character if character.is_control() => return None,
            character => value.push(character),
        }
    }
    None
}

fn scan_double_quoted(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    value: &mut String,
    dialect: ShellDialect,
) -> Option<()> {
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(()),
            '$' | '`' => return None,
            '(' if dialect == ShellDialect::Fish => return None,
            '\\' => {
                let escaped = characters.next()?;
                match dialect {
                    ShellDialect::Fish if matches!(escaped, '\\' | '"' | '$') => {
                        value.push(escaped);
                    }
                    ShellDialect::Fish => return None,
                    ShellDialect::Posix if matches!(escaped, '\\' | '"' | '$' | '`') => {
                        value.push(escaped);
                    }
                    ShellDialect::Posix => {
                        value.push('\\');
                        value.push(escaped);
                    }
                }
            }
            character if character.is_control() => return None,
            character => value.push(character),
        }
    }
    None
}

fn is_ambiguous_unquoted(character: char, dialect: ShellDialect) -> bool {
    match character {
        ';' | '|' | '&' | '<' | '>' | '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '#'
        | '\n' | '\r' => true,
        '(' | ')' => true,
        '%' if dialect == ShellDialect::Fish => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_posix_cd_forms() {
        assert_eq!(
            parse_literal_cd("cd", ShellDialect::Posix),
            Some(LiteralCd::Home)
        );
        assert_eq!(
            parse_literal_cd(" cd - ", ShellDialect::Posix),
            Some(LiteralCd::Previous)
        );
        assert_eq!(
            parse_literal_cd("cd -- 'a b'", ShellDialect::Posix),
            Some(LiteralCd::Path {
                path: PathBuf::from("a b"),
                expand_home: false,
            })
        );
        assert_eq!(
            parse_literal_cd("cd one\\ two", ShellDialect::Posix),
            Some(LiteralCd::Path {
                path: PathBuf::from("one two"),
                expand_home: false,
            })
        );
        assert_eq!(
            parse_literal_cd("cd ~/work", ShellDialect::Posix),
            Some(LiteralCd::Path {
                path: PathBuf::from("~/work"),
                expand_home: true,
            })
        );
        assert_eq!(
            parse_literal_cd("cd ~/'work tree'", ShellDialect::Posix),
            Some(LiteralCd::Path {
                path: PathBuf::from("~/work tree"),
                expand_home: true,
            })
        );
    }

    #[test]
    fn ambiguous_cd_is_delegated_unchanged() {
        for command in [
            "cd $HOME",
            "cd *.rs",
            "cd one; pwd",
            "cd one two",
            "c'd' /tmp",
            "cd \"$(pwd)\"",
            "cd ''",
            "cd ~someone",
        ] {
            assert_eq!(
                parse_literal_cd(command, ShellDialect::Posix),
                None,
                "{command}"
            );
        }
    }

    #[test]
    fn fish_unquoted_backslash_escapes_are_delegated() {
        for command in [
            r"cd \x2ftmp",
            r"cd \141",
            r"cd \u0061",
            r"cd \n",
            r"cd one\ two",
        ] {
            assert_eq!(
                parse_literal_cd(command, ShellDialect::Fish),
                None,
                "{command}"
            );
        }
    }

    #[test]
    fn quoted_dash_is_a_path_not_previous_directory() {
        assert_eq!(
            parse_literal_cd("cd '-'", ShellDialect::Fish),
            Some(LiteralCd::Path {
                path: PathBuf::from("-"),
                expand_home: false,
            })
        );
    }

    #[test]
    fn quoted_or_escaped_tilde_remains_a_literal_path() {
        for command in ["cd '~'", "cd \\~", "cd \"~/work\""] {
            let Some(LiteralCd::Path { path, expand_home }) =
                parse_literal_cd(command, ShellDialect::Posix)
            else {
                panic!("expected a literal path for {command}");
            };
            assert!(!expand_home, "{command}");
            assert!(path.to_string_lossy().starts_with('~'), "{command}");
        }
    }
}
