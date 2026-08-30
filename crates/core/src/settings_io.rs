use crate::{
    FindNameMode, HotlistEntry, OverwritePolicy, PanelListingFormat, PanelizePreset, Settings,
    SortField,
};
use rc_shell::{ShellDialect, ShellHistoryMode, ShellMode, ShellSettings};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MC_CONFIG_SECTION: &str = "Midnight-Commander";
const MC_SKIN_KEY: &str = "skin";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsPaths {
    pub mc_ini_path: Option<PathBuf>,
    pub rc_ini_path: Option<PathBuf>,
}

pub fn settings_paths() -> SettingsPaths {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    SettingsPaths {
        mc_ini_path: home.as_ref().map(|root| root.join(".config/mc/ini")),
        rc_ini_path: home.map(|root| root.join(".config/rc/settings.ini")),
    }
}

pub fn load_settings(paths: &SettingsPaths) -> io::Result<Settings> {
    let mut settings = Settings::default();

    if let Some(path) = paths.rc_ini_path.as_deref() {
        let source = match fs::read_to_string(path) {
            Ok(source) => Some(source),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(source) = source {
            apply_rc_settings_ini(&mut settings, &source);
        }
    }

    if let Some(path) = paths.mc_ini_path.as_deref()
        && let Some(skin) = read_skin_from_mc_ini(path)?
    {
        settings.appearance.skin = skin;
    }

    settings.save_setup.dirty = false;
    Ok(settings)
}

pub fn save_settings(paths: &SettingsPaths, settings: &Settings) -> io::Result<()> {
    if let Some(path) = paths.mc_ini_path.as_deref() {
        write_skin_to_mc_ini(path, &settings.appearance.skin)?;
    }
    if let Some(path) = paths.rc_ini_path.as_deref() {
        write_rc_settings_ini(path, settings)?;
    }
    Ok(())
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub fn read_skin_from_mc_ini(path: &Path) -> io::Result<Option<String>> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut in_mc_section = false;
    for raw_line in source.lines() {
        let line = raw_line.trim();
        if let Some(section_name) = parse_ini_section_name(line) {
            in_mc_section = section_name.eq_ignore_ascii_case(MC_CONFIG_SECTION);
            continue;
        }
        if !in_mc_section || line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(MC_SKIN_KEY) {
            let value = value.trim();
            if value.is_empty() {
                return Ok(None);
            }
            return Ok(Some(value.to_string()));
        }
    }

    Ok(None)
}

pub fn write_skin_to_mc_ini(path: &Path, skin: &str) -> io::Result<()> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let updated = upsert_skin_in_mc_ini(&source, skin);
    write_atomic(path, &updated)
}

fn write_rc_settings_ini(path: &Path, settings: &Settings) -> io::Result<()> {
    let source = render_rc_settings_ini(settings);
    write_atomic(path, &source)
}

fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings");
    let (tmp, tmp_file) = create_atomic_temp_file(path, stem)?;
    let mut temp = AtomicTempCleanup::new(tmp, tmp_file);
    temp.file_mut().write_all(content.as_bytes())?;
    temp.file_mut().sync_all()?;
    temp.close();
    #[cfg(windows)]
    {
        match fs::rename(temp.path(), path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                fs::remove_file(path)?;
                fs::rename(temp.path(), path)?;
            }
            Err(error) => return Err(error),
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(temp.path(), path)?;
    }
    temp.disarm();
    sync_parent_dir(path)
}

fn create_atomic_temp_file(path: &Path, stem: &str) -> io::Result<(PathBuf, fs::File)> {
    const CREATE_ATTEMPTS: usize = 16;

    for _ in 0..CREATE_ATTEMPTS {
        let nonce = atomic_temp_nonce()?;
        let tmp = path.with_file_name(format!("{stem}.tmp-{nonce:032x}"));
        match open_new_atomic_temp(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique settings temp file",
    ))
}

#[cfg(any(unix, windows))]
fn atomic_temp_nonce() -> io::Result<u128> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| io::Error::other(format!("failed to generate temp-file name: {error}")))?;
    Ok(u128::from_ne_bytes(random))
}

#[cfg(not(any(unix, windows)))]
fn atomic_temp_nonce() -> io::Result<u128> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(timestamp ^ u128::from(counter))
}

