use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::{AppState, ExternalEditRequest, resolve_external_editor_command, settings_io};

const CONFIG_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConfigurationFile {
    Extensions,
    Menu,
    Highlighting,
}

impl ConfigurationFile {
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Extensions => "mc.ext.ini",
            Self::Menu => "mc.menu",
            Self::Highlighting => "filehighlight.ini",
        }
    }

    pub const fn bundled(self) -> &'static str {
        match self {
            Self::Extensions => include_str!("../assets/mc.ext.ini"),
            Self::Menu => include_str!("../assets/mc.menu"),
            Self::Highlighting => include_str!("../assets/filehighlight.ini"),
        }
    }
}

/// Shared discovery for configuration readers and the configuration editor commands.
#[derive(Clone, Debug)]
pub struct ConfigurationFiles {
    pub user_directory: Option<PathBuf>,
    pub system_directories: Vec<PathBuf>,
}

impl Default for ConfigurationFiles {
    fn default() -> Self {
        Self {
            user_directory: settings_io::settings_paths()
                .mc_ini_path
                .and_then(|path| path.parent().map(Path::to_path_buf)),
            system_directories: [
                "/opt/homebrew/etc/mc",
                "/usr/local/etc/mc",
                "/etc/mc",
                "/opt/homebrew/share/mc",
                "/usr/local/share/mc",
                "/usr/share/mc",
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        }
    }
}

impl ConfigurationFiles {
    /// None means the bundled default. A malformed or unreadable candidate is an
    /// error, so a broken user configuration cannot silently enable system actions.
    pub fn effective_path(
        &self,
        file: ConfigurationFile,
        local_directory: Option<&Path>,
    ) -> io::Result<Option<PathBuf>> {
        if file == ConfigurationFile::Menu
            && let Some(directory) = local_directory
            && let Some(path) = trusted_local_menu(directory)?
        {
            return Ok(Some(path));
        }
        for directory in self.user_directory.iter().chain(&self.system_directories) {
            let path = directory.join(file.filename());
            if regular_configuration(&path)? {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    pub fn read(
        &self,
        file: ConfigurationFile,
        local_directory: Option<&Path>,
    ) -> io::Result<String> {
        match self.effective_path(file, local_directory)? {
            Some(path) => {
                let is_local = file == ConfigurationFile::Menu
                    && local_directory.is_some_and(|directory| path == directory.join(".mc.menu"));
                read_configuration(&path, is_local)
            }
            None => Ok(file.bundled().to_owned()),
        }
    }

    /// Copy installed/bundled defaults to a private user file before editing.
    /// Persist without replacement, even if another process creates it concurrently.
    pub fn prepare_for_edit(
        &self,
        file: ConfigurationFile,
        local_directory: Option<&Path>,
    ) -> io::Result<PathBuf> {
        if file == ConfigurationFile::Menu
            && let Some(directory) = local_directory
            && let Some(path) = trusted_local_menu(directory)?
        {
            return Ok(path);
        }
        let directory = self.user_directory.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "user configuration directory is unavailable",
            )
        })?;
        let path = directory.join(file.filename());
        if regular_configuration(&path)? {
            return Ok(path);
        }
        let content = self.read(file, None)?;
        fs::create_dir_all(directory)?;
        let mut temp = tempfile::NamedTempFile::new_in(directory)?;
        temp.write_all(content.as_bytes())?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                if !regular_configuration(&path)? {
                    return Err(error.error);
                }
            }
            Err(error) => return Err(error.error),
        }
        Ok(path)
    }
}

fn regular_configuration(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        // A dangling configuration symlink is an error, not an absent override.
        Ok(_) => {
            let metadata = fs::metadata(path)?;
            if metadata.is_file() {
                Ok(true)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a regular configuration file", path.display()),
                ))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn trusted_local_menu(directory: &Path) -> io::Result<Option<PathBuf>> {
    let path = directory.join(".mc.menu");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(trusted_local_metadata(&metadata).then_some(path))
}

fn trusted_local_metadata(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.is_file()
            && (metadata.uid() == 0 || metadata.uid() == nix::unistd::geteuid().as_raw())
            && metadata.mode() & 0o022 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn read_configuration(path: &Path, local_menu: bool) -> io::Result<String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use nix::fcntl::OFlag;
        use std::os::unix::fs::OpenOptionsExt;
        let flags = OFlag::O_NONBLOCK
            | if local_menu {
                OFlag::O_NOFOLLOW
            } else {
                OFlag::empty()
            };
        options.custom_flags(flags.bits());
    }
    let file = options.open(path)?;
    // Validate the opened descriptor, not only the earlier path lookup. A replaced
    // FIFO must not hang the UI; a replaced local menu must pass the same trust check.
    let metadata = file.metadata()?;
    if !metadata.is_file() || (local_menu && !trusted_local_metadata(&metadata)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configuration changed or is not a trusted regular file",
        ));
    }
    let mut content = String::new();
    file.take(CONFIG_LIMIT + 1).read_to_string(&mut content)?;
    if content.len() as u64 > CONFIG_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration exceeds 1 MiB",
        ));
    }
    Ok(content)
}

impl AppState {
    pub fn set_configuration_files(&mut self, files: ConfigurationFiles) {
        self.configuration_files = files;
        self.reload_file_highlights();
        self.reload_extension_rules();
    }

    pub(crate) fn edit_configuration_file(&mut self, file: ConfigurationFile) {
        let Some(editor_command) =
            resolve_external_editor_command(self.settings.configuration.editor_command.as_deref())
        else {
            self.set_status("No external editor configured");
            return;
        };
        let local =
            (!self.active_panel().is_archive()).then_some(self.active_panel().cwd.as_path());
        match self.configuration_files.prepare_for_edit(file, local) {
            Ok(path) => {
                let cwd = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                self.set_status(format!("Editing {}", path.display()));
                self.pending_external_edit_requests
                    .push(ExternalEditRequest {
                        editor_command,
                        path,
                        cwd,
                    });
            }
            Err(error) => self.set_status(format!("Cannot edit {}: {error}", file.filename())),
        }
    }
}
