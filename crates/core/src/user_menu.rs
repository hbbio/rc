use std::path::Path;

use globset::GlobBuilder;
use regex::bytes::Regex;

use crate::{
    AppState, ConfigurationFile, DialogResult, DialogState, FileEntry, PanelState,
    PendingDialogAction, Route,
};

#[derive(Clone, Debug)]
pub(crate) struct UserMenuEntry {
    pub hotkey: char,
    pub label: String,
    pub command: String,
}

#[derive(Clone, Debug)]
pub(crate) struct UserMenu {
    pub entries: Vec<UserMenuEntry>,
    selected: usize,
    debug: Vec<String>,
}

impl UserMenu {
    fn parse(source: &str, panels: [&PanelState; 2]) -> Result<Self, String> {
        if source.len() > 1024 * 1024 || source.contains('\0') {
            return Err("menu exceeds 1 MiB or contains NUL".into());
        }
        let mut menu = Self {
            entries: Vec::new(),
            selected: 0,
            debug: Vec::new(),
        };
        let mut shell_patterns = true;
        let mut saw_content = false;
        let mut addition = true;
        let mut default = false;
        let mut selected_default = false;
        let mut current: Option<usize> = None;
        let mut count = 0;
        for (line_index, line) in source.lines().enumerate() {
            let error = |message| format!("line {}: {message}", line_index + 1);
            if line.is_empty() {
                if let Some(index) = current {
                    menu.entries[index].command.push('\n');
                }
                continue;
            }
            if line.starts_with([' ', '\t']) {
                if let Some(index) = current {
                    menu.entries[index]
                        .command
                        .push_str(line.trim_start_matches([' ', '\t']));
                    menu.entries[index].command.push('\n');
                }
                continue;
            }
            current = None;
            if line.starts_with('#') {
                addition = true;
                default = false;
                continue;
            }
            if let Some(value) = line.strip_prefix("shell_patterns=") {
                if saw_content {
                    return Err(error("shell_patterns must precede entries".into()));
                }
                shell_patterns = match value.split('#').next().unwrap_or_default().trim() {
                    "0" => false,
                    "1" => true,
                    _ => return Err(error("shell_patterns must be 0 or 1".into())),
                };
                saw_content = true;
                continue;
            }
            saw_content = true;
            if line.starts_with(['+', '=']) {
                let prefix_len = line
                    .bytes()
                    .take_while(|b| matches!(b, b'+' | b'='))
                    .count();
                let prefix = &line[..prefix_len];
                if !matches!(prefix, "+" | "=" | "+=" | "=+") {
                    return Err(error("invalid condition prefix".into()));
                }
                let expression = &line[prefix_len..];
                let (debug, expression) = expression
                    .strip_prefix('?')
                    .map_or((false, expression), |e| (true, e));
                let result = evaluate(expression, shell_patterns, panels).map_err(error)?;
                if prefix.contains('+') {
                    addition = result;
                }
                if prefix.contains('=') {
                    default = result;
                }
                if debug {
                    menu.debug.push(format!(
                        "Line {}: {} → {}",
                        line_index + 1,
                        expression.trim(),
                        result
                    ));
                }
                continue;
            }
            count += 1;
            if count > 1024 {
                return Err(error("menu exceeds 1024 entries".into()));
            }
            let hotkey = line.chars().next().expect("nonempty line");
            if hotkey.is_control() {
                return Err(error("invalid menu hotkey".into()));
            }
            let label = line[hotkey.len_utf8()..].trim().to_owned();
            if label.is_empty() {
                return Err(error("missing menu title".into()));
            }
            if addition {
                let index = menu.entries.len();
                if default && !selected_default {
                    menu.selected = index;
                    selected_default = true;
                }
                menu.entries.push(UserMenuEntry {
                    hotkey,
                    label,
                    command: String::new(),
                });
                current = Some(index);
            }
            addition = true;
            default = false;
        }
        if menu.entries.is_empty() {
            return Err("no applicable entries".into());
        }
        Ok(menu)
    }
}