fn open_new_atomic_temp(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

struct AtomicTempCleanup {
    path: PathBuf,
    file: Option<fs::File>,
    armed: bool,
}

impl AtomicTempCleanup {
    fn new(path: PathBuf, file: fs::File) -> Self {
        Self {
            path,
            file: Some(file),
            armed: true,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file_mut(&mut self) -> &mut fs::File {
        self.file.as_mut().expect("atomic temp file is open")
    }

    fn close(&mut self) {
        self.file.take();
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AtomicTempCleanup {
    fn drop(&mut self) {
        self.close();
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(windows)]
fn sync_parent_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::File::open(parent)?.sync_all()
}

pub fn upsert_skin_in_mc_ini(source: &str, skin: &str) -> String {
    let mut lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
    let mut section_start = None;

    for (index, line) in lines.iter().enumerate() {
        if let Some(section_name) = parse_ini_section_name(line)
            && section_name.eq_ignore_ascii_case(MC_CONFIG_SECTION)
        {
            section_start = Some(index);
            break;
        }
    }

    match section_start {
        Some(start) => {
            let section_end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find_map(|(index, line)| parse_ini_section_name(line).map(|_| index))
                .unwrap_or(lines.len());
            let skin_line = (start + 1..section_end).find(|line_index| {
                let line = lines[*line_index].trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                    return false;
                }
                line.split_once('=')
                    .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(MC_SKIN_KEY))
            });

            if let Some(line_index) = skin_line {
                lines[line_index] = format!("{MC_SKIN_KEY}={skin}");
            } else {
                lines.insert(section_end, format!("{MC_SKIN_KEY}={skin}"));
            }
        }
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{MC_CONFIG_SECTION}]"));
            lines.push(format!("{MC_SKIN_KEY}={skin}"));
        }
    }

    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn parse_ini_section_name(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with('[') && line.ends_with(']') {
        return Some(line[1..line.len() - 1].trim());
    }
    None
}

fn render_hotlist_entry(entry: &HotlistEntry) -> String {
    format!(
        "{}\t{}",
        escape_settings_field(&entry.label),
        escape_settings_field(&entry.path.to_string_lossy())
    )
}

fn parse_hotlist_entry(value: &str) -> Option<HotlistEntry> {
    let (label, path) = value.split_once('\t')?;
    let label = unescape_settings_field(label)?;
    let path = PathBuf::from(unescape_settings_field(path)?);
    if path.as_os_str().is_empty() {
        return None;
    }
    if label.is_empty() {
        Some(HotlistEntry::from_legacy_path(path))
    } else {
        Some(HotlistEntry::new(label, path))
    }
}

fn render_panelize_preset(preset: &PanelizePreset) -> String {
    format!(
        "{}\t{}",
        escape_settings_field(&preset.label),
        escape_settings_field(&preset.command)
    )
}

fn parse_panelize_preset(value: &str) -> Option<PanelizePreset> {
    let (label, command) = value.split_once('\t')?;
    let label = unescape_settings_field(label)?;
    let command = unescape_settings_field(command)?;
    if label.is_empty() || command.is_empty() {
        return None;
    }
    Some(PanelizePreset::new(label, command))
}

fn escape_settings_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            ' ' => escaped.push_str("\\s"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            character if character.is_whitespace() || character.is_control() => {
                escaped.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn unescape_settings_field(value: &str) -> Option<String> {
    let mut unescaped = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }
        match characters.next()? {
            '\\' => unescaped.push('\\'),
            's' => unescaped.push(' '),
            't' => unescaped.push('\t'),
            'n' => unescaped.push('\n'),
            'r' => unescaped.push('\r'),
            'u' => {
                if characters.next()? != '{' {
                    return None;
                }
                let mut value = 0_u32;
                let mut digits = 0_usize;
                loop {
                    let character = characters.next()?;
                    if character == '}' {
                        if digits == 0 {
                            return None;
                        }
                        break;
                    }
                    let digit = character.to_digit(16)?;
                    value = value.checked_mul(16)?.checked_add(digit)?;
                    digits += 1;
                    if digits > 6 {
                        return None;
                    }
                }
                unescaped.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }
    Some(unescaped)
}

fn apply_rc_settings_ini(settings: &mut Settings, source: &str) {
    let mut section = String::new();
    let mut saw_configuration_section = false;
    let mut saw_hotlist = false;
    let mut saw_panelize_presets = false;
    let mut saw_skin_dirs = false;
    let mut shell = RawShellSection::default();

    for raw_line in source.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if let Some(section_name) = parse_ini_section_name(line) {
            section = section_name.to_ascii_lowercase();
            if section == "configuration" {
                saw_configuration_section = true;
            }
            if section == "shell" {
                shell.present = true;
            }
            continue;
        }

        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim().to_ascii_lowercase();
        let value = raw_value.trim();

        match (section.as_str(), key.as_str()) {
            ("shell", "mode") => shell.mode = Some(value.to_string()),
            ("shell", "program") => match unescape_settings_field(value) {
                Some(program) => shell.program = Some(program),
                None => {
                    shell.invalid_field = Some(String::from("invalid escaped program value"));
                }
            },
            ("shell", "dialect") => shell.dialect = Some(value.to_string()),
            ("shell", "arg") => {
                shell.saw_argument = true;
                match unescape_settings_field(value) {
                    Some(argument) => shell.arguments.push(argument),
                    None => shell.invalid_field = Some(String::from("invalid escaped arg value")),
                }
            }
            ("shell", "history") => shell.history = Some(value.to_string()),
            ("configuration", "overwrite_policy") => {
                if let Some(policy) = parse_overwrite_policy(value) {
                    settings.configuration.default_overwrite_policy = policy;
                }
            }
            ("configuration", "macos_option_symbols") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.configuration.macos_option_symbols = parsed;
                }
            }
            ("configuration", "editor_command") => {
                settings.configuration.editor_command =
                    (!value.is_empty()).then(|| value.to_string());
            }
            ("configuration", "use_internal_editor") => {
                // Legacy rc setting. Internal editing is no longer implemented.
            }
            ("configuration", "keymap_override") => {
                if value.is_empty() {
                    settings.configuration.keymap_override = None;
                } else {
                    settings.configuration.keymap_override = Some(PathBuf::from(value));
                }
            }
            ("configuration", "hotlist") | ("configuration", "hotlist_entry") => {
                if !saw_hotlist {
                    settings.configuration.hotlist.clear();
                    saw_hotlist = true;
                }
                let entry = if key == "hotlist_entry" {
                    parse_hotlist_entry(value)
                } else {
                    (!value.is_empty())
                        .then(|| HotlistEntry::from_legacy_path(PathBuf::from(value)))
                };
                if let Some(entry) = entry {
                    settings.configuration.hotlist.push(entry);
                }
            }
            ("configuration", "panelize_preset") | ("configuration", "panelize_preset_entry") => {
                if !saw_panelize_presets {
                    settings.configuration.panelize_presets.clear();
                    saw_panelize_presets = true;
                }
                let preset = if key == "panelize_preset_entry" {
                    parse_panelize_preset(value)
                } else {
                    (!value.is_empty())
                        .then(|| PanelizePreset::from_legacy_command(value.to_string()))
                };
                if let Some(preset) = preset {
                    settings.configuration.panelize_presets.push(preset);
                }
            }
            ("layout", "show_menu_bar") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.layout.show_menu_bar = parsed;
                }
            }
            ("layout", "show_button_bar") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.layout.show_button_bar = parsed;
                }
            }
            ("layout", "show_debug_status") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.layout.show_debug_status = parsed;
                }
            }
            ("layout", "show_panel_totals") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.layout.show_panel_totals = parsed;
                }
            }
            ("layout", "status_message_timeout_seconds") => {
                if let Ok(parsed) = value.parse::<u64>() {
                    settings.layout.status_message_timeout_seconds = parsed;
                }
            }
            ("layout", "jobs_dialog_width") => {
                if let Ok(parsed) = value.parse::<u16>() {
                    settings.layout.jobs_dialog_width = parsed;
                }
            }
            ("layout", "jobs_dialog_height") => {
                if let Ok(parsed) = value.parse::<u16>() {
                    settings.layout.jobs_dialog_height = parsed;
                }
            }
            ("layout", "help_dialog_width") => {
                if let Ok(parsed) = value.parse::<u16>() {
                    settings.layout.help_dialog_width = parsed;
                }
            }
            ("layout", "help_dialog_height") => {
                if let Ok(parsed) = value.parse::<u16>() {
                    settings.layout.help_dialog_height = parsed;
                }
            }
            ("panel_options", "show_hidden_files") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.show_hidden_files = parsed;
                }
            }
            ("panel_options", "sort_field") => {
                if let Some(parsed) = parse_sort_field(value) {
                    for sort_mode in &mut settings.panel_options.sort_modes {
                        sort_mode.field = parsed;
                    }
                }
            }
            ("panel_options", "sort_reverse") => {
                if let Some(parsed) = parse_bool(value) {
                    for sort_mode in &mut settings.panel_options.sort_modes {
                        sort_mode.reverse = parsed;
                    }
                }
            }
            ("panel_options", "left_sort_field") => {
                if let Some(parsed) = parse_sort_field(value) {
                    settings.panel_options.sort_modes[0].field = parsed;
                }
            }
            ("panel_options", "left_sort_reverse") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.sort_modes[0].reverse = parsed;
                }
            }
            ("panel_options", "right_sort_field") => {
                if let Some(parsed) = parse_sort_field(value) {
                    settings.panel_options.sort_modes[1].field = parsed;
                }
            }
            ("panel_options", "right_sort_reverse") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.sort_modes[1].reverse = parsed;
                }
            }
            ("panel_options", "left_listing_format") => {
                if let Some(parsed) = parse_listing_format(value) {
                    settings.panel_options.listing_formats[0] = parsed;
                }
            }
            ("panel_options", "right_listing_format") => {
                if let Some(parsed) = parse_listing_format(value) {
                    settings.panel_options.listing_formats[1] = parsed;
                }
            }
            ("panel_options", "left_filter_pattern") => {
                if let Some(parsed) = unescape_settings_field(value) {
                    settings.panel_options.filters[0].pattern = parsed;
                }
            }
            ("panel_options", "left_filter_files_only") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.filters[0].files_only = parsed;
                }
            }
            ("panel_options", "left_filter_mode") => {
                if let Some(parsed) = parse_filter_mode(value) {
                    settings.panel_options.filters[0].name_mode = parsed;
                }
            }
            ("panel_options", "left_filter_case_sensitive") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.filters[0].case_sensitive = parsed;
                }
            }
            ("panel_options", "right_filter_pattern") => {
                if let Some(parsed) = unescape_settings_field(value) {
                    settings.panel_options.filters[1].pattern = parsed;
                }
            }
            ("panel_options", "right_filter_files_only") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.filters[1].files_only = parsed;
                }
            }
            ("panel_options", "right_filter_mode") => {
                if let Some(parsed) = parse_filter_mode(value) {
                    settings.panel_options.filters[1].name_mode = parsed;
                }
            }
            ("panel_options", "right_filter_case_sensitive") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.filters[1].case_sensitive = parsed;
                }
            }
            ("confirmation", "confirm_delete") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.confirmation.confirm_delete = parsed;
                }
            }
            ("confirmation", "confirm_overwrite") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.confirmation.confirm_overwrite = parsed;
                }
            }
            ("confirmation", "confirm_quit") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.confirmation.confirm_quit = parsed;
                }
            }
            ("confirmation", "confirm_hotlist_delete") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.confirmation.confirm_hotlist_delete = parsed;
                }
            }
            ("appearance", "skin") if !value.is_empty() => {
                settings.appearance.skin = value.to_string();
            }
            ("appearance", "skin_dir") => {
                if !saw_skin_dirs {
                    settings.appearance.skin_dirs.clear();
                    saw_skin_dirs = true;
                }
                settings.appearance.skin_dirs.push(PathBuf::from(value));
            }
            ("display_bits", "utf8_output") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.display_bits.utf8_output = parsed;
                }
            }
            ("display_bits", "eight_bit_input") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.display_bits.eight_bit_input = parsed;
                }
            }
            ("learn_keys", "last_learned_binding") => {
                if value.is_empty() {
                    settings.learn_keys.last_learned_binding = None;
                } else {
                    settings.learn_keys.last_learned_binding = Some(value.to_string());
                }
            }
            ("virtual_fs", "vfs_enabled") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.virtual_fs.vfs_enabled = parsed;
                }
            }
            ("virtual_fs", "ftp_enabled") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.virtual_fs.ftp_enabled = parsed;
                }
            }
            ("virtual_fs", "shell_link_enabled") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.virtual_fs.shell_link_enabled = parsed;
                }
            }
            ("virtual_fs", "sftp_enabled") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.virtual_fs.sftp_enabled = parsed;
                }
            }
            ("advanced", "page_step") => {
                if let Ok(parsed) = value.parse::<usize>() {
                    settings.advanced.page_step = parsed.max(1);
                }
            }
            ("advanced", "viewer_page_step") => {
                if let Ok(parsed) = value.parse::<usize>() {
                    settings.advanced.viewer_page_step = parsed.max(1);
                }
            }
            ("advanced", "max_find_results") => {
                if let Ok(parsed) = value.parse::<usize>() {
                    settings.advanced.max_find_results = parsed.max(1);
                }
            }
            ("advanced", "tree_max_depth") => {
                if let Ok(parsed) = value.parse::<usize>() {
                    settings.advanced.tree_max_depth = parsed.max(1);
                }
            }
            ("advanced", "tree_max_entries") => {
                if let Ok(parsed) = value.parse::<usize>() {
                    settings.advanced.tree_max_entries = parsed.max(1);
                }
            }
            _ => {}
        }
    }

    if saw_configuration_section && !saw_panelize_presets {
        settings.configuration.panelize_presets.clear();
    }
    if shell.present {
        settings.shell = parse_shell_section(shell).unwrap_or_else(|error| {
            tracing::warn!(error = %error, "ignored invalid persisted shell configuration");
            ShellSettings::default()
                .with_diagnostic(format!("ignored invalid [shell] configuration: {error}"))
        });
    }
    for (panel_index, filter) in settings.panel_options.filters.iter_mut().enumerate() {
        if let Err(error) = filter.validate() {
            tracing::warn!(
                panel_index,
                error = %error,
                "ignored invalid persisted panel filter"
            );
            filter.pattern.clear();
        }
    }
}

