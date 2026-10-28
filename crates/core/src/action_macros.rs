//! MC action macros. Quoted values travel through environment variables, never
//! through shell source, including values containing native non-UTF-8 bytes.
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

const ACTION_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MacroPanel {
    pub directory: PathBuf,
    pub selected: Option<PathBuf>,
    pub tagged: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MacroContext {
    /// Active panel first, passive panel second.
    pub panels: [MacroPanel; 2],
    /// A local copy for an extension action on a virtual file, used by %f.
    pub local_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewOptions {
    pub hex: bool,
    pub nroff: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionOutput {
    #[default]
    Terminal,
    Viewer(ViewOptions),
    ChangeDirectory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedAction {
    /// Shell source, or a literal native path for ChangeDirectory.
    pub script: OsString,
    pub environment: Vec<(String, OsString)>,
    pub output: ActionOutput,
    /// Apply only after execution has been successfully scheduled. Prompt
    /// cancellation and expansion errors must leave the real panel tags intact.
    pub untag: [Vec<PathBuf>; 2],
}

#[derive(Clone, Debug)]
enum Token {
    Text(String),
    Panel {
        letter: char,
        quoted: bool,
    },
    Prompt {
        label: String,
        quoted: bool,
    },
    Variable {
        name: String,
        default: String,
        quoted: bool,
    },
}

#[derive(Clone, Debug)]
pub struct ActionTemplate {
    tokens: Vec<Token>,
    output: ActionOutput,
}

impl ActionTemplate {
    /// Quoting is enabled by default; %0 explicitly inserts raw shell source.
    /// Here-document templates are rejected because their interpolation rules
    /// differ from shell arguments. Pass macros as arguments to a separate script.
    pub fn parse(source: &str) -> Result<Self, String> {
        if source.len() > ACTION_LIMIT || source.contains('\0') {
            return Err("action exceeds 64 KiB or contains NUL".into());
        }
        let mut tokens = Vec::new();
        let (source, mut output) = match source.trim_start().strip_prefix("%cd") {
            Some(path) if path.chars().next().is_none_or(char::is_whitespace) => {
                (path.trim_start(), ActionOutput::ChangeDirectory)
            }
            _ => (source, ActionOutput::Terminal),
        };
        let mut offset = 0;
        let mut syntax = ShellQuotes::default();
        while let Some(relative) = source[offset..].find('%') {
            let start = offset + relative;
            if start > offset {
                syntax.scan(&source.as_bytes()[offset..start]);
                tokens.push(Token::Text(source[offset..start].into()));
            }
            offset = start + 1;
            if output != ActionOutput::ChangeDirectory && syntax.in_comment() {
                tokens.push(Token::Text("%".into()));
                continue;
            }
            let mut quoted = true;
            if let Some(flag @ ('0' | '1')) = source[offset..].chars().next() {
                quoted = flag == '1';
                offset += 1;
            }
            let remainder = &source[offset..];
            if remainder.starts_with("view")
                && remainder[4..]
                    .chars()
                    .next()
                    .is_none_or(|ch| ch == '{' || ch.is_whitespace())
            {
                if output != ActionOutput::Terminal {
                    return Err("only one %view directive is allowed".into());
                }
                offset += 4;
                let mut options = ViewOptions::default();
                if source[offset..].starts_with('{') {
                    let (value, next) = braced(source, offset)?;
                    for option in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        match option {
                            "ascii" => options.hex = false,
                            "hex" => options.hex = true,
                            "nroff" => options.nroff = true,
                            "unform" => options.nroff = false,
                            _ => return Err(format!("unknown %view option {option}")),
                        }
                    }
                    offset = next;
                }
                output = ActionOutput::Viewer(options);
                continue;
            }
            if remainder.starts_with("var{") {
                let (value, next) = braced(source, offset + 3)?;
                let (name, default) = value.split_once(':').unwrap_or((value, ""));
                if !valid_variable_name(name) {
                    return Err("invalid environment variable in %var".into());
                }
                tokens.push(Token::Variable {
                    name: name.into(),
                    default: default.into(),
                    quoted,
                });
                syntax.value();
                offset = next;
                continue;
            }
            if remainder.starts_with('{') {
                let (label, next) = braced(source, offset)?;
                tokens.push(Token::Prompt {
                    label: label.into(),
                    quoted,
                });
                syntax.value();
                offset = next;
                continue;
            }
            let Some(letter) = remainder.chars().next() else {
                return Err("incomplete action macro".into());
            };
            offset += letter.len_utf8();
            match letter {
                '%' => tokens.push(Token::Text("%".into())),
                'f' | 'p' | 'n' | 'x' | 'd' | 't' | 's' | 'u' | 'F' | 'P' | 'N' | 'X' | 'D'
                | 'T' | 'S' | 'U' => tokens.push(Token::Panel { letter, quoted }),
                _ => return Err(format!("unknown action macro %{letter}")),
            }
            syntax.value();
        }
        if offset < source.len() {
            syntax.scan(&source.as_bytes()[offset..]);
            tokens.push(Token::Text(source[offset..].into()));
        }
        if output != ActionOutput::ChangeDirectory && syntax.has_heredoc {
            return Err("here-document action templates are not supported; pass arguments to a separate script instead".into());
        }
        Ok(Self { tokens, output })
    }

    pub fn prompts(&self) -> Vec<&str> {
        self.tokens
            .iter()
            .filter_map(|token| match token {
                Token::Prompt { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn expand(
        &self,
        context: &MacroContext,
        answers: &[String],
        mut environment: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<ExpandedAction, String> {
        if answers.len() != self.prompts().len() {
            return Err("all action prompts must be answered before execution".into());
        }
        let mut expanded = ExpandedAction {
            script: OsString::new(),
            environment: Vec::new(),
            output: self.output,
            untag: [vec![], vec![]],
        };
        let mut panels = context.panels.clone();
        let mut answer_index = 0;
        let mut quotes = ShellQuotes::default();
        let mut directory_quotes = PathQuotes::default();
        let mut environment_bytes = 0;
        for token in &self.tokens {
            let (values, quoted) = match token {
                Token::Text(text) => {
                    if self.output == ActionOutput::ChangeDirectory {
                        expanded.script.push(directory_quotes.literal(text)?);
                    } else {
                        expanded.script.push(text);
                        quotes.scan(text.as_bytes());
                    }
                    continue;
                }
                Token::Panel { letter, quoted } => {
                    let index = usize::from(letter.is_ascii_uppercase());
                    let panel = &mut panels[index];
                    let values = panel_values(
                        panel,
                        letter.to_ascii_lowercase(),
                        (index == 0)
                            .then_some(context.local_file.as_deref())
                            .flatten(),
                    )?;
                    if letter.eq_ignore_ascii_case(&'u') {
                        expanded.untag[index].append(&mut panel.tagged);
                    }
                    (values, *quoted)
                }
                Token::Prompt { quoted, .. } => {
                    let value = OsString::from(&answers[answer_index]);
                    answer_index += 1;
                    (vec![value], *quoted)
                }
                Token::Variable {
                    name,
                    default,
                    quoted,
                } => (
                    vec![environment(name).unwrap_or_else(|| OsString::from(default))],
                    *quoted,
                ),
            };
            if self.output == ActionOutput::ChangeDirectory {
                if values.len() != 1 || directory_quotes.escaped {
                    return Err("%cd requires one path; do not escape a path macro".into());
                }
                check_value(&values[0])?;
                expanded.script.push(&values[0]);
            } else if quotes.escaped() {
                return Err("remove the shell backslash before an action macro; use %% for a literal percent".into());
            } else if quoted {
                // Close any template quote, emit separately quoted parameters,
                // then reopen it. Prefixes/suffixes still concatenate normally.
                let quote = quotes.quote();
                if let Some(ch) = quote {
                    expanded.script.push(ch.to_string());
                }
                for (index, value) in values.iter().enumerate() {
                    check_value(value)?;
                    if index > 0 {
                        expanded.script.push(" ");
                    }
                    let name = format!("RC_MC_VALUE_{}", expanded.environment.len());
                    expanded.script.push(format!("\"${{{name}}}\""));
                    environment_bytes += name.len() + value.as_encoded_bytes().len() + 2;
                    expanded.environment.push((name, value.clone()));
                    if expanded.script.len().saturating_add(environment_bytes) > ACTION_LIMIT {
                        return Err("expanded action and arguments exceed 64 KiB".into());
                    }
                }
                if let Some(ch) = quote {
                    expanded.script.push(ch.to_string());
                }
                if !values.is_empty() || quote.is_some() {
                    quotes.value();
                }
            } else {
                for (index, value) in values.iter().enumerate() {
                    check_value(value)?;
                    if index > 0 {
                        expanded.script.push(" ");
                        quotes.scan(b" ");
                    }
                    expanded.script.push(value);
                    quotes.scan(value.as_encoded_bytes());
                    if expanded.script.len().saturating_add(environment_bytes) > ACTION_LIMIT {
                        return Err("expanded action and arguments exceed 64 KiB".into());
                    }
                }
            }
            if expanded.script.len().saturating_add(environment_bytes) > ACTION_LIMIT {
                return Err("expanded action and arguments exceed 64 KiB".into());
            }
        }
        if expanded.script.len().saturating_add(environment_bytes) > ACTION_LIMIT {
            return Err("expanded action and arguments exceed 64 KiB".into());
        }
        if self.output == ActionOutput::ChangeDirectory
            && (directory_quotes.quote.is_some()
                || directory_quotes.escaped
                || expanded.script.is_empty())
        {
            return Err("%cd requires a nonempty path with balanced quotes".into());
        }
        Ok(expanded)
    }
}

fn braced(source: &str, start: usize) -> Result<(&str, usize), String> {
    let end = source[start + 1..]
        .find('}')
        .map(|index| start + 1 + index)
        .ok_or("unclosed action macro braces")?;
    Ok((&source[start + 1..end], end + 1))
}

fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn check_value(value: &OsStr) -> Result<(), String> {
    if value.as_encoded_bytes().contains(&0) {
        Err("action argument contains NUL".into())
    } else {
        Ok(())
    }
}

fn panel_values(
    panel: &MacroPanel,
    letter: char,
    local_file: Option<&std::path::Path>,
) -> Result<Vec<OsString>, String> {
    let relative = |path: &std::path::Path| {
        path.strip_prefix(&panel.directory)
            .unwrap_or(path)
            .as_os_str()
            .to_owned()
    };
    match letter {
        'd' => return Ok(vec![panel.directory.as_os_str().to_owned()]),
        't' | 'u' => return Ok(panel.tagged.iter().map(|path| relative(path)).collect()),
        's' if !panel.tagged.is_empty() => {
            return Ok(panel.tagged.iter().map(|path| relative(path)).collect());
        }
        _ => {}
    }
    let path = panel
        .selected
        .as_deref()
        .ok_or("action macro requires a selected file")?;
    let value = match letter {
        'f' => local_file
            .map(|path| path.as_os_str().to_owned())
            .unwrap_or_else(|| relative(path)),
        'p' => path.file_name().unwrap_or(path.as_os_str()).to_owned(),
        's' => relative(path),
        'n' | 'x' => {
            let name = relative(path);
            let bytes = name.as_encoded_bytes();
            let basename = bytes
                .iter()
                .rposition(|byte| *byte == b'/')
                .map_or(0, |index| index + 1);
            let dot = bytes[basename..]
                .iter()
                .rposition(|byte| *byte == b'.')
                .map(|index| basename + index);
            let bytes = if letter == 'n' {
                &bytes[..dot.unwrap_or(bytes.len())]
            } else {
                dot.map_or(&b""[..], |index| &bytes[index + 1..])
            };
            os_from_bytes(bytes)?
        }
        _ => return Err("invalid panel macro".into()),
    };
    Ok(vec![value])
}

fn os_from_bytes(bytes: &[u8]) -> Result<OsString, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(OsStr::from_bytes(bytes).to_owned())
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes.to_vec())
            .map(OsString::from)
            .map_err(|_| "filename is not representable on this platform".into())
    }
}

#[derive(Default)]
struct PathQuotes {
    quote: Option<u8>,
    escaped: bool,
}

impl PathQuotes {
    fn literal(&mut self, text: &str) -> Result<OsString, String> {
        let mut bytes = Vec::with_capacity(text.len());
        for byte in text.bytes() {
            if self.escaped {
                self.escaped = false;
                if self.quote == Some(b'"') && !b"$`\"\\\n".contains(&byte) {
                    bytes.push(b'\\');
                }
                if byte != b'\n' {
                    bytes.push(byte);
                }
                continue;
            }
            if byte == b'\\' && self.quote != Some(b'\'') {
                self.escaped = true;
                continue;
            }
            if (byte == b'\'' || byte == b'"') && (self.quote.is_none() || self.quote == Some(byte))
            {
                self.quote = if self.quote.is_some() {
                    None
                } else {
                    Some(byte)
                };
            } else {
                bytes.push(byte);
            }
        }
        os_from_bytes(&bytes)
    }
}

#[derive(Clone, Copy)]
struct QuoteFrame {
    quote: Option<char>,
    escaped: bool,
    parens: usize,
    backtick: bool,
    arithmetic: bool,
    comment: bool,
    word_start: bool,
}

impl Default for QuoteFrame {
    fn default() -> Self {
        Self {
            quote: None,
            escaped: false,
            parens: 0,
            backtick: false,
            arithmetic: false,
            comment: false,
            word_start: true,
        }
    }
}

#[derive(Default)]
struct ShellQuotes {
    frames: Vec<QuoteFrame>,
    has_heredoc: bool,
}

impl ShellQuotes {
    fn value(&mut self) {
        if self.frames.is_empty() {
            self.frames.push(QuoteFrame::default());
        }
        self.frames.last_mut().expect("base quote frame").word_start = false;
    }
    fn in_comment(&self) -> bool {
        self.frames.last().is_some_and(|frame| frame.comment)
    }
    fn quote(&self) -> Option<char> {
        self.frames.last().and_then(|frame| frame.quote)
    }
    fn escaped(&self) -> bool {
        self.frames.last().is_some_and(|frame| frame.escaped)
    }

    fn scan(&mut self, bytes: &[u8]) {
        if self.frames.is_empty() {
            self.frames.push(QuoteFrame::default());
        }
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            let frame = self.frames.last_mut().expect("base quote frame");
            if frame.comment {
                if byte == b'\n' {
                    frame.comment = false;
                    frame.word_start = true;
                }
                index += 1;
                continue;
            }
            if frame.escaped {
                frame.escaped = false;
                index += 1;
                continue;
            }
            if byte == b'#' && frame.quote.is_none() && frame.word_start {
                frame.comment = true;
                index += 1;
                continue;
            }
            frame.word_start =
                frame.quote.is_none() && (byte.is_ascii_whitespace() || b";&|<>()".contains(&byte));
            if byte == b'<' && bytes.get(index + 1) == Some(&b'<') && frame.quote.is_none() {
                self.has_heredoc |= !frame.arithmetic;
            }
            match byte {
                b'\\' if frame.quote != Some('\'') => frame.escaped = true,
                b'\'' if frame.quote != Some('"') => {
                    frame.quote = (frame.quote != Some('\'')).then_some('\'')
                }
                b'"' if frame.quote != Some('\'') => {
                    frame.quote = (frame.quote != Some('"')).then_some('"')
                }
                b'$' if frame.quote != Some('\'') && bytes.get(index + 1) == Some(&b'(') => {
                    self.frames.push(QuoteFrame {
                        parens: 1,
                        arithmetic: bytes.get(index + 2) == Some(&b'('),
                        ..Default::default()
                    });
                    index += 1;
                }
                b'`' if frame.quote != Some('\'') => {
                    if frame.backtick {
                        self.frames.pop();
                    } else {
                        self.frames.push(QuoteFrame {
                            backtick: true,
                            ..Default::default()
                        });
                    }
                }
                b'(' if frame.quote.is_none() && frame.parens > 0 => frame.parens += 1,
                b')' if frame.quote.is_none() && frame.parens > 0 => {
                    frame.parens -= 1;
                    if frame.parens == 0 {
                        self.frames.pop();
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> MacroContext {
        MacroContext {
            panels: [
                MacroPanel {
                    directory: "/active".into(),
                    selected: Some("/active/file name.tar.gz".into()),
                    tagged: vec!["/active/one file".into(), "/active/two'file".into()],
                },
                MacroPanel {
                    directory: "/other dir".into(),
                    selected: Some("/other dir/other.txt".into()),
                    tagged: vec!["/other dir/tagged".into()],
                },
            ],
            local_file: None,
        }
    }

    #[cfg(unix)]
    fn run(template: &str, context: &MacroContext, answers: &[String]) -> Vec<u8> {
        let expanded = ActionTemplate::parse(template)
            .unwrap()
            .expand(context, answers, |_| None)
            .unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&expanded.script)
            .envs(
                expanded
                    .environment
                    .iter()
                    .map(|(name, value)| (name, value)),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[cfg(unix)]
    #[test]
    fn expands_native_current_passive_tagged_and_filename_macros() {
        let output = run(
            "printf '%%s\\0' %f %p %n %x %d %F %D %t %T %s",
            &context(),
            &[],
        );
        assert_eq!(output, b"file name.tar.gz\0file name.tar.gz\0file name.tar\0gz\0/active\0other.txt\0/other dir\0one file\0two'file\0tagged\0one file\0two'file\0");
    }

    #[cfg(unix)]
    #[test]
    fn quoted_macros_preserve_arguments_inside_template_quotes_and_substitutions() {
        let ctx = context();
        for template in [
            "printf '%%s\\0' %f",
            "printf '%%s\\0' \"%f\"",
            "printf '%%s\\0' '%f'",
            "printf '%%s\\0' \"$(printf '%%s' %f)\"",
            "printf '%%s\\0' \"$(printf '%%s' \"%f\")\"",
            "printf '%%s\\0' \"`printf '%%s' %f`\"",
        ] {
            assert_eq!(
                run(template, &ctx, &[]),
                b"file name.tar.gz\0",
                "{template}"
            );
        }
        assert_eq!(
            run("printf '%%s\\0' 'pre%f/post'", &ctx, &[]),
            b"prefile name.tar.gz/post\0"
        );
        assert_eq!(
            run("printf '%%s\\0' \"%t\"", &ctx, &[]),
            b"one file\0two'file\0"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filenames_and_prompt_answers_are_data_not_shell_source() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("executed");
        let malicious = format!(
            "$(touch {})`touch {}`'\"; * ? $HOME\n",
            marker.display(),
            marker.display()
        );
        let mut ctx = context();
        ctx.panels[0].directory = root.path().into();
        ctx.panels[0].selected = Some(root.path().join(&malicious));
        let answers = [malicious.clone()];
        for template in [
            "printf '%%s\\0' %f",
            "printf '%%s\\0' \"%f\"",
            "printf '%%s\\0' '%f'",
            "printf '%%s\\0' %{Input}",
        ] {
            let supplied = if template.contains("%{") {
                &answers[..]
            } else {
                &[]
            };
            let mut expected = malicious.as_bytes().to_vec();
            expected.push(0);
            assert_eq!(run(template, &ctx, supplied), expected, "{template}");
            assert!(!marker.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn byte_names_and_newlines_survive_without_command_substitution_trimming() {
        use std::os::unix::ffi::OsStrExt;
        let mut ctx = context();
        ctx.panels[0].selected = Some(
            ctx.panels[0]
                .directory
                .join(OsStr::from_bytes(b"raw\xff name\n\n")),
        );
        assert_eq!(run("printf '%%s\\0' %f", &ctx, &[]), b"raw\xff name\n\n\0");
    }

    #[cfg(unix)]
    #[test]
    fn comments_do_not_change_quote_context_or_request_prompts() {
        let source =
            "# unmatched ' quote %unknown %{not a prompt}\nprintf '%%s\\0' %f\n# \" %view{hex}\n";
        let template = ActionTemplate::parse(source).unwrap();
        assert!(template.prompts().is_empty());
        assert_eq!(run(source, &context(), &[]), b"file name.tar.gz\0");
        assert_eq!(
            run("printf '%%s\\0' %f#%p", &context(), &[]),
            b"file name.tar.gz#file name.tar.gz\0"
        );
    }

    #[test]
    fn view_prompts_environment_and_untagging_are_explicit_results() {
        let template = ActionTemplate::parse(
            "%view{hex,nroff,unform} echo %u %t %s %U %var{HOME:fallback} %{Label} %%",
        )
        .unwrap();
        assert_eq!(template.prompts(), vec!["Label"]);
        assert!(template.expand(&context(), &[], |_| None).is_err());
        let expanded = template
            .expand(&context(), &["answer".into()], |_| {
                Some("home value".into())
            })
            .unwrap();
        assert_eq!(
            expanded.output,
            ActionOutput::Viewer(ViewOptions {
                hex: true,
                nroff: false
            })
        );
        assert_eq!(expanded.untag[0], context().panels[0].tagged);
        assert_eq!(expanded.untag[1], context().panels[1].tagged);
        let values: Vec<_> = expanded
            .environment
            .iter()
            .map(|(_, value)| value.clone())
            .collect();
        assert_eq!(
            values,
            [
                "one file",
                "two'file",
                "file name.tar.gz",
                "tagged",
                "home value",
                "answer"
            ]
            .map(OsString::from)
        );
        let fallback = ActionTemplate::parse("%var{UNSET:a:b}")
            .unwrap()
            .expand(&context(), &[], |_| None)
            .unwrap();
        assert_eq!(fallback.environment[0].1, "a:b");
    }

    #[test]
    fn raw_mode_is_explicit_and_local_file_only_replaces_current_f() {
        let mut ctx = context();
        ctx.local_file = Some("/temporary/native file".into());
        let expanded = ActionTemplate::parse("%0{Command} %1f %p %F")
            .unwrap()
            .expand(&ctx, &["cat --".into()], |_| None)
            .unwrap();
        assert!(expanded.script.to_str().unwrap().starts_with("cat -- "));
        assert_eq!(
            expanded
                .environment
                .iter()
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>(),
            ["/temporary/native file", "file name.tar.gz", "other.txt"].map(OsString::from)
        );
    }

    #[test]
    fn cd_expands_one_literal_path_without_shell_evaluation() {
        for (source, expected) in [
            ("%cd %d/sub dir", "/active/sub dir"),
            ("%cd \"%f\"", "file name.tar.gz"),
            ("%cd '#folder'/%f", "#folder/file name.tar.gz"),
            ("%cd #folder/%f", "#folder/file name.tar.gz"),
            (
                "%cd \"$(touch never)\"/%p",
                "$(touch never)/file name.tar.gz",
            ),
            ("%cd sub\\ dir/%p", "sub dir/file name.tar.gz"),
            ("%cd \"\\q\"", "\\q"),
            ("%cd \"\\$HOME\"", "$HOME"),
            ("%cd \"a\\\\b\"", "a\\b"),
        ] {
            let expanded = ActionTemplate::parse(source)
                .unwrap()
                .expand(&context(), &[], |_| None)
                .unwrap();
            assert_eq!(expanded.output, ActionOutput::ChangeDirectory);
            assert_eq!(expanded.script, expected);
            assert!(expanded.environment.is_empty());
        }
        for source in ["%cd", "%cd %t", "%cd \"unclosed", "%cd trailing\\"] {
            assert!(
                ActionTemplate::parse(source)
                    .unwrap()
                    .expand(&context(), &[], |_| None)
                    .is_err()
            );
        }
    }

    #[test]
    fn rejects_incomplete_unknown_nul_and_oversized_expansions() {
        for source in [
            "%",
            "%z",
            "%2f",
            "%{prompt",
            "%var{bad-name:no}",
            "%view{bad}",
            "%view %view",
            "x\0y",
        ] {
            assert!(ActionTemplate::parse(source).is_err(), "{source:?}");
        }
        for answer in ["x\0y".into(), "x".repeat(ACTION_LIMIT)] {
            assert!(
                ActionTemplate::parse("%{x}")
                    .unwrap()
                    .expand(&context(), &[answer], |_| None)
                    .is_err()
            );
        }
        assert!(
            ActionTemplate::parse("\\%f")
                .unwrap()
                .expand(&context(), &[], |_| None)
                .is_err()
        );
    }

    #[test]
    fn heredoc_macros_report_an_error_instead_of_emitting_variable_names_or_quotes() {
        for source in [
            "cat <<'EOF'\n%f\nEOF\n",
            "cat <<EOF\n%f\nEOF\n",
            "echo %f; cat <<EOF\ntext\nEOF\n",
            "cat <<EOF\n\"literal quote\nEOF\nprintf '%%s\\0' %f",
            "cat <<EOF\n# %f\nEOF\n",
        ] {
            assert!(
                ActionTemplate::parse(source)
                    .unwrap_err()
                    .contains("here-document")
            );
        }
        assert!(ActionTemplate::parse("%view cat <<'EOF'\nplain text\nEOF\n").is_err());
        assert!(ActionTemplate::parse("printf '<<EOF' %f").is_ok());
        assert!(ActionTemplate::parse("echo $((1 << 2)) %f").is_ok());
    }
}
