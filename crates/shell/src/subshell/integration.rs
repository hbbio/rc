use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use portable_pty::CommandBuilder;

use crate::{ResolvedShell, ShellDialect, find_program_on_path, native_argument_environment};

const BASH_PROMPT: &str = r#"
__rc_original_prompt_commands=("${PROMPT_COMMAND[@]}")
__rc_prompt() {
    local __rc_last_status=$?
    local __rc_hook
    for __rc_hook in "${__rc_original_prompt_commands[@]}"; do
        __rc_restore_status "$__rc_last_status"
        eval "$__rc_hook"
    done
    __rc_emit_prompt "$__rc_last_status" p
    return "$__rc_last_status"
}
PROMPT_COMMAND=__rc_prompt
PS1='\[$(__rc_emit_prompt "$?" r)\]'"${PS1:-$ }"
HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace"
"#;

const POSIX_RUN: &str = r#"
__rc_begin_command
while IFS=' ' read -r __rc_reply_id __rc_control < "$RC_SUBSHELL_FILES/control"; do
    [ "$__rc_reply_id" = "$__rc_command_id" ] && break
done
if [ "$__rc_reply_id" = "$__rc_command_id" ] && [ "$__rc_control" = go ]; then
    __rc_restore_status "$__rc_saved_status"
    . "$RC_SUBSHELL_FILES/$__rc_command_file"
else
    __rc_restore_status 130
fi
"#;

const FISH_RUN: &str = r#"
__rc_begin_command
while read -l __rc_reply_id __rc_control < "$RC_SUBSHELL_FILES/control"
    if test "$__rc_reply_id" = "$__rc_command_id"
        break
    end
end
if test "$__rc_reply_id" = "$__rc_command_id"; and test "$__rc_control" = go
    __rc_restore_status $__rc_saved_status
    source "$RC_SUBSHELL_FILES/$__rc_command_file"
else
    __rc_restore_status 130
end
"#;