fn render_rc_settings_ini(settings: &Settings) -> String {
    let mut lines = vec![String::from("[configuration]")];
    lines.push(format!(
        "overwrite_policy={}",
        overwrite_policy_label(settings.configuration.default_overwrite_policy)
    ));
    lines.push(format!(
        "macos_option_symbols={}",
        settings.configuration.macos_option_symbols
    ));
    lines.push(format!(
        "editor_command={}",
        settings
            .configuration
            .editor_command
            .as_deref()
            .unwrap_or_default()
    ));
    if let Some(path) = settings.configuration.keymap_override.as_ref() {
        lines.push(format!("keymap_override={}", path.to_string_lossy()));
    } else {
        lines.push(String::from("keymap_override="));
    }
    for entry in &settings.configuration.hotlist {
        lines.push(format!("hotlist_entry={}", render_hotlist_entry(entry)));
    }
    for preset in &settings.configuration.panelize_presets {
        lines.push(format!(
            "panelize_preset_entry={}",
            render_panelize_preset(preset)
        ));
    }

    lines.push(String::new());
    lines.push(String::from("[shell]"));
    match &settings.shell.mode {
        ShellMode::Auto => lines.push(String::from("mode=auto")),
        ShellMode::Custom(custom) => {
            lines.push(String::from("mode=custom"));
            lines.push(format!(
                "program={}",
                escape_settings_field(&custom.program)
            ));
            lines.push(format!("dialect={}", custom.dialect.label()));
            for argument in &custom.arguments {
                lines.push(format!("arg={}", escape_settings_field(argument)));
            }
        }
    }
    lines.push(format!("history={}", settings.shell.history.label()));

    lines.push(String::new());
    lines.push(String::from("[layout]"));
    lines.push(format!("show_menu_bar={}", settings.layout.show_menu_bar));
    lines.push(format!(
        "show_button_bar={}",
        settings.layout.show_button_bar
    ));
    lines.push(format!(
        "show_debug_status={}",
        settings.layout.show_debug_status
    ));
    lines.push(format!(
        "show_panel_totals={}",
        settings.layout.show_panel_totals
    ));
    lines.push(format!(
        "status_message_timeout_seconds={}",
        settings.layout.status_message_timeout_seconds
    ));
    lines.push(format!(
        "jobs_dialog_width={}",
        settings.layout.jobs_dialog_width
    ));
    lines.push(format!(
        "jobs_dialog_height={}",
        settings.layout.jobs_dialog_height
    ));
    lines.push(format!(
        "help_dialog_width={}",
        settings.layout.help_dialog_width
    ));
    lines.push(format!(
        "help_dialog_height={}",
        settings.layout.help_dialog_height
    ));

    lines.push(String::new());
    lines.push(String::from("[panel_options]"));
    lines.push(format!(
        "show_hidden_files={}",
        settings.panel_options.show_hidden_files
    ));
    lines.push(format!(
        "left_sort_field={}",
        sort_field_label(settings.panel_options.sort_modes[0].field)
    ));
    lines.push(format!(
        "left_sort_reverse={}",
        settings.panel_options.sort_modes[0].reverse
    ));
    lines.push(format!(
        "right_sort_field={}",
        sort_field_label(settings.panel_options.sort_modes[1].field)
    ));
    lines.push(format!(
        "right_sort_reverse={}",
        settings.panel_options.sort_modes[1].reverse
    ));
    lines.push(format!(
        "left_listing_format={}",
        settings.panel_options.listing_formats[0].title_label()
    ));
    lines.push(format!(
        "right_listing_format={}",
        settings.panel_options.listing_formats[1].title_label()
    ));
    for (prefix, filter) in ["left", "right"]
        .into_iter()
        .zip(&settings.panel_options.filters)
    {
        lines.push(format!(
            "{prefix}_filter_pattern={}",
            escape_settings_field(&filter.pattern)
        ));
        lines.push(format!("{prefix}_filter_files_only={}", filter.files_only));
        lines.push(format!("{prefix}_filter_mode={}", filter.name_mode.label()));
        lines.push(format!(
            "{prefix}_filter_case_sensitive={}",
            filter.case_sensitive
        ));
    }

    lines.push(String::new());
    lines.push(String::from("[confirmation]"));
    lines.push(format!(
        "confirm_delete={}",
        settings.confirmation.confirm_delete
    ));
    lines.push(format!(
        "confirm_overwrite={}",
        settings.confirmation.confirm_overwrite
    ));
    lines.push(format!(
        "confirm_quit={}",
        settings.confirmation.confirm_quit
    ));
    lines.push(format!(
        "confirm_hotlist_delete={}",
        settings.confirmation.confirm_hotlist_delete
    ));

    lines.push(String::new());
    lines.push(String::from("[appearance]"));
    lines.push(format!("skin={}", settings.appearance.skin));
    for skin_dir in &settings.appearance.skin_dirs {
        lines.push(format!("skin_dir={}", skin_dir.to_string_lossy()));
    }

    lines.push(String::new());
    lines.push(String::from("[display_bits]"));
    lines.push(format!("utf8_output={}", settings.display_bits.utf8_output));
    lines.push(format!(
        "eight_bit_input={}",
        settings.display_bits.eight_bit_input
    ));

    lines.push(String::new());
    lines.push(String::from("[learn_keys]"));
    if let Some(binding) = settings.learn_keys.last_learned_binding.as_ref() {
        lines.push(format!("last_learned_binding={binding}"));
    } else {
        lines.push(String::from("last_learned_binding="));
    }

    lines.push(String::new());
    lines.push(String::from("[virtual_fs]"));
    lines.push(format!("vfs_enabled={}", settings.virtual_fs.vfs_enabled));
    lines.push(format!("ftp_enabled={}", settings.virtual_fs.ftp_enabled));
    lines.push(format!(
        "shell_link_enabled={}",
        settings.virtual_fs.shell_link_enabled
    ));
    lines.push(format!("sftp_enabled={}", settings.virtual_fs.sftp_enabled));

    lines.push(String::new());
    lines.push(String::from("[advanced]"));
    lines.push(format!("page_step={}", settings.advanced.page_step));
    lines.push(format!(
        "viewer_page_step={}",
        settings.advanced.viewer_page_step
    ));
    lines.push(format!(
        "max_find_results={}",
        settings.advanced.max_find_results
    ));
    lines.push(format!(
        "tree_max_depth={}",
        settings.advanced.tree_max_depth
    ));
    lines.push(format!(
        "tree_max_entries={}",
        settings.advanced.tree_max_entries
    ));

    let mut rendered = lines.join("\n");
    rendered.push('\n');
    rendered
}

