use crate::{OverwritePolicy, Settings, SettingsSortField};
use std::fs;
use std::io;
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
    let tmp = path.with_file_name(format!("{stem}.tmp-{}", std::process::id()));
    fs::write(&tmp, content)?;
    #[cfg(windows)]
    {
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                fs::remove_file(path)?;
                fs::rename(tmp, path)
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(tmp, path)
    }
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

fn apply_rc_settings_ini(settings: &mut Settings, source: &str) {
    let mut section = String::new();
    let mut saw_configuration_section = false;
    let mut saw_hotlist = false;
    let mut saw_panelize_presets = false;
    let mut saw_skin_dirs = false;

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
            continue;
        }

        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim().to_ascii_lowercase();
        let value = raw_value.trim();

        match (section.as_str(), key.as_str()) {
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
            ("configuration", "use_internal_editor") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.configuration.use_internal_editor = parsed;
                }
            }
            ("configuration", "keymap_override") => {
                if value.is_empty() {
                    settings.configuration.keymap_override = None;
                } else {
                    settings.configuration.keymap_override = Some(PathBuf::from(value));
                }
            }
            ("configuration", "hotlist") => {
                if !saw_hotlist {
                    settings.configuration.hotlist.clear();
                    saw_hotlist = true;
                }
                settings.configuration.hotlist.push(PathBuf::from(value));
            }
            ("configuration", "panelize_preset") => {
                if !saw_panelize_presets {
                    settings.configuration.panelize_presets.clear();
                    saw_panelize_presets = true;
                }
                settings
                    .configuration
                    .panelize_presets
                    .push(value.to_string());
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
                    settings.panel_options.sort_field = parsed;
                }
            }
            ("panel_options", "sort_reverse") => {
                if let Some(parsed) = parse_bool(value) {
                    settings.panel_options.sort_reverse = parsed;
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
            ("appearance", "skin") => {
                if !value.is_empty() {
                    settings.appearance.skin = value.to_string();
                }
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
            ("advanced", "disk_usage_cache_ttl_ms") => {
                if let Ok(parsed) = value.parse::<u64>() {
                    settings.advanced.disk_usage_cache_ttl_ms = parsed.max(1);
                }
            }
            ("advanced", "disk_usage_cache_max_entries") => {
                if let Ok(parsed) = value.parse::<usize>() {
                    settings.advanced.disk_usage_cache_max_entries = parsed.max(1);
                }
            }
            _ => {}
        }
    }

    if saw_configuration_section && !saw_panelize_presets {
        settings.configuration.panelize_presets.clear();
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
        "use_internal_editor={}",
        settings.configuration.use_internal_editor
    ));
    if let Some(path) = settings.configuration.keymap_override.as_ref() {
        lines.push(format!("keymap_override={}", path.to_string_lossy()));
    } else {
        lines.push(String::from("keymap_override="));
    }
    for hotlist in &settings.configuration.hotlist {
        lines.push(format!("hotlist={}", hotlist.to_string_lossy()));
    }
    for command in &settings.configuration.panelize_presets {
        lines.push(format!("panelize_preset={command}"));
    }

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
        "sort_field={}",
        sort_field_label(settings.panel_options.sort_field)
    ));
    lines.push(format!(
        "sort_reverse={}",
        settings.panel_options.sort_reverse
    ));

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
    lines.push(format!(
        "disk_usage_cache_ttl_ms={}",
        settings.advanced.disk_usage_cache_ttl_ms
    ));
    lines.push(format!(
        "disk_usage_cache_max_entries={}",
        settings.advanced.disk_usage_cache_max_entries
    ));

    let mut rendered = lines.join("\n");
    rendered.push('\n');
    rendered
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

fn parse_sort_field(value: &str) -> Option<SettingsSortField> {
    match value.trim().to_ascii_lowercase().as_str() {
        "name" => Some(SettingsSortField::Name),
        "size" => Some(SettingsSortField::Size),
        "modified" | "mtime" => Some(SettingsSortField::Modified),
        _ => None,
    }
}

fn sort_field_label(field: SettingsSortField) -> &'static str {
    match field {
        SettingsSortField::Name => "name",
        SettingsSortField::Size => "size",
        SettingsSortField::Modified => "modified",
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
        settings.configuration.hotlist = vec![PathBuf::from("/tmp"), PathBuf::from("/var")];
        settings.configuration.panelize_presets =
            vec![String::from("find . -type f"), String::from("git ls-files")];
        settings.configuration.default_overwrite_policy = OverwritePolicy::Rename;
        settings.panel_options.sort_field = SettingsSortField::Modified;

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
        assert_eq!(parsed.panel_options.sort_field, SettingsSortField::Modified);
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
        settings.configuration.hotlist = vec![PathBuf::from("/tmp"), PathBuf::from("/var")];
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
        assert!(rc_ini.contains("hotlist=/tmp"));

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
}