pub(super) fn command(
    shell: &ResolvedShell,
    directory: &Path,
    files: &Path,
    nonce: &str,
    read_startup: bool,
) -> io::Result<CommandBuilder> {
    if shell.argument_template != [OsStr::new("-c"), OsStr::new("{command}")] {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "persistent shell requires the standard -c command template",
        ));
    }
    let mut command = CommandBuilder::new(&shell.program);
    command.cwd(directory);
    command.env(
        "TERM",
        std::env::var_os("TERM").unwrap_or_else(|| "xterm-256color".into()),
    );
    command.env("RC_SUBSHELL", "1");
    let od = find_program_on_path(OsStr::new("od"), directory).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "od is needed by the shell prompt protocol",
        )
    })?;
    let tr = find_program_on_path(OsStr::new("tr"), directory).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "tr is needed by the shell prompt protocol",
        )
    })?;
    command.env("RC_SUBSHELL_OD", od);
    command.env("RC_SUBSHELL_TR", tr);
    command.env("RC_SUBSHELL_FILES", files);
    let posix = format!(
        r#"
__rc_restore_status() {{ return "$1"; }}
__rc_clear_native() {{ :; }}
__rc_begin_command() {{ printf '\033]777;rc;{nonce};s%s\007' "$__rc_command_id"; }}
__rc_command_id=0
__rc_emit_prompt() {{
    if [ "${{2-}}" != r ]; then
        __rc_next_hex=$(printf '%s' "$PWD" | "$RC_SUBSHELL_OD" -An -v -tx1 | "$RC_SUBSHELL_TR" -d ' \n')
        if [ -n "$__rc_next_hex" ]; then __rc_prompt_hex=$__rc_next_hex; fi
    fi
    printf '\033]777;rc;{nonce};%s%s;%s;%s\007' "${{2-}}" "$__rc_command_id" "$1" "$__rc_prompt_hex"
}}
HISTFILE="$RC_SUBSHELL_FILES/history"
"#
    );
    let basename = shell
        .program
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    match (shell.dialect, basename) {
        (ShellDialect::Fish, _) => {
            let script = format!(
                r#"
function __rc_restore_status
    return $argv[1]
end
function __rc_begin_command
    printf '\033]777;rc;{nonce};s%s\007' $__rc_command_id
end
set -g __rc_command_id 0
set -g __rc_native_variables
set -g __rc_prompt_hex ''
function __rc_emit_prompt --on-event fish_prompt
    set -g __rc_last_status $status
    set -l __rc_next_hex (printf '%s' "$PWD" | "$RC_SUBSHELL_OD" -An -v -tx1 | "$RC_SUBSHELL_TR" -d ' \n')
    if test -n "$__rc_next_hex"
        set -g __rc_prompt_hex "$__rc_next_hex"
    end
    printf '\033]777;rc;{nonce};p%s;%s;%s\007' $__rc_command_id $__rc_last_status "$__rc_prompt_hex" >&2
    return $__rc_last_status
end
functions --copy fish_prompt __rc_original_fish_prompt
function fish_prompt
    set -l __rc_status $status
    __rc_restore_status $__rc_status
    __rc_original_fish_prompt $argv
    if contains -- --final-rendering $argv
        return $__rc_status
    end
    printf '\033]777;rc;{nonce};r%s;%s;%s\007' $__rc_command_id $__rc_last_status "$__rc_prompt_hex"
    return $__rc_status
end
function __rc_repaint --on-event fish_cancel --on-signal INT
    commandline -f repaint 2>/dev/null
end
# An empty-line cancellation need not repaint. The owner uses this private
# binding for explicit Ctrl+C after observing a prompt. Clear synchronously
# before acknowledging; never execute the editor's input.
function __rc_cancel_input
    commandline --replace ''
    or return
    commandline -f repaint
    printf '\033]777;rc;{nonce};r%s;130;%s\007' $__rc_command_id "$__rc_prompt_hex"
end
for __rc_mode in default insert (bind --list-modes)
    # Fish ignores unknown CSI key numbers; use Ctrl-Alt-Shift-F12.
    bind --mode $__rc_mode \e\[24\;8\~ __rc_cancel_input
end
set -g fish_history ''
"#
            );
            let path = files.join("integration.fish");
            fs::write(&path, script)?;
            if !read_startup {
                command.arg("--no-config");
            }
            command.arg("-i");
            command.arg("--init-command");
            command.arg(format!(
                "source {}",
                String::from_utf8(quote(path.as_os_str(), ShellDialect::Fish))
                    .expect("ASCII fish byte quotes")
            ));
        }
        (_, "bash") => {
            let startup = if read_startup {
                "if [ -r \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\n"
            } else {
                ""
            };
            let script = format!("{startup}{posix}{BASH_PROMPT}");
            let path = files.join("bashrc");
            fs::write(&path, script)?;
            command.arg("--noprofile");
            command.arg("--rcfile");
            command.arg(path);
            command.arg("-i");
        }
        (_, "zsh") => {
            command.env(
                "RC_SUBSHELL_USER_ZDOTDIR",
                std::env::var_os("ZDOTDIR")
                    .or_else(|| std::env::var_os("HOME"))
                    .unwrap_or_default(),
            );
            command.env("ZDOTDIR", files);
            let env = if read_startup {
                r#"
ZDOTDIR=$RC_SUBSHELL_USER_ZDOTDIR
if [[ -r "$ZDOTDIR/.zshenv" ]]; then source "$ZDOTDIR/.zshenv"; fi
RC_SUBSHELL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}
ZDOTDIR=$RC_SUBSHELL_FILES
"#
            } else {
                ""
            };
            fs::write(files.join(".zshenv"), env)?;
            let startup = if read_startup {
                r#"
ZDOTDIR=$RC_SUBSHELL_USER_ZDOTDIR
if [[ -r "$ZDOTDIR/.zshrc" ]]; then source "$ZDOTDIR/.zshrc"; fi
"#
            } else {
                ""
            };
            let script = format!(
                r#"{startup}{posix}
if (( $+functions[precmd] )); then functions[__rc_original_precmd]=$functions[precmd]; fi
precmd() {{
    __rc_last_status=$?
    if (( $+functions[__rc_original_precmd] )); then __rc_restore_status "$__rc_last_status"; __rc_original_precmd; fi
}}
__rc_report_prompt() {{ __rc_emit_prompt "$__rc_last_status" p; }}
precmd_functions=(${{precmd_functions:#__rc_report_prompt}} __rc_report_prompt)
if (( $+widgets[zle-line-init] )); then zle -A zle-line-init __rc_original_line_init; fi
__rc_line_init() {{
    if (( $+widgets[__rc_original_line_init] )); then zle __rc_original_line_init; fi
    __rc_emit_prompt "$__rc_last_status" r
}}
zle -N zle-line-init __rc_line_init
setopt HIST_IGNORE_SPACE
"#
            );
            fs::write(files.join(".zshrc"), script)?;
            command.arg("-i");
        }
        (_, "sh" | "dash") => {
            if read_startup && let Some(env) = std::env::var_os("ENV") {
                command.env("RC_SUBSHELL_USER_ENV", env);
            }
            let startup = if read_startup {
                "if [ -n \"$RC_SUBSHELL_USER_ENV\" ] && [ -r \"$RC_SUBSHELL_USER_ENV\" ]; then . \"$RC_SUBSHELL_USER_ENV\"; fi\n"
            } else {
                ""
            };
            fs::write(files.join("bash-prompt"), BASH_PROMPT)?;
            let script = format!(
                "{startup}{posix}\nif [ -n \"$BASH_VERSION\" ]; then\n. \"$RC_SUBSHELL_FILES/bash-prompt\"\nelse\nPS1='$(__rc_emit_prompt \"$?\")'\"${{PS1:-$ }}\"\nfi\n"
            );
            let path = files.join("env");
            fs::write(&path, script)?;
            command.env("ENV", path);
            command.arg("-i");
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "persistent shell supports bash, zsh, fish, sh and dash",
            ));
        }
    }
    fs::write(
        files.join("run"),
        if shell.dialect == ShellDialect::Fish {
            FISH_RUN
        } else {
            POSIX_RUN
        },
    )?;
    Ok(command)
}