#[derive(Default)]
struct RawShellSection {
    present: bool,
    mode: Option<String>,
    program: Option<String>,
    dialect: Option<String>,
    arguments: Vec<String>,
    saw_argument: bool,
    history: Option<String>,
    invalid_field: Option<String>,
}

fn parse_shell_section(raw: RawShellSection) -> Result<ShellSettings, String> {
    if let Some(error) = raw.invalid_field {
        return Err(error);
    }
    let history = match raw.history.as_deref() {
        Some(value) => ShellHistoryMode::parse(value)
            .ok_or_else(|| format!("unsupported history mode '{value}'"))?,
        None => ShellHistoryMode::Session,
    };
    match raw
        .mode
        .as_deref()
        .unwrap_or("auto")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "auto" => {
            if raw.program.as_ref().is_some_and(|value| !value.is_empty())
                || raw.dialect.as_ref().is_some_and(|value| !value.is_empty())
                || raw.saw_argument
            {
                return Err(String::from(
                    "mode=auto cannot include program, dialect, or arg values",
                ));
            }
            Ok(ShellSettings::auto(history))
        }
        "custom" => {
            let program = raw
                .program
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| String::from("mode=custom requires program"))?;
            let dialect_value = raw
                .dialect
                .ok_or_else(|| String::from("mode=custom requires dialect"))?;
            let dialect = ShellDialect::parse(&dialect_value)
                .ok_or_else(|| format!("unsupported shell dialect '{dialect_value}'"))?;
            let arguments = raw.saw_argument.then_some(raw.arguments);
            ShellSettings::custom(program, dialect, arguments, history)
        }
        other => Err(format!("unsupported shell mode '{other}'")),
    }
}