// MC combines boolean operators in source order, without AND precedence.
fn evaluate(mut source: &str, glob: bool, panels: [&PanelState; 2]) -> Result<bool, String> {
    let mut result = None;
    let mut operator = '&';
    loop {
        source = source.trim_start();
        let mut negate = false;
        while let Some(rest) = source.strip_prefix('!') {
            negate = !negate;
            source = rest.trim_start();
        }
        let condition = source.chars().next().ok_or("missing condition")?;
        if !matches!(
            condition,
            'f' | 'F' | 'd' | 'D' | 't' | 'T' | 'x' | 'X' | 'y' | 'Y'
        ) {
            return Err(format!("unknown condition {condition}"));
        }
        source = &source[condition.len_utf8()..];
        let (argument, rest) = argument(source)?;
        source = rest.trim_start();
        let panel = panels[usize::from(condition.is_ascii_uppercase())];
        let mut value = match condition.to_ascii_lowercase() {
            'f' => matches_pattern(&argument, selected_name(panel), glob)?,
            'd' => matches_pattern(&argument, Some(panel.cwd.as_os_str()), glob)?,
            't' => matches_type(&argument, panel)?,
            'x' => is_executable(&panels[0].cwd.join(&argument)),
            // Syntax conditions belong to the editor's menu, not the file manager.
            'y' => false,
            _ => unreachable!(),
        };
        if negate {
            value = !value;
        }
        result = Some(match result {
            None => value,
            Some(previous) => {
                if operator == '&' {
                    previous && value
                } else {
                    previous || value
                }
            }
        });
        if source.is_empty() {
            return Ok(result.unwrap());
        }
        operator = source.chars().next().unwrap();
        if !matches!(operator, '&' | '|') {
            return Err("expected & or | between conditions".into());
        }
        source = &source[1..];
        // Installed MC menus contain conditions such as `= t r &`.
        // MC keeps the last condition value for this empty trailing operand.
        if source.trim().is_empty() {
            return Ok(result.unwrap());
        }
    }
}

// Preserve regex escapes; only a backslash before whitespace quotes that whitespace.
fn argument(source: &str) -> Result<(String, &str), String> {
    let source = source.trim_start();
    let mut argument = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if ch.is_whitespace() {
            return Ok((argument, &source[offset..]));
        }
        if ch == '\\' && chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
            argument.push(chars.next().unwrap().1);
        } else {
            argument.push(ch);
        }
    }
    if argument.is_empty() {
        Err("missing condition argument".into())
    } else {
        Ok((argument, ""))
    }
}

fn matches_pattern(
    pattern: &str,
    value: Option<&std::ffi::OsStr>,
    glob: bool,
) -> Result<bool, String> {
    if glob {
        let matcher = GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
            .map_err(|e| format!("invalid glob: {e}"))?
            .compile_matcher();
        Ok(value.is_some_and(|value| matcher.is_match(Path::new(value))))
    } else {
        let matcher = Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;
        Ok(value.is_some_and(|value| matcher.is_match(value.as_encoded_bytes())))
    }
}

fn selected_name(panel: &PanelState) -> Option<&std::ffi::OsStr> {
    let entry = panel.selected_entry()?;
    if entry.is_parent() {
        return Some(std::ffi::OsStr::new(".."));
    }
    let base = match &panel.source {
        crate::PanelListingSource::Panelize { .. } => Some(&panel.cwd),
        crate::PanelListingSource::FindResults { base_dir, .. } => Some(base_dir),
        _ => None,
    };
    Some(match base {
        Some(base) => entry
            .path
            .strip_prefix(base)
            .unwrap_or(&entry.path)
            .as_os_str(),
        None => entry.path.file_name().unwrap_or(entry.path.as_os_str()),
    })
}

fn matches_type(types: &str, panel: &PanelState) -> Result<bool, String> {
    let mut result = false;
    for kind in types.chars() {
        if !"nrdlcbfsxt".contains(kind) {
            return Err(format!("unknown file type {kind}"));
        }
        if kind == 't' {
            result |= !panel.tagged.is_empty();
            continue;
        }
        let Some(entry) = panel.selected_entry() else {
            continue;
        };
        let mode = entry_mode(entry);
        result |= match kind {
            'n' => mode & 0o170000 != 0o040000,
            'r' => mode & 0o170000 == 0o100000,
            'd' => mode & 0o170000 == 0o040000,
            'l' => mode & 0o170000 == 0o120000,
            'c' => mode & 0o170000 == 0o020000,
            'b' => mode & 0o170000 == 0o060000,
            'f' => mode & 0o170000 == 0o010000,
            's' => mode & 0o170000 == 0o140000,
            'x' => mode & 0o111 != 0,
            _ => unreachable!(),
        };
    }
    Ok(result)
}