pub(super) fn command_line(id: u64, filename: &OsStr, dialect: ShellDialect) -> Vec<u8> {
    let mut line = if dialect == ShellDialect::Fish {
        format!(" set -g __rc_saved_status $status; set -g __rc_command_id {id}; set -g __rc_command_file ").into_bytes()
    } else {
        format!(" __rc_saved_status=$?; __rc_command_id={id}; __rc_command_file=").into_bytes()
    };
    line.extend(quote(filename, dialect));
    line.extend_from_slice(if dialect == ShellDialect::Fish {
        b"; source \"$RC_SUBSHELL_FILES/run\"\r"
    } else {
        b"; . \"$RC_SUBSHELL_FILES/run\"\r"
    });
    line
}

pub(super) fn quote(value: &OsStr, dialect: ShellDialect) -> Vec<u8> {
    if dialect == ShellDialect::Fish {
        if value.is_empty() {
            return b"''".to_vec();
        }
        value
            .as_bytes()
            .iter()
            .flat_map(|byte| format!("\\X{byte:02x}").into_bytes())
            .collect()
    } else {
        let mut result = vec![b'\''];
        for byte in value.as_bytes() {
            if *byte == b'\'' {
                result.extend_from_slice(b"'\\''");
            } else {
                result.push(*byte);
            }
        }
        result.push(b'\'');
        result
    }
}

pub(super) fn command_script(command: &str, dialect: ShellDialect) -> Vec<u8> {
    let environment = native_argument_environment(command);
    let mut script = match dialect {
        ShellDialect::Posix => b"__rc_saved_status=$?\n".to_vec(),
        ShellDialect::Fish => b"set -l __rc_saved_status $status\n".to_vec(),
    };
    // The shell survives each invocation, so retire the previous command's
    // synthetic environment before installing this command's native arguments.
    match dialect {
        ShellDialect::Posix => {
            script.extend_from_slice(b"__rc_clear_native\n__rc_clear_native() { :;");
            for (name, _) in &environment {
                script.extend_from_slice(b" unset ");
                script.extend_from_slice(name.as_bytes());
                script.push(b';');
            }
            script.extend_from_slice(b" }\n");
        }
        ShellDialect::Fish => {
            script.extend_from_slice(b"for __rc_var in $__rc_native_variables\nset -e $__rc_var\nend\nset -g __rc_native_variables");
            for (name, _) in &environment {
                script.push(b' ');
                script.extend_from_slice(name.as_bytes());
            }
            script.push(b'\n');
        }
    }
    for (name, value) in environment {
        script.extend_from_slice(match dialect {
            ShellDialect::Posix => b"export ",
            ShellDialect::Fish => b"set -gx ",
        });
        script.extend_from_slice(name.as_bytes());
        script.push(if dialect == ShellDialect::Fish {
            b' '
        } else {
            b'='
        });
        script.extend(quote(&value, dialect));
        script.push(b'\n');
    }
    script.extend_from_slice(b"__rc_restore_status \"$__rc_saved_status\"\n");
    script.extend_from_slice(command.as_bytes());
    script
}
