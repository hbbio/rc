use std::ffi::OsStr;
use std::process::Command;

use crate::*;

pub(crate) fn comparison_process(
    left: &Path,
    right: &Path,
    directories: bool,
    cwd: &Path,
) -> io::Result<Command> {
    comparison_process_with_lookup(
        left,
        right,
        directories,
        cwd,
        rc_shell::find_program_on_path,
    )
}

fn comparison_process_with_lookup(
    left: &Path,
    right: &Path,
    directories: bool,
    cwd: &Path,
    lookup: impl FnMut(&OsStr, &Path) -> Option<PathBuf>,
) -> io::Result<Command> {
    let left = cwd.join(left);
    let right = cwd.join(right);
    for path in [&left, &right] {
        let metadata = fs::metadata(path)?;
        if if directories {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        } {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                if directories {
                    "comparison requires two directories"
                } else {
                    "comparison requires two regular files"
                },
            ));
        }
    }
    build_comparison_process(&left, &right, directories, cwd, lookup)
}

fn build_comparison_process(
    left: &Path,
    right: &Path,
    directories: bool,
    cwd: &Path,
    mut lookup: impl FnMut(&OsStr, &Path) -> Option<PathBuf>,
) -> io::Result<Command> {
    let difft = (!directories)
        .then(|| lookup(OsStr::new("difft"), cwd))
        .flatten();
    let mut command = if let Some(difft) = difft {
        let mut command = Command::new(difft);
        command.args(["--color=never", "--display=inline", "--exit-code"]);
        command
    } else {
        let diff = lookup(OsStr::new("diff"), cwd).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Install difftastic or diff to compare files",
            )
        })?;
        let mut command = Command::new(diff);
        if directories {
            command.args(["-r", "-q"]);
        } else {
            command.arg("-u");
        }
        command
    };
    command.arg("--").arg(left).arg(right);
    Ok(command)
}

impl AppState {
    fn comparison_panels_are_local(&mut self) -> bool {
        if self.panels.iter().any(PanelState::is_virtual) {
            self.set_status("Copy archive entries to local directories before comparing");
            return false;
        }
        if self.panel_views != [PanelViewMode::Listing; 2] {
            self.set_status("Comparison requires two file listing panels");
            return false;
        }
        true
    }

    pub(crate) fn compare_files(&mut self) {
        if !self.comparison_panels_are_local() {
            return;
        }
        let paths: Vec<PathBuf> = [self.active_panel(), self.passive_panel()]
            .into_iter()
            .filter_map(|panel| {
                panel
                    .selected_entry()
                    .filter(|entry| !entry.is_dir())
                    .map(|entry| entry.path.clone())
            })
            .collect();
        let [left, right] = paths.as_slice() else {
            self.set_status("Select a file in each panel to compare");
            return;
        };
        self.start_command_capture(CommandCaptureSpec {
            cwd: self.active_panel().cwd.clone(),
            command: CapturedCommand::Compare {
                left: left.clone(),
                right: right.clone(),
                directories: false,
            },
        });
    }

    pub(crate) fn compare_directories(&mut self) {
        if !self.comparison_panels_are_local() {
            return;
        }
        if self.panels.iter().any(|panel| panel.is_panelized()) {
            self.set_status("Leave panelized results before comparing directories");
            return;
        }
        self.start_command_capture(CommandCaptureSpec {
            cwd: self.active_panel().cwd.clone(),
            command: CapturedCommand::Compare {
                left: self.active_panel().cwd.clone(),
                right: self.passive_panel().cwd.clone(),
                directories: true,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_comparison_prefers_difft_and_preserves_literal_arguments() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("-left ' $(echo unsafe)");
        let right = root.path().join("right\nfile");
        let command = build_comparison_process(&left, &right, false, root.path(), |program, _| {
            assert_eq!(program, "difft");
            Some(PathBuf::from("/tools/difft"))
        })
        .unwrap();
        assert_eq!(command.get_program(), "/tools/difft");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            &args[..4],
            &["--color=never", "--display=inline", "--exit-code", "--"]
        );
        assert_eq!(args[4], left.as_os_str());
        assert_eq!(args[5], right.as_os_str());
    }

    #[test]
    fn comparison_falls_back_to_diff_and_checks_file_types() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        fs::write(&file, "x").unwrap();
        let lookup =
            |program: &OsStr, _: &Path| (program == "diff").then(|| PathBuf::from("/tools/diff"));
        let command =
            comparison_process_with_lookup(&file, &file, false, root.path(), lookup).unwrap();
        assert_eq!(command.get_program(), "/tools/diff");
        assert_eq!(command.get_args().next().unwrap(), "-u");
        let command =
            comparison_process_with_lookup(root.path(), root.path(), true, root.path(), lookup)
                .unwrap();
        assert_eq!(
            command.get_args().take(3).collect::<Vec<_>>(),
            ["-r", "-q", "--"]
        );
        assert!(
            comparison_process_with_lookup(&file, root.path(), false, root.path(), lookup).is_err()
        );
    }

    #[test]
    #[cfg(unix)]
    fn comparison_arguments_preserve_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
        let command = build_comparison_process(&path, &path, false, root.path(), |_, _| {
            Some(PathBuf::from("difft"))
        })
        .unwrap();
        assert_eq!(command.get_args().last().unwrap(), path.as_os_str());
    }
}