fn entry_mode(entry: &FileEntry) -> u32 {
    if entry.metadata.symlink_target.is_some() {
        return 0o120777;
    }
    entry
        .metadata
        .mode
        .unwrap_or(if entry.is_dir() { 0o040000 } else { 0o100000 })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

impl AppState {
    pub fn user_menu_is_active(&self) -> bool {
        matches!(self.routes.last(), Some(Route::Dialog(dialog))
            if matches!(dialog.action(), Some(PendingDialogAction::UserMenu(_))))
    }

    pub(crate) fn open_user_menu(&mut self) {
        if self.active_panel().is_virtual() {
            self.set_status("User menu requires a local directory");
            return;
        }
        let result = self
            .configuration_files
            .read(ConfigurationFile::Menu, Some(&self.active_panel().cwd))
            .map_err(|error| error.to_string())
            .and_then(|source| {
                UserMenu::parse(
                    &source,
                    [
                        self.active_panel(),
                        &self.panels[self.active_panel.other().index()],
                    ],
                )
            });
        match result {
            Ok(menu) if !menu.debug.is_empty() => {
                self.push_dialog(
                    DialogState::listbox_with_hint(
                        "User menu condition results",
                        menu.debug.clone(),
                        0,
                        "Enter: continue to menu  Esc: cancel",
                    ),
                    PendingDialogAction::UserMenuDebug(menu),
                );
            }
            Ok(menu) => self.show_user_menu(menu),
            Err(error) => self.set_status(format!("User menu: {error}")),
        }
    }

    pub(crate) fn show_user_menu(&mut self, menu: UserMenu) {
        let items = menu
            .entries
            .iter()
            .map(|entry| format!("{}  {}", entry.hotkey, entry.label))
            .collect();
        self.push_dialog(
            DialogState::listbox_with_hint(
                "User menu",
                items,
                menu.selected,
                "Shortcut / Enter: run  Esc: cancel",
            ),
            PendingDialogAction::UserMenu(menu),
        );
        self.set_status("Choose a user menu command");
    }

    pub(crate) fn accept_user_menu_hotkey(&mut self, ch: char) -> bool {
        let index = match self.routes.last() {
            Some(Route::Dialog(dialog)) => match dialog.action() {
                Some(PendingDialogAction::UserMenu(menu)) => {
                    menu.entries.iter().position(|entry| entry.hotkey == ch)
                }
                _ => None,
            },
            _ => None,
        };
        let Some(index) = index else {
            return false;
        };
        self.finish_dialog(DialogResult::ListboxSubmitted {
            index: Some(index),
            value: None,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppCommand, ConfigurationFiles, KeyContext, tests::app_with_loaded_panels};

    fn app(root: &Path) -> AppState {
        std::fs::write(root.join("selected file.rs"), "text").unwrap();
        let mut app = app_with_loaded_panels(root.to_owned());
        app.panels[0].cursor = app.panels[0]
            .entries
            .iter()
            .position(|e| e.name == "selected file.rs")
            .unwrap();
        app.panels[1].cursor = app.panels[0].cursor;
        app.set_configuration_files(ConfigurationFiles {
            user_directory: None,
            system_directories: vec![],
        });
        app
    }

    #[test]
    fn conditions_filter_entries_in_order_and_pick_first_visible_default() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path());
        let source = "shell_patterns=1 # glob\n+ f *.missing\nh Hidden\n echo hidden\na First\n echo first\n+= f *.rs & T r\nb Default\n echo default\n= t r\nc Later default\n echo later\n";
        let menu = UserMenu::parse(source, [&app.panels[0], &app.panels[1]]).unwrap();
        assert_eq!(
            menu.entries.iter().map(|e| e.hotkey).collect::<String>(),
            "abc"
        );
        assert_eq!(menu.selected, 1);
        assert_eq!(menu.entries[1].command, "echo default\n");
    }

    #[test]
    fn conditions_use_left_to_right_logic_native_patterns_and_tag_state() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        app.panels[1].tagged.insert(root.path().join("hidden-tag"));
        let panels = [&app.panels[0], &app.panels[1]];
        for condition in [
            r"f selected\ file.rs",
            "F *.rs & T t & ! t t",
            "!! t r",
            "t r &",
            "d *",
        ] {
            assert!(evaluate(condition, true, panels).unwrap(), "{condition}");
        }
        assert!(!evaluate("t r | t d & t t", true, panels).unwrap());
        assert!(evaluate(r"f \.rs$", false, panels).unwrap());
        assert!(!evaluate("y rust", true, panels).unwrap());
        for condition in ["", "z value", "t ?", "f [", "t", "t r & !", "t r f *.rs"] {
            assert!(evaluate(condition, true, panels).is_err(), "{condition}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let name = std::ffi::OsString::from_vec(b"raw\xff.rs".to_vec());
            assert!(matches_pattern("*.rs", Some(&name), true).unwrap());
            assert!(matches_pattern(r"(?-u:\xFF)\.rs$", Some(&name), false).unwrap());
        }
    }

    #[test]
    fn disabled_entry_conditions_do_not_hide_following_entry() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path());
        let menu = UserMenu::parse(
            "+ f *.c\n#c Compile\n make %f\nv View\n %view\n",
            [&app.panels[0], &app.panels[1]],
        )
        .unwrap();
        assert_eq!(menu.entries.len(), 1);
        assert_eq!(menu.entries[0].hotkey, 'v');
    }

    #[test]
    fn filename_conditions_match_parent_and_panelized_relative_names() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        app.panels[0].cursor = 0;
        assert!(evaluate("f ..", true, [&app.panels[0], &app.panels[1]]).unwrap());
        app.panels[0].source = crate::PanelListingSource::Panelize {
            command: "unused".into(),
        };
        app.panels[0].entries = vec![crate::FileEntry::file(
            "src/main.rs".into(),
            root.path().join("src/main.rs"),
            0,
            None,
        )];
        assert!(evaluate("f src/*.rs", true, [&app.panels[0], &app.panels[1]]).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn executable_and_symlink_conditions_use_local_metadata() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        let script = root.path().join("selected file.rs");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            evaluate(
                r"x selected\ file.rs",
                true,
                [&app.panels[0], &app.panels[1]]
            )
            .unwrap()
        );
        symlink(&script, root.path().join("link")).unwrap();
        app.panels[0].refresh().unwrap();
        app.panels[0].cursor = app.panels[0]
            .entries
            .iter()
            .position(|e| e.name == "link")
            .unwrap();
        assert!(matches_type("l", &app.panels[0]).unwrap());
        assert!(!matches_type("r", &app.panels[0]).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn menu_hotkeys_run_quoted_macros_prompts_and_view_output() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        std::fs::write(
            root.path().join(".mc.menu"),
            "h Action\n %view{ascii} printf '%%s\\n' %u %F %{Message}\n",
        )
        .unwrap();
        app.panels[0]
            .tagged
            .insert(root.path().join("selected file.rs"));
        app.apply(AppCommand::OpenUserMenu).unwrap();
        assert!(app.user_menu_is_active());
        app.apply(AppCommand::DialogInputChar('h')).unwrap();
        assert_eq!(app.key_context(), KeyContext::Input);
        app.apply(AppCommand::DialogCancel).unwrap();
        assert!(!app.panels[0].tagged.is_empty());
        assert!(app.take_pending_external_action_requests().is_empty());
        app.apply(AppCommand::OpenUserMenu).unwrap();
        app.apply(AppCommand::DialogInputChar('h')).unwrap();
        for ch in "$(touch nope)".chars() {
            app.apply(AppCommand::DialogInputChar(ch)).unwrap();
        }
        app.apply(AppCommand::DialogAccept).unwrap();
        assert!(app.panels[0].tagged.is_empty());
        crate::tests::drain_background(&mut app);
        assert_eq!(
            app.active_viewer().unwrap().content(),
            "selected file.rs\nselected file.rs\n$(touch nope)\n"
        );
        assert!(!root.path().join("nope").exists());
    }

    #[test]
    fn debug_results_precede_menu_and_reopening_reads_edits() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        let path = root.path().join(".mc.menu");
        std::fs::write(&path, "+=? t r\na Action\n true\n").unwrap();
        app.apply(AppCommand::OpenUserMenu).unwrap();
        assert!(!app.user_menu_is_active());
        app.apply(AppCommand::DialogAccept).unwrap();
        assert!(app.user_menu_is_active());
        app.apply(AppCommand::DialogCancel).unwrap();
        std::fs::write(path, "+ invalid\na Broken\n true\n").unwrap();
        app.apply(AppCommand::OpenUserMenu).unwrap();
        assert_eq!(app.key_context(), KeyContext::FileManager);
        assert!(app.status_line.contains("line 1"));
        assert!(app.take_pending_external_action_requests().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn menu_macros_include_parent_selection_on_both_panels() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        app.panels[0].cursor = 0;
        app.panels[1].cursor = 0;
        std::fs::write(
            root.path().join(".mc.menu"),
            "p Parent\n %view printf '%%s\\n' %f %F %p %P\n",
        )
        .unwrap();
        app.apply(AppCommand::OpenUserMenu).unwrap();
        app.apply(AppCommand::DialogAccept).unwrap();
        crate::tests::drain_background(&mut app);
        assert_eq!(app.active_viewer().unwrap().content(), "..\n..\n..\n..\n");
    }
}