fn parse_overwrite_policy(value: &str) -> Option<OverwritePolicy> {
    match value.trim().to_ascii_lowercase().as_str() {
        "overwrite" => Some(OverwritePolicy::Overwrite),
        "skip" => Some(OverwritePolicy::Skip),
        "rename" => Some(OverwritePolicy::Rename),
        _ => None,
    }
}

fn overwrite_policy_label(policy: OverwritePolicy) -> &'static str {
    match policy {
        OverwritePolicy::Overwrite => "overwrite",
        OverwritePolicy::Skip => "skip",
        OverwritePolicy::Rename => "rename",
    }
}

fn parse_sort_field(value: &str) -> Option<SortField> {
    match value.trim().to_ascii_lowercase().as_str() {
        "name" => Some(SortField::Name),
        "version" | "natural" => Some(SortField::Version),
        "extension" | "ext" => Some(SortField::Extension),
        "modified" | "mtime" => Some(SortField::Modified),
        "accessed" | "atime" => Some(SortField::Accessed),
        "changed" | "ctime" => Some(SortField::Changed),
        "size" => Some(SortField::Size),
        "inode" => Some(SortField::Inode),
        "unsorted" | "none" => Some(SortField::Unsorted),
        _ => None,
    }
}

fn sort_field_label(field: SortField) -> &'static str {
    field.label()
}

fn parse_listing_format(value: &str) -> Option<PanelListingFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "full" => Some(PanelListingFormat::Full),
        "brief" => Some(PanelListingFormat::Brief),
        "long" => Some(PanelListingFormat::Long),
        _ => None,
    }
}

fn parse_filter_mode(value: &str) -> Option<FindNameMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "glob" | "shell" | "shell-pattern" => Some(FindNameMode::Glob),
        "regex" | "regexp" => Some(FindNameMode::Regex),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn read_skin_from_mc_ini_uses_midnight_commander_section() {
        let source = "\
[Midnight-Commander]
skin=darkfar
";
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let path = env::temp_dir().join(format!("rc-read-mc-ini-{stamp}.ini"));
        fs::write(&path, source).expect("test ini should be written");

        let skin = read_skin_from_mc_ini(&path).expect("skin should parse from ini");
        assert_eq!(skin, Some(String::from("darkfar")));

        fs::remove_file(&path).expect("test ini should be removed");
    }

    #[test]
    fn upsert_skin_in_mc_ini_updates_existing_skin_value() {
        let source = "\
[Midnight-Commander]
verbose=true
skin=default
";
        let updated = upsert_skin_in_mc_ini(source, "xoria256");
        assert!(updated.contains("skin=xoria256"));
        assert!(
            !updated.contains("skin=default"),
            "previous skin key should be replaced"
        );
    }

    #[test]
    fn upsert_skin_in_mc_ini_adds_section_when_missing() {
        let updated = upsert_skin_in_mc_ini("[Layout]\nmenubar_visible=true\n", "julia256");
        assert!(updated.contains("[Midnight-Commander]"));
        assert!(updated.contains("skin=julia256"));
    }

    #[test]
    fn rc_settings_round_trip_preserves_hotlist_and_presets() {
        let mut settings = Settings::default();
        settings.configuration.hotlist = vec![
            HotlistEntry::new("Temporary files", PathBuf::from("/tmp")),
            HotlistEntry::new("Variable data", PathBuf::from("/var")),
        ];
        settings.configuration.panelize_presets = vec![
            PanelizePreset::new("All project files", "find . -type f"),
            PanelizePreset::new("Git files", "git ls-files"),
        ];
        settings.configuration.default_overwrite_policy = OverwritePolicy::Rename;
        settings.panel_options.sort_modes = [
            crate::SortMode {
                field: SortField::Modified,
                reverse: true,
            },
            crate::SortMode {
                field: SortField::Inode,
                reverse: false,
            },
        ];
        settings.panel_options.listing_formats =
            [PanelListingFormat::Brief, PanelListingFormat::Long];
        settings.panel_options.filters = [
            crate::PanelFilter {
                pattern: String::from("*.rs"),
                files_only: true,
                name_mode: FindNameMode::Glob,
                case_sensitive: false,
            },
            crate::PanelFilter {
                pattern: String::from(r"^release notes\\d+$"),
                files_only: false,
                name_mode: FindNameMode::Regex,
                case_sensitive: true,
            },
        ];
        settings.layout.status_message_timeout_seconds = 42;
        settings.confirmation.confirm_hotlist_delete = false;

        let source = render_rc_settings_ini(&settings);
        let mut parsed = Settings::default();
        apply_rc_settings_ini(&mut parsed, &source);

        assert_eq!(parsed.configuration.hotlist, settings.configuration.hotlist);
        assert_eq!(
            parsed.configuration.panelize_presets,
            settings.configuration.panelize_presets
        );
        assert_eq!(
            parsed.configuration.default_overwrite_policy,
            OverwritePolicy::Rename
        );
        assert_eq!(
            parsed.panel_options.sort_modes,
            settings.panel_options.sort_modes
        );
        assert_eq!(
            parsed.panel_options.listing_formats,
            [PanelListingFormat::Brief, PanelListingFormat::Long]
        );
        assert_eq!(parsed.panel_options.filters, settings.panel_options.filters);
        assert_eq!(parsed.layout.status_message_timeout_seconds, 42);
        assert!(!parsed.confirmation.confirm_hotlist_delete);
    }

    #[test]
    fn legacy_global_sort_settings_apply_to_both_panels() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(
            &mut settings,
            "[panel_options]\nsort_field=size\nsort_reverse=true\n",
        );

        let expected = crate::SortMode {
            field: SortField::Size,
            reverse: true,
        };
        assert_eq!(settings.panel_options.sort_modes, [expected; 2]);
    }

    #[test]
    fn invalid_persisted_filter_is_safely_disabled() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(
            &mut settings,
            "[panel_options]\nleft_filter_pattern=[\nleft_filter_files_only=false\n",
        );

        assert!(!settings.panel_options.filters[0].is_active());
        assert!(!settings.panel_options.filters[0].files_only);
    }

    #[test]
    fn legacy_path_only_hotlist_entries_migrate_to_labels() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(&mut settings, "[configuration]\nhotlist=/tmp\nhotlist=/\n");

        assert_eq!(
            settings.configuration.hotlist,
            [
                HotlistEntry::new("/tmp", PathBuf::from("/tmp")),
                HotlistEntry::new("/", PathBuf::from("/")),
            ]
        );
        assert!(
            settings.confirmation.confirm_hotlist_delete,
            "legacy settings should retain the safe confirmation default"
        );
    }

    #[test]
    fn hotlist_delete_confirmation_setting_round_trips() {
        let mut settings = Settings::default();
        settings.confirmation.confirm_hotlist_delete = false;

        let source = render_rc_settings_ini(&settings);
        assert!(source.contains("confirm_hotlist_delete=false"));
        let mut parsed = Settings::default();
        apply_rc_settings_ini(&mut parsed, &source);

        assert!(!parsed.confirmation.confirm_hotlist_delete);
    }

    #[test]
    fn hotlist_entry_encoding_round_trips_spaces_tabs_and_backslashes() {
        let entry = HotlistEntry::new(
            "Work tree\tprimary",
            PathBuf::from(r"C:\Users\Example Project"),
        );
        let rendered = render_hotlist_entry(&entry);
        assert_eq!(parse_hotlist_entry(&rendered), Some(entry));
    }

    #[test]
    fn legacy_command_only_panelize_presets_migrate_to_labels() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(
            &mut settings,
            "[configuration]\npanelize_preset=find . -type f\npanelize_preset=git ls-files\n",
        );

        assert_eq!(
            settings.configuration.panelize_presets,
            [
                PanelizePreset::new("All files", "find . -type f"),
                PanelizePreset::new("git ls-files", "git ls-files"),
            ]
        );
    }

    #[test]
    fn named_panelize_preset_encoding_round_trips_special_characters() {
        let preset = PanelizePreset::new("Work tree\ttracked", "printf 'a b\\\\c\\n'");
        let rendered = render_panelize_preset(&preset);

        assert_eq!(parse_panelize_preset(&rendered), Some(preset));
    }

    #[test]
    fn malformed_named_panelize_presets_are_skipped_safely() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(
            &mut settings,
            "[configuration]\n\
             panelize_preset_entry=missing-separator\n\
             panelize_preset_entry=Empty\\scommand\t\n\
             panelize_preset_entry=Git\\sfiles\tgit\\sls-files\n",
        );

        assert_eq!(
            settings.configuration.panelize_presets,
            [PanelizePreset::new("Git files", "git ls-files")]
        );
    }

    #[test]
    fn rc_settings_round_trip_preserves_empty_panelize_presets() {
        let mut settings = Settings::default();
        settings.configuration.panelize_presets.clear();

        let source = render_rc_settings_ini(&settings);
        let mut parsed = Settings::default();
        apply_rc_settings_ini(&mut parsed, &source);

        assert!(
            parsed.configuration.panelize_presets.is_empty(),
            "empty panelize presets should remain empty after reload"
        );
    }

    #[test]
    fn load_settings_prefers_mc_skin_over_rc_skin() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-load-settings-precedence-{stamp}"));
        fs::create_dir_all(&root).expect("test directory should be created");

        let mc_ini_path = root.join("mc.ini");
        let rc_ini_path = root.join("settings.ini");
        fs::write(
            &mc_ini_path,
            "\
[Midnight-Commander]
skin=mc-skin
",
        )
        .expect("mc ini should be written");

        let mut settings = Settings::default();
        settings.appearance.skin = String::from("rc-skin");
        fs::write(&rc_ini_path, render_rc_settings_ini(&settings))
            .expect("rc ini should be written");

        let loaded = load_settings(&SettingsPaths {
            mc_ini_path: Some(mc_ini_path.clone()),
            rc_ini_path: Some(rc_ini_path.clone()),
        })
        .expect("settings should load");
        assert_eq!(loaded.appearance.skin, "mc-skin");

        fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[test]
    fn save_settings_writes_mc_and_rc_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-save-settings-{stamp}"));
        fs::create_dir_all(&root).expect("test directory should be created");

        let mc_ini_path = root.join("mc.ini");
        let rc_ini_path = root.join("settings.ini");
        let mut settings = Settings::default();
        settings.appearance.skin = String::from("xoria256");
        settings.configuration.hotlist = vec![
            HotlistEntry::new("Temporary files", PathBuf::from("/tmp")),
            HotlistEntry::new("Variable data", PathBuf::from("/var")),
        ];
        settings.configuration.default_overwrite_policy = OverwritePolicy::Rename;

        save_settings(
            &SettingsPaths {
                mc_ini_path: Some(mc_ini_path.clone()),
                rc_ini_path: Some(rc_ini_path.clone()),
            },
            &settings,
        )
        .expect("settings should save");

        let mc_ini = fs::read_to_string(&mc_ini_path).expect("mc ini should exist");
        let rc_ini = fs::read_to_string(&rc_ini_path).expect("rc ini should exist");
        assert!(mc_ini.contains("[Midnight-Commander]"));
        assert!(mc_ini.contains("skin=xoria256"));
        assert!(rc_ini.contains("[configuration]"));
        assert!(rc_ini.contains("overwrite_policy=rename"));
        assert!(rc_ini.contains("hotlist_entry=Temporary\\sfiles\t/tmp"));
        assert!(rc_ini.contains("panelize_preset_entry=All\\sfiles\tfind\\s.\\s-type\\sf"));

        fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[test]
    fn save_settings_can_replace_existing_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-save-settings-replace-{stamp}"));
        fs::create_dir_all(&root).expect("test directory should be created");

        let mc_ini_path = root.join("mc.ini");
        let rc_ini_path = root.join("settings.ini");
        let paths = SettingsPaths {
            mc_ini_path: Some(mc_ini_path.clone()),
            rc_ini_path: Some(rc_ini_path.clone()),
        };

        let mut settings = Settings::default();
        settings.appearance.skin = String::from("first-skin");
        settings.configuration.default_overwrite_policy = OverwritePolicy::Rename;
        save_settings(&paths, &settings).expect("first save should succeed");

        settings.appearance.skin = String::from("second-skin");
        settings.configuration.default_overwrite_policy = OverwritePolicy::Skip;
        save_settings(&paths, &settings).expect("second save should succeed");

        let mc_ini = fs::read_to_string(&mc_ini_path).expect("mc ini should exist");
        let rc_ini = fs::read_to_string(&rc_ini_path).expect("rc ini should exist");
        assert!(mc_ini.contains("skin=second-skin"));
        assert!(rc_ini.contains("overwrite_policy=skip"));

        fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[test]
    fn save_settings_cleans_up_atomic_temp_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-save-settings-temp-cleanup-{stamp}"));
        fs::create_dir_all(&root).expect("test directory should be created");

        let mc_ini_path = root.join("mc.ini");
        let rc_ini_path = root.join("settings.ini");
        let paths = SettingsPaths {
            mc_ini_path: Some(mc_ini_path.clone()),
            rc_ini_path: Some(rc_ini_path.clone()),
        };
        let mut settings = Settings::default();
        settings.appearance.skin = String::from("temp-cleanup");
        save_settings(&paths, &settings).expect("settings should save");

        let leftovers = fs::read_dir(&root)
            .expect("settings directory should be readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("mc.ini.tmp-") || name.starts_with("settings.ini.tmp-"))
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "atomic settings writes should not leave temp files behind: {leftovers:?}"
        );

        fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_temp_creation_is_exclusive_randomized_and_private() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::fs::symlink;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = env::temp_dir().join(format!("rc-settings-temp-symlink-{stamp}"));
        fs::create_dir_all(&root).expect("test directory should be created");
        let target = root.join("target");
        let tmp = root.join("settings.ini.tmp-attacker");
        fs::write(&target, b"preserve me").expect("target should be writable");
        symlink(&target, &tmp).expect("temp symlink should be creatable");

        let error = open_new_atomic_temp(&tmp).expect_err("create_new must reject the symlink");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&target).expect("target should remain readable"),
            b"preserve me"
        );

        let settings_path = root.join("settings.ini");
        let (first_path, first_file) =
            create_atomic_temp_file(&settings_path, "settings.ini").expect("create first temp");
        let (second_path, second_file) =
            create_atomic_temp_file(&settings_path, "settings.ini").expect("create second temp");
        assert_ne!(first_path, second_path);
        assert_eq!(
            first_file
                .metadata()
                .expect("read temp metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop((first_file, second_file));
        fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn settings_field_codec_round_trips_unicode_whitespace_and_controls() {
        let value = " \u{2003}line\nnext\r\t\u{0085}\\tail ";
        let escaped = escape_settings_field(value);
        assert!(!escaped.chars().any(char::is_whitespace));
        assert_eq!(unescape_settings_field(&escaped).as_deref(), Some(value));
        assert_eq!(unescape_settings_field("\\u{}"), None);
        assert_eq!(unescape_settings_field("\\u{110000}"), None);
    }

    #[test]
    fn shell_settings_round_trip_structured_arguments() {
        let settings = Settings {
            shell: ShellSettings::custom(
                " \u{2003}/opt/Example Shell/bin/sh\nvariant\\name\u{0085} ",
                ShellDialect::Posix,
                Some(vec![
                    String::from("--leading= value "),
                    String::from("\\path"),
                    String::from("{command}"),
                ]),
                ShellHistoryMode::Off,
            )
            .expect("custom shell should be valid"),
            ..Settings::default()
        };

        let rendered = render_rc_settings_ini(&settings);
        assert!(rendered.contains("program=\\s\\u{2003}/opt/Example\\sShell/bin/sh"));
        assert!(rendered.contains("variant\\\\name\\u{85}\\s"));
        let mut parsed = Settings::default();
        apply_rc_settings_ini(&mut parsed, &rendered);
        assert_eq!(parsed.shell.mode, settings.shell.mode);
        assert_eq!(parsed.shell.history, ShellHistoryMode::Off);
    }

    #[test]
    fn invalid_shell_section_is_rejected_as_one_unit() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(
            &mut settings,
            "[shell]\nmode=custom\nprogram=fish\ndialect=fish\narg=prefix-{command}\nhistory=off\n",
        );
        assert_eq!(settings.shell.mode, ShellMode::Auto);
        assert_eq!(settings.shell.history, ShellHistoryMode::Session);
        assert!(
            settings
                .shell
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("ignored invalid [shell]"))
        );
    }

    #[test]
    fn auto_shell_rejects_execution_fields_without_partially_applying_them() {
        let mut settings = Settings::default();
        apply_rc_settings_ini(
            &mut settings,
            "[shell]\nmode=auto\nprogram=fish\ndialect=fish\nhistory=off\n",
        );
        assert_eq!(settings.shell.mode, ShellMode::Auto);
        assert_eq!(settings.shell.history, ShellHistoryMode::Session);
    }
}
